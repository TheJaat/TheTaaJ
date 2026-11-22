//! Drawing, with no machine in it.
//!
//! A screen is memory: a row of pixels, then the next row, each pixel
//! a 32-bit number whose bits say how much red, green and blue. This
//! crate draws into such memory - the framebuffer itself, or any buffer
//! shaped like one - and knows nothing of where it came from. So every
//! function here runs as an ordinary test on the host, against a Vec.
//!
//!   - `Format`: where red, green and blue sit in a pixel's 32 bits.
//!     The PC's screen and QEMU's AArch64 one happen to agree; a real
//!     card may not.
//!   - `Rect`: a rectangle, and the one operation everything else
//!     leans on - cutting it down to what is inside another (`clip`).
//!   - `Surface`: pixels to draw on: fill, put, copy a rectangle from an
//!     `Image` (pixels to read), text in TheTaaJ's font.
//!   - `Pointer`: the arrow that follows the mouse. It saves what it
//!     covers before it draws, and puts it back when it moves, so
//!     nothing under it has to be redrawn.
//!
//! Coordinates are signed: a rectangle may start left of the screen or
//! above it, and is cut at the edge - a pointer half off the screen, or
//! (later) a window dragged partly out of sight.

#![cfg_attr(not(test), no_std)]

use console::font;

/// Where each colour lives in a pixel: how many bits, and how far up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Format {
    pub red_size: u8,
    pub red_shift: u8,
    pub green_size: u8,
    pub green_shift: u8,
    pub blue_size: u8,
    pub blue_shift: u8,
}

impl Format {
    /// 0x00RRGGBB: eight bits each, red at the top. What QEMU's screens
    /// use on both machines (DRM calls it XRGB8888).
    pub const XRGB8888: Format = Format { red_size: 8, red_shift: 16, green_size: 8, green_shift: 8, blue_size: 8, blue_shift: 0 };

    /// The pixel for 8-bit red, green and blue, each channel cut to its
    /// size and put where this format wants it.
    pub fn pixel(&self, red: u8, green: u8, blue: u8) -> u32 {
        let channel = |value: u8, size: u8, shift: u8| -> u32 {
            let size = u32::from(size.min(8));
            if size == 0 {
                return 0;
            }
            (u32::from(value) >> (8 - size)) << shift
        };
        channel(red, self.red_size, self.red_shift)
            | channel(green, self.green_size, self.green_shift)
            | channel(blue, self.blue_size, self.blue_shift)
    }
}

/// A rectangle: its top-left corner, and its size. Empty if either size
/// is 0.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// One past the right edge, and one past the bottom. In i64: a
    /// rectangle near i32::MAX must not wrap round to the left.
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && i64::from(x) < self.right() && y >= self.y && i64::from(y) < self.bottom()
    }

    /// The part of this rectangle that is also inside `other` - empty
    /// (all zeros) if they do not meet.
    pub fn clip(&self, other: &Rect) -> Rect {
        let left = i64::from(self.x.max(other.x));
        let top = i64::from(self.y.max(other.y));
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return Rect::default();
        }
        Rect { x: left as i32, y: top as i32, width: (right - left) as u32, height: (bottom - top) as u32 }
    }

    /// The same rectangle, moved.
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x.saturating_add(dx), y: self.y.saturating_add(dy), ..*self }
    }
}

/// Pixels to read from: `width` x `height`, each row starting `stride`
/// pixels after the one before (a screen's rows may be longer than what
/// it shows).
#[derive(Clone, Copy)]
pub struct Image<'a> {
    pixels: &'a [u32],
    width: u32,
    height: u32,
    stride: usize,
}

impl<'a> Image<'a> {
    /// None if `pixels` is too short for the size and stride said.
    pub fn new(pixels: &'a [u32], width: u32, height: u32, stride: usize) -> Option<Image<'a>> {
        fits(pixels.len(), width, height, stride).then_some(Image { pixels, width, height, stride })
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        self.bounds().contains(x, y).then(|| self.pixels[y as usize * self.stride + x as usize])
    }
}

/// Whether `length` pixels hold `height` rows of `width`, `stride` apart.
fn fits(length: usize, width: u32, height: u32, stride: usize) -> bool {
    if height == 0 || width == 0 {
        return true;
    }
    (width as usize) <= stride
        && (height as usize - 1).checked_mul(stride).and_then(|rows| rows.checked_add(width as usize)).is_some_and(|needed| needed <= length)
}

/// Pixels to draw on.
pub struct Surface<'a> {
    pixels: &'a mut [u32],
    width: u32,
    height: u32,
    stride: usize,
}

