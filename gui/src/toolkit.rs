//! A small toolkit: the colours, the window frame the server draws, and
//! the things a program puts in its window - text and buttons.
//!
//! Everything draws on a `gfx::Surface` and reads a pointer's position
//! as plain numbers, so it runs on the host like the rest of the crate.
//! A widget keeps only what it needs to answer "what did that click
//! mean?"; the program owns the text it shows and asks for a redraw.

use gfx::{Format, Rect, Surface};

use crate::desktop::{Window, BORDER, LEFT, TITLE_HEIGHT};

/// The font's size at scale 1: 5 x 8, and a column of space after each
/// character.
const GLYPH_WIDTH: u32 = 5;
const GLYPH_HEIGHT: u32 = 8;
/// Text in windows is drawn at twice the font's size.
pub const TEXT_SCALE: u32 = 2;

/// How wide `text` is when drawn at TEXT_SCALE.
pub fn text_width(text: &[u8]) -> u32 {
    text.len() as u32 * (GLYPH_WIDTH + 1) * TEXT_SCALE
}

/// How tall a line of text is.
pub fn text_height() -> u32 {
    GLYPH_HEIGHT * TEXT_SCALE
}

/// The colours, as pixels of the screen's format.
///
/// None of them is pure white (the pointer's inside) or the terminal's
/// pale grey-blue text (0xE0E8F0): the boot tests find the arrow and
/// count the terminal's text by those colours, so nothing else may
/// look like them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    pub border: u32,
    pub title_focused: u32,
    pub title_unfocused: u32,
    pub title_text: u32,
    pub close: u32,
    pub close_mark: u32,
    /// A program's window, behind what it draws.
    pub panel: u32,
    pub text: u32,
    pub button: u32,
    pub button_pressed: u32,
    pub button_edge: u32,
    pub button_text: u32,
}

impl Theme {
    pub fn new(format: Format) -> Theme {
        let p = |r, g, b| format.pixel(r, g, b);
        Theme {
            border: p(0x08, 0x0C, 0x18),
            // The amber of the band at the top of the screen: the window
            // with the keys looks like it belongs to the system's name.
            title_focused: p(0xE8, 0xA8, 0x38),
            title_unfocused: p(0x70, 0x80, 0x98),
            title_text: p(0x10, 0x18, 0x30),
            close: p(0xC0, 0x40, 0x30),
            close_mark: p(0xF4, 0xEC, 0xE0),
            panel: p(0x28, 0x30, 0x44),
            text: p(0xF0, 0xD8, 0xA0),
            button: p(0x40, 0x58, 0x80),
            button_pressed: p(0x58, 0x78, 0xA8),
            button_edge: p(0x90, 0xA8, 0xC8),
            button_text: p(0xF4, 0xF0, 0xE0),
        }
    }
}

/// Draws a window's frame - border, title bar, title, close box - with
/// the screen's (0, 0) at (`dx`, `dy`) of `surface`. Not its content:
/// that is the program's.
pub fn draw_frame(surface: &mut Surface, window: &Window, focused: bool, theme: &Theme, dx: i32, dy: i32) {
    let frame = window.frame().offset(dx, dy);
    let b = BORDER;
    // The border: four strips, not a filled rectangle - what is inside
    // is drawn by others, and filling it first would only flicker.
    surface.fill(Rect::new(frame.x, frame.y, frame.width, b), theme.border);
    surface.fill(Rect::new(frame.x, frame.y + (frame.height - b) as i32, frame.width, b), theme.border);
    surface.fill(Rect::new(frame.x, frame.y, b, frame.height), theme.border);
    surface.fill(Rect::new(frame.x + (frame.width - b) as i32, frame.y, b, frame.height), theme.border);
    let bar = window.title_bar().offset(dx, dy);
    surface.fill(bar, if focused { theme.title_focused } else { theme.title_unfocused });
    // The title, from the left, cut before the close box.
    let room = match window.close_box() {
        Some(close) => (close.x - window.title_bar().x) as u32,
        None => bar.width,
    };
    let fits = (room.saturating_sub(8) / ((GLYPH_WIDTH + 1) * TEXT_SCALE)) as usize;
    let title = &window.title()[..window.title().len().min(fits)];
    let text_y = bar.y + ((TITLE_HEIGHT - text_height()) / 2) as i32;
    surface.text(bar.x + 6, text_y, title, TEXT_SCALE, theme.title_text);
    if let Some(close) = window.close_box() {
        let close = close.offset(dx, dy);
        surface.fill(close, theme.close);
        // An x: the two diagonals, inset by 3.
        for n in 3..close.width as i32 - 3 {
            surface.put(close.x + n, close.y + n, theme.close_mark);
            surface.put(close.x + close.width as i32 - 1 - n, close.y + n, theme.close_mark);
        }
    }
}

