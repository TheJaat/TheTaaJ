//! A USB keyboard's and mouse's reports, as the rest of the input path
//! takes them.
//!
//! A USB keyboard or mouse that offers the "boot protocol" (every one
//! does: the BIOS setup screen relies on it) sends fixed reports, so a
//! driver can read them without parsing the device's report descriptor:
//!
//! ```text
//!   keyboard, 8 bytes   0 modifiers (bit 0 left Ctrl, 1 left Shift,
//!                         2 left Alt, 3 left GUI, 4-7 the right ones)
//!                       1 reserved
//!                       2-7 the keys held, up to six, by HID usage
//!                         (0x04 'a' ... 0x1D 'z'); 0 is no key
//!   mouse, 3+ bytes     0 buttons (bit 0 left, 1 right, 2 middle)
//!                       1 X movement, 2 Y movement: signed, down
//!                         positive
//! ```
//!
//! A keyboard report is not "key went down" but "these keys are down
//! now": the driver compares it with the last one to find presses and
//! releases (`BootKeyboard`). Each is turned into Linux's key code
//! (`key_code`), and from there into the set-1 scancode the PS/2 path
//! already decodes (`evdev::scancode`) - so a USB keyboard has the same
//! layout and modifiers as the PS/2 one, decided in one place.
//!
//! A USB keyboard does not repeat a held key by itself, as a PS/2 one
//! does: it says once that the key is down. The driver repeats it
//! (`Repeat`) for the serial terminal; with a display, the display
//! repeats every keyboard's keys itself (`keyboard`, K116), with this
//! `Repeat` and its timing.

use crate::pointer::Report;

/// HID usage IDs (the keyboard page) 0x00-0x67, as Linux's key codes -
/// Linux's own table (hid-input.c), for the keys a PC keyboard has. 0:
/// no such key here.
const KEYS: [u8; 0x68] = [
    0, 0, 0, 0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, // 0x00: -, -, -, -, a-l
    50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, 2, 3, // 0x10: m-z, 1, 2
    4, 5, 6, 7, 8, 9, 10, 11, 28, 1, 14, 15, 57, 12, 13, 26, // 0x20: 3-0, Enter, Esc, Backspace, Tab, Space, - = [
    27, 43, 43, 39, 40, 41, 51, 52, 53, 58, 59, 60, 61, 62, 63, 64, // 0x30: ] \ # ; ' ` , . / CapsLock F1-F7
    65, 66, 67, 68, 87, 88, 99, 70, 119, 110, 102, 104, 111, 107, 109, 106, // 0x40: F8-F12, PrtSc, ScrLk, Pause, Ins, Home, PgUp, Del, End, PgDn, Right
    105, 108, 103, 69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77, 71, // 0x50: Left, Down, Up, NumLock, KP / * - + Enter 1-7
    72, 73, 82, 83, 86, 127, 116, 117, // 0x60: KP 8 9 0 ., 102nd, Compose, Power, KP =
];

/// The modifier bits, in order, as Linux's key codes: left Ctrl, Shift,
/// Alt, GUI; right Ctrl, Shift, Alt, GUI.
const MODIFIERS: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];

/// The Linux key code of HID keyboard usage `usage`.
pub fn key_code(usage: u8) -> Option<u16> {
    match usage {
        0xE0..=0xE7 => Some(MODIFIERS[usize::from(usage - 0xE0)]),
        _ => KEYS.get(usize::from(usage)).copied().filter(|&code| code != 0).map(u16::from),
    }
}

/// "Too many keys at once" (ErrorRollOver): every key slot says 1. The
/// report says nothing about which keys are down, so it is ignored.
const ROLL_OVER: u8 = 0x01;

/// A boot keyboard's reports, turned into presses and releases.
#[derive(Default)]
pub struct BootKeyboard {
    modifiers: u8,
    keys: [u8; 6],
}

impl BootKeyboard {
    pub const fn new() -> BootKeyboard {
        BootKeyboard { modifiers: 0, keys: [0; 6] }
    }

