//! One keyboard, as the display sees it (K116): key codes going down and
//! up, from whichever driver - kbd (PS/2), hid (virtio), usb - turned
//! into what a window or the terminal is told.
//!
//! The drivers used to make the terminal's bytes themselves, each with
//! the US layout built in, and each repeated a held key its own way: a
//! PS/2 keyboard repeats by itself, usb had a timer, hid dropped
//! repeats. Here, in the display, once for all of them:
//!
//!   - **the layout**, chosen while running (`display layout dvorak`);
//!   - **repeat**: a held key repeats after half a second, then 25 times
//!     a second (`usb::REPEAT_DELAY`, `REPEAT_INTERVAL`), whatever the
//!     keyboard. A keyboard's own repeats - a key going down that is
//!     already down - are dropped, so a PS/2 key does not repeat twice
//!     as fast as a virtio one;
//!   - **the bytes** a terminal sends for each press (`terminal_bytes`).
//!
//! The drivers still make bytes too: for the serial terminal, where
//! there is no display (init.rs), and that path is unchanged.
//!
//! Pure logic, with the time passed in, so it is tested on the host.

use crate::usb::Repeat;
use crate::{evdev, terminal_bytes, Decoder, Layout, TerminalBytes};

/// Key codes kept track of: Linux's go up to 0x2FF (KEY_MAX); the
/// keyboards here send codes below 112.
const KEYS: usize = 0x300;

/// What a key going down or up, or repeating, comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    /// Linux's key code: 30 is the A key, whatever the layout makes of it.
    pub code: u16,
    pub down: bool,
    /// Made here because the key is held, not by the keyboard.
    pub repeat: bool,
    /// What a terminal sends for it - none for an up, or a modifier.
    pub bytes: Option<TerminalBytes>,
}

pub struct Keyboard {
    decoder: Decoder,
    /// One bit per key code: down now.
    down: [u64; KEYS / 64],
    repeat: Repeat,
    /// The key `repeat` is repeating, if any.
    repeating: Option<u16>,
}

impl Default for Keyboard {
    fn default() -> Keyboard {
        Keyboard::new()
    }
}

/// Modifiers and Caps Lock do not repeat: holding Shift is not pressing
/// it again and again. Left and right Shift, Control, Alt; Caps Lock.
fn repeats(code: u16) -> bool {
    !matches!(code, 29 | 42 | 54 | 56 | 58 | 97 | 100)
}

impl Keyboard {
    pub const fn new() -> Keyboard {
        Keyboard { decoder: Decoder::new(), down: [0; KEYS / 64], repeat: Repeat::new(), repeating: None }
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.decoder.set_layout(layout);
    }

    pub fn layout(&self) -> Layout {
        self.decoder.layout()
    }

    pub fn is_down(&self, code: u16) -> bool {
        let code = usize::from(code);
        code < KEYS && self.down[code / 64] & (1 << (code % 64)) != 0
    }

    /// Key `code` went down or up, at tick `now`. None for a key that
    /// went down while it was down - the keyboard's own repeat, dropped
    /// (repeat is made here) - and for codes past KEY_MAX.
    ///
    /// An up for a key not down is passed on: it may have gone down
    /// before the display started, and Shift's up must still end Shift.
    pub fn key(&mut self, code: u16, down: bool, now: u64) -> Option<Press> {
        let index = usize::from(code);
        if index >= KEYS || (down && self.is_down(code)) {
            return None;
        }
        if down {
            self.down[index / 64] |= 1 << (index % 64);
        } else {
            self.down[index / 64] &= !(1 << (index % 64));
        }
        let bytes = self.decode(code, down);
        // The newest key down repeats, as on every desktop; its up ends
        // that - an older key still held does not start again.
        if down && repeats(code) {
            self.repeating = Some(code);
            self.repeat.held(None, now); // a new key: its delay from now
            self.repeat.held(Some(code), now);
        } else if !down && self.repeating == Some(code) {
            self.repeating = None;
            self.repeat.held(None, now);
        }
        Some(Press { code, down, repeat: false, bytes })
    }

