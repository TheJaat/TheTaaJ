//! The window protocol: what programs and the window server say to each
//! other, as bytes.
//!
//! A program opens a window by sending OPEN on the window server's
//! channel (the registry's "windows"), with two handles:
//!
//!   1. one end of a new channel - the window's own, from then on;
//!   2. a memory object of at least width x height pixels (4 bytes
//!      each): the window's contents, which the program draws in and
//!      the server reads.
//!
//! Everything after that is on the window's channel, both ways:
//!
//! ```text
//!   program                                server
//!      │── OPEN (on "windows") ──────────────▶│
//!      │◀──────────── OPENED id, format ───────│   or REFUSED, why
//!      │   draws into the memory               │
//!      │── DAMAGE x, y, w, h ────────────────▶│   copies that part to the screen
//!      │◀──────── POINTER x, y, buttons ───────│   a click in the window
//!      │◀──────── KEY bytes ───────────────────│   typed while it has the focus
//!      │◀──────── FOCUS yes/no ────────────────│
//!      │◀──────── CLOSE ───────────────────────│   its close box was clicked
//!      │── (closes its end) ─────────────────▶│   the window goes
//! ```
//!
//! The pixels never travel in messages: they are shared memory, and a
//! DAMAGE message only says which part changed. A window is closed by
//! closing the channel - so a program that crashes closes its windows
//! without saying anything, because the kernel closes its handles.
//!
//! Numbers are little-endian, as everywhere else in TheTaaJ.

use gfx::{Format, Rect};

/// The longest title, in bytes.
pub const TITLE_MAX: usize = 32;
/// The largest window, either way, in pixels.
pub const SIZE_MAX: u32 = 2048;

/// On the server's channel: open a window.
pub const OPEN: u8 = 1;

/// A request to open a window: its size (the part the program draws -
/// the frame around it is the server's) and its title.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Open<'a> {
    pub width: u32,
    pub height: u32,
    pub title: &'a [u8],
}

impl<'a> Open<'a> {
    /// OPEN, width and height (u16 each), then the title. None if the
    /// title is too long, the size out of range, or `out` too small.
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        if !valid_size(self.width, self.height) || self.title.len() > TITLE_MAX {
            return None;
        }
        let length = 5 + self.title.len();
        let out = out.get_mut(..length)?;
        out[0] = OPEN;
        out[1..3].copy_from_slice(&(self.width as u16).to_le_bytes());
        out[3..5].copy_from_slice(&(self.height as u16).to_le_bytes());
        out[5..].copy_from_slice(self.title);
        Some(length)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Open<'a>> {
        if bytes.len() < 5 || bytes[0] != OPEN || bytes.len() - 5 > TITLE_MAX {
            return None;
        }
        let width = u32::from(u16::from_le_bytes([bytes[1], bytes[2]]));
        let height = u32::from(u16::from_le_bytes([bytes[3], bytes[4]]));
        valid_size(width, height).then_some(Open { width, height, title: &bytes[5..] })
    }

    /// The bytes the window's memory must hold at least.
    pub fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }
}

fn valid_size(width: u32, height: u32) -> bool {
    (1..=SIZE_MAX).contains(&width) && (1..=SIZE_MAX).contains(&height)
}

// What the server sends on a window's channel.
const OPENED: u8 = 1;
const REFUSED: u8 = 2;
const KEY: u8 = 3;
const POINTER: u8 = 4;
const FOCUS: u8 = 5;
const CLOSE: u8 = 6;

