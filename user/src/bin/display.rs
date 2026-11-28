//! The display service: the window server. It owns the screen, from
//! ring 3.
//!
//! Startup handles, from init:
//!   1  the framebuffer's memory object (READ and WRITE)
//!   2  a channel of display messages (abi::display): first INFO - the
//!      framebuffer's description - then TEXT from whoever was given
//!      the sending end (the shell), and POINTER from the drivers of
//!      the mouse or tablet
//!   3  pings from init (abi::PING)
//!   4  the keyboard drivers' keys: the bytes a terminal sends
//!   5  the shell's keys: where those go while the terminal has the focus
//!   6  the window server's channel (the registry's "windows"): OPEN
//!      requests from programs (gui::protocol)
//!   7  memory init keeps for the terminal's text, if it had some
//!
//! What is on the screen, bottom to top: the desktop (the amber band
//! with the system's name, and the navy-to-teal gradient), then the
//! windows in their stacking order, each in a frame with a title bar,
//! then the pointer's arrow. The first window is the terminal: the text
//! console the shell writes to, kept in a grid of characters (the
//! `console` crate) and drawn into pixels of its own. The others are
//! programs' windows, whose pixels are memory the program shares.
//!
//! The screen is *composed*: a damaged rectangle is drawn again, bottom
//! to top, into a buffer the size of the screen (the back buffer), and
//! then copied to the framebuffer in one go. Drawing straight onto the
//! screen, the desktop would show for a moment where a window is about
//! to be drawn: that flicker is what the back buffer is for. Only what
//! changed is composed: a window's damage, a moved window's old and new
//! place, the terminal's changed cells.
//!
//! Where things go, and who decides:
//!
//!   - The pointer's reports go to `gui::desktop::Desktop`, which says
//!     what they mean: a window raised, dragged, asked to close, or a
//!     click that belongs to a program.
//!   - Keys go to the window with the focus: to the shell while that is
//!     the terminal, to the program as KEY events otherwise.
//!   - Text from the shell goes into the terminal's grid. Text is taken
//!     as fast as it comes - every message already waiting goes into
//!     the grid before anything is drawn - so lines that scroll past
//!     between two paints are never drawn at all.
//!
//! The desktop benchmark (K114) is measured here, where the screen is
//! drawn: `display bench` times one composition of the whole screen and
//! a window dragged a pixel at a time (`Server::bench`); `display keys N`
//! times the next N keys from their arrival here to the composition that
//! shows the terminal's echo of them (`KeyTimes`).

#![no_std]
#![no_main]

use abi::display::{Pointer as Report, ABSOLUTE_MAX};
use abi::FramebufferInfo;
use console::{font, Grid, MAX_COLUMNS, MAX_ROWS};
use gfx::{Format, Image, Pointer, Rect, Surface};
use gui::desktop::{Damage, Desktop, Id, Part, Response, BORDER, LEFT, TITLE_HEIGHT, WINDOWS_MAX};
use gui::protocol::{Event, Open, Request};
use gui::toolkit::{draw_frame, Theme};
use taaj::{println, Buffer, Error, Handle};

taaj::program!(main);

/// The grid when init gave no memory to keep it in. In static memory:
/// at 38 KiB it would crowd the 64 KiB stack.
static mut GRID: Grid = Grid::new(MAX_COLUMNS, MAX_ROWS);

// The kept grid must fit in the memory init keeps for it.
const _: () = assert!(core::mem::size_of::<Grid>() as u64 <= abi::display::KEPT_SIZE);

/// What the terminal's pixels show now: the character drawn in each
/// cell, and where the cursor is drawn. 0 is "nothing drawn yet", which
/// no cell of the grid ever holds - so the first paint draws everything.
struct Shown {
    cells: [[u8; MAX_COLUMNS]; MAX_ROWS],
    cursor: (usize, usize),
}
static mut SHOWN: Shown = Shown { cells: [[0; MAX_COLUMNS]; MAX_ROWS], cursor: (usize::MAX, usize::MAX) };

/// The desktop benchmark's key to screen (K114): from a key's arrival
/// here, on its way to the shell, to the end of the composition that
/// puts the shell's echo of it on the screen.
///
/// The keys are matched to their echoes by count: while keys wait, each
/// printable byte of text from the shell is one echo, and a composition
/// after text arrived ends as many waits as echoes came, oldest first.
/// The test types single letters at the prompt, so the text that comes
/// is their echo and nothing else. What is not measured: the driver's
/// part, from the keyboard's interrupt to the key reaching this program
/// - a channel message, microseconds - and the framebuffer's way to the
/// monitor.
struct KeyTimes {
    /// How many keys this measurement takes; 0 when none is running.
    wanted: usize,
    /// When each key still waiting for its echo arrived (monotonic ns),
    /// oldest first.
    arrived: [u64; KEYS_WAITING],
    waiting: usize,
    /// Echoes taken in since the last composition.
    echoed: usize,
    /// How long each measured key took, in ns.
    took: [u64; abi::display::KEYS_MAX as usize],
    done: usize,
}