    /// When the held key next repeats, if one is held.
    pub fn next(&self) -> Option<u64> {
        self.repeat.next()
    }

    /// The held key pressed again, if its time has come at `now`.
    pub fn due(&mut self, now: u64) -> Option<Press> {
        let code = self.repeat.due(now)?;
        let bytes = self.decode(code, true);
        Some(Press { code, down: true, repeat: true, bytes })
    }

    /// Through the decoder, as set-1 bytes (`evdev`): the one place
    /// that knows Shift, Caps Lock, Control and the layout.
    fn decode(&mut self, code: u16, down: bool) -> Option<TerminalBytes> {
        let (bytes, length) = evdev::scancode(code, down)?;
        let mut event = None;
        for &byte in &bytes[..length] {
            event = self.decoder.feed(byte);
        }
        terminal_bytes(event?, self.decoder.control_held())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::{REPEAT_DELAY, REPEAT_INTERVAL};

    const SHIFT: u16 = 42;
    const CONTROL: u16 = 29;

    /// Each key down and up at once; the bytes made.
    fn typed(keyboard: &mut Keyboard, codes: &[u16]) -> Vec<u8> {
        let mut out = Vec::new();
        for &code in codes {
            for down in [true, false] {
                if let Some(bytes) = keyboard.key(code, down, 0).and_then(|press| press.bytes) {
                    out.extend_from_slice(bytes.as_bytes());
                }
            }
        }
        out
    }

    // The keys of "hello" on the keyboard: h e l l o.
    const HELLO: [u16; 5] = [35, 18, 38, 38, 24];

    #[test]
    fn the_layout_decides_the_characters() {
        let mut keyboard = Keyboard::new();
        assert_eq!(keyboard.layout(), Layout::Us);
        assert_eq!(typed(&mut keyboard, &HELLO), b"hello");
        keyboard.set_layout(Layout::Dvorak);
        // On Dvorak those keys are d . n n r.
        assert_eq!(typed(&mut keyboard, &HELLO), b"d.nnr");
        // And "hello" is typed on the keys j d p p s.
        assert_eq!(typed(&mut keyboard, &[36, 32, 25, 25, 31]), b"hello");
        // Digits, Enter and the space stay where they were.
        assert_eq!(typed(&mut keyboard, &[2, 57, 28]), b"1 \n");
        keyboard.set_layout(Layout::Us);
        assert_eq!(typed(&mut keyboard, &HELLO), b"hello");
    }

    #[test]
    fn shift_and_control_follow_the_layout() {
        let mut keyboard = Keyboard::new();
        keyboard.set_layout(Layout::Dvorak);
        // Shift and the Q key: Dvorak's '"' there.
        keyboard.key(SHIFT, true, 0);
        assert_eq!(typed(&mut keyboard, &[16, 35]), b"\"D");
        keyboard.key(SHIFT, false, 0);
        // Control and the key Dvorak puts C on (US I): Ctrl-C, 0x03 - the
        // character, not the key, as terminals do.
        keyboard.key(CONTROL, true, 0);
        assert_eq!(typed(&mut keyboard, &[23]), [3]);
        keyboard.key(CONTROL, false, 0);
        assert_eq!(typed(&mut keyboard, &[23]), b"c");
    }

    #[test]
    fn layouts_by_name() {
        assert_eq!(Layout::from_name(b"us"), Some(Layout::Us));
        assert_eq!(Layout::from_name(b"dvorak"), Some(Layout::Dvorak));
        assert_eq!(Layout::from_name(b"qwertz"), None);
        assert_eq!(Layout::from_name(b""), None);
        for layout in Layout::ALL {
            assert_eq!(Layout::from_name(layout.name().as_bytes()), Some(layout));
        }
    }

    #[test]
    fn a_keyboards_own_repeat_is_dropped() {
        let mut keyboard = Keyboard::new();
        let first = keyboard.key(30, true, 0).unwrap();
        assert_eq!(first.bytes.unwrap().as_bytes(), b"a");
        // A PS/2 keyboard sends the key down again while it is held.
        assert_eq!(keyboard.key(30, true, 10), None);
        assert_eq!(keyboard.key(30, true, 20), None);
        assert!(keyboard.is_down(30));
        let up = keyboard.key(30, false, 30).unwrap();
        assert!(!up.down && up.bytes.is_none());
        assert!(!keyboard.is_down(30));
        // Down again after the up: a new press.
        assert!(keyboard.key(30, true, 40).is_some());
    }

    #[test]
    fn a_held_key_repeats_after_the_delay_then_at_the_interval() {
        let mut keyboard = Keyboard::new();
        keyboard.key(30, true, 100);
        assert_eq!(keyboard.next(), Some(100 + REPEAT_DELAY));
        assert_eq!(keyboard.due(100 + REPEAT_DELAY - 1), None);
        let again = keyboard.due(100 + REPEAT_DELAY).unwrap();
        assert_eq!(again, Press { code: 30, down: true, repeat: true, bytes: again.bytes });
        assert_eq!(again.bytes.unwrap().as_bytes(), b"a");
        assert_eq!(keyboard.next(), Some(100 + REPEAT_DELAY + REPEAT_INTERVAL));
        assert!(keyboard.due(100 + REPEAT_DELAY + REPEAT_INTERVAL).is_some());
        keyboard.key(30, false, 200);
        assert_eq!(keyboard.next(), None);
        assert_eq!(keyboard.due(1000), None);
    }

    #[test]
    fn a_repeat_is_shifted_while_shift_is_held() {
        let mut keyboard = Keyboard::new();
        keyboard.key(30, true, 0);
        keyboard.key(SHIFT, true, 10); // Shift down while A is held
        assert_eq!(keyboard.due(REPEAT_DELAY).unwrap().bytes.unwrap().as_bytes(), b"A");
    }

    #[test]
    fn modifiers_do_not_repeat() {
        let mut keyboard = Keyboard::new();
        for code in [SHIFT, 54, CONTROL, 97, 56, 100, 58] {
            keyboard.key(code, true, 0);
            assert_eq!(keyboard.next(), None, "key {code}");
            keyboard.key(code, false, 0);
        }
        // Caps Lock, pressed once in that loop, is on; again, off.
        assert_eq!(typed(&mut keyboard, &[30]), b"A");
        typed(&mut keyboard, &[58]);
        assert_eq!(typed(&mut keyboard, &[30]), b"a");
    }

    #[test]
    fn the_newest_key_repeats_and_its_up_ends_it() {
        let mut keyboard = Keyboard::new();
        keyboard.key(30, true, 0); // a
        keyboard.key(48, true, 20); // b, while a is held: b repeats, its delay from 20
        assert_eq!(keyboard.next(), Some(20 + REPEAT_DELAY));
        assert_eq!(keyboard.due(20 + REPEAT_DELAY).unwrap().bytes.unwrap().as_bytes(), b"b");
        // a's up does not stop b.
        keyboard.key(30, false, 80);
        assert!(keyboard.next().is_some());
        // b's up does; a is not down to start again anyway.
        keyboard.key(48, false, 90);
        assert_eq!(keyboard.next(), None);
        // b's up while a is still held: nothing repeats (a was not
        // pressed again).
        keyboard.key(30, true, 100);
        keyboard.key(48, true, 110);
        keyboard.key(48, false, 120);
        assert_eq!(keyboard.next(), None);
    }

    #[test]
    fn an_up_never_seen_down_still_ends_shift() {
        let mut keyboard = Keyboard::new();
        // Shift went down before anyone was listening: its up arrives.
        let up = keyboard.key(SHIFT, false, 0).unwrap();
        assert!(!up.down);
        assert_eq!(typed(&mut keyboard, &[30]), b"a");
    }

    #[test]
    fn codes_past_key_max_are_refused() {
        let mut keyboard = Keyboard::new();
        assert_eq!(keyboard.key(0x300, true, 0), None);
        assert_eq!(keyboard.key(u16::MAX, false, 0), None);
        assert!(!keyboard.is_down(0x300));
        // A key with no set-1 code: told down and up, with no bytes.
        let press = keyboard.key(0x160, true, 0).unwrap();
        assert!(press.down && press.bytes.is_none());
    }
}