impl<'a> Surface<'a> {
    /// None if `pixels` is too short for the size and stride said.
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32, stride: usize) -> Option<Surface<'a>> {
        fits(pixels.len(), width, height, stride).then_some(Surface { pixels, width, height, stride })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// The same pixels, to read.
    pub fn image(&self) -> Image<'_> {
        Image { pixels: self.pixels, width: self.width, height: self.height, stride: self.stride }
    }

    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        self.image().get(x, y)
    }

    /// One pixel; nothing if it is off the surface.
    pub fn put(&mut self, x: i32, y: i32, pixel: u32) {
        if self.bounds().contains(x, y) {
            self.pixels[y as usize * self.stride + x as usize] = pixel;
        }
    }

    /// The pixels of row `y` from column `x`, `count` of them - inside
    /// the surface: callers clip first.
    fn row(&mut self, x: i32, y: i32, count: u32) -> &mut [u32] {
        let start = y as usize * self.stride + x as usize;
        &mut self.pixels[start..start + count as usize]
    }

    /// Fills the part of `rect` that is on the surface.
    pub fn fill(&mut self, rect: Rect, pixel: u32) {
        let area = rect.clip(&self.bounds());
        for y in area.y..area.y + area.height as i32 {
            self.row(area.x, y, area.width).fill(pixel);
        }
    }

    /// Copies `from` (a rectangle of `source`) so that its top-left
    /// corner lands at (x, y). Whatever falls outside either - off the
    /// source, or off this surface - is left out, and the rest stays
    /// where it would have been.
    pub fn copy(&mut self, source: &Image, from: Rect, x: i32, y: i32) {
        // How far everything moves - from the rectangle as asked, not as
        // cut: a corner off the source does not move the rest. Then what
        // of `from` exists, where it lands, cut to this surface, and
        // back to the source by the same offset.
        let (dx, dy) = (i64::from(x) - i64::from(from.x), i64::from(y) - i64::from(from.y));
        let from = from.clip(&source.bounds());
        let Some(landed) = shift(from, dx, dy) else { return };
        let to = landed.clip(&self.bounds());
        if to.is_empty() {
            return;
        }
        let (sx, sy) = ((i64::from(to.x) - dx) as usize, (i64::from(to.y) - dy) as usize);
        for row in 0..to.height as usize {
            let start = (sy + row) * source.stride + sx;
            let line = &source.pixels[start..start + to.width as usize];
            self.row(to.x, to.y + row as i32, to.width).copy_from_slice(line);
        }
    }

    /// `text` in TheTaaJ's font, each font pixel `scale` x `scale`, the
    /// first character's top-left corner at (x, y), one font column of
    /// space between characters. Only the glyphs' pixels are drawn: what
    /// is behind them shows through. Returns the width drawn.
    pub fn text(&mut self, x: i32, y: i32, text: &[u8], scale: u32, pixel: u32) -> u32 {
        let scale = scale.max(1);
        let advance = (font::WIDTH as u32 + 1) * scale;
        for (n, &c) in text.iter().enumerate() {
            let left = x.saturating_add((n as u32 * advance) as i32);
            for gy in 0..font::HEIGHT {
                for gx in 0..font::WIDTH {
                    if font::pixel(c, gx, gy) {
                        let cell = Rect::new(left + (gx as u32 * scale) as i32, y + (gy as u32 * scale) as i32, scale, scale);
                        self.fill(cell, pixel);
                    }
                }
            }
        }
        text.len() as u32 * advance
    }
}

/// `rect` moved by (dx, dy), or None if it would leave i32.
fn shift(rect: Rect, dx: i64, dy: i64) -> Option<Rect> {
    let x = i32::try_from(i64::from(rect.x) + dx).ok()?;
    let y = i32::try_from(i64::from(rect.y) + dy).ok()?;
    Some(Rect { x, y, ..rect })
}