/// Keys waiting for their echo at once, at most: more are not timed.
const KEYS_WAITING: usize = 64;

impl KeyTimes {
    const fn new() -> KeyTimes {
        KeyTimes { wanted: 0, arrived: [0; KEYS_WAITING], waiting: 0, echoed: 0, took: [0; abi::display::KEYS_MAX as usize], done: 0 }
    }

    fn start(&mut self, count: u32) {
        *self = KeyTimes::new();
        self.wanted = (count as usize).min(self.took.len());
        println!("[display] key to screen: timing the next {} keys", self.wanted);
    }

    /// Keys on their way to the shell.
    fn key(&mut self, bytes: &[u8]) {
        if self.wanted == 0 {
            return;
        }
        let now = taaj::monotonic_ns();
        for _ in bytes.iter().filter(|&&b| printable(b)) {
            if self.waiting < KEYS_WAITING && self.done + self.waiting < self.wanted {
                self.arrived[self.waiting] = now;
                self.waiting += 1;
            }
        }
    }

    /// Text from the shell, taken into the grid.
    fn text(&mut self, bytes: &[u8]) {
        if self.waiting > 0 {
            self.echoed += bytes.iter().filter(|&&b| printable(b)).count();
        }
    }

    /// A composition has just ended: the echoes taken in since the last
    /// one are on the screen.
    fn shown(&mut self) {
        if self.echoed == 0 || self.waiting == 0 {
            self.echoed = 0;
            return;
        }
        let now = taaj::monotonic_ns();
        let ended = self.echoed.min(self.waiting);
        for &arrived in &self.arrived[..ended] {
            self.took[self.done] = now.saturating_sub(arrived);
            self.done += 1;
        }
        self.arrived.copy_within(ended..self.waiting, 0);
        self.waiting -= ended;
        self.echoed = 0;
        if self.done == self.wanted {
            let took = &mut self.took[..self.done];
            took.sort_unstable();
            println!(
                "[display] key to screen: {} keys, median {} us, worst {} us",
                took.len(),
                took[took.len() / 2] / 1000,
                took[took.len() - 1] / 1000
            );
            self.wanted = 0;
        }
    }
}

fn printable(byte: u8) -> bool {
    (0x20..0x7F).contains(&byte)
}

/// Most messages taken in before the screen is brought up to date: a
/// program that never stops printing must not keep the screen frozen.
const BATCH: usize = 64;

/// Pixels of our own, in a memory object: the back buffer, and the
/// terminal's.
struct Pixels {
    base: *mut u32,
    width: u32,
    height: u32,
}

impl Pixels {
    fn new(width: u32, height: u32) -> Option<Pixels> {
        let bytes = (u64::from(width) * u64::from(height) * 4).div_ceil(4096) * 4096;
        let memory = taaj::memory_create(bytes).ok()?;
        let base = taaj::memory_map(memory, true).ok()?.cast::<u32>();
        // The mapping stays as long as the process: the handle is not
        // needed again.
        let _ = taaj::close(memory);
        Some(Pixels { base, width, height })
    }

    fn surface(&mut self) -> Surface<'_> {
        // SAFETY: a mapping of at least width x height pixels, made in
        // `new`, this process's alone, written only through this slice
        // while `self` is borrowed mutably.
        let pixels = unsafe { core::slice::from_raw_parts_mut(self.base, self.width as usize * self.height as usize) };
        Surface::new(pixels, self.width, self.height, self.width as usize).expect("exactly width x height")
    }

    fn image(&self) -> Image<'_> {
        // SAFETY: as in `surface`, read while `self` is borrowed.
        let pixels = unsafe { core::slice::from_raw_parts(self.base, self.width as usize * self.height as usize) };
        Image::new(pixels, self.width, self.height, self.width as usize).expect("exactly width x height")
    }
}

/// The screen itself.
struct Screen {
    fb: FramebufferInfo,
    base: *mut u8,
    format: Format,
}

impl Screen {
    /// The framebuffer as a surface.
    fn surface(&mut self) -> Surface<'_> {
        let stride = self.fb.pitch as usize / 4;
        // SAFETY: the mapping holds `height` rows of `pitch` bytes, and
        // a framebuffer of 32 bits a pixel has a pitch that is a
        // multiple of four (checked in main). The slice lives while
        // `self` is borrowed mutably.
        let pixels = unsafe { core::slice::from_raw_parts_mut(self.base.cast::<u32>(), stride * self.fb.height as usize) };
        Surface::new(pixels, self.fb.width, self.fb.height, stride).expect("the slice is exactly as big as the surface")
    }
}

