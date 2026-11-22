//! Text to edit: lines of characters, a cursor in them, the part of
//! them a window shows, and the keys that change them.
//!
//! The editor program (`edit`) reads a file, hands its bytes to an
//! `Editor`, gives it every byte the keyboard sends, draws the rows it
//! says changed, and writes the text back when it says "save". All the
//! deciding - what Backspace at the start of a line does, where the
//! cursor goes on a shorter line, when the view scrolls - is here, and
//! tested on the host.
//!
//! The text is a list of lines, each a list of `char`s. Not one long
//! string: nearly every key changes one line, and a line is short, so
//! inserting into it is cheap; and a column is an index, not a walk
//! along UTF-8 bytes. A file is the lines joined by '\n' - so the bytes
//! read are exactly the bytes written back, a final newline or not.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The biggest file the editor takes: a journal entry is tens of
/// kilobytes, and each character costs four bytes in memory here.
pub const FILE_MAX: usize = 1 << 20;

/// Tab inserts spaces, up to the next multiple of this.
pub const TAB: usize = 4;

/// A key, once the bytes for it have all arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Control with a letter: `Control(b's')`.
    Control(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum State {
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// After ESC [, with the number so far.
    Csi(u8),
}

/// Turns the bytes a keyboard driver sends (`input::terminal_bytes`)
/// back into keys: arrows and the like arrive as ESC [ sequences,
/// characters beyond ASCII as several UTF-8 bytes. One byte at a time,
/// because nothing promises a key's bytes arrive in one message.
#[derive(Default)]
pub struct Keys {
    state: State,
    utf8: [u8; 4],
    have: usize,
    need: usize,
}

impl Keys {
    pub fn new() -> Keys {
        Keys::default()
    }

    /// The next byte; the key, if this byte finished one.
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => {
                self.state = State::Ground;
                if byte == b'[' {
                    self.state = State::Csi(0);
                    None
                } else {
                    // ESC alone (the Escape key) means nothing here; the
                    // byte after it is a key of its own.
                    self.ground(byte)
                }
            }
            State::Csi(number) => {
                if byte.is_ascii_digit() {
                    self.state = State::Csi(number.saturating_mul(10).saturating_add(byte - b'0'));
                    return None;
                }
                self.state = State::Ground;
                match (byte, number) {
                    (b'A', _) => Some(Key::Up),
                    (b'B', _) => Some(Key::Down),
                    (b'C', _) => Some(Key::Right),
                    (b'D', _) => Some(Key::Left),
                    (b'H', _) | (b'~', 1 | 7) => Some(Key::Home),
                    (b'F', _) | (b'~', 4 | 8) => Some(Key::End),
                    (b'~', 3) => Some(Key::Delete),
                    (b'~', 5) => Some(Key::PageUp),
                    (b'~', 6) => Some(Key::PageDown),
                    // Insert (2~) and whatever else: nothing to do.
                    _ => None,
                }
            }
        }
    }

    fn ground(&mut self, byte: u8) -> Option<Key> {
        // In the middle of a character, only its continuation bytes
        // belong; anything else means the character was cut short, and
        // what came of it is dropped.
        if self.need > 0 {
            if byte & 0xC0 == 0x80 {
                self.utf8[self.have] = byte;
                self.have += 1;
                if self.have < self.need {
                    return None;
                }
                let character = core::str::from_utf8(&self.utf8[..self.need]).ok().and_then(|s| s.chars().next());
                self.need = 0;
                return character.map(Key::Char);
            }
            self.need = 0;
        }
        match byte {
            b'\n' | b'\r' => Some(Key::Enter),
            0x08 | 0x7F => Some(Key::Backspace),
            b'\t' => Some(Key::Tab),
            0x1B => {
                self.state = State::Escape;
                None
            }
            1..=26 => Some(Key::Control(b'a' + byte - 1)),
            0x20..=0x7E => Some(Key::Char(byte as char)),
            // The first byte of a longer character: how long it says.
            0xC2..=0xF4 => {
                self.utf8[0] = byte;
                self.have = 1;
                self.need = match byte {
                    0xC2..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    _ => 4,
                };
                None
            }
            _ => None,
        }
    }
}

