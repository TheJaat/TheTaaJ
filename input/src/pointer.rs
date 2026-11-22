//! Pointers: a mouse that says how far it moved, or a tablet that says
//! where it is - and the bytes and events each arrives as.
//!
//! Two sources, one result (`Report`):
//!
//!   - A PS/2 mouse, on the PC's keyboard controller, sends three bytes
//!     per movement (`Ps2Mouse`):
//!
//!     ```text
//!       byte 0   bit 0 left, 1 right, 2 middle button; bit 3 always 1;
//!                bit 4 the sign of X, 5 the sign of Y; 6, 7 overflow
//!       byte 1   X movement, the low eight bits of nine
//!       byte 2   Y movement, likewise - and up is positive
//!     ```
//!
//!   - A virtio input device sends Linux's input events, eight bytes
//!     each: a type, a code and a value (`Events`). A mouse sends
//!     relative movements (EV_REL), a tablet absolute positions
//!     (EV_ABS), both send buttons as keys (EV_KEY with BTN_LEFT...),
//!     and a SYN event says "that was one report: act on it now".

/// Buttons held, one bit each.
pub const LEFT: u8 = 1 << 0;
pub const RIGHT: u8 = 1 << 1;
pub const MIDDLE: u8 = 1 << 2;

/// A tablet's position is scaled to 0..=ABSOLUTE_MAX on each axis,
/// whatever range the device itself uses; the screen scales it again to
/// its own size.
pub const ABSOLUTE_MAX: i32 = 0x7FFF;

/// What a pointer said, once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Report {
    /// Moved by (dx, dy) pixels - right and *down* are positive, as on
    /// the screen.
    Relative { dx: i32, dy: i32, buttons: u8 },
    /// Is at (x, y), each 0..=ABSOLUTE_MAX across the screen.
    Absolute { x: i32, y: i32, buttons: u8 },
}

impl Report {
    pub fn buttons(&self) -> u8 {
        match *self {
            Report::Relative { buttons, .. } | Report::Absolute { buttons, .. } => buttons,
        }
    }
}

/// A PS/2 mouse's packets, a byte at a time.
#[derive(Default)]
pub struct Ps2Mouse {
    packet: [u8; 3],
    count: usize,
}

impl Ps2Mouse {
    pub const fn new() -> Ps2Mouse {
        Ps2Mouse { packet: [0; 3], count: 0 }
    }

    /// Feeds one byte from the mouse; a report once three have come.
    ///
    /// Bit 3 of the first byte is always set. A byte lost somewhere
    /// would shift every packet after it by one - buttons read as
    /// movement, for ever - so a first byte without bit 3 is thrown
    /// away until one with it comes. That finds the start again within
    /// a packet or two (a movement byte can have bit 3 set by chance;
    /// the next packets put it right).
    pub fn feed(&mut self, byte: u8) -> Option<Report> {
        if self.count == 0 && byte & 0x08 == 0 {
            return None;
        }
        self.packet[self.count] = byte;
        self.count += 1;
        if self.count < 3 {
            return None;
        }
        self.count = 0;
        let [flags, x, y] = self.packet;
        // Nine-bit numbers: the sign is in the first byte. On overflow
        // the movement is too big to say, and is better dropped than
        // read wrong.
        let nine = |low: u8, negative: bool, overflow: bool| -> i32 {
            match (overflow, negative) {
                (true, _) => 0,
                (false, true) => i32::from(low) - 256,
                (false, false) => i32::from(low),
            }
        };
        let dx = nine(x, flags & 0x10 != 0, flags & 0x40 != 0);
        let dy = nine(y, flags & 0x20 != 0, flags & 0x80 != 0);
        Some(Report::Relative { dx, dy: -dy, buttons: flags & (LEFT | RIGHT | MIDDLE) })
    }

    /// Forgets a packet half read - after the mouse was reset.
    pub fn restart(&mut self) {
        self.count = 0;
    }
}

/// Linux's input event types and codes - what virtio input devices
/// send (the virtio specification takes them from Linux's
/// `input-event-codes.h`).
pub mod codes {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
    /// Below this, a key code is a keyboard's key; from here up, buttons.
    pub const BTN_MISC: u16 = 0x100;
}