/// The desktop behind the windows: a function of y alone (and the name
/// in the band), so any part of it can be drawn again on its own.
struct Backdrop {
    height: u32,
    band_end: u32,
    band: u32,
    /// The system's name in the band: where, how big, in what colour.
    title_x: i32,
    title_y: i32,
    title_scale: u32,
    navy: u32,
    format: Format,
}

impl Backdrop {
    fn colour(&self, y: u32) -> u32 {
        if y < self.band_end {
            self.band
        } else {
            let t = (y * 255 / self.height) as u8;
            self.format.pixel(0x10, 0x18 + t / 6, 0x30 + t / 4)
        }
    }

    /// Draws the part of the desktop inside `area` (screen coordinates)
    /// on `region`, whose (0, 0) is `area`'s corner.
    fn draw(&self, region: &mut Surface, area: Rect) {
        for row in 0..area.height {
            region.fill(Rect::new(0, row as i32, area.width, 1), self.colour(area.y as u32 + row));
        }
        if (area.y as u32) < self.band_end {
            region.text(self.title_x - area.x, self.title_y - area.y, b"TheTaaJ", self.title_scale, self.navy);
        }
    }
}

/// The terminal: the grid of characters, drawn into pixels of its own.
struct Terminal {
    id: Id,
    pixels: Pixels,
    scale: usize,
    /// Space between the window's edge and the cells, all round.
    pad: usize,
    cell_width: usize,
    cell_height: usize,
    text: u32,
    background: u32,
}

impl Terminal {
    /// Redraws one cell, with the cursor's underline if it is there.
    fn cell(&mut self, column: usize, row: usize, c: u8, cursor: bool) {
        let (x0, y0) = (self.pad + column * self.cell_width, self.pad + row * self.cell_height);
        let (scale, cell_width, cell_height, text, background) = (self.scale, self.cell_width, self.cell_height, self.text, self.background);
        let mut surface = self.pixels.surface();
        surface.fill(Rect::new(x0 as i32, y0 as i32, cell_width as u32, cell_height as u32), background);
        let glyph = font::glyph(c);
        for dy in 0..cell_height {
            let font_row = dy / scale;
            // The glyph's rows, then one row of underline for the cursor.
            let bits = match font_row {
                r if r < font::HEIGHT => glyph[r],
                r if r == font::HEIGHT && cursor => 0xFF,
                _ => 0,
            };
            if bits == 0 {
                continue;
            }
            for dx in 0..cell_width {
                let font_column = dx / scale;
                if font_column < font::WIDTH && bits & (1 << (font::WIDTH - 1 - font_column)) != 0 {
                    surface.put((x0 + dx) as i32, (y0 + dy) as i32, text);
                }
            }
        }
    }

    /// Brings the pixels in line with the grid: draws the cells that
    /// differ from what is shown, and moves the cursor. Returns the
    /// rectangle (in the terminal's pixels) holding every cell drawn.
    fn sync(&mut self, grid: &Grid, shown: &mut Shown) -> Rect {
        let (columns, rows) = grid.size();
        let cursor = grid.cursor();
        let mut changed = Rect::default();
        for row in 0..rows {
            for column in 0..columns {
                let c = grid.at(column, row);
                let here = (column, row);
                // A cell is stale if its character changed, or if the
                // cursor arrived or left.
                if shown.cells[row][column] != c || (here == cursor) != (here == shown.cursor) {
                    self.cell(column, row, c, here == cursor);
                    shown.cells[row][column] = c;
                    let (x, y) = (self.pad + column * self.cell_width, self.pad + row * self.cell_height);
                    let cell = Rect::new(x as i32, y as i32, self.cell_width as u32, self.cell_height as u32);
                    changed = changed.union(&cell);
                }
            }
        }
        shown.cursor = cursor;
        changed
    }
}

/// A program's window.
struct Client {
    id: Id,
    channel: Handle,
    memory: Handle,
    pixels: *const u32,
    width: u32,
    height: u32,
}

impl Client {
    fn image(&self) -> Image<'_> {
        // SAFETY: mapped read-only in `open`, at least width x height
        // pixels (its size was checked), until `close` unmaps it. The
        // program may write while this reads: the worst is a torn frame,
        // drawn right at the next damage - never memory outside it.
        let pixels = unsafe { core::slice::from_raw_parts(self.pixels, self.width as usize * self.height as usize) };
        Image::new(pixels, self.width, self.height, self.width as usize).expect("exactly width x height")
    }

    fn send(&self, event: Event) {
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        if let Some(n) = event.encode(&mut message) {
            // A program that does not read its events loses them; the
            // server never waits for one.
            let _ = taaj::send(self.channel, &message[..n], &[]);
        }
    }
}

/// Where the pointer is, and the buttons that were held.
struct Mouse {
    x: i32,
    y: i32,
    buttons: u8,
    arrow: Pointer,
}