/// What the program must do after a key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Nothing,
    /// Write `Editor::text` to the file, then call `saved`.
    Save,
    /// End: the text is saved, or the user said twice to drop it.
    Quit,
    /// Ctrl+Q with unsaved changes: not yet - the message says why,
    /// and another Ctrl+Q straight after quits.
    Warned,
}

/// Which rows of the view must be drawn again, counted from the top
/// of the view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Damage {
    None,
    /// `first` to `last`, both drawn.
    Rows { first: usize, last: usize },
    /// The view moved: every row.
    All,
}

/// The file isn't UTF-8, so it can't be shown or edited as text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NotText;

/// The text, the cursor, and the view of `rows` x `columns` cells.
pub struct Editor {
    lines: Vec<Vec<char>>,
    /// The cursor: a line and a column in it (0 = before the first
    /// character, `len` = after the last).
    row: usize,
    column: usize,
    /// The column Up and Down try to keep: going through a short line
    /// does not lose it.
    wanted: usize,
    /// The view's first line and first column.
    top: usize,
    left: usize,
    rows: usize,
    columns: usize,
    modified: bool,
    /// Ctrl+Q (or the close box) was pressed once with unsaved changes.
    quit_asked: bool,
    keys: Keys,
    damage: Damage,
    message: String,
}

impl Editor {
    /// An empty text - a new file - shown in `rows` x `columns`.
    pub fn new(rows: usize, columns: usize) -> Editor {
        Editor {
            lines: vec![Vec::new()],
            row: 0,
            column: 0,
            wanted: 0,
            top: 0,
            left: 0,
            rows: rows.max(1),
            columns: columns.max(1),
            modified: false,
            quit_asked: false,
            keys: Keys::new(),
            damage: Damage::All,
            message: String::new(),
        }
    }

