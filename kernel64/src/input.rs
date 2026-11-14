//! Keyboard input: from the bytes a PS/2 keyboard sends to key events.
//!
//! A keyboard does not send characters. It sends a *scancode* for each
//! key pressed and another for each key released - "key 0x23 went down",
//! "key 0x23 came up". Which character that is depends on the layout and
//! on which modifier keys are held, and that is decided here, in
//! software.
//!
//! Scancode set 1, which the PS/2 controller translates every keyboard
//! to by default:
//!   - one byte per press; the same byte with bit 7 set on release
//!   - keys added after the original PC keyboard (arrows, right Ctrl, ...)
//!     are prefixed by 0xE0
//!
//! The decoder is a small state machine: it remembers a pending 0xE0 and
//! which modifiers are down.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function(u8),
    Shift,
    Control,
    Alt,
    CapsLock,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub pressed: bool,
}

/// US layout, scancodes 0x00-0x39, without and with Shift. 0 = not a
/// character key.
const PLAIN: &[u8; 0x3A] = b"\0\x1b1234567890-=\x08\tqwertyuiop[]\n\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
const SHIFTED: &[u8; 0x3A] = b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\n\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

#[derive(Default)]
pub struct Decoder {
    extended: bool,
    left_shift: bool,
    right_shift: bool,
    control: bool,
    alt: bool,
    caps_lock: bool,
}

impl Decoder {
    pub const fn new() -> Decoder {
        Decoder {
            extended: false,
            left_shift: false,
            right_shift: false,
            control: false,
            alt: false,
            caps_lock: false,
        }
    }

    pub fn control_held(&self) -> bool {
        self.control
    }

    /// Feeds one byte from the keyboard. Returns an event once a whole
    /// scancode has arrived (a prefix alone produces nothing).
    pub fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if byte == 0xE0 {
            self.extended = true;
            return None;
        }
        let extended = core::mem::take(&mut self.extended);
        let pressed = byte & 0x80 == 0;
        let code = byte & 0x7F;

        let key = if extended {
            match code {
                0x1C => Key::Enter, // keypad Enter
                0x1D => Key::Control,
                0x38 => Key::Alt,
                0x47 => Key::Home,
                0x48 => Key::Up,
                0x49 => Key::PageUp,
                0x4B => Key::Left,
                0x4D => Key::Right,
                0x4F => Key::End,
                0x50 => Key::Down,
                0x51 => Key::PageDown,
                0x52 => Key::Insert,
                0x53 => Key::Delete,
                0x35 => Key::Char('/'), // keypad /
                // 0x2A/0x37 are halves of Print Screen's "fake shift"
                // sequence; ignore them rather than report Shift.
                _ => return None,
            }
        } else {
            match code {
                0x01 => Key::Escape,
                0x0E => Key::Backspace,
                0x0F => Key::Tab,
                0x1C => Key::Enter,
                0x1D => Key::Control,
                0x2A | 0x36 => Key::Shift,
                0x38 => Key::Alt,
                0x3A => Key::CapsLock,
                0x3B..=0x44 => Key::Function(code - 0x3B + 1),
                0x57 => Key::Function(11),
                0x58 => Key::Function(12),
                _ => match self.character(code) {
                    Some(c) => Key::Char(c),
                    None => Key::Unknown(code),
                },
            }
        };

