//! Keys that arrive over a serial line.
//!
//! A PS/2 keyboard sends scancodes, and the decoder in this crate turns
//! them into the bytes a terminal would send. A serial line already
//! has a terminal on its other end - a program on another machine, or
//! the window QEMU runs in - and what arrives is those bytes, nearly:
//!
//!   - Enter arrives as carriage return (0x0D); programs here read "\n"
//!   - Backspace arrives as DEL (0x7F) from most terminals; here it is
//!     0x08
//!   - arrows and the like arrive as the same escape sequences the
//!     keyboard driver makes (ESC [ A), a byte at a time
//!
//! So this is mostly a pipe with two bytes changed. The one thing it
//! must do carefully is keep an escape sequence together: its bytes
//! are held until the last has come, and passed on as one piece - so
//! that a reader is never left with half of one, and so that the two
//! function keys the drivers use for testing themselves (F11, F12) can
//! be taken out whole.
//!
//! A sequence begins with ESC. But ESC is also a key. Nothing in the
//! bytes tells the two apart; only time does - a sequence's bytes come
//! together, a person's next key does not. Whoever feeds this calls
//! `flush` when nothing more has come for a moment.

/// The longest sequence kept together; a longer one is passed on as
/// the bytes it is.
const HELD_MAX: usize = 8;

/// What a byte from the line turned into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Nothing yet: the byte is part of a sequence still arriving.
    Nothing,
    /// Bytes for whoever reads the keyboard.
    Bytes(Bytes),
    /// A function key (F1 is 1): not passed on, the driver's own.
    Function(u8),
}

/// A few bytes, by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bytes {
    bytes: [u8; HELD_MAX + 1],
    length: usize,
}