    /// Replaces the text with a file's bytes, the cursor at the start.
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), NotText> {
        let text = core::str::from_utf8(bytes).map_err(|_| NotText)?;
        self.lines = text.split('\n').map(|line| line.chars().collect()).collect();
        (self.row, self.column, self.wanted, self.top, self.left) = (0, 0, 0, 0, 0);
        self.modified = false;
        self.quit_asked = false;
        self.damage = Damage::All;
        Ok(())
    }

    /// The text as a file holds it: the lines joined by '\n'.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for (n, line) in self.lines.iter().enumerate() {
            if n > 0 {
                text.push('\n');
            }
            text.extend(line.iter());
        }
        text
    }

    /// The program wrote the text: it is no longer modified.
    pub fn saved(&mut self) {
        self.modified = false;
        self.quit_asked = false;
    }

    pub fn modified(&self) -> bool {
        self.modified
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The cursor's line and column in the text, from 0.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.column)
    }

    /// The view's first line, from 0.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The cursor in the view: (column, row). After every key the view
    /// has moved to keep the cursor in it.
    pub fn screen_cursor(&self) -> (usize, usize) {
        (self.column - self.left, self.row - self.top)
    }

    /// What row `row` of the view shows: at most `columns` characters,
    /// from the view's first column. Below the last line, nothing.
    pub fn visible(&self, row: usize) -> &[char] {
        match self.lines.get(self.top + row) {
            Some(line) if self.left < line.len() => &line[self.left..line.len().min(self.left + self.columns)],
            _ => &[],
        }
    }

    /// A line for the status bar, for the program to show.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn set_message(&mut self, message: &str) {
        self.message.clear();
        self.message.push_str(message);
    }

    /// The rows changed since the last call.
    pub fn take_damage(&mut self) -> Damage {
        core::mem::replace(&mut self.damage, Damage::None)
    }

    /// Marks the cursor's row for drawing: for when only its look
    /// changes, as when the window gains or loses the focus.
    pub fn touch_cursor(&mut self) {
        self.touch(self.row);
    }

    /// The next byte from the keyboard.
    pub fn feed(&mut self, byte: u8) -> Outcome {
        match self.keys.feed(byte) {
            Some(key) => self.command(key),
            None => Outcome::Nothing,
        }
    }

    /// A click on cell (`column`, `row`) of the view: the cursor goes
    /// there, or to the nearest place in the text.
    pub fn click(&mut self, column: usize, row: usize) {
        let before = (self.row, self.top, self.left);
        self.quit_asked = false;
        self.message.clear();
        self.row = (self.top + row).min(self.lines.len() - 1);
        self.column = (self.left + column).min(self.lines[self.row].len());
        self.wanted = self.column;
        self.follow(before);
    }

    /// The close box: true if the window may close now. With unsaved
    /// changes, the first time only warns.
    pub fn close(&mut self) -> bool {
        if !self.modified || self.quit_asked {
            return true;
        }
        self.quit_asked = true;
        self.set_message("unsaved changes: close again to drop them");
        false
    }

    /// One key.
    pub fn command(&mut self, key: Key) -> Outcome {
        let before = (self.row, self.top, self.left);
        let asked = core::mem::replace(&mut self.quit_asked, false);
        self.message.clear();
        let mut outcome = Outcome::Nothing;
        match key {
            Key::Char(c) => self.insert(c),
            Key::Tab => {
                for _ in 0..TAB - self.column % TAB {
                    self.insert(' ');
                }
            }
            Key::Enter => {
                let rest = self.lines[self.row].split_off(self.column);
                self.lines.insert(self.row + 1, rest);
                self.touch_from(self.row);
                self.row += 1;
                self.column = 0;
                self.modified = true;
            }
            Key::Backspace => {
                if self.column > 0 {
                    self.column -= 1;
                    self.lines[self.row].remove(self.column);
                    self.touch(self.row);
                    self.modified = true;
                } else if self.row > 0 {
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.column = self.lines[self.row].len();
                    self.lines[self.row].extend(line);
                    self.touch_from(self.row);
                    self.modified = true;
                }
            }
            Key::Delete => {
                if self.column < self.lines[self.row].len() {
                    self.lines[self.row].remove(self.column);
                    self.touch(self.row);
                    self.modified = true;
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].extend(next);
                    self.touch_from(self.row);
                    self.modified = true;
                }
            }
            Key::Left => {
                if self.column > 0 {
                    self.column -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.column = self.lines[self.row].len();
                }
            }
            Key::Right => {
                if self.column < self.lines[self.row].len() {
                    self.column += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.column = 0;
                }
            }
            Key::Up => self.go_to(self.row.saturating_sub(1)),
            Key::Down => self.go_to(self.row + 1),
            Key::Home => self.column = 0,
            Key::End => self.column = self.lines[self.row].len(),
            // A page is the view less one line, so the line that was
            // last stays in sight as the first: the view and the
            // cursor move together.
            Key::PageUp => {
                let page = self.page();
                self.top = self.top.saturating_sub(page);
                self.go_to(self.row.saturating_sub(page));
            }
            Key::PageDown => {
                let page = self.page();
                self.top = (self.top + page).min(self.lines.len() - 1);
                self.go_to(self.row + page);
            }
            Key::Control(b's') => outcome = Outcome::Save,
            Key::Control(b'q') => {
                if !self.modified || asked {
                    outcome = Outcome::Quit;
                } else {
                    self.quit_asked = true;
                    self.set_message("unsaved changes: ctrl+q again to drop them");
                    outcome = Outcome::Warned;
                }
            }
            Key::Control(_) => {}
        }
        // Up, Down and the pages keep the column they were trying for;
        // everything else sets it to where the cursor now is.
        if !matches!(key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
            self.wanted = self.column;
        }
        self.follow(before);
        outcome
    }

    fn insert(&mut self, c: char) {
        self.lines[self.row].insert(self.column, c);
        self.column += 1;
        self.touch(self.row);
        self.modified = true;
    }

    /// To line `row` (or the last), as near the wanted column as it has.
    fn go_to(&mut self, row: usize) {
        self.row = row.min(self.lines.len() - 1);
        self.column = self.wanted.min(self.lines[self.row].len());
    }

    fn page(&self) -> usize {
        (self.rows - 1).max(1)
    }

    /// Moves the view so the cursor is in it, and marks what must be
    /// drawn: everything if the view moved, else the row the cursor
    /// left and the row it is on.
    fn follow(&mut self, (row, top, left): (usize, usize, usize)) {
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + self.rows {
            self.top = self.row + 1 - self.rows;
        }
        if self.column < self.left {
            self.left = self.column;
        } else if self.column >= self.left + self.columns {
            self.left = self.column + 1 - self.columns;
        }
        if (self.top, self.left) != (top, left) {
            self.damage = Damage::All;
        } else {
            self.touch(row);
            self.touch(self.row);
        }
    }

    /// Line `line` must be drawn again, if it is in the view.
    fn touch(&mut self, line: usize) {
        if let Some(row) = line.checked_sub(self.top).filter(|&row| row < self.rows) {
            self.mark(row, row);
        }
    }

    /// Line `line` and every one below it in the view: lines moved up
    /// or down.
    fn touch_from(&mut self, line: usize) {
        let first = line.saturating_sub(self.top);
        if first < self.rows {
            self.mark(first, self.rows - 1);
        }
    }

    fn mark(&mut self, first: usize, last: usize) {
        self.damage = match self.damage {
            Damage::None => Damage::Rows { first, last },
            Damage::Rows { first: f, last: l } => Damage::Rows { first: f.min(first), last: l.max(last) },
            Damage::All => Damage::All,
        };
    }
}

