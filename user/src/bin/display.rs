//! The display service: owns the screen, from ring 3.
//!
//! Startup handles, from init:
//!   1  the framebuffer's memory object (READ and WRITE)
//!   2  a channel of display messages (abi::display): first INFO - the
//!      framebuffer's description - then TEXT from whoever was given
//!      the sending end (the shell)
//!
//! It maps the framebuffer - the same physical pixels the graphics card
//! scans out - paints the background, and runs a text console on it:
//! the `console` crate keeps the grid of characters, this draws them
//! with TheTaaJ's own font. It can touch nothing but the screen, and can
//! crash without taking the kernel along.
//!
//! Drawing is the slow part, so it is done as seldom as possible:
//!
//!   - The grid is what the screen *should* show; `SHOWN` is what it
//!     *does* show. Drawing is bringing the second in line with the
//!     first: only cells that differ are painted. A scroll moves every
//!     line, but most cells of most lines are blank before and after.
//!   - Text is taken as fast as it comes: every message already waiting
//!     goes into the grid before anything is drawn. A program printing
//!     a thousand lines costs a few screens' worth of drawing, not a
//!     thousand - lines that scroll past between two paints are never
//!     drawn at all, as in any terminal.

#![no_std]
#![no_main]

use abi::FramebufferInfo;
use console::{font, Grid, MAX_COLUMNS, MAX_ROWS};
use taaj::{println, Error};

taaj::program!(main);

/// The grid lives in static memory: at 38 KiB it would crowd the 64 KiB
/// stack.
static mut GRID: Grid = Grid::new(MAX_COLUMNS, MAX_ROWS);

/// What is on the screen now: the character drawn in each cell, and
/// where the cursor is drawn. 0 is "nothing drawn yet", which no cell of
/// the grid ever holds - so the first paint draws everything.
struct Shown {
    cells: [[u8; MAX_COLUMNS]; MAX_ROWS],
    cursor: (usize, usize),
}
static mut SHOWN: Shown = Shown { cells: [[0; MAX_COLUMNS]; MAX_ROWS], cursor: (usize::MAX, usize::MAX) };

/// Most messages taken in before the screen is brought up to date: a
/// program that never stops printing must not keep the screen frozen.
const BATCH: usize = 64;

struct Screen {
    fb: FramebufferInfo,
    base: *mut u8,
    /// Each font pixel is scale x scale screen pixels.
    scale: usize,
    /// Where the grid starts, and the size of one cell.
    left: usize,
    top: usize,
    cell_width: usize,
    cell_height: usize,
    text: u32,
}

impl Screen {
    /// Builds a pixel value from 8-bit red, green and blue, placing each
    /// channel where *this* framebuffer wants it. Hard-coding 0x00RRGGBB
    /// would work in QEMU and show wrong colours on hardware that orders
    /// the channels differently.
    fn pixel(&self, red: u8, green: u8, blue: u8) -> u32 {
        let channel = |value: u8, size: u8, shift: u8| -> u32 {
            (u32::from(value) >> (8 - u32::from(size.min(8)))) << shift
        };
        channel(red, self.fb.red_size, self.fb.red_shift)
            | channel(green, self.fb.green_size, self.fb.green_shift)
            | channel(blue, self.fb.blue_size, self.fb.blue_shift)
    }

    fn band(&self) -> core::ops::Range<u32> {
        self.fb.height / 3..self.fb.height / 3 + self.fb.height / 12
    }

    /// The background at line `y`: dark navy shading to deep teal, with
    /// an amber band a third of the way down. A function of y alone, so
    /// erasing anything is just painting this again.
    fn background(&self, y: usize) -> u32 {
        if self.band().contains(&(y as u32)) {
            self.pixel(0xE8, 0xA8, 0x38)
        } else {
            let t = (y as u32 * 255 / self.fb.height) as u8;
            self.pixel(0x10, 0x18 + t / 6, 0x30 + t / 4)
        }
    }

    fn put(&self, x: usize, y: usize, colour: u32) {
        if x < self.fb.width as usize && y < self.fb.height as usize {
            // SAFETY: inside the mapped framebuffer (checked above).
            // Volatile: device memory, writes must all happen.
            unsafe { (self.base.add(y * self.fb.pitch as usize) as *mut u32).add(x).write_volatile(colour) };
        }
    }

    fn paint_background(&self) {
        for y in 0..self.fb.height as usize {
            let colour = self.background(y);
            for x in 0..self.fb.width as usize {
                self.put(x, y, colour);
            }
        }
    }

    /// Draws `c` with its top-left corner at (x, y), `scale` times size.
    fn glyph(&self, c: u8, x: usize, y: usize, scale: usize, colour: u32) {
        for gy in 0..font::HEIGHT * scale {
            for gx in 0..font::WIDTH * scale {
                if font::pixel(c, gx / scale, gy / scale) {
                    self.put(x + gx, y + gy, colour);
                }
            }
        }
    }

