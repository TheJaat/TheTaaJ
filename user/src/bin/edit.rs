//! edit FILE - a text editor in a window.
//!
//! It reads FILE (or starts it empty, if there is none), shows it in a
//! window of 76 x 24 characters with a status line under them, and
//! takes keys while the window has the focus:
//!
//!   characters, Enter, Tab      typed in at the cursor
//!   Backspace, Delete           remove before / under the cursor
//!   arrows, Home, End           move
//!   Page Up, Page Down          move a page, the view with the cursor
//!   Ctrl+S                      save
//!   Ctrl+Q                      quit (twice, with unsaved changes)
//!   a click in the text         puts the cursor there
//!   the close box               as Ctrl+Q
//!
//! The editing is `gui::text`'s; this program reads and writes the
//! file, draws, and passes the window's events on. It is given two
//! services by the shell: "files", the channel that can change them,
//! and "windows".
//!
//! What it says for the record goes to the kernel's log (its standard
//! output is the terminal window, which a test does not read): the file
//! opened, every save, the view moving, quitting.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use gfx::Rect;
use gui::protocol::{Event, TITLE_MAX};
use gui::text::{glyph, Damage, Editor, Outcome, FILE_MAX};
use gui::toolkit::Theme;
use taaj::files::{FileError, Files, Kind, Status};
use taaj::window::{Window, MESSAGE};
use taaj::{eprintln, Buffer};

taaj::program!(main);

const COLUMNS: usize = 76;
const ROWS: usize = 24;
/// A character's cell: the font's 5x8 drawn twice the size, with a
/// column of space after it and four rows between lines.
const CELL_WIDTH: u32 = 12;
const CELL_HEIGHT: u32 = 20;
/// Space round the text.
const PAD: u32 = 6;
const WIDTH: u32 = COLUMNS as u32 * CELL_WIDTH + 2 * PAD;
/// The text's rows and the status line.
const HEIGHT: u32 = (ROWS as u32 + 1) * CELL_HEIGHT + 2 * PAD;
const LEFT: u8 = 1;

/// A line for the log.
macro_rules! log {
    ($($arg:tt)*) => {{
        let mut line = Buffer::<200>::new();
        let _ = write!(line, "[edit] ");
        let _ = writeln!(line, $($arg)*);
        let _ = taaj::debug_write(line.as_bytes());
    }};
}

struct Edit {
    window: Window,
    theme: Theme,
    editor: Editor,
    files: Files,
    /// The file's name as given, and as a whole path.
    name: String,
    path: String,
    focused: bool,
    buttons: u8,
}

impl Edit {
    /// The rectangle of row `row` of the text in the window (ROWS is
    /// the status line).
    fn row_rect(row: usize) -> Rect {
        Rect::new(PAD as i32, (PAD + row as u32 * CELL_HEIGHT) as i32, COLUMNS as u32 * CELL_WIDTH, CELL_HEIGHT)
    }

    /// Draws row `row` of the view, and the cursor if it is on it.
    fn draw_row(&mut self, row: usize) -> Rect {
        let rect = Self::row_rect(row);
        let theme = self.theme;
        let mut glyphs = [0u8; COLUMNS];
        let visible = self.editor.visible(row);
        for (cell, &c) in glyphs.iter_mut().zip(visible) {
            *cell = glyph(c);
        }
        let count = visible.len();
        let (cursor_column, cursor_row) = self.editor.screen_cursor();
        let mut surface = self.window.surface();
        surface.fill(rect, theme.panel);
        surface.text(rect.x, rect.y + 2, &glyphs[..count], 2, theme.text);
        if cursor_row == row {
            let x = rect.x + (cursor_column as u32 * CELL_WIDTH) as i32;
            if self.focused {
                // A block, with the character under it drawn out of it.
                surface.fill(Rect::new(x, rect.y, CELL_WIDTH - 2, CELL_HEIGHT), theme.text);
                if cursor_column < count {
                    surface.text(x, rect.y + 2, &glyphs[cursor_column..=cursor_column], 2, theme.panel);
                }
            } else {
                // Without the keys, only a line under where it is.
                surface.fill(Rect::new(x, rect.y + CELL_HEIGHT as i32 - 3, CELL_WIDTH - 2, 2), theme.text);
            }
        }
        rect
    }

    /// The status line: the file, where the cursor is, whether there
    /// are unsaved changes, and the last message.
    fn draw_status(&mut self) -> Rect {
        let rect = Self::row_rect(ROWS);
        let (row, column) = self.editor.cursor();
        let mut status = String::new();
        let _ = write!(status, " {}  line {}/{}, column {}", self.name, row + 1, self.editor.line_count(), column + 1);
        if self.editor.modified() {
            let _ = write!(status, "  [modified]");
        }
        if !self.editor.message().is_empty() {
            let _ = write!(status, "  {}", self.editor.message());
        }
        let mut glyphs = [0u8; COLUMNS];
        let mut count = 0;
        for (cell, c) in glyphs.iter_mut().zip(status.chars()) {
            *cell = glyph(c);
            count += 1;
        }
        let theme = self.theme;
        let mut surface = self.window.surface();
        surface.fill(rect, theme.button);
        surface.text(rect.x, rect.y + 2, &glyphs[..count], 2, theme.button_text);
        rect
    }