/// The byte TheTaaJ's font draws for `c`: itself if it is printable
/// ASCII, else '?' - the font has nothing beyond ASCII yet, and the
/// character is kept in the text all the same.
pub fn glyph(c: char) -> u8 {
    if c == ' ' || c.is_ascii_graphic() {
        c as u8
    } else {
        b'?'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor holding `text`, shown 5 rows x 10 columns.
    fn editor(text: &str) -> Editor {
        let mut editor = Editor::new(5, 10);
        editor.load(text.as_bytes()).unwrap();
        editor.take_damage();
        editor
    }

    fn type_bytes(editor: &mut Editor, bytes: &[u8]) -> Vec<Outcome> {
        bytes.iter().map(|&b| editor.feed(b)).filter(|&o| o != Outcome::Nothing).collect()
    }

    fn keys(bytes: &[u8]) -> Vec<Key> {
        let mut keys = Keys::new();
        bytes.iter().filter_map(|&b| keys.feed(b)).collect()
    }

    #[test]
    fn the_keyboards_bytes_become_keys() {
        assert_eq!(keys(b"a\n\x08\t"), [Key::Char('a'), Key::Enter, Key::Backspace, Key::Tab]);
        assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D"), [Key::Up, Key::Down, Key::Right, Key::Left]);
        assert_eq!(keys(b"\x1b[H\x1b[F\x1b[3~\x1b[5~\x1b[6~"), [Key::Home, Key::End, Key::Delete, Key::PageUp, Key::PageDown]);
        assert_eq!(keys(b"\x13\x11"), [Key::Control(b's'), Key::Control(b'q')]);
        // Insert means nothing; Escape alone means nothing, and the
        // key after it is still a key.
        assert_eq!(keys(b"\x1b[2~x\x1bq"), [Key::Char('x'), Key::Char('q')]);
    }

    #[test]
    fn characters_beyond_ascii_arrive_whole() {
        assert_eq!(keys("é€😀".as_bytes()), [Key::Char('é'), Key::Char('€'), Key::Char('😀')]);
        // Cut short: the broken one is dropped, the next byte is kept -
        // and a continuation byte after that cannot finish it.
        assert_eq!(keys(b"\xE2\x82a\xACb"), [Key::Char('a'), Key::Char('b')]);
        // A continuation byte alone is nothing.
        assert_eq!(keys(b"\x82b"), [Key::Char('b')]);
    }

    #[test]
    fn a_file_comes_back_byte_for_byte() {
        for text in ["", "one", "one\n", "one\ntwo", "\n\n", "tab\there é\n"] {
            let editor = editor(text);
            assert_eq!(editor.text(), text);
        }
        assert_eq!(Editor::new(5, 10).load(b"bad \xFF byte"), Err(NotText));
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_marks_its_row() {
        let mut e = editor("ac");
        e.command(Key::Right);
        e.take_damage();
        type_bytes(&mut e, b"b");
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), (0, 2));
        assert!(e.modified());
        assert_eq!(e.take_damage(), Damage::Rows { first: 0, last: 0 });
    }

    #[test]
    fn enter_splits_a_line_and_backspace_joins_it_again() {
        let mut e = editor("hello\nend");
        e.command(Key::Right);
        e.command(Key::Right);
        e.take_damage();
        e.command(Key::Enter);
        assert_eq!(e.text(), "he\nllo\nend");
        assert_eq!(e.cursor(), (1, 0));
        // Every row from the split down moved.
        assert_eq!(e.take_damage(), Damage::Rows { first: 0, last: 4 });
        e.command(Key::Backspace);
        assert_eq!(e.text(), "hello\nend");
        assert_eq!(e.cursor(), (0, 2));
        // At the very start, Backspace changes nothing.
        let mut e = editor("x");
        e.command(Key::Backspace);
        assert!(!e.modified());
    }

    #[test]
    fn delete_removes_under_the_cursor_or_joins_the_next_line() {
        let mut e = editor("ab\ncd");
        e.command(Key::Delete);
        assert_eq!(e.text(), "b\ncd");
        e.command(Key::End);
        e.command(Key::Delete);
        assert_eq!(e.text(), "bcd");
        e.command(Key::End);
        e.command(Key::Delete);
        assert_eq!(e.text(), "bcd");
    }

    #[test]
    fn tab_inserts_spaces_to_the_next_stop() {
        let mut e = editor("ab");
        e.command(Key::End);
        e.command(Key::Tab);
        assert_eq!(e.text(), "ab  ");
        e.command(Key::Tab);
        assert_eq!(e.text(), "ab      ");
    }

    #[test]
    fn up_and_down_keep_the_column_through_a_short_line() {
        let mut e = editor("long line\nab\nanother line");
        for _ in 0..7 {
            e.command(Key::Right);
        }
        e.command(Key::Down);
        assert_eq!(e.cursor(), (1, 2));
        e.command(Key::Down);
        assert_eq!(e.cursor(), (2, 7));
        e.command(Key::Down);
        assert_eq!(e.cursor(), (2, 7));
        // Left sets a new column to keep.
        e.command(Key::Left);
        e.command(Key::Up);
        e.command(Key::Up);
        assert_eq!(e.cursor(), (0, 6));
    }

    #[test]
    fn left_and_right_cross_line_ends() {
        let mut e = editor("ab\ncd");
        e.command(Key::End);
        e.command(Key::Right);
        assert_eq!(e.cursor(), (1, 0));
        e.command(Key::Left);
        assert_eq!(e.cursor(), (0, 2));
        e.command(Key::Home);
        e.command(Key::Left);
        assert_eq!(e.cursor(), (0, 0));
    }

    #[test]
    fn the_view_follows_the_cursor_down_and_back() {
        let text: Vec<String> = (1..=12).map(|n| alloc::format!("line {n}")).collect();
        let mut e = editor(&text.join("\n"));
        for _ in 0..3 {
            e.command(Key::Down);
        }
        e.take_damage();
        e.command(Key::Down);
        assert_eq!(e.top(), 0);
        assert_eq!(e.take_damage(), Damage::Rows { first: 3, last: 4 });
        e.command(Key::Down);
        assert_eq!((e.top(), e.screen_cursor()), (1, (0, 4)));
        assert_eq!(e.take_damage(), Damage::All);
        assert_eq!(e.visible(0), "line 2".chars().collect::<Vec<_>>());
        for _ in 0..5 {
            e.command(Key::Up);
        }
        assert_eq!((e.top(), e.cursor()), (0, (0, 0)));
    }

    #[test]
    fn pages_move_the_view_and_the_cursor_together() {
        let text: Vec<String> = (1..=12).map(|n| alloc::format!("{n}")).collect();
        let mut e = editor(&text.join("\n"));
        e.command(Key::PageDown);
        // 5 rows: a page is 4 lines.
        assert_eq!((e.top(), e.cursor()), (4, (4, 0)));
        e.command(Key::PageDown);
        e.command(Key::PageDown);
        // The last line, and the view does not run past it.
        assert_eq!((e.top(), e.cursor()), (11, (11, 0)));
        assert_eq!(e.visible(1), &[] as &[char]);
        e.command(Key::PageUp);
        assert_eq!((e.top(), e.cursor()), (7, (7, 0)));
        for _ in 0..3 {
            e.command(Key::PageUp);
        }
        assert_eq!((e.top(), e.cursor()), (0, (0, 0)));
    }

    #[test]
    fn a_long_line_scrolls_sideways() {
        let mut e = editor("0123456789abcdef");
        e.command(Key::End);
        // 10 columns: the cursor after 'f' is in the last one.
        assert_eq!(e.screen_cursor(), (9, 0));
        assert_eq!(e.visible(0), "789abcdef".chars().collect::<Vec<_>>());
        e.command(Key::Home);
        assert_eq!(e.screen_cursor(), (0, 0));
        assert_eq!(e.visible(0).len(), 10);
    }

    #[test]
    fn save_and_quit() {
        let mut e = editor("x");
        assert_eq!(type_bytes(&mut e, b"\x11"), [Outcome::Quit]);
        type_bytes(&mut e, b"y");
        // Unsaved: the first Ctrl+Q only warns, and another key in
        // between starts that over.
        assert_eq!(type_bytes(&mut e, b"\x11"), [Outcome::Warned]);
        assert!(e.message().contains("unsaved"));
        assert_eq!(type_bytes(&mut e, b"\x1b[C\x11"), [Outcome::Warned]);
        assert_eq!(type_bytes(&mut e, b"\x11"), [Outcome::Quit]);
        // Saved, it quits at once.
        assert_eq!(type_bytes(&mut e, b"\x13"), [Outcome::Save]);
        e.saved();
        assert_eq!(type_bytes(&mut e, b"\x11"), [Outcome::Quit]);
    }

    #[test]
    fn the_close_box_asks_twice_only_with_unsaved_changes() {
        let mut e = editor("x");
        assert!(e.close());
        e.command(Key::Char('y'));
        assert!(!e.close());
        assert!(e.close());
    }

    #[test]
    fn a_click_puts_the_cursor_in_the_text() {
        let mut e = editor("abc\nde");
        e.click(1, 1);
        assert_eq!(e.cursor(), (1, 1));
        // Past a line's end, or below the last line: the nearest place.
        e.click(9, 0);
        assert_eq!(e.cursor(), (0, 3));
        e.click(9, 4);
        assert_eq!(e.cursor(), (1, 2));
    }

    #[test]
    fn only_printable_ascii_is_drawn_as_itself() {
        assert_eq!(glyph('a'), b'a');
        assert_eq!(glyph(' '), b' ');
        assert_eq!(glyph('é'), b'?');
        assert_eq!(glyph('\t'), b'?');
    }
}