/// Text in a rectangle: drawn on the panel colour, cut to the rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Label {
    pub rect: Rect,
}

impl Label {
    pub fn draw(&self, surface: &mut Surface, text: &[u8], theme: &Theme) {
        let (mut area, _) = surface.region(self.rect);
        area.fill(Rect::new(0, 0, self.rect.width, self.rect.height), theme.panel);
        let y = (self.rect.height.saturating_sub(text_height()) / 2) as i32;
        area.text(0, y, text, TEXT_SCALE, theme.text);
    }
}

/// What a pointer report did to a button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    /// Nothing that shows.
    None,
    /// It looks different now (pressed, or no longer): draw it again.
    Changed,
    /// It was clicked: pressed, and let go over it. Draw it again too.
    Clicked,
}

/// A button: a rectangle with a word on it, clicked by pressing the
/// left button over it and letting go over it - moving off before
/// letting go takes the click back, as on every desktop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Button {
    pub rect: Rect,
    pub label: &'static [u8],
    /// The left button went down on it, and is still down.
    held: bool,
    /// ... and the pointer is over it: it is drawn pressed.
    pressed: bool,
    /// The left button was down at the last report - so a press is the
    /// report where it goes down, not every report while it is held.
    down: bool,
}

impl Button {
    pub const fn new(rect: Rect, label: &'static [u8]) -> Button {
        Button { rect, label, held: false, pressed: false, down: false }
    }

    pub fn pressed(&self) -> bool {
        self.pressed
    }

    /// The pointer at (x, y) - the window's coordinates - with `buttons`.
    pub fn pointer(&mut self, x: i32, y: i32, buttons: u8) -> Press {
        let over = self.rect.contains(x, y);
        let down = buttons & LEFT != 0;
        let was = self.pressed;
        if down && !self.down {
            // The left button just went down: a press of this button
            // only if it is over it.
            self.held = over;
        }
        self.down = down;
        if !down {
            let clicked = self.held && over;
            self.held = false;
            self.pressed = false;
            return if clicked { Press::Clicked } else if was { Press::Changed } else { Press::None };
        }
        self.pressed = self.held && over;
        if self.pressed != was { Press::Changed } else { Press::None }
    }

