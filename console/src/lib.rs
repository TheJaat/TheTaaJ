//! A text console: a font, and a grid of characters with a cursor.
//!
//! Two parts, both pure logic, so they are tested here on the host and
//! the display service only has to turn cells into pixels:
//!
//!   - the font: TheTaaJ's own 5x8 bitmap font for printable ASCII,
//!     drawn by hand in font.txt and turned into bits at compile time
//!   - the grid: what a terminal does with a stream of bytes - print,
//!     wrap at the right edge, scroll at the bottom, and the control
//!     codes a shell sends: newline, carriage return, backspace (move
//!     left, no erasing - "\x08 \x08" erases), tab, form feed (clear).
//!
//! The grid reports what changed through a callback, so the screen is
//! redrawn only where it must be.

#![cfg_attr(not(test), no_std)]

pub mod font {
    /// Glyph size in pixels.
    pub const WIDTH: usize = 5;
    pub const HEIGHT: usize = 8;

    const SOURCE: &[u8] = include_bytes!("font.txt");
    const LINE: usize = 2 + 8 * (WIDTH + 1); // "c " + 8 rows of 5 and a separator

    /// Bits for 0x20..=0x7E. In each row, bit 4 is the leftmost pixel.
    static GLYPHS: [[u8; HEIGHT]; 95] = parse();

    /// Reads font.txt at compile time. A malformed line, or one out of
    /// order, stops the build rather than drawing garbage.
    const fn parse() -> [[u8; HEIGHT]; 95] {
        assert!(SOURCE.len() == 95 * LINE, "font.txt: 95 lines of 49 characters");
        let mut glyphs = [[0u8; HEIGHT]; 95];
        let mut g = 0;
        while g < 95 {
            let line = g * LINE;
            assert!(SOURCE[line] == 0x20 + g as u8, "font.txt: characters out of order");
            let mut row = 0;
            while row < HEIGHT {
                let mut bits = 0u8;
                let mut x = 0;
                while x < WIDTH {
                    bits <<= 1;
                    match SOURCE[line + 2 + row * (WIDTH + 1) + x] {
                        b'#' => bits |= 1,
                        b'.' => {}
                        _ => panic!("font.txt: pixels are '#' or '.'"),
                    }
                    x += 1;
                }
                glyphs[g][row] = bits;
                row += 1;
            }
            g += 1;
        }
        glyphs
    }

    /// The glyph for `c`; anything not printable ASCII shows as '?'.
    pub fn glyph(c: u8) -> &'static [u8; HEIGHT] {
        let c = if (0x20..=0x7E).contains(&c) { c } else { b'?' };
        &GLYPHS[(c - 0x20) as usize]
    }

    /// Whether pixel (x, y) of `c`'s glyph is set.
    pub fn pixel(c: u8, x: usize, y: usize) -> bool {
        x < WIDTH && y < HEIGHT && glyph(c)[y] & (1 << (WIDTH - 1 - x)) != 0
    }
}

/// Largest grid: enough for 1920x1080 at the smallest cell size.
pub const MAX_COLUMNS: usize = 320;
pub const MAX_ROWS: usize = 120;

/// What a write changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    /// One cell now holds `character`.
    Cell { column: usize, row: usize, character: u8 },
    /// Everything moved (a scroll) or went (a clear): redraw it all.
    Scrolled,
}

pub struct Grid {
    cells: [[u8; MAX_COLUMNS]; MAX_ROWS],
    columns: usize,
    rows: usize,
    column: usize,
    row: usize,
    /// Inside a UTF-8 sequence: its remaining continuation bytes.
    continuation: u8,
}