/// A virtio pointer's events, gathered into reports.
#[derive(Default)]
pub struct Events {
    dx: i32,
    dy: i32,
    x: Option<i32>,
    y: Option<i32>,
    /// The range each absolute axis uses: (minimum, maximum).
    x_range: (i32, i32),
    y_range: (i32, i32),
    buttons: u8,
    /// Whether anything has changed since the last report.
    changed: bool,
    absolute: bool,
}

impl Events {
    /// For a device whose absolute axes (if it has any) run over these
    /// ranges - what the device says in its configuration.
    pub fn new(x_range: (i32, i32), y_range: (i32, i32)) -> Events {
        Events { x_range, y_range, ..Events::default() }
    }

    /// Takes one event. A report when it ends one (EV_SYN) and something
    /// changed.
    pub fn feed(&mut self, kind: u16, code: u16, value: u32) -> Option<Report> {
        let value = value as i32;
        match (kind, code) {
            (codes::EV_REL, codes::REL_X) => self.dx = self.dx.saturating_add(value),
            (codes::EV_REL, codes::REL_Y) => self.dy = self.dy.saturating_add(value),
            (codes::EV_ABS, codes::ABS_X) => {
                self.x = Some(scale(value, self.x_range));
                self.absolute = true;
            }
            (codes::EV_ABS, codes::ABS_Y) => {
                self.y = Some(scale(value, self.y_range));
                self.absolute = true;
            }
            (codes::EV_KEY, codes::BTN_LEFT | codes::BTN_RIGHT | codes::BTN_MIDDLE) => {
                let bit = match code {
                    codes::BTN_LEFT => LEFT,
                    codes::BTN_RIGHT => RIGHT,
                    _ => MIDDLE,
                };
                // 0 is up; 1 down, 2 "still down" (repeat).
                if value == 0 {
                    self.buttons &= !bit;
                } else {
                    self.buttons |= bit;
                }
            }
            (codes::EV_SYN, _) => {
                if !self.changed {
                    return None;
                }
                self.changed = false;
                return Some(if self.absolute {
                    // An axis that did not move keeps its last place.
                    Report::Absolute { x: self.x.unwrap_or(0), y: self.y.unwrap_or(0), buttons: self.buttons }
                } else {
                    let report = Report::Relative { dx: self.dx, dy: self.dy, buttons: self.buttons };
                    (self.dx, self.dy) = (0, 0);
                    report
                });
            }
            _ => return None,
        }
        self.changed = true;
        None
    }
}