impl Mouse {
    /// Takes a report: moves, clamped to the screen. Says so if a
    /// button went down - one line, where: what the tests check
    /// positions by.
    fn take(&mut self, report: Report, width: u32, height: u32) {
        let (right, bottom) = (width as i32 - 1, height as i32 - 1);
        if report.absolute {
            // 0..=ABSOLUTE_MAX across: the first pixel to the last.
            self.x = (i64::from(report.x) * i64::from(right) / i64::from(ABSOLUTE_MAX)) as i32;
            self.y = (i64::from(report.y) * i64::from(bottom) / i64::from(ABSOLUTE_MAX)) as i32;
        } else {
            self.x = self.x.saturating_add(report.x).clamp(0, right);
            self.y = self.y.saturating_add(report.y).clamp(0, bottom);
        }
        let pressed = report.buttons & !self.buttons;
        self.buttons = report.buttons;
        for (bit, name) in [(1, "left"), (2, "right"), (4, "middle"), (8, "side"), (16, "extra")] {
            if pressed & bit != 0 {
                println!("[display] {name} button at ({}, {})", self.x, self.y);
            }
        }
    }
}

/// Everything the server keeps.
struct Server {
    screen: Screen,
    back: Pixels,
    backdrop: Backdrop,
    theme: Theme,
    desktop: Desktop,
    terminal: Terminal,
    clients: [Option<Client>; WINDOWS_MAX],
    shell_keys: Handle,
    damage: Damage,
    key_times: KeyTimes,
    /// The modifier keys held now, from the drivers' KEY messages (K42).
    modifiers: u8,
}

impl Server {
    fn client(&self, id: Id) -> Option<&Client> {
        self.clients.iter().flatten().find(|client| client.id == id)
    }

    fn title(&self, id: Id) -> Buffer<40> {
        let mut title = Buffer::<40>::new();
        if let Some(window) = self.desktop.window(id) {
            for &byte in window.title() {
                let _ = core::fmt::Write::write_char(&mut title, if byte.is_ascii_graphic() || byte == b' ' { byte as char } else { '?' });
            }
        }
        title
    }

    /// Does what a Response says: damage, events for programs, lines
    /// for the log.
    fn apply(&mut self, response: Response) {
        for rect in response.damage.rects() {
            self.damage.add(*rect);
        }
        if let Some((id, x, y, buttons)) = response.deliver {
            if let Some(client) = self.client(id) {
                client.send(Event::Pointer { x, y, buttons });
            }
        }
        if let Some((from, to)) = response.focus {
            if let Some(client) = from.and_then(|from| self.client(from)) {
                client.send(Event::Focus(false));
            }
            if let Some(client) = self.client(to) {
                client.send(Event::Focus(true));
            }
            let title = self.title(to);
            println!("[display] focus: window {to} \"{}\"", core::str::from_utf8(title.as_bytes()).unwrap_or("?"));
        }
        if let Some((id, x, y)) = response.moved {
            println!("[display] window {id} moved to ({x}, {y})");
        }
        if let Some(id) = response.close {
            if let Some(client) = self.client(id) {
                client.send(Event::Close);
                println!("[display] window {id}: its program is asked to close it");
            }
        }
    }

    /// Keys from the drivers: to the window with the focus.
    fn keys(&mut self, bytes: &[u8]) {
        match self.desktop.focus().and_then(|id| self.client(id)) {
            Some(client) => client.send(Event::Key(bytes)),
            // The terminal: to the shell, as the driver would have sent
            // them. A shell that lags loses keys, as before.
            None => {
                self.key_times.key(bytes);
                let _ = taaj::send(self.shell_keys, bytes, &[]);
            }
        }
    }

    /// The wheel turned (K46): told to the program whose window's content
    /// is under the pointer - not the one with the focus, as desktops do,
    /// so that a window behind can be scrolled without clicking it. The
    /// terminal, under it, has nothing to scroll yet.
    fn wheel(&mut self, x: i32, y: i32, clicks: i8) {
        if let Some((id, Part::Content)) = self.desktop.at(x, y) {
            let content = self.desktop.window(id).map(|window| window.content);
            if let (Some(client), Some(content)) = (self.client(id), content) {
                client.send(Event::Wheel { x: x - content.x, y: y - content.y, clicks });
            }
        }
    }

    /// A key went down or up (K42): the modifiers kept up to date, and the
    /// key told to a program's window with the focus. The terminal is not
    /// told: what it needs, the bytes, comes from `keys`.
    fn key_code(&mut self, code: u16, down: bool) {
        let bit = match code {
            42 | 54 => gui::protocol::SHIFT,   // left and right Shift
            29 | 97 => gui::protocol::CONTROL, // left and right Control
            56 | 100 => gui::protocol::ALT,    // left and right Alt
            _ => 0,
        };
        if down {
            self.modifiers |= bit;
        } else {
            self.modifiers &= !bit;
        }
        if let Some(client) = self.desktop.focus().and_then(|id| self.client(id)) {
            client.send(Event::KeyCode { code, down, modifiers: self.modifiers });
        }
    }