impl Grid {
    /// An empty grid, clamped to the maximum size.
    pub const fn new(columns: usize, rows: usize) -> Grid {
        let columns = if columns > MAX_COLUMNS { MAX_COLUMNS } else if columns == 0 { 1 } else { columns };
        let rows = if rows > MAX_ROWS { MAX_ROWS } else if rows == 0 { 1 } else { rows };
        Grid { cells: [[b' '; MAX_COLUMNS]; MAX_ROWS], columns, rows, column: 0, row: 0, continuation: 0 }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.column, self.row)
    }

    pub fn at(&self, column: usize, row: usize) -> u8 {
        self.cells[row][column]
    }

    /// One byte of a terminal stream.
    pub fn write(&mut self, byte: u8, damage: &mut impl FnMut(Damage)) {
        // UTF-8: the font has ASCII only, so a multi-byte character is
        // one '?' - shown for its first byte, its continuations skipped.
        if self.continuation > 0 && byte & 0xC0 == 0x80 {
            self.continuation -= 1;
            return;
        }
        self.continuation = 0;
        match byte {
            b'\n' => self.newline(damage),
            b'\r' => self.column = 0,
            0x0C => {
                // Form feed: a new page - clear, cursor home.
                for row in &mut self.cells[..self.rows] {
                    *row = [b' '; MAX_COLUMNS];
                }
                (self.column, self.row) = (0, 0);
                damage(Damage::Scrolled);
            }
            0x08 => self.column = self.column.saturating_sub(1),
            b'\t' => {
                let stop = ((self.column / 8 + 1) * 8).min(self.columns);
                while self.column < stop {
                    self.put(b' ', damage);
                }
            }
            0x00..=0x1F | 0x7F => {} // other control codes: ignored
            0xC0..=0xF7 => {
                self.continuation = if byte >= 0xF0 { 3 } else if byte >= 0xE0 { 2 } else { 1 };
                self.put(b'?', damage);
            }
            _ => self.put(byte, damage),
        }
    }

    pub fn write_all(&mut self, bytes: &[u8], damage: &mut impl FnMut(Damage)) {
        for &b in bytes {
            self.write(b, damage);
        }
    }

    fn put(&mut self, c: u8, damage: &mut impl FnMut(Damage)) {
        if self.column == self.columns {
            self.newline(damage); // wrap
        }
        self.cells[self.row][self.column] = c;
        damage(Damage::Cell { column: self.column, row: self.row, character: c });
        self.column += 1;
    }

    fn newline(&mut self, damage: &mut impl FnMut(Damage)) {
        self.column = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        // At the bottom: everything up one line, a blank line below.
        self.cells.copy_within(1..self.rows, 0);
        self.cells[self.rows - 1] = [b' '; MAX_COLUMNS];
        damage(Damage::Scrolled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(g: &Grid, row: usize) -> String {
        let (columns, _) = g.size();
        (0..columns).map(|c| g.at(c, row) as char).collect::<String>().trim_end().to_string()
    }

    fn write(g: &mut Grid, s: &str) -> Vec<Damage> {
        let mut damage = Vec::new();
        g.write_all(s.as_bytes(), &mut |d| damage.push(d));
        damage
    }

    #[test]
    fn the_font_has_every_printable_character() {
        // A few glyphs whose shape is easy to state.
        assert_eq!(font::glyph(b' '), &[0; 8], "space is empty");
        assert_eq!(font::glyph(b'|'), &[0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0]);
        assert_eq!(font::glyph(b'-')[3], 0b11111);
        assert_eq!(font::glyph(b'_')[7], 0b11111, "underscore on the bottom row");
        assert!(font::pixel(b'L', 0, 0) && font::pixel(b'L', 4, 6) && !font::pixel(b'L', 4, 0));
        // Every other glyph has some ink, and no two are the same.
        let glyphs: Vec<_> = (0x21..=0x7Eu8).map(font::glyph).collect();
        assert!(glyphs.iter().all(|g| g.iter().any(|&row| row != 0)));
        for (i, a) in glyphs.iter().enumerate() {
            for (j, b) in glyphs.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{:?} and {:?} look the same", (0x21 + i) as u8 as char, (0x21 + j) as u8 as char);
            }
        }
        assert_eq!(font::glyph(0x7F), font::glyph(b'?'), "unprintable shows as ?");
    }

    #[test]
    fn text_flows_and_wraps() {
        let mut g = Grid::new(5, 3);
        let damage = write(&mut g, "ab");
        assert_eq!(
            damage,
            vec![
                Damage::Cell { column: 0, row: 0, character: b'a' },
                Damage::Cell { column: 1, row: 0, character: b'b' }
            ]
        );
        write(&mut g, "cdefg");
        assert_eq!(row_text(&g, 0), "abcde");
        assert_eq!(row_text(&g, 1), "fg", "wrapped at the right edge");
        assert_eq!(g.cursor(), (2, 1));
    }

    #[test]
    fn newline_at_the_bottom_scrolls() {
        let mut g = Grid::new(10, 2);
        write(&mut g, "one\ntwo");
        let damage = write(&mut g, "\nthree");
        assert_eq!(damage[0], Damage::Scrolled);
        assert_eq!(row_text(&g, 0), "two");
        assert_eq!(row_text(&g, 1), "three");
    }

    #[test]
    fn backspace_moves_and_space_erases() {
        let mut g = Grid::new(10, 2);
        write(&mut g, "abc\x08 \x08");
        assert_eq!(row_text(&g, 0), "ab");
        assert_eq!(g.cursor(), (2, 0));
        write(&mut g, "\x08\x08\x08\x08");
        assert_eq!(g.cursor(), (0, 0), "not past the start of the line");
        write(&mut g, "xy\rZ");
        assert_eq!(row_text(&g, 0), "Zy", "carriage return goes back to column 0");
    }

    #[test]
    fn form_feed_clears() {
        let mut g = Grid::new(10, 3);
        write(&mut g, "one\ntwo");
        assert_eq!(write(&mut g, "\x0c"), vec![Damage::Scrolled]);
        assert_eq!((row_text(&g, 0), row_text(&g, 1), g.cursor()), (String::new(), String::new(), (0, 0)));
    }

    #[test]
    fn tabs_and_unicode_and_control_codes() {
        let mut g = Grid::new(20, 2);
        write(&mut g, "a\tb");
        assert_eq!(g.at(8, 0), b'b');
        let mut g = Grid::new(20, 2);
        write(&mut g, "é€😀!\x07\x1b");
        assert_eq!(row_text(&g, 0), "???!", "one ? per character; bell and escape ignored");
    }

    #[test]
    fn silly_sizes_are_clamped() {
        assert_eq!(Grid::new(0, 0).size(), (1, 1));
        assert_eq!(Grid::new(10_000, 10_000).size(), (MAX_COLUMNS, MAX_ROWS));
        let mut g = Grid::new(1, 1);
        write(&mut g, "abc"); // wraps and scrolls every character
        assert_eq!(row_text(&g, 0), "c");
    }
}