    /// Feeds one report; `each(code, pressed)` for every key that went
    /// down or came up since the last - releases first, so that a key
    /// moved from one slot to another is not seen as released after it
    /// was pressed again.
    pub fn feed(&mut self, report: &[u8], mut each: impl FnMut(u16, bool)) {
        if report.len() < 8 {
            return;
        }
        let modifiers = report[0];
        let mut keys = [0u8; 6];
        keys.copy_from_slice(&report[2..8]);
        if keys.contains(&ROLL_OVER) {
            return;
        }
        for (bit, &code) in MODIFIERS.iter().enumerate() {
            let (was, is) = (self.modifiers & (1 << bit) != 0, modifiers & (1 << bit) != 0);
            if was && !is {
                each(code, false);
            }
        }
        for &usage in self.keys.iter().filter(|&&usage| usage != 0 && !keys.contains(&usage)) {
            if let Some(code) = key_code(usage) {
                each(code, false);
            }
        }
        for (bit, &code) in MODIFIERS.iter().enumerate() {
            let (was, is) = (self.modifiers & (1 << bit) != 0, modifiers & (1 << bit) != 0);
            if is && !was {
                each(code, true);
            }
        }
        for &usage in keys.iter().filter(|&&usage| usage != 0 && !self.keys.contains(&usage)) {
            if let Some(code) = key_code(usage) {
                each(code, true);
            }
        }
        self.modifiers = modifiers;
        self.keys = keys;
    }

    /// The key that should repeat while it is held: the last one
    /// pressed that is not a modifier, as Linux's code - or None.
    pub fn held(&self) -> Option<u16> {
        self.keys.iter().rev().find(|&&usage| usage != 0).and_then(|&usage| key_code(usage))
    }
}

/// A held key repeats after DELAY ticks, then every INTERVAL: half a
/// second, then 25 a second - what a PS/2 keyboard does by default.
pub const REPEAT_DELAY: u64 = 50;
pub const REPEAT_INTERVAL: u64 = 4;

/// When a held key next repeats.
#[derive(Default)]
pub struct Repeat {
    key: Option<u16>,
    at: u64,
}

impl Repeat {
    pub const fn new() -> Repeat {
        Repeat { key: None, at: 0 }
    }

    /// After a report, at time `now`: the key now held (from
    /// `BootKeyboard::held`). A different key starts the delay again.
    pub fn held(&mut self, key: Option<u16>, now: u64) {
        if key != self.key {
            self.key = key;
            self.at = now + REPEAT_DELAY;
        }
    }

    /// When to look again, if a key is held.
    pub fn next(&self) -> Option<u64> {
        self.key.map(|_| self.at)
    }

    /// The key to press again, if its time has come at `now`.
    pub fn due(&mut self, now: u64) -> Option<u16> {
        let key = self.key?;
        if now < self.at {
            return None;
        }
        self.at = now + REPEAT_INTERVAL;
        Some(key)
    }
}