    pub fn draw(&self, surface: &mut Surface, theme: &Theme) {
        let r = self.rect;
        surface.fill(r, theme.button_edge);
        let face = Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(2), r.height.saturating_sub(2));
        surface.fill(face, if self.pressed { theme.button_pressed } else { theme.button });
        // The word in the middle; pressed, a pixel down and right, as if
        // pushed in.
        let shift = i32::from(self.pressed);
        let x = r.x + (r.width.saturating_sub(text_width(self.label)) / 2) as i32 + shift;
        let y = r.y + (r.height.saturating_sub(text_height()) / 2) as i32 + shift;
        let (mut inside, area) = surface.region(face);
        inside.text(x - area.x, y - area.y, self.label, TEXT_SCALE, theme.button_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::Desktop;

    fn theme() -> Theme {
        Theme::new(Format::XRGB8888)
    }

    #[test]
    fn no_colour_looks_like_the_pointer_or_the_terminals_text() {
        let t = theme();
        let all = [
            t.border, t.title_focused, t.title_unfocused, t.title_text, t.close, t.close_mark, t.panel, t.text, t.button, t.button_pressed,
            t.button_edge, t.button_text,
        ];
        for pixel in all {
            assert_ne!(pixel, 0xFF_FF_FF, "pure white is the pointer's");
            let (r, g, b) = ((pixel >> 16) & 0xFF, (pixel >> 8) & 0xFF, pixel & 0xFF);
            let near = |c: u32, want: u32| c.abs_diff(want) <= 8;
            assert!(!(near(r, 0xE0) && near(g, 0xE8) && near(b, 0xF0)), "{pixel:06x} looks like the terminal's text");
        }
    }

    #[test]
    fn a_frame_is_drawn_round_the_content_and_not_on_it() {
        let mut d = Desktop::new(400, 300);
        let (id, _) = d.open(b"hi", 100, 50, true, Some((10, 20))).unwrap();
        let window = *d.window(id).unwrap();
        let mut pixels = vec![0u32; 400 * 300];
        let mut surface = Surface::new(&mut pixels, 400, 300, 400).unwrap();
        let t = theme();
        draw_frame(&mut surface, &window, true, &t, 0, 0);
        // Border corners, the title bar's colour, the close box.
        assert_eq!(surface.get(10, 20), Some(t.border));
        assert_eq!(surface.get(111, 93), Some(t.border));
        assert_eq!(surface.get(12, 22), Some(t.title_focused));
        let close = window.close_box().unwrap();
        assert_eq!(surface.get(close.x, close.y), Some(t.close));
        assert_eq!(surface.get(close.x + 3, close.y + 3), Some(t.close_mark));
        // Some of the title's pixels, in the title colour.
        let bar = window.title_bar();
        let title_pixels = (bar.y..bar.y + 22).flat_map(|y| (bar.x..bar.x + 30).map(move |x| (x, y)));
        assert!(title_pixels.filter(|&(x, y)| surface.get(x, y) == Some(t.title_text)).count() > 10);
        // Not one pixel of the content.
        let c = window.content;
        for y in c.y..c.y + c.height as i32 {
            for x in c.x..c.x + c.width as i32 {
                assert_eq!(surface.get(x, y), Some(0), "({x}, {y})");
            }
        }
        // Unfocused: the other colour. Moved by (dx, dy): drawn there.
        draw_frame(&mut surface, &window, false, &t, 100, 100);
        assert_eq!(surface.get(112, 122), Some(t.title_unfocused));
    }

    #[test]
    fn a_long_title_stops_before_the_close_box() {
        let mut d = Desktop::new(400, 300);
        let (id, _) = d.open(b"a title far too long for this", 80, 40, true, Some((0, 0))).unwrap();
        let window = *d.window(id).unwrap();
        let mut pixels = vec![0u32; 400 * 300];
        let mut surface = Surface::new(&mut pixels, 400, 300, 400).unwrap();
        let t = theme();
        draw_frame(&mut surface, &window, true, &t, 0, 0);
        let close = window.close_box().unwrap();
        for y in close.y..close.y + close.height as i32 {
            for x in close.x - 2..close.x + close.width as i32 {
                assert_ne!(surface.get(x, y), Some(t.title_text), "title text at ({x}, {y})");
            }
        }
    }

    #[test]
    fn a_button_clicks_only_if_let_go_over_it() {
        let mut b = Button::new(Rect::new(10, 10, 50, 20), b"+1");
        // Press on it: drawn pressed.
        assert_eq!(b.pointer(20, 15, LEFT), Press::Changed);
        assert!(b.pressed());
        // Held, still over it: nothing new.
        assert_eq!(b.pointer(21, 15, LEFT), Press::None);
        // Let go over it: a click.
        assert_eq!(b.pointer(21, 15, 0), Press::Clicked);
        assert!(!b.pressed());
        // Press, move off (drawn up again), let go off it: no click.
        b.pointer(20, 15, LEFT);
        assert_eq!(b.pointer(100, 15, LEFT), Press::Changed);
        assert!(!b.pressed());
        assert_eq!(b.pointer(100, 15, 0), Press::None);
        // Press, off, back on (pressed again), let go: a click after all.
        b.pointer(20, 15, LEFT);
        b.pointer(100, 15, LEFT);
        assert_eq!(b.pointer(20, 15, LEFT), Press::Changed);
        assert_eq!(b.pointer(20, 15, 0), Press::Clicked);
        // A press that began elsewhere and comes over it: not its press.
        assert_eq!(b.pointer(100, 15, LEFT), Press::None);
        assert_eq!(b.pointer(20, 15, LEFT), Press::None);
        assert_eq!(b.pointer(20, 15, 0), Press::None);
        // The right button does nothing.
        assert_eq!(b.pointer(20, 15, 2), Press::None);
        assert_eq!(b.pointer(20, 15, 0), Press::None);
    }

    #[test]
    fn a_button_and_a_label_draw_inside_their_rectangles() {
        let t = theme();
        let mut pixels = vec![0u32; 100 * 60];
        let mut surface = Surface::new(&mut pixels, 100, 60, 100).unwrap();
        let mut b = Button::new(Rect::new(10, 10, 40, 24), b"quit");
        b.draw(&mut surface, &t);
        assert_eq!(surface.get(10, 10), Some(t.button_edge));
        assert_eq!(surface.get(12, 12), Some(t.button));
        let label_pixels = |s: &Surface, colour| (10..50).flat_map(|x| (10..34).map(move |y| (x, y))).filter(|&(x, y)| s.get(x, y) == Some(colour)).count();
        assert!(label_pixels(&surface, t.button_text) > 20);
        b.pointer(20, 20, LEFT);
        b.draw(&mut surface, &t);
        assert_eq!(surface.get(12, 12), Some(t.button_pressed));
        // A label far too long for its rectangle is cut at its edge.
        let label = Label { rect: Rect::new(0, 40, 30, 20) };
        label.draw(&mut surface, b"clicks: 1234567890", &t);
        assert_eq!(surface.get(0, 40), Some(t.panel));
        assert!((30..100).all(|x| (40..60).all(|y| surface.get(x, y) == Some(0))));
    }
}