/// `value` within `(min, max)`, scaled to 0..=ABSOLUTE_MAX - clamped,
/// so a device that strays outside its own range lands on the edge.
fn scale(value: i32, (min, max): (i32, i32)) -> i32 {
    if max <= min {
        return value.clamp(0, ABSOLUTE_MAX);
    }
    let value = i64::from(value.clamp(min, max)) - i64::from(min);
    (value * i64::from(ABSOLUTE_MAX) / (i64::from(max) - i64::from(min))) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(mouse: &mut Ps2Mouse, bytes: [u8; 3]) -> Option<Report> {
        assert_eq!(mouse.feed(bytes[0]), None);
        assert_eq!(mouse.feed(bytes[1]), None);
        mouse.feed(bytes[2])
    }

    #[test]
    fn a_ps2_packet_is_movement_and_buttons() {
        let mut mouse = Ps2Mouse::new();
        // Right 5, up 3, left button: the screen's y goes down.
        assert_eq!(packet(&mut mouse, [0x09, 5, 3]), Some(Report::Relative { dx: 5, dy: -3, buttons: LEFT }));
        // Left 1 and down 2 (both negative): nine-bit numbers.
        assert_eq!(packet(&mut mouse, [0x08 | 0x10 | 0x20, 0xFF, 0xFE]), Some(Report::Relative { dx: -1, dy: 2, buttons: 0 }));
        // The farthest each way.
        assert_eq!(packet(&mut mouse, [0x08 | 0x10, 0x00, 0xFF]), Some(Report::Relative { dx: -256, dy: -255, buttons: 0 }));
        // Overflow: movement dropped, buttons kept.
        assert_eq!(packet(&mut mouse, [0x08 | 0x40 | 0x80 | 0x06, 0x12, 0x34]), Some(Report::Relative { dx: 0, dy: 0, buttons: RIGHT | MIDDLE }));
    }

    #[test]
    fn a_ps2_mouse_finds_the_start_of_a_packet_again() {
        let mut mouse = Ps2Mouse::new();
        // A stray byte without bit 3 before a packet: thrown away.
        assert_eq!(mouse.feed(0x00), None);
        assert_eq!(mouse.feed(0x20), None);
        assert_eq!(packet(&mut mouse, [0x08, 7, 0]), Some(Report::Relative { dx: 7, dy: 0, buttons: 0 }));
        // A packet cut short, and the mouse reset: start over.
        assert_eq!(mouse.feed(0x08), None);
        mouse.restart();
        assert_eq!(packet(&mut mouse, [0x08, 0, 1]), Some(Report::Relative { dx: 0, dy: -1, buttons: 0 }));
    }

    #[test]
    fn a_virtio_mouse_reports_at_each_sync() {
        use codes::*;
        let mut events = Events::new((0, 0), (0, 0));
        assert_eq!(events.feed(EV_REL, REL_X, 3), None);
        assert_eq!(events.feed(EV_REL, REL_X, 4), None);
        assert_eq!(events.feed(EV_REL, REL_Y, (-2i32) as u32), None);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Relative { dx: 7, dy: -2, buttons: 0 }));
        // Nothing new: no report.
        assert_eq!(events.feed(EV_SYN, 0, 0), None);
        // A button alone, then held while moving, then let go.
        assert_eq!(events.feed(EV_KEY, BTN_LEFT, 1), None);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Relative { dx: 0, dy: 0, buttons: LEFT }));
        events.feed(EV_REL, REL_Y, 1);
        events.feed(EV_KEY, BTN_RIGHT, 1);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Relative { dx: 0, dy: 1, buttons: LEFT | RIGHT }));
        events.feed(EV_KEY, BTN_LEFT, 0);
        events.feed(EV_KEY, BTN_MIDDLE, 2);
        assert_eq!(events.feed(EV_SYN, 0, 0).map(|report| report.buttons()), Some(RIGHT | MIDDLE));
        // Codes it does not know change nothing.
        assert_eq!(events.feed(EV_REL, 8, 1), None); // the wheel
        assert_eq!(events.feed(EV_KEY, 30, 1), None); // a keyboard's 'a'
        assert_eq!(events.feed(EV_SYN, 0, 0), None);
    }

    #[test]
    fn a_tablet_is_scaled_to_one_range() {
        use codes::*;
        // QEMU's tablet: 0..=32767 on both axes.
        let mut events = Events::new((0, 32767), (0, 32767));
        events.feed(EV_ABS, ABS_X, 16384);
        events.feed(EV_ABS, ABS_Y, 0);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Absolute { x: 16384, y: 0, buttons: 0 }));
        // Only Y moved: X stays.
        events.feed(EV_ABS, ABS_Y, 32767);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Absolute { x: 16384, y: ABSOLUTE_MAX, buttons: 0 }));
        // Another device's range, and a value outside it.
        let mut events = Events::new((100, 300), (-50, 50));
        events.feed(EV_ABS, ABS_X, 200);
        events.feed(EV_ABS, ABS_Y, 1000);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Absolute { x: ABSOLUTE_MAX / 2, y: ABSOLUTE_MAX, buttons: 0 }));
        // A range that says nothing: the value as it is, clamped.
        let mut events = Events::new((0, 0), (5, 5));
        events.feed(EV_ABS, ABS_X, 40000);
        events.feed(EV_ABS, ABS_Y, 7);
        assert_eq!(events.feed(EV_SYN, 0, 0), Some(Report::Absolute { x: ABSOLUTE_MAX, y: 7, buttons: 0 }));
        assert_eq!(scale(i32::MIN, (i32::MIN, i32::MAX)), 0);
        assert_eq!(scale(i32::MAX, (i32::MIN, i32::MAX)), ABSOLUTE_MAX);
    }
}