/// The arrow: '#' its outline, '.' inside it, ' ' not part of it. Its
/// tip - the pixel that points - is the top-left corner.
const ARROW: [&[u8; POINTER_WIDTH]; POINTER_HEIGHT] = [
    b"#           ",
    b"##          ",
    b"#.#         ",
    b"#..#        ",
    b"#...#       ",
    b"#....#      ",
    b"#.....#     ",
    b"#......#    ",
    b"#.......#   ",
    b"#........#  ",
    b"#.........# ",
    b"#......#####",
    b"#...#..#    ",
    b"#..# #..#   ",
    b"#.#  #..#   ",
    b"##    #..#  ",
    b"#     #..#  ",
    b"       ##   ",
    b"            ",
];
pub const POINTER_WIDTH: usize = 12;
pub const POINTER_HEIGHT: usize = 19;

/// Whether the arrow covers pixel (x, y) of its picture, and with
/// which of its two colours: Some(true) for the outline.
pub fn arrow(x: usize, y: usize) -> Option<bool> {
    match ARROW.get(y)?.get(x)? {
        b'#' => Some(true),
        b'.' => Some(false),
        _ => None,
    }
}

/// The pointer on a surface: where it is, and what it covers.
pub struct Pointer {
    outline: u32,
    inside: u32,
    /// Where it is drawn now (its tip), if it is.
    shown: Option<(i32, i32)>,
    /// What was there before it was drawn: the rectangle under it, cut
    /// to the surface, row by row.
    saved: [u32; POINTER_WIDTH * POINTER_HEIGHT],
    saved_rect: Rect,
}

impl Pointer {
    pub fn new(outline: u32, inside: u32) -> Pointer {
        Pointer { outline, inside, shown: None, saved: [0; POINTER_WIDTH * POINTER_HEIGHT], saved_rect: Rect::default() }
    }

    /// Where it is drawn, if it is.
    pub fn shown(&self) -> Option<(i32, i32)> {
        self.shown
    }

    /// Draws it with its tip at (x, y), first saving what it covers.
    /// If it is already drawn somewhere, that is put back first.
    pub fn show(&mut self, surface: &mut Surface, x: i32, y: i32) {
        self.hide(surface);
        let under = Rect::new(x, y, POINTER_WIDTH as u32, POINTER_HEIGHT as u32).clip(&surface.bounds());
        for row in 0..under.height as usize {
            let line = surface.row(under.x, under.y + row as i32, under.width);
            self.saved[row * POINTER_WIDTH..row * POINTER_WIDTH + line.len()].copy_from_slice(line);
        }
        self.saved_rect = under;
        for py in 0..POINTER_HEIGHT {
            for px in 0..POINTER_WIDTH {
                if let Some(edge) = arrow(px, py) {
                    surface.put(x + px as i32, y + py as i32, if edge { self.outline } else { self.inside });
                }
            }
        }
        self.shown = Some((x, y));
    }

    /// Puts back what it covered. Nothing if it is not drawn.
    pub fn hide(&mut self, surface: &mut Surface) {
        if self.shown.take().is_none() {
            return;
        }
        let under = self.saved_rect;
        for row in 0..under.height as usize {
            let saved = &self.saved[row * POINTER_WIDTH..row * POINTER_WIDTH + under.width as usize];
            surface.row(under.x, under.y + row as i32, under.width).copy_from_slice(saved);
        }
    }