impl Bytes {
    fn new(first: &[u8], then: &[u8]) -> Bytes {
        let mut bytes = [0; HELD_MAX + 1];
        bytes[..first.len()].copy_from_slice(first);
        bytes[first.len()..first.len() + then.len()].copy_from_slice(then);
        Bytes { bytes, length: first.len() + then.len() }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

const ESCAPE: u8 = 0x1B;

/// The state between bytes: the part of an escape sequence that has
/// arrived.
#[derive(Debug, Default)]
pub struct Decoder {
    held: [u8; HELD_MAX],
    length: usize,
}

impl Decoder {
    pub const fn new() -> Decoder {
        Decoder { held: [0; HELD_MAX], length: 0 }
    }

    /// Whether part of a sequence is being held - and `flush` should be
    /// called if nothing more arrives soon.
    pub fn holding(&self) -> bool {
        self.length > 0
    }

    /// What is held, given up as it is: the sequence did not go on.
    /// (An ESC alone is the Escape key.)
    pub fn flush(&mut self) -> Option<Bytes> {
        let held = Bytes::new(&self.held[..self.length], &[]);
        self.length = 0;
        (held.length > 0).then_some(held)
    }

    /// One byte from the line.
    pub fn feed(&mut self, byte: u8) -> Input {
        let plain = |byte| match byte {
            b'\r' => b'\n',
            0x7F => 0x08,
            other => other,
        };
        match self.length {
            0 if byte == ESCAPE => self.hold(byte),
            0 => Input::Bytes(Bytes::new(&[plain(byte)], &[])),
            // After ESC, only '[' goes on with a sequence of the kind
            // kept together here.
            1 if byte == b'[' => self.hold(byte),
            1 if byte == ESCAPE => Input::Bytes(Bytes::new(&[ESCAPE], &[])), // Escape twice: one out, one held
            1 => {
                self.length = 0;
                Input::Bytes(Bytes::new(&[ESCAPE], &[plain(byte)]))
            }
            // Inside ESC [ ...: numbers and separators go on, a letter
            // or '~' ends it.
            _ if (0x20..=0x3F).contains(&byte) && self.length < HELD_MAX => self.hold(byte),
            _ => {
                let sequence = Bytes::new(&self.held[..self.length], &[byte]);
                self.length = 0;
                match sequence.as_bytes() {
                    b"\x1b[23~" => Input::Function(11),
                    b"\x1b[24~" => Input::Function(12),
                    _ => Input::Bytes(sequence),
                }
            }
        }
    }

    fn hold(&mut self, byte: u8) -> Input {
        self.held[self.length] = byte;
        self.length += 1;
        Input::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything `bytes` turns into, with a mark where a function key
    /// was taken out.
    fn fed(decoder: &mut Decoder, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &byte in bytes {
            match decoder.feed(byte) {
                Input::Nothing => {}
                Input::Bytes(bytes) => out.extend_from_slice(bytes.as_bytes()),
                Input::Function(n) => out.extend_from_slice(format!("<F{n}>").as_bytes()),
            }
        }
        out
    }

    #[test]
    fn ordinary_bytes_pass_and_two_are_changed() {
        let mut decoder = Decoder::new();
        assert_eq!(fed(&mut decoder, b"ls -l\r"), b"ls -l\n");
        assert_eq!(fed(&mut decoder, b"ab\x7fc"), b"ab\x08c");
        // Control codes are the terminal's business already: Ctrl+C,
        // Ctrl+D, Tab, and a line feed that came as one.
        assert_eq!(fed(&mut decoder, b"\x03\x04\t\n"), b"\x03\x04\t\n");
        // Bytes above 127 (UTF-8) are not this code's to judge.
        assert_eq!(fed(&mut decoder, "é".as_bytes()), "é".as_bytes());
        assert!(!decoder.holding());
    }

    #[test]
    fn a_sequence_comes_out_whole_or_not_at_all() {
        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(0x1B), Input::Nothing);
        assert!(decoder.holding());
        assert_eq!(decoder.feed(b'['), Input::Nothing);
        assert_eq!(fed(&mut decoder, b"A"), b"\x1b[A");
        assert!(!decoder.holding());
        // With a number: Delete is ESC [ 3 ~.
        assert_eq!(fed(&mut decoder, b"\x1b[3"), b"");
        assert_eq!(fed(&mut decoder, b"~x"), b"\x1b[3~x");
        // And between two ordinary keys.
        assert_eq!(fed(&mut decoder, b"a\x1b[Db"), b"a\x1b[Db");
    }

    #[test]
    fn the_test_keys_are_taken_out() {
        let mut decoder = Decoder::new();
        assert_eq!(fed(&mut decoder, b"a\x1b[24~b"), b"a<F12>b");
        assert_eq!(fed(&mut decoder, b"\x1b[23~"), b"<F11>");
        // F10 is nobody's: passed on.
        assert_eq!(fed(&mut decoder, b"\x1b[21~"), b"\x1b[21~");
    }

    #[test]
    fn escape_alone_is_a_key() {
        let mut decoder = Decoder::new();
        assert_eq!(fed(&mut decoder, b"\x1b"), b"");
        // Nothing followed: the caller gives up waiting.
        assert_eq!(decoder.flush().unwrap().as_bytes(), b"\x1b");
        assert!(!decoder.holding());
        assert_eq!(decoder.flush(), None);
        // Escape and then a letter, quickly: both, in order, the
        // letter as itself (Enter after Escape is still "\n").
        assert_eq!(fed(&mut decoder, b"\x1bq"), b"\x1bq");
        assert_eq!(fed(&mut decoder, b"\x1b\r"), b"\x1b\n");
        // Escape twice: the first out, the second may begin a sequence.
        assert_eq!(fed(&mut decoder, b"\x1b\x1b"), b"\x1b");
        assert_eq!(fed(&mut decoder, b"[B"), b"\x1b[B");
        // Half a sequence, then silence: given up as the bytes it was.
        assert_eq!(fed(&mut decoder, b"\x1b[1"), b"");
        assert_eq!(decoder.flush().unwrap().as_bytes(), b"\x1b[1");
    }

    #[test]
    fn a_sequence_too_long_is_passed_on_not_kept() {
        let mut decoder = Decoder::new();
        // ESC [ and six digits fill what is held; the next byte, digit
        // or not, ends it - nine bytes out, nothing lost.
        assert_eq!(fed(&mut decoder, b"\x1b[1234567"), b"\x1b[1234567");
        assert!(!decoder.holding());
        assert_eq!(fed(&mut decoder, b"8~"), b"8~");
    }
}