    /// An OPEN request, with the handles it brought.
    fn open(&mut self, message: &[u8], handles: &[Handle]) {
        let (request, &[channel, memory]) = (Open::decode(message), handles) else {
            for &handle in handles {
                let _ = taaj::close(handle);
            }
            return;
        };
        let refuse = |why: &str| {
            let mut answer = [0u8; 80];
            if let Some(n) = (Event::Refused(why.as_bytes())).encode(&mut answer) {
                let _ = taaj::send(channel, &answer[..n], &[]);
            }
            let _ = taaj::close(channel);
            let _ = taaj::close(memory);
        };
        let Some(request) = request else { return refuse("not a request to open a window") };
        if !taaj::memory_size(memory).is_ok_and(|size| size >= request.bytes()) {
            return refuse("the memory is smaller than the window");
        }
        let Some(slot) = self.clients.iter().position(Option::is_none) else { return refuse("no room for another window") };
        let pixels = match taaj::memory_map(memory, false) {
            Ok(pixels) => pixels.cast::<u32>().cast_const(),
            Err(_) => return refuse("the window's memory cannot be read"),
        };
        let (id, response) = match self.desktop.open(request.title, request.width, request.height, true, None) {
            Ok(opened) => opened,
            Err(refusal) => {
                let _ = taaj::memory_unmap(pixels.cast_mut().cast());
                return refuse(refusal.describe());
            }
        };
        let client = Client { id, channel, memory, pixels, width: request.width, height: request.height };
        client.send(Event::Opened { id, format: self.screen.format });
        self.clients[slot] = Some(client);
        let frame = self.desktop.window(id).expect("just opened").frame();
        let title = self.title(id);
        println!(
            "[display] window {id} \"{}\" opened, {}x{} at ({}, {})",
            core::str::from_utf8(title.as_bytes()).unwrap_or("?"),
            request.width,
            request.height,
            frame.x,
            frame.y
        );
        self.apply(response);
    }

    /// A message on a program's window channel; false if the channel is
    /// closed - the program closed the window, or ended.
    fn request(&mut self, slot: usize) -> bool {
        let Some(client) = &self.clients[slot] else { return false };
        let (id, channel) = (client.id, client.channel);
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let n = match receive(channel, &mut message) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let Some(window) = self.desktop.window(id) else { return true };
        let content = window.content;
        match Request::decode(&message[..n]) {
            Some(Request::Damage(rect)) => {
                let rect = rect.clip(&Rect::new(0, 0, content.width, content.height));
                self.damage.add(rect.offset(content.x, content.y));
            }
            Some(Request::Title(title)) => {
                if let Some(bar) = self.desktop.set_title(id, title) {
                    self.damage.add(bar);
                }
            }
            None => {}
        }
        true
    }

    /// Closes a program's window: its handles, its mapping, its place.
    fn close(&mut self, slot: usize) {
        let Some(client) = self.clients[slot].take() else { return };
        let _ = taaj::memory_unmap(client.pixels.cast_mut().cast());
        let _ = taaj::close(client.memory);
        let _ = taaj::close(client.channel);
        println!("[display] window {} closed", client.id);
        let response = self.desktop.close(client.id);
        self.apply(response);
    }

    /// Everything damaged, to the screen.
    fn flush(&mut self) {
        let damage = core::mem::take(&mut self.damage);
        for rect in damage.rects() {
            self.compose(*rect);
        }
    }

    /// The desktop benchmark (K114, `display bench`): one composition of
    /// the whole screen - the most a frame can cost - and the terminal
    /// dragged by its title bar a pixel at a time, 100 to the right and
    /// back, each step through the same code a mouse's drag goes
    /// through (`Desktop::pointer`, the damage, the compositions). The
    /// drag's rate is what composing can do, not what a mouse sends:
    /// QEMU's mouse moves come far slower. The terminal ends where it
    /// was; the arrow is put back.
    fn bench(&mut self, terminal: Id, mouse: &mut Mouse) {
        let drawn = mouse.arrow.shown().is_some();
        mouse.arrow.hide(&mut self.screen.surface());

        const TIMES: usize = 21;
        let mut took = [0u64; TIMES];
        for time in took.iter_mut() {
            let start = taaj::monotonic_ns();
            self.compose(self.desktop.screen());
            *time = taaj::monotonic_ns() - start;
        }
        took.sort_unstable();
        let screen = self.desktop.screen();
        println!(
            "[display] one composition of the {}x{} screen: median {} us, best {} us ({TIMES} times)",
            screen.width,
            screen.height,
            took[TIMES / 2] / 1000,
            took[0] / 1000
        );

        const STEPS: i32 = 100;
        let frame = self.desktop.window(terminal).expect("never closed").frame();
        // On the title bar, clear of the close box at its right end.
        let (x, y) = (frame.x + 60, frame.y + (BORDER + TITLE_HEIGHT / 2) as i32);
        let start = taaj::monotonic_ns();
        let step = |server: &mut Server, x: i32, buttons: u8| {
            let response = server.desktop.pointer(x, y, buttons);
            server.apply(response);
            server.flush();
        };
        step(self, x, LEFT);
        for dx in (1..=STEPS).chain((0..STEPS).rev()) {
            step(self, x + dx, LEFT);
        }
        step(self, x, 0);
        let elapsed = (taaj::monotonic_ns() - start).max(1);
        let frames = 2 * STEPS as u64;
        println!(
            "[display] dragging a window a pixel at a time: {frames} frames in {} ms, {} frames/s",
            elapsed / 1_000_000,
            frames * 1_000_000_000 / elapsed
        );

        if drawn {
            mouse.arrow.show(&mut self.screen.surface(), mouse.x, mouse.y);
        }
    }