    /// Redraws one cell of the grid, with the cursor if it is there.
    ///
    /// Each pixel is written once - text colour where the glyph (or the
    /// cursor's underline) has a dot, background elsewhere - a row at a
    /// time through a pointer to the row's start, rather than painting
    /// the background and then the glyph over it, pixel by checked pixel.
    fn cell(&self, column: usize, row: usize, c: u8, cursor: bool) {
        let (x0, y0) = (self.left + column * self.cell_width, self.top + row * self.cell_height);
        if x0 + self.cell_width > self.fb.width as usize || y0 + self.cell_height > self.fb.height as usize {
            return; // not on the screen: the grid was sized to fit, but check
        }
        let glyph = font::glyph(c);
        for dy in 0..self.cell_height {
            let font_row = dy / self.scale;
            let background = self.background(y0 + dy);
            // The glyph's rows, then one row of underline for the cursor.
            let bits = match font_row {
                r if r < font::HEIGHT => glyph[r],
                r if r == font::HEIGHT && cursor => 0xFF,
                _ => 0,
            };
            // SAFETY: row y0 + dy, columns x0 .. x0 + cell_width: inside
            // the mapped framebuffer, checked above. Volatile: device
            // memory, every write must happen.
            let line = unsafe { (self.base.add((y0 + dy) * self.fb.pitch as usize) as *mut u32).add(x0) };
            for dx in 0..self.cell_width {
                let font_column = dx / self.scale;
                let lit = font_column < font::WIDTH && bits & (1 << (font::WIDTH - 1 - font_column)) != 0;
                unsafe { line.add(dx).write_volatile(if lit { self.text } else { background }) };
            }
        }
    }

    /// Brings the screen in line with the grid: paints the cells that
    /// differ from what is shown, and moves the cursor. Returns how
    /// many cells it painted.
    fn sync(&self, grid: &Grid, shown: &mut Shown) -> usize {
        let (columns, rows) = grid.size();
        let cursor = grid.cursor();
        let mut painted = 0;
        for row in 0..rows {
            for column in 0..columns {
                let c = grid.at(column, row);
                let here = (column, row);
                // A cell is stale if its character changed, or if the
                // cursor arrived or left.
                if shown.cells[row][column] != c || (here == cursor) != (here == shown.cursor) {
                    self.cell(column, row, c, here == cursor);
                    shown.cells[row][column] = c;
                    painted += 1;
                }
            }
        }
        shown.cursor = cursor;
        painted
    }
}

fn main() -> i64 {
    let (Some(memory), Some(messages)) = (taaj::startup_handle(0), taaj::startup_handle(1)) else {
        return 1;
    };
    let mut message = [0u8; abi::MESSAGE_MAX as usize];

    // INFO first. Anything before it is text from a previous display's
    // time, with no screen yet to show it on.
    let fb = loop {
        match taaj::receive(messages, &mut message, &mut []) {
            Ok((n, _)) if n > 0 && message[0] == abi::display::INFO => {
                match FramebufferInfo::from_bytes(&message[1..n]) {
                    Some(fb) => break fb,
                    None => return 2,
                }
            }
            Ok(_) => continue,
            Err(_) => return 2,
        }
    };
    if fb.bpp != 32 || fb.width == 0 || fb.height == 0 {
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

    let scale = if fb.width >= 800 { 2 } else { 1 };
    let mut screen = Screen {
        fb,
        base,
        scale,
        left: 16 * scale,
        top: 0,
        cell_width: (font::WIDTH + 1) * scale,
        cell_height: (font::HEIGHT + 2) * scale,
        text: 0,
    };
    screen.text = screen.pixel(0xE0, 0xE8, 0xF0);
    screen.top = screen.band().end as usize + 8 * scale;
    screen.paint_background();

    // The name, in the band, in the navy of the top of the screen.
    let title_scale = 2 * scale;
    let band = screen.band();
    let title_y = band.start as usize + (band.len().saturating_sub(font::HEIGHT * title_scale)) / 2;
    let navy = screen.pixel(0x10, 0x18, 0x30);
    for (n, &c) in b"TheTaaJ".iter().enumerate() {
        screen.glyph(c, screen.left + n * (font::WIDTH + 1) * title_scale, title_y, title_scale, navy);
    }

    let columns = (fb.width as usize - 2 * screen.left) / screen.cell_width;
    let rows = (fb.height as usize - screen.top - 8 * scale) / screen.cell_height;
    // SAFETY: the only reference to GRID, taken once; one thread.
    let grid = unsafe { (&raw mut GRID).as_mut() }.expect("a static is never null");
    *grid = Grid::new(columns, rows);
    let (columns, rows) = grid.size();
    // SAFETY: as for GRID.
    let shown = unsafe { (&raw mut SHOWN).as_mut() }.expect("a static is never null");
    screen.sync(grid, shown);
    println!(
        "[display] painted the {}x{} screen from ring 3 (framebuffer at {base:p}); console {columns}x{rows}",
        fb.width, fb.height
    );

    loop {
        // Wait for text; then take whatever else is already waiting,
        // up to BATCH messages, before drawing any of it.
        let mut taken = 0;
        loop {
            let n = match taaj::receive(messages, &mut message, &mut []) {
                Ok((n, _)) => n,
                Err(Error::PeerClosed) => return 0,
                Err(_) => return 5,
            };
            if n > 0 && message[0] == abi::display::TEXT {
                grid.write_all(&message[1..n], &mut |_| {});
            }
            taken += 1;
            // More, right now? (A closed channel also counts as ready:
            // the receive above then ends the service.)
            if taken == BATCH || !matches!(taaj::poll(&[messages]), Ok(Some(_))) {
                break;
            }
        }
        screen.sync(grid, shown);
    }
}
