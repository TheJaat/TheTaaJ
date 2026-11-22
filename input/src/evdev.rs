//! A virtio keyboard's keys, as the PS/2 keyboard would have sent them.
//!
//! A virtio input device names keys by Linux's key codes (KEY_A is 30,
//! KEY_UP is 103), one event per press and per release. The PS/2 path
//! already turns scancode set 1 into key events (`Decoder`), with Shift,
//! Caps Lock and the layout. So a virtio key is turned into the set-1
//! bytes for the same key and fed to the same decoder - one place that
//! knows what Shift-2 is.
//!
//! That is less of a translation than it sounds. Linux numbered its key
//! codes after set 1: codes 1 to 88 *are* the set-1 scancodes (Escape
//! is 1 in both, F12 is 88). The keys the original PC keyboard did not
//! have - the arrows, Home, Delete - are where they differ: Linux gave
//! them codes of their own above 88, set 1 gives them a 0xE0 prefix and
//! a code that a key of the old keyboard also has.

/// The set-1 bytes for key `code` going down or up: one or two, and how
/// many. None for keys set 1 has no code for here.
pub fn scancode(code: u16, pressed: bool) -> Option<([u8; 2], usize)> {
    let release = if pressed { 0 } else { 0x80 };
    let extended = match code {
        1..=88 => return Some(([code as u8 | release, 0], 1)),
        96 => 0x1C,  // KEY_KPENTER
        97 => 0x1D,  // KEY_RIGHTCTRL
        98 => 0x35,  // KEY_KPSLASH
        100 => 0x38, // KEY_RIGHTALT
        102 => 0x47, // KEY_HOME
        103 => 0x48, // KEY_UP
        104 => 0x49, // KEY_PAGEUP
        105 => 0x4B, // KEY_LEFT
        106 => 0x4D, // KEY_RIGHT
        107 => 0x4F, // KEY_END
        108 => 0x50, // KEY_DOWN
        109 => 0x51, // KEY_PAGEDOWN
        110 => 0x52, // KEY_INSERT
        111 => 0x53, // KEY_DELETE
        _ => return None,
    };
    Some(([0xE0, extended | release], 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decoder, Key, KeyEvent};

    /// What the decoder makes of a virtio key.
    fn decode(decoder: &mut Decoder, code: u16, pressed: bool) -> Option<KeyEvent> {
        let (bytes, length) = scancode(code, pressed)?;
        let mut event = None;
        for &byte in &bytes[..length] {
            event = decoder.feed(byte);
        }
        event
    }

    #[test]
    fn old_keys_are_their_own_scancodes() {
        assert_eq!(scancode(30, true), Some(([0x1E, 0], 1))); // a
        assert_eq!(scancode(30, false), Some(([0x9E, 0], 1)));
        assert_eq!(scancode(1, true), Some(([0x01, 0], 1))); // Escape
        assert_eq!(scancode(88, false), Some(([0xD8, 0], 1))); // F12 up
        assert_eq!(scancode(0, true), None);
        assert_eq!(scancode(89, true), None);
        assert_eq!(scancode(0x110, true), None); // a mouse button, not a key
    }

    #[test]
    fn new_keys_get_the_prefix() {
        assert_eq!(scancode(103, true), Some(([0xE0, 0x48], 2))); // up
        assert_eq!(scancode(111, false), Some(([0xE0, 0xD3], 2))); // delete, up
    }

    #[test]
    fn the_decoder_reads_them_as_the_same_keys() {
        let mut decoder = Decoder::new();
        assert_eq!(decode(&mut decoder, 30, true), Some(KeyEvent { key: Key::Char('a'), pressed: true }));
        // Shift held: Shift-2 is '@' on this layout.
        decode(&mut decoder, 42, true);
        assert_eq!(decode(&mut decoder, 3, true), Some(KeyEvent { key: Key::Char('@'), pressed: true }));
        decode(&mut decoder, 42, false);
        assert_eq!(decode(&mut decoder, 3, true), Some(KeyEvent { key: Key::Char('2'), pressed: true }));
        for (code, key) in [
            (28, Key::Enter),
            (14, Key::Backspace),
            (96, Key::Enter),
            (102, Key::Home),
            (103, Key::Up),
            (104, Key::PageUp),
            (105, Key::Left),
            (106, Key::Right),
            (107, Key::End),
            (108, Key::Down),
            (109, Key::PageDown),
            (110, Key::Insert),
            (111, Key::Delete),
            (59, Key::Function(1)),
            (87, Key::Function(11)),
            (88, Key::Function(12)),
            (98, Key::Char('/')),
        ] {
            assert_eq!(decode(&mut decoder, code, true), Some(KeyEvent { key, pressed: true }), "code {code}");
            assert_eq!(decode(&mut decoder, code, false), Some(KeyEvent { key, pressed: false }), "code {code}");
        }
        // Right Control is Control: Ctrl-C.
        decode(&mut decoder, 97, true);
        assert!(decoder.control_held());
        decode(&mut decoder, 97, false);
        assert!(!decoder.control_held());
    }
}