        match (key, code) {
            (Key::Shift, 0x2A) => self.left_shift = pressed,
            (Key::Shift, _) => self.right_shift = pressed,
            (Key::Control, _) => self.control = pressed,
            (Key::Alt, _) => self.alt = pressed,
            (Key::CapsLock, _) if pressed => self.caps_lock = !self.caps_lock,
            _ => {}
        }
        Some(KeyEvent { key, pressed })
    }

    fn character(&self, code: u8) -> Option<char> {
        let index = code as usize;
        let plain = *PLAIN.get(index)?;
        if plain == 0 || plain.is_ascii_control() {
            return None; // Escape, Backspace, Tab, Enter are handled above
        }
        let shift = self.left_shift || self.right_shift;
        // Caps Lock flips the case of letters only - not digits or symbols.
        let use_shifted = if plain.is_ascii_alphabetic() { shift != self.caps_lock } else { shift };
        let byte = if use_shifted { SHIFTED[index] } else { plain };
        Some(byte as char)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(d: &mut Decoder, codes: &[u8]) -> Vec<KeyEvent> {
        codes.iter().filter_map(|&b| d.feed(b)).collect()
    }

    /// Types `codes` (press then release each) and returns the characters.
    fn typed(d: &mut Decoder, codes: &[u8]) -> String {
        let mut text = String::new();
        for &code in codes {
            for event in press(d, &[code, code | 0x80]) {
                if let (Key::Char(c), true) = (event.key, event.pressed) {
                    text.push(c);
                }
            }
        }
        text
    }

    #[test]
    fn letters_digits_and_space() {
        let mut d = Decoder::new();
        // h i space 4 2
        assert_eq!(typed(&mut d, &[0x23, 0x17, 0x39, 0x05, 0x03]), "hi 42");
    }

    #[test]
    fn release_is_bit_7() {
        let mut d = Decoder::new();
        assert_eq!(d.feed(0x1E), Some(KeyEvent { key: Key::Char('a'), pressed: true }));
        assert_eq!(d.feed(0x9E), Some(KeyEvent { key: Key::Char('a'), pressed: false }));
    }

    #[test]
    fn shift_changes_letters_and_symbols() {
        let mut d = Decoder::new();
        d.feed(0x2A); // left shift down
        assert_eq!(typed(&mut d, &[0x23, 0x02, 0x0C]), "H!_");
        d.feed(0xAA); // left shift up
        assert_eq!(typed(&mut d, &[0x23, 0x02, 0x0C]), "h1-");
    }

    #[test]
    fn both_shifts_are_tracked_separately() {
        let mut d = Decoder::new();
        d.feed(0x2A); // left down
        d.feed(0x36); // right down
        d.feed(0xAA); // left up - right still held
        assert_eq!(typed(&mut d, &[0x1E]), "A");
        d.feed(0xB6);
        assert_eq!(typed(&mut d, &[0x1E]), "a");
    }

    #[test]
    fn caps_lock_toggles_letters_only() {
        let mut d = Decoder::new();
        press(&mut d, &[0x3A, 0xBA]);
        assert_eq!(typed(&mut d, &[0x1E, 0x02]), "A1");
        d.feed(0x2A); // shift with caps lock: lower case again
        assert_eq!(typed(&mut d, &[0x1E]), "a");
        d.feed(0xAA);
        press(&mut d, &[0x3A, 0xBA]); // off again
        assert_eq!(typed(&mut d, &[0x1E]), "a");
    }

    #[test]
    fn extended_keys() {
        let mut d = Decoder::new();
        assert_eq!(press(&mut d, &[0xE0, 0x48]), vec![KeyEvent { key: Key::Up, pressed: true }]);
        assert_eq!(press(&mut d, &[0xE0, 0xC8]), vec![KeyEvent { key: Key::Up, pressed: false }]);
        // Without the prefix, 0x48 is keypad 8 - not an arrow.
        assert_ne!(d.feed(0x48).unwrap().key, Key::Up);
        // Right Ctrl counts as Ctrl.
        d.feed(0xE0);
        d.feed(0x1D);
        assert!(d.control_held());
    }

    #[test]
    fn special_keys() {
        let mut d = Decoder::new();
        let keys: Vec<Key> = press(&mut d, &[0x1C, 0x0E, 0x0F, 0x01, 0x3B, 0x58]).iter().map(|e| e.key).collect();
        assert_eq!(
            keys,
            vec![Key::Enter, Key::Backspace, Key::Tab, Key::Escape, Key::Function(1), Key::Function(12)]
        );
    }
}