/// What the server tells a program about its window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event<'a> {
    /// The window is open: its number, and how the program must lay out
    /// its pixels - the screen's own format, so the server copies them
    /// as they are.
    Opened { id: u32, format: Format },
    /// It was not opened, and why (words for a person).
    Refused(&'a [u8]),
    /// Keys typed while the window has the focus: the bytes a terminal
    /// sends ("a", "\r", "\x1b[A" for Up).
    Key(&'a [u8]),
    /// A button went down or up inside the window, or the pointer moved
    /// while a button that went down inside it is held. Where, from the
    /// top-left corner of what the program draws - it may be outside
    /// the window, while a button is held - and the buttons held then
    /// (1 left, 2 right, 4 middle).
    Pointer { x: i32, y: i32, buttons: u8 },
    /// The window got the focus (true) or lost it.
    Focus(bool),
    /// Its close box was clicked: the program is asked to close it. It
    /// decides - a program with unsaved work may ask first.
    Close,
}

impl<'a> Event<'a> {
    /// The message, in `out`. None if it does not fit.
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let mut writer = Writer { out, length: 0 };
        match *self {
            Event::Opened { id, format } => {
                writer.byte(OPENED)?;
                writer.bytes(&id.to_le_bytes())?;
                writer.bytes(&[format.red_size, format.red_shift, format.green_size, format.green_shift, format.blue_size, format.blue_shift])?;
            }
            Event::Refused(why) => {
                writer.byte(REFUSED)?;
                writer.bytes(why)?;
            }
            Event::Key(bytes) => {
                writer.byte(KEY)?;
                writer.bytes(bytes)?;
            }
            Event::Pointer { x, y, buttons } => {
                writer.byte(POINTER)?;
                writer.bytes(&x.to_le_bytes())?;
                writer.bytes(&y.to_le_bytes())?;
                writer.byte(buttons)?;
            }
            Event::Focus(focused) => {
                writer.byte(FOCUS)?;
                writer.byte(u8::from(focused))?;
            }
            Event::Close => writer.byte(CLOSE)?,
        }
        Some(writer.length)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Event<'a>> {
        let (&kind, rest) = bytes.split_first()?;
        match (kind, rest.len()) {
            (OPENED, 10) => {
                let id = u32::from_le_bytes(rest[0..4].try_into().ok()?);
                let f = &rest[4..10];
                let format = Format { red_size: f[0], red_shift: f[1], green_size: f[2], green_shift: f[3], blue_size: f[4], blue_shift: f[5] };
                Some(Event::Opened { id, format })
            }
            (REFUSED, _) => Some(Event::Refused(rest)),
            (KEY, 1..) => Some(Event::Key(rest)),
            (POINTER, 9) => Some(Event::Pointer {
                x: i32::from_le_bytes(rest[0..4].try_into().ok()?),
                y: i32::from_le_bytes(rest[4..8].try_into().ok()?),
                buttons: rest[8],
            }),
            (FOCUS, 1) if rest[0] <= 1 => Some(Event::Focus(rest[0] == 1)),
            (CLOSE, 0) => Some(Event::Close),
            _ => None,
        }
    }
}

// What a program sends on its window's channel.
const DAMAGE: u8 = 1;
const TITLE: u8 = 2;

/// What a program asks of the server about its window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request<'a> {
    /// This part of the window has new pixels: show them. In the
    /// window's own coordinates; the server cuts it to the window.
    Damage(Rect),
    /// A new title.
    Title(&'a [u8]),
}

impl<'a> Request<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let mut writer = Writer { out, length: 0 };
        match *self {
            Request::Damage(rect) => {
                // Only a rectangle inside a window of the largest size can
                // be said: negative corners or sizes past u16 are the
                // program's mistake, caught here rather than wrapped.
                let fields = [rect.x, rect.y, rect.width as i32, rect.height as i32];
                if fields.iter().any(|&f| !(0..=SIZE_MAX as i32).contains(&f)) || rect.width > SIZE_MAX || rect.height > SIZE_MAX {
                    return None;
                }
                writer.byte(DAMAGE)?;
                for field in fields {
                    writer.bytes(&(field as u16).to_le_bytes())?;
                }
            }
            Request::Title(title) => {
                if title.len() > TITLE_MAX {
                    return None;
                }
                writer.byte(TITLE)?;
                writer.bytes(title)?;
            }
        }
        Some(writer.length)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Request<'a>> {
        let (&kind, rest) = bytes.split_first()?;
        match (kind, rest.len()) {
            (DAMAGE, 8) => {
                let field = |n: usize| u16::from_le_bytes([rest[2 * n], rest[2 * n + 1]]);
                Some(Request::Damage(Rect::new(i32::from(field(0)), i32::from(field(1)), u32::from(field(2)), u32::from(field(3)))))
            }
            (TITLE, 0..=TITLE_MAX) => Some(Request::Title(rest)),
            _ => None,
        }
    }
}