    /// Whether, drawn where it is, it overlaps `rect` - so whoever is
    /// about to draw there knows to hide it first.
    pub fn covers(&self, rect: &Rect) -> bool {
        self.shown.is_some() && !self.saved_rect.clip(rect).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(height: u32, stride: usize) -> Vec<u32> {
        (0..stride * height as usize).map(|n| n as u32).collect()
    }

    #[test]
    fn a_pixel_is_put_where_the_format_says() {
        assert_eq!(Format::XRGB8888.pixel(0x12, 0x34, 0x56), 0x0012_3456);
        // Blue at the top, as some cards have it.
        let bgr = Format { red_size: 8, red_shift: 0, green_size: 8, green_shift: 8, blue_size: 8, blue_shift: 16 };
        assert_eq!(bgr.pixel(0x12, 0x34, 0x56), 0x0056_3412);
        // Five, six and five bits: the top bits of each 8-bit value.
        let rgb565 = Format { red_size: 5, red_shift: 11, green_size: 6, green_shift: 5, blue_size: 5, blue_shift: 0 };
        assert_eq!(rgb565.pixel(0xFF, 0xFF, 0xFF), 0xFFFF);
        // red 10000, green 010000, blue 00001
        assert_eq!(rgb565.pixel(0x80, 0x40, 0x08), 0b1000_0010_0000_0001);
        let none = Format { red_size: 0, ..Format::XRGB8888 };
        assert_eq!(none.pixel(0xFF, 0, 0), 0);
    }

    #[test]
    fn a_rectangle_is_cut_to_another() {
        let screen = Rect::new(0, 0, 100, 50);
        assert_eq!(Rect::new(10, 10, 20, 20).clip(&screen), Rect::new(10, 10, 20, 20));
        assert_eq!(Rect::new(-5, -7, 20, 20).clip(&screen), Rect::new(0, 0, 15, 13));
        assert_eq!(Rect::new(90, 45, 20, 20).clip(&screen), Rect::new(90, 45, 10, 5));
        assert!(Rect::new(100, 0, 5, 5).clip(&screen).is_empty());
        assert!(Rect::new(-5, 0, 5, 5).clip(&screen).is_empty());
        assert!(Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX).clip(&screen).is_empty());
        assert_eq!(Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX).clip(&screen), Rect::new(0, 0, 100, 50));
        assert!(Rect::new(10, 10, 0, 5).is_empty());
        assert!(Rect::new(3, 4, 2, 2).contains(4, 5) && !Rect::new(3, 4, 2, 2).contains(5, 5));
        assert_eq!(Rect::new(i32::MAX - 1, 0, 1, 1).offset(5, 0).x, i32::MAX);
    }

    #[test]
    fn a_surface_must_have_its_pixels() {
        let mut pixels = vec![0u32; 10 * 4];
        assert!(Surface::new(&mut pixels, 10, 4, 10).is_some());
        assert!(Surface::new(&mut pixels, 8, 4, 10).is_some());
        assert!(Surface::new(&mut pixels, 10, 5, 10).is_none());
        assert!(Surface::new(&mut pixels, 11, 1, 10).is_none()); // wider than its stride
        // The last row need not be followed by the rest of a stride.
        assert!(Surface::new(&mut pixels, 8, 5, 8).is_some());
        assert!(Surface::new(&mut pixels, 1, usize::MAX as u32, usize::MAX).is_none());
        assert!(Image::new(&pixels, 0, 0, 0).is_some());
    }

    #[test]
    fn filling_stays_inside_the_surface_and_the_rectangle() {
        // A stride wider than the surface: the extra pixels must never
        // be touched - on a screen they may not be pixels at all.
        let mut pixels = canvas(6, 10);
        let before = pixels.clone();
        let mut surface = Surface::new(&mut pixels, 8, 6, 10).unwrap();
        surface.fill(Rect::new(-3, 4, 6, 9), 0xAA);
        surface.fill(Rect::new(6, -1, 5, 2), 0xBB);
        for y in 0..6 {
            for x in 0..10 {
                let expected = match (x, y) {
                    (0..=2, 4..=5) => 0xAA,
                    (6..=7, 0) => 0xBB,
                    _ => before[y * 10 + x],
                };
                assert_eq!(pixels[y * 10 + x], expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn copying_lands_where_asked_and_is_cut_at_both_ends() {
        let source_pixels: Vec<u32> = (0..6 * 5).map(|n| 1000 + n).collect();
        let source = Image::new(&source_pixels, 6, 5, 6).unwrap();
        let mut pixels = vec![0u32; 8 * 7];
        let mut surface = Surface::new(&mut pixels, 8, 7, 8).unwrap();
        // From (1, 1) of the source, 3x2, to (4, 2).
        surface.copy(&source, Rect::new(1, 1, 3, 2), 4, 2);
        assert_eq!(surface.get(4, 2), source.get(1, 1));
        assert_eq!(surface.get(6, 3), source.get(3, 2));
        assert_eq!(surface.get(7, 2), Some(0));
        assert_eq!(surface.get(3, 2), Some(0));
        // Off the left and top of the surface: what is left lands where
        // it would have.
        surface.copy(&source, Rect::new(0, 0, 6, 5), -2, -3);
        assert_eq!(surface.get(0, 0), source.get(2, 3));
        assert_eq!(surface.get(3, 1), source.get(5, 4));
        assert_eq!(surface.get(4, 0), Some(0));
        // Asked for more than the source has: only what it has, at the
        // place it would have had.
        let mut pixels = vec![0u32; 8 * 7];
        let mut surface = Surface::new(&mut pixels, 8, 7, 8).unwrap();
        surface.copy(&source, Rect::new(-2, -1, 10, 10), 0, 0);
        assert_eq!(surface.get(2, 1), source.get(0, 0));
        assert_eq!(surface.get(7, 5), source.get(5, 4));
        assert_eq!(surface.get(1, 1), Some(0));
        // Nowhere near: nothing, and no panic.
        surface.copy(&source, Rect::new(0, 0, 6, 5), i32::MAX, i32::MIN);
        surface.copy(&source, Rect::new(i32::MIN, 0, 6, 5), 0, 0);
    }

    #[test]
    fn text_is_the_font_scaled() {
        let mut pixels = vec![0u32; 40 * 20];
        let mut surface = Surface::new(&mut pixels, 40, 20, 40).unwrap();
        let width = surface.text(1, 2, b"Hi", 2, 7);
        assert_eq!(width, 2 * (font::WIDTH as u32 + 1) * 2);
        for (n, c) in b"Hi".iter().enumerate() {
            for gy in 0..font::HEIGHT {
                for gx in 0..font::WIDTH {
                    let lit = font::pixel(*c, gx, gy);
                    for (sx, sy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (x, y) = (1 + n as i32 * 12 + gx as i32 * 2 + sx, 2 + gy as i32 * 2 + sy);
                        assert_eq!(surface.get(x, y), Some(if lit { 7 } else { 0 }), "{} ({x}, {y})", *c as char);
                    }
                }
            }
        }
        // Off the edge: cut, no panic.
        surface.text(35, 15, b"Wide", 3, 9);
        surface.text(-100, -100, b"x", 1, 9);
    }

    #[test]
    fn the_pointer_leaves_nothing_behind() {
        let mut pixels = canvas(30, 44);
        let before = pixels.clone();
        let mut surface = Surface::new(&mut pixels, 40, 30, 44).unwrap();
        let mut pointer = Pointer::new(1, 2);
        assert_eq!(pointer.shown(), None);
        pointer.show(&mut surface, 5, 6);
        assert_eq!(surface.get(5, 6), Some(1)); // the tip is outline
        assert_eq!(surface.get(6, 8), Some(2)); // inside
        assert_eq!(surface.get(16, 6), before.get(6 * 44 + 16).copied()); // not part of the arrow
        assert!(pointer.covers(&Rect::new(10, 10, 1, 1)) && !pointer.covers(&Rect::new(30, 0, 5, 5)));
        // Moved: the first place is as it was, the second has it.
        pointer.show(&mut surface, 30, 20);
        assert_eq!(surface.get(5, 6), Some(before[6 * 44 + 5]));
        assert_eq!(surface.get(30, 20), Some(1));
        // Half off the bottom right, then off the top left.
        pointer.show(&mut surface, 35, 25);
        pointer.show(&mut surface, -3, -10);
        assert_eq!(pointer.shown(), Some((-3, -10)));
        pointer.hide(&mut surface);
        pointer.hide(&mut surface); // twice: nothing more
        assert_eq!(pointer.shown(), None);
        assert!(!pointer.covers(&Rect::new(0, 0, 40, 30)));
        // (The surface's borrow of the pixels ends here, at its last use.)
        assert!(pixels == before, "the surface is not as it was");
        // Entirely off: nothing drawn, nothing to restore.
        let mut surface = Surface::new(&mut pixels, 40, 30, 44).unwrap();
        pointer.show(&mut surface, 100, 100);
        pointer.hide(&mut surface);
        assert!(pixels == before);
    }

    #[test]
    fn the_arrow_is_drawn_as_its_picture_says() {
        assert_eq!(arrow(0, 0), Some(true));
        assert_eq!(arrow(1, 2), Some(false));
        assert_eq!(arrow(5, 0), None);
        assert_eq!(arrow(POINTER_WIDTH, 0), None);
        assert_eq!(arrow(0, POINTER_HEIGHT), None);
        // Every row of the picture is in the picture's alphabet, and the
        // outline closes: no inside pixel touches the outside.
        for y in 0..POINTER_HEIGHT {
            for x in 0..POINTER_WIDTH {
                if arrow(x, y) == Some(false) {
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        let neighbour = if nx < 0 || ny < 0 { None } else { arrow(nx as usize, ny as usize) };
                        assert!(neighbour.is_some(), "inside pixel ({x}, {y}) touches the outside");
                    }
                }
            }
        }
    }
}