    /// Composes `rect` of the screen into the back buffer - desktop,
    /// then every window bottom to top - and copies it to the screen.
    fn compose(&mut self, rect: Rect) {
        let area = rect.clip(&self.desktop.screen());
        if area.is_empty() {
            return;
        }
        let focus = self.desktop.focus();
        // The top window that reaches into the area: if its content
        // covers all of it, nothing else can show there - copy just that.
        let top = self.desktop.stack().rev().find(|window| !window.frame().clip(&area).is_empty()).copied();
        {
            let Server { back, backdrop, theme, desktop, terminal, clients, .. } = self;
            let mut back = back.surface();
            let (mut region, area) = back.region(area);
            let content_of = |id: Id| content(terminal, clients, id);
            match top {
                Some(window) if window.content.clip(&area) == area => {
                    if let Some(image) = content_of(window.id) {
                        region.copy(&image, image.bounds(), window.content.x - area.x, window.content.y - area.y);
                    }
                }
                _ => {
                    backdrop.draw(&mut region, area);
                    for window in desktop.stack() {
                        if window.frame().clip(&area).is_empty() {
                            continue;
                        }
                        draw_frame(&mut region, window, focus == Some(window.id), theme, -area.x, -area.y);
                        if let Some(image) = content_of(window.id) {
                            region.copy(&image, image.bounds(), window.content.x - area.x, window.content.y - area.y);
                        }
                    }
                }
            }
        }
        let back = self.back.image();
        self.screen.surface().copy(&back, area, area.x, area.y);
    }
}

/// The pixels of window `id`: the terminal's, or a program's.
fn content<'a>(terminal: &'a Terminal, clients: &'a [Option<Client>], id: Id) -> Option<Image<'a>> {
    if id == terminal.id {
        Some(terminal.pixels.image())
    } else {
        clients.iter().flatten().find(|client| client.id == id).map(Client::image)
    }
}