/// Bytes appended to a buffer, failing - not panicking - at its end.
struct Writer<'o> {
    out: &'o mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, bytes: &[u8]) -> Option<()> {
        let end = self.length.checked_add(bytes.len())?;
        self.out.get_mut(self.length..end)?.copy_from_slice(bytes);
        self.length = end;
        Some(())
    }

    fn byte(&mut self, byte: u8) -> Option<()> {
        self.bytes(&[byte])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_request_goes_there_and_back() {
        let open = Open { width: 240, height: 120, title: b"counter" };
        let mut out = [0u8; 64];
        let n = open.encode(&mut out).unwrap();
        assert_eq!(&out[..n], b"\x01\xF0\x00\x78\x00counter");
        assert_eq!(Open::decode(&out[..n]), Some(open));
        assert_eq!(open.bytes(), 240 * 120 * 4);
        // An empty title is a title.
        let n = Open { title: b"", ..open }.encode(&mut out).unwrap();
        assert_eq!(Open::decode(&out[..n]).unwrap().title, b"");
    }

    #[test]
    fn an_open_request_out_of_range_is_refused_both_ways() {
        let mut out = [0u8; 64];
        let long = [b'x'; TITLE_MAX + 1];
        assert_eq!(Open { width: 10, height: 10, title: &long }.encode(&mut out), None);
        assert_eq!(Open { width: 0, height: 10, title: b"" }.encode(&mut out), None);
        assert_eq!(Open { width: 10, height: SIZE_MAX + 1, title: b"" }.encode(&mut out), None);
        assert_eq!(Open { width: 10, height: 10, title: b"too small" }.encode(&mut out[..6]), None);
        // As received: a zero size, a size past the largest, a long title,
        // cut short, another kind of message.
        assert_eq!(Open::decode(b"\x01\x00\x00\x05\x00"), None);
        assert_eq!(Open::decode(b"\x01\x01\x08\x05\x00"), None); // 2049 wide
        let mut long_message = vec![1, 10, 0, 10, 0];
        long_message.extend_from_slice(&long);
        assert_eq!(Open::decode(&long_message), None);
        assert_eq!(Open::decode(b"\x01\x0A\x00"), None);
        assert_eq!(Open::decode(b"\x02\x0A\x00\x0A\x00"), None);
    }

    #[test]
    fn every_event_goes_there_and_back() {
        let events = [
            Event::Opened { id: 7, format: Format::XRGB8888 },
            Event::Refused(b"no room for another window"),
            Event::Key(b"\x1b[A"),
            Event::Pointer { x: -3, y: 70_000, buttons: 5 },
            Event::Focus(true),
            Event::Focus(false),
            Event::Close,
        ];
        let mut out = [0u8; 64];
        for event in events {
            let n = event.encode(&mut out).unwrap();
            assert_eq!(Event::decode(&out[..n]), Some(event), "{event:?}");
        }
        // Too small a buffer: None, not a panic. Only CLOSE fits in one byte.
        for event in events {
            let fits = event.encode(&mut out[..1]).is_some();
            assert_eq!(fits, event == Event::Close, "{event:?}");
        }
    }

    #[test]
    fn a_broken_event_is_none() {
        for bytes in [
            &b""[..],
            b"\x01\x07\x00\x00",                 // OPENED cut short
            b"\x03",                             // a key with no bytes
            b"\x04\x00\x00\x00\x00\x00\x00\x00", // POINTER cut short
            b"\x05\x02",                         // focus neither yes nor no
            b"\x06\x00",                         // CLOSE with something after it
            b"\x09",                             // no such event
        ] {
            assert_eq!(Event::decode(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn requests_go_there_and_back_and_out_of_range_ones_do_not() {
        let mut out = [0u8; 64];
        for request in [Request::Damage(Rect::new(4, 5, 100, 30)), Request::Title(b"counter: 3"), Request::Title(b"")] {
            let n = request.encode(&mut out).unwrap();
            assert_eq!(Request::decode(&out[..n]), Some(request));
        }
        assert_eq!(Request::Damage(Rect::new(-1, 0, 5, 5)).encode(&mut out), None);
        assert_eq!(Request::Damage(Rect::new(0, 0, SIZE_MAX + 1, 5)).encode(&mut out), None);
        assert_eq!(Request::Damage(Rect::new(0, 0, u32::MAX, 5)).encode(&mut out), None);
        assert_eq!(Request::Title(&[b'x'; TITLE_MAX + 1]).encode(&mut out), None);
        assert_eq!(Request::decode(b"\x01\x00\x00"), None);
        let mut long = vec![TITLE];
        long.extend_from_slice(&[b'x'; TITLE_MAX + 1]);
        assert_eq!(Request::decode(&long), None);
        assert_eq!(Request::decode(b"\x03"), None);
    }
}