    /// Draws what changed, and tells the server where.
    fn redraw(&mut self) {
        let mut damage = self.draw_status();
        let rows = match self.editor.take_damage() {
            Damage::None => 0..0,
            Damage::Rows { first, last } => first..last + 1,
            Damage::All => 0..ROWS,
        };
        for row in rows {
            damage = damage.union(&self.draw_row(row));
        }
        self.window.damage(damage);
    }

    fn save(&mut self) {
        let text = self.editor.text();
        match self.files.write(&self.name, text.as_bytes()) {
            Ok(size) => {
                self.editor.saved();
                self.editor.set_message(&format!("saved, {size} bytes"));
                log!("saved {}: {} bytes, {} lines", self.path, size, self.editor.line_count());
            }
            Err(error) => {
                self.editor.set_message(&format!("not saved: {}", error.describe()));
                log!("{} not saved: {}", self.path, error.describe());
            }
        }
    }

    /// A pointer event: a press of the left button in the text puts
    /// the cursor there.
    fn pointer(&mut self, x: i32, y: i32, buttons: u8) {
        let pressed = buttons & LEFT != 0 && self.buttons & LEFT == 0;
        self.buttons = buttons;
        let text = Rect::new(PAD as i32, PAD as i32, COLUMNS as u32 * CELL_WIDTH, ROWS as u32 * CELL_HEIGHT);
        if pressed && text.contains(x, y) {
            let column = (x - text.x) as u32 / CELL_WIDTH;
            let row = (y - text.y) as u32 / CELL_HEIGHT;
            self.editor.click(column as usize, row as usize);
        }
    }
}

/// The file, read whole; None, with the reason said, if it cannot be
/// edited. A file that is not there is a new one: empty.
fn read(files: &Files, name: &str) -> Result<Option<Vec<u8>>, &'static str> {
    match files.stat(name) {
        Ok((Kind::Directory, _)) => Err("it is a directory"),
        Ok((_, size)) if size > FILE_MAX as u64 => Err("it is too big for the editor"),
        Ok(_) => {
            let mut bytes = Vec::new();
            files.read(name, |piece| bytes.extend_from_slice(piece)).map_err(FileError::describe)?;
            Ok(Some(bytes))
        }
        Err(FileError::Refused(Status::NotFound)) => Ok(None),
        Err(error) => Err(error.describe()),
    }
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    let [name] = arguments.as_slice() else {
        eprintln!("usage: edit FILE");
        return 1;
    };
    let Some(files) = taaj::file_service() else {
        eprintln!("edit: there is no file service for it");
        return 1;
    };
    let path = files.resolve(name);
    let mut editor = Editor::new(ROWS, COLUMNS);
    match read(&files, name) {
        Ok(Some(bytes)) => {
            if editor.load(&bytes).is_err() {
                eprintln!("edit: {name}: it is not text (not UTF-8)");
                return 1;
            }
            log!("opened {}: {} lines, {} bytes", path, editor.line_count(), bytes.len());
        }
        Ok(None) => {
            editor.set_message("a new file");
            log!("new file {}", path);
        }
        Err(why) => {
            eprintln!("edit: {name}: {why}");
            return 1;
        }
    }

    // The title: "edit NAME", as much of it as fits - cut at a
    // character's start, so what is left is still text.
    let mut title = String::from("edit ");
    for c in name.chars() {
        if title.len() + c.len_utf8() > TITLE_MAX {
            break;
        }
        title.push(c);
    }
    let window = match Window::open(title.as_bytes(), WIDTH, HEIGHT) {
        Ok(window) => window,
        Err(error) => {
            eprintln!("edit: no window: {}", error.describe());
            return 1;
        }
    };
    let theme = Theme::new(window.format());
    let mut edit = Edit { window, theme, editor, files, name: name.clone(), path, focused: true, buttons: 0 };
    edit.window.surface().fill(Rect::new(0, 0, WIDTH, HEIGHT), theme.panel);
    edit.window.damage(Rect::new(0, 0, WIDTH, HEIGHT));
    edit.redraw();
    log!("window {} open, {}x{}: {} x {} characters", edit.window.id(), WIDTH, HEIGHT, COLUMNS, ROWS);

    let mut top = edit.editor.top();
    let mut buffer = [0u8; MESSAGE];
    loop {
        let event = match edit.window.event(&mut buffer) {
            Ok(event) => event,
            Err(_) => {
                log!("the window server is gone");
                return 2;
            }
        };
        match event {
            Event::Key(bytes) => {
                for &byte in bytes {
                    match edit.editor.feed(byte) {
                        Outcome::Nothing => {}
                        Outcome::Save => edit.save(),
                        Outcome::Warned => log!("unsaved changes: ctrl+q again to drop them"),
                        Outcome::Quit => {
                            log!("quit");
                            return 0;
                        }
                    }
                }
            }
            Event::Pointer { x, y, buttons } => edit.pointer(x, y, buttons),
            Event::Focus(focused) => {
                edit.focused = focused;
                edit.editor.touch_cursor();
            }
            Event::Close => {
                if edit.editor.close() {
                    log!("closed by its close box");
                    return 0;
                }
                log!("the close box: unsaved changes, asked again");
            }
            Event::Opened { .. } | Event::Refused(_) => {}
        }
        if edit.editor.top() != top {
            top = edit.editor.top();
            log!("view from line {}", top + 1);
        }
        edit.redraw();
    }
}