/// A boot mouse's report: three bytes or more (the rest - a wheel, more
/// buttons - is ignored).
pub fn boot_mouse(report: &[u8]) -> Option<Report> {
    let &[buttons, dx, dy, ..] = report else { return None };
    // A fourth byte, when the mouse sends one, is its wheel (positive
    // away from the user, as Linux counts); bits 3 and 4 of the first,
    // its side buttons (K46).
    let wheel = report.get(3).map_or(0, |&w| i32::from(w as i8));
    Some(Report::Relative { dx: i32::from(dx as i8), dy: i32::from(dy as i8), buttons: buttons & 0x1F, wheel })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{evdev, Decoder, Key, KeyEvent};
    use std::vec::Vec;

    fn report(modifiers: u8, keys: &[u8]) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = modifiers;
        r[2..2 + keys.len()].copy_from_slice(keys);
        r
    }

    fn feed(keyboard: &mut BootKeyboard, r: [u8; 8]) -> Vec<(u16, bool)> {
        let mut out = Vec::new();
        keyboard.feed(&r, |code, pressed| out.push((code, pressed)));
        out
    }

    #[test]
    fn usages_are_linux_key_codes() {
        assert_eq!(key_code(0x04), Some(30)); // a
        assert_eq!(key_code(0x1D), Some(44)); // z
        assert_eq!(key_code(0x1E), Some(2)); // 1
        assert_eq!(key_code(0x27), Some(11)); // 0
        assert_eq!(key_code(0x28), Some(28)); // Enter
        assert_eq!(key_code(0x2C), Some(57)); // Space
        assert_eq!(key_code(0x45), Some(88)); // F12
        assert_eq!(key_code(0x52), Some(103)); // Up
        assert_eq!(key_code(0x4C), Some(111)); // Delete
        assert_eq!(key_code(0xE1), Some(42)); // left Shift
        assert_eq!(key_code(0xE6), Some(100)); // right Alt
        assert_eq!(key_code(0x00), None);
        assert_eq!(key_code(0x01), None); // roll over: no key
        assert_eq!(key_code(0x68), None);
        assert_eq!(key_code(0xE8), None);
    }

    #[test]
    fn every_letter_and_digit_decodes_as_itself() {
        let mut decoder = Decoder::new();
        let typed: Vec<char> = (0x04..=0x27u8)
            .filter_map(|usage| {
                let (bytes, n) = evdev::scancode(key_code(usage)?, true)?;
                let mut event = None;
                for &byte in &bytes[..n] {
                    event = decoder.feed(byte);
                }
                match event {
                    Some(KeyEvent { key: Key::Char(c), pressed: true }) => Some(c),
                    _ => None,
                }
            })
            .collect();
        let expected: Vec<char> = "abcdefghijklmnopqrstuvwxyz1234567890".chars().collect();
        assert_eq!(typed, expected);
    }

    #[test]
    fn reports_become_presses_and_releases() {
        let mut keyboard = BootKeyboard::new();
        assert_eq!(feed(&mut keyboard, report(0, &[0x04])), [(30, true)]);
        assert_eq!(feed(&mut keyboard, report(0, &[0x04])), [], "the same report again: nothing new");
        // Shift down, then 'b' with 'a' still held.
        assert_eq!(feed(&mut keyboard, report(0x02, &[0x04, 0x05])), [(42, true), (48, true)]);
        // 'a' up: it leaves its slot, 'b' moves to the first.
        assert_eq!(feed(&mut keyboard, report(0x02, &[0x05])), [(30, false)]);
        assert_eq!(feed(&mut keyboard, report(0, &[])), [(42, false), (48, false)]);
        // Right Ctrl and 'c' at once.
        assert_eq!(feed(&mut keyboard, report(0x10, &[0x06])), [(97, true), (46, true)]);
    }

    #[test]
    fn a_roll_over_report_changes_nothing() {
        let mut keyboard = BootKeyboard::new();
        feed(&mut keyboard, report(0, &[0x04]));
        assert_eq!(feed(&mut keyboard, report(0, &[1, 1, 1, 1, 1, 1])), []);
        assert_eq!(feed(&mut keyboard, report(0, &[])), [(30, false)], "and 'a' is still the one held before it");
        assert_eq!(feed(&mut keyboard, report(0, &[])), []);
        let mut short = Vec::new();
        keyboard.feed(&[0x02, 0, 0x04], |code, pressed| short.push((code, pressed)));
        assert!(short.is_empty(), "a report too short to be a boot keyboard's");
    }

    #[test]
    fn a_held_key_repeats_after_the_delay() {
        let mut keyboard = BootKeyboard::new();
        let mut repeat = Repeat::new();
        feed(&mut keyboard, report(0x02, &[0x04]));
        repeat.held(keyboard.held(), 100);
        assert_eq!(repeat.next(), Some(100 + REPEAT_DELAY));
        assert_eq!(repeat.due(100 + REPEAT_DELAY - 1), None);
        assert_eq!(repeat.due(100 + REPEAT_DELAY), Some(30));
        assert_eq!(repeat.due(100 + REPEAT_DELAY + 1), None);
        assert_eq!(repeat.due(100 + REPEAT_DELAY + REPEAT_INTERVAL), Some(30));
        // The same key in a new report: the timing goes on.
        feed(&mut keyboard, report(0x02, &[0x04]));
        repeat.held(keyboard.held(), 200);
        assert_eq!(repeat.next(), Some(100 + REPEAT_DELAY + 2 * REPEAT_INTERVAL));
        // Another key pressed: it repeats instead, after a new delay.
        feed(&mut keyboard, report(0x02, &[0x04, 0x05]));
        repeat.held(keyboard.held(), 300);
        assert_eq!(repeat.due(300 + REPEAT_DELAY), Some(48));
        // Only Shift held: nothing repeats.
        feed(&mut keyboard, report(0x02, &[]));
        repeat.held(keyboard.held(), 400);
        assert_eq!(repeat.next(), None);
        assert_eq!(repeat.due(10_000), None);
    }

    #[test]
    fn a_mouse_report_is_a_relative_move() {
        assert_eq!(boot_mouse(&[0x01, 5, 0xFB]), Some(Report::Relative { dx: 5, dy: -5, buttons: 1, wheel: 0 }));
        assert_eq!(boot_mouse(&[0xFF, 0x80, 0x7F, 3]), Some(Report::Relative { dx: -128, dy: 127, buttons: 0x1F, wheel: 3 }));
        assert_eq!(boot_mouse(&[0x01, 5]), None);
    }
}