fn main() -> i64 {
    let (Some(memory), Some(messages), Some(keys_in), Some(shell_keys), Some(windows)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
        taaj::startup_handle(5),
    ) else {
        println!("[display] started without its handles");
        return 1;
    };
    let mut pings = taaj::startup_handle(2);
    let mut message = [0u8; abi::MESSAGE_MAX as usize];

    // INFO first. Anything before it is text from a previous display's
    // time, with no screen yet to show it on.
    let fb = loop {
        match receive(messages, &mut message) {
            Ok(n) if n > 0 && message[0] == abi::display::INFO => match FramebufferInfo::from_bytes(&message[1..n]) {
                Some(fb) => break fb,
                None => return 2,
            },
            Ok(_) => continue,
            Err(_) => return 2,
        }
    };
    if fb.bpp != 32 || fb.width == 0 || fb.height == 0 || fb.pitch % 4 != 0 || fb.pitch / 4 < fb.width {
        println!("[display] {} bits per pixel: not supported yet", fb.bpp);
        return 3;
    }
    let base = match taaj::memory_map(memory, true) {
        Ok(base) => base,
        Err(error) => {
            println!("[display] cannot map the framebuffer: {error:?}");
            return 4;
        }
    };
    let format = Format {
        red_size: fb.red_size,
        red_shift: fb.red_shift,
        green_size: fb.green_size,
        green_shift: fb.green_shift,
        blue_size: fb.blue_size,
        blue_shift: fb.blue_shift,
    };
    let scale = if fb.width >= 800 { 2 } else { 1 };
    let band_end = fb.height / 12;
    let title_scale = 2 * scale as u32;
    let backdrop = Backdrop {
        height: fb.height,
        band_end,
        band: format.pixel(0xE8, 0xA8, 0x38),
        title_x: 16 * scale as i32,
        title_y: (band_end.saturating_sub(font::HEIGHT as u32 * title_scale) / 2) as i32,
        title_scale,
        navy: format.pixel(0x10, 0x18, 0x30),
        format,
    };

    // The terminal fills the screen below the band, with a margin all
    // round: as many whole cells as fit in its frame.
    let (cell_width, cell_height) = ((font::WIDTH + 1) * scale, (font::HEIGHT + 2) * scale);
    let margin = 8 * scale as u32;
    let pad = 3 * scale;
    let frame_top = band_end + margin;
    let room_x = fb.width.saturating_sub(2 * margin + 2 * BORDER) as usize - 2 * pad;
    let room_y = fb.height.saturating_sub(frame_top + margin + TITLE_HEIGHT + 2 * BORDER) as usize - 2 * pad;
    let columns = (room_x / cell_width).clamp(1, MAX_COLUMNS);
    let rows = (room_y / cell_height).clamp(1, MAX_ROWS);
    let (content_width, content_height) = ((columns * cell_width + 2 * pad) as u32, (rows * cell_height + 2 * pad) as u32);
    let frame_x = ((fb.width - (content_width + 2 * BORDER)) / 2) as i32;

    let (Some(back), Some(term_pixels)) = (Pixels::new(fb.width, fb.height), Pixels::new(content_width, content_height)) else {
        println!("[display] no memory for the screen's pixels");
        return 4;
    };
    let mut desktop = Desktop::new(fb.width, fb.height);
    let Ok((terminal_id, _)) = desktop.open(b"terminal", content_width, content_height, false, Some((frame_x, frame_top as i32))) else {
        println!("[display] the terminal does not fit on a {}x{} screen", fb.width, fb.height);
        return 3;
    };
    let mut server = Server {
        screen: Screen { fb, base, format },
        back,
        backdrop,
        theme: Theme::new(format),
        desktop,
        terminal: Terminal {
            id: terminal_id,
            pixels: term_pixels,
            scale,
            pad,
            cell_width,
            cell_height,
            text: format.pixel(0xE0, 0xE8, 0xF0),
            background: format.pixel(0x10, 0x18, 0x30),
        },
        clients: [const { None }; WINDOWS_MAX],
        shell_keys,
        damage: Damage::default(),
        key_times: KeyTimes::new(),
        modifiers: 0,
    };

    // Where the text lives. If init gave memory for it, there: that
    // memory is init's, and is still there when this process is not -
    // so a display that replaces one that crashed or hung finds the
    // terminal's text as it was, and shows it again.
    let kept = taaj::startup_handle(6)
        .filter(|&memory| taaj::memory_size(memory).is_ok_and(|size| size >= core::mem::size_of::<Grid>() as u64))
        .and_then(|memory| taaj::memory_map(memory, true).ok());
    let grid: &mut Grid = match kept {
        // SAFETY: a mapping at least as big as a Grid, page-aligned,
        // this process's alone while it runs; and every field of Grid is
        // a plain integer, so whatever bytes are there are *a* Grid -
        // whether one to trust is `adopt`'s question, below.
        Some(base) => unsafe { &mut *base.cast::<Grid>() },
        // SAFETY: the only reference to GRID, taken once; one thread.
        None => unsafe { (&raw mut GRID).as_mut() }.expect("a static is never null"),
    };
    let found = kept.is_some() && grid.adopt(columns, rows);
    if !found {
        *grid = Grid::new(columns, rows);
    }
    // SAFETY: as for GRID.
    let shown = unsafe { (&raw mut SHOWN).as_mut() }.expect("a static is never null");
    // The margin round the cells is never drawn by a cell: paint it once.
    let background = server.terminal.background;
    server.terminal.pixels.surface().fill(Rect::new(0, 0, content_width, content_height), background);
    server.terminal.sync(grid, shown);
    server.compose(server.desktop.screen());
    println!(
        "[display] painted the {}x{} screen from ring 3 (framebuffer at {base:p}); console {columns}x{rows}",
        fb.width, fb.height
    );
    let frame = server.desktop.window(terminal_id).expect("just opened").frame();
    println!("[display] window {terminal_id} \"terminal\" opened, {content_width}x{content_height} at ({}, {})", frame.x, frame.y);
    if found && grid.rows_used() > 0 {
        println!("[display] the text on the screen was kept: {} lines shown again", grid.rows_used());
    }
    // The pointer starts in the middle; it is drawn once something
    // moves it - a screen with no mouse shows no arrow.
    let mut mouse = Mouse {
        x: fb.width as i32 / 2,
        y: fb.height as i32 / 2,
        buttons: 0,
        arrow: Pointer::new(format.pixel(0, 0, 0), format.pixel(0xFF, 0xFF, 0xFF)),
    };

    // The fixed handles waited on, then each program's window.
    const FIXED: usize = 4;
    loop {
        // First, between batches, whoever asks if this loop is turning.
        if let Some(channel) = pings {
            if !taaj::answer_pings(channel) {
                pings = None;
            }
        }
        let mut waited = [0 as Handle; FIXED + WINDOWS_MAX];
        let mut slots = [0usize; FIXED + WINDOWS_MAX];
        waited[..3].copy_from_slice(&[messages, keys_in, windows]);
        let mut count = 3;
        if let Some(channel) = pings {
            waited[count] = channel;
            count += 1;
        }
        let fixed = count;
        for (slot, client) in server.clients.iter().enumerate() {
            if let Some(client) = client {
                waited[count] = client.channel;
                slots[count] = slot;
                count += 1;
            }
        }
        let waited = &waited[..count];
        if taaj::object_wait(waited).is_err() {
            return 5;
        }
        // Everything already waiting, up to BATCH, before any drawing.
        let mut taken = 0;
        let mut stall = 0;
        let mut moved = false;
        let mut bench = false;
        while taken < BATCH {
            let Ok(Some(ready)) = taaj::poll(waited) else { break };
            taken += 1;
            if ready == 0 {
                let n = match receive(messages, &mut message) {
                    Ok(n) => n,
                    Err(Error::PeerClosed) => return 0,
                    Err(_) => return 5,
                };
                if n > 0 && message[0] == abi::display::TEXT {
                    grid.write_all(&message[1..n], &mut |_| {});
                    server.key_times.text(&message[1..n]);
                }
                if let Some(report) = Report::from_message(&message[..n]) {
                    mouse.take(report, fb.width, fb.height);
                    moved = true;
                    let response = server.desktop.pointer(mouse.x, mouse.y, mouse.buttons);
                    server.apply(response);
                    if report.wheel != 0 {
                        server.wheel(mouse.x, mouse.y, report.wheel);
                    }
                }
                if n == 9 && message[0] == abi::display::STALL {
                    let mut ticks = [0u8; 8];
                    ticks.copy_from_slice(&message[1..9]);
                    stall = u64::from_le_bytes(ticks).min(abi::STALL_MAX);
                }
                if n == abi::display::KEY_SIZE && message[0] == abi::display::KEY && message[3] <= 1 {
                    server.key_code(u16::from_le_bytes([message[1], message[2]]), message[3] == 1);
                }
                if n == 1 && message[0] == abi::display::BENCH {
                    bench = true;
                }
                if n == 5 && message[0] == abi::display::KEYS {
                    let mut count = [0u8; 4];
                    count.copy_from_slice(&message[1..5]);
                    server.key_times.start(u32::from_le_bytes(count).min(abi::display::KEYS_MAX));
                }
            } else if ready == 1 {
                match receive(keys_in, &mut message) {
                    Ok(n) if n > 0 => server.keys(&message[..n]),
                    Ok(_) => {}
                    Err(_) => return 5,
                }
            } else if ready == 2 {
                let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
                match taaj::receive(windows, &mut message, &mut handles) {
                    Ok((n, given)) => server.open(&message[..n], &handles[..given]),
                    Err(_) => return 5,
                }
            } else if ready < fixed {
                // A ping: answered at the top of the loop.
                break;
            } else if !server.request(slots[ready]) {
                server.close(slots[ready]);
                // The list of handles changed: wait again with the new one.
                break;
            }
        }
        // The terminal's changed cells, then everything damaged, to the
        // screen - the arrow off while that is drawn, back on after.
        let changed = server.terminal.sync(grid, shown);
        if !changed.is_empty() {
            let content = server.desktop.window(terminal_id).expect("never closed").content;
            server.damage.add(changed.offset(content.x, content.y));
        }
        let damage = core::mem::take(&mut server.damage);
        let drawn = mouse.arrow.shown().is_some();
        if !damage.is_empty() || moved {
            mouse.arrow.hide(&mut server.screen.surface());
            for rect in damage.rects() {
                server.compose(*rect);
            }
            if drawn || moved {
                mouse.arrow.show(&mut server.screen.surface(), mouse.x, mouse.y);
            }
        }
        // What this batch drew is on the screen: keys whose echo it
        // drew have reached it.
        server.key_times.shown();
        if bench {
            server.bench(terminal_id, &mut mouse);
        }
        // The test of a display that has stopped: what was asked for
        // is on the screen, and then nothing is done.
        if stall > 0 {
            println!("[display] asked to stall: drawing nothing for {stall} ticks");
            taaj::sleep(stall);
            println!("[display] drawing again");
        }
    }
}

/// The next message's length. Handles are not part of these messages,
/// but a message may bring some - and one that did not fit would stay
/// at the head of the queue, refused again forever. So they are taken,
/// and closed.
fn receive(channel: Handle, message: &mut [u8]) -> Result<usize, Error> {
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let (n, count) = taaj::receive(channel, message, &mut handles)?;
    for &handle in &handles[..count] {
        let _ = taaj::close(handle);
    }
    Ok(n)
}
