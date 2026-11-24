//! What a USB device says about itself: its descriptors, and the
//! standard requests that fetch them.
//!
//! The device descriptor (18 bytes) says who it is: its USB version,
//! endpoint 0's packet size, vendor and product. The configuration
//! descriptor comes with everything under it, one after another, each
//! with its length first and its kind second:
//!
//! ```text
//!   configuration (9): total length, its number
//!     interface (9): number, class, subclass, protocol
//!       (a class's own descriptors - HID's, say)
//!       endpoint (7): address (bit 7: IN), kind, packet size, interval
//!     interface ...
//! ```

use crate::xhci::Setup;

/// Descriptor kinds.
pub const DEVICE: u8 = 1;
pub const CONFIGURATION: u8 = 2;
pub const INTERFACE: u8 = 4;
pub const ENDPOINT: u8 = 5;

/// Standard requests.
pub const GET_DESCRIPTOR: u8 = 6;
pub const SET_CONFIGURATION: u8 = 9;
/// HID class requests.
pub const HID_SET_IDLE: u8 = 0x0A;
pub const HID_SET_PROTOCOL: u8 = 0x0B;

/// Interface classes, as far as this driver cares.
pub const CLASS_HID: u8 = 3;
pub const CLASS_HUB: u8 = 9;
pub const CLASS_STORAGE: u8 = 8;
/// HID: the boot subclass, and its two protocols.
pub const HID_BOOT: u8 = 1;
pub const HID_KEYBOARD: u8 = 1;
pub const HID_MOUSE: u8 = 2;

/// "Give me descriptor `kind`, `length` bytes of it."
pub const fn get_descriptor(kind: u8, length: u16) -> Setup {
    Setup { request_type: 0x80, request: GET_DESCRIPTOR, value: (kind as u16) << 8, index: 0, length }
}

pub const fn set_configuration(value: u8) -> Setup {
    Setup { request_type: 0x00, request: SET_CONFIGURATION, value: value as u16, index: 0, length: 0 }
}

/// HID: report in the boot protocol (0), not the report protocol.
pub const fn set_boot_protocol(interface: u8) -> Setup {
    Setup { request_type: 0x21, request: HID_SET_PROTOCOL, value: 0, index: interface as u16, length: 0 }
}

/// HID: report only when something changes (idle rate 0).
pub const fn set_idle(interface: u8) -> Setup {
    Setup { request_type: 0x21, request: HID_SET_IDLE, value: 0, index: interface as u16, length: 0 }
}

/// The device descriptor's fields used here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub usb: u16,
    pub class: u8,
    /// Endpoint 0's packet size: bytes, or (USB 3) a power of two.
    pub packet0: u8,
    pub vendor: u16,
    pub product: u16,
    pub configurations: u8,
}

pub fn device(bytes: &[u8]) -> Option<Device> {
    if bytes.len() < 18 || bytes[1] != DEVICE {
        return None;
    }
    let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    Some(Device { usb: word(2), class: bytes[4], packet0: bytes[7], vendor: word(8), product: word(10), configurations: bytes[17] })
}

/// One interface of a configuration, and its first interrupt IN
/// endpoint if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    /// (address, packet size, bInterval)
    pub interrupt_in: Option<(u8, u16, u8)>,
}

/// A configuration: its number, and its interfaces (the first eight).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub total: u16,
    pub interfaces: [Option<Interface>; 8],
}

/// The total length a configuration's first nine bytes give.
pub fn configuration_length(bytes: &[u8]) -> Option<u16> {
    (bytes.len() >= 9 && bytes[1] == CONFIGURATION).then(|| u16::from_le_bytes([bytes[2], bytes[3]]))
}

/// Walks a configuration and everything under it. A descriptor whose
/// length is 0 or runs past the end ends the walk - a device's mistake
/// must not become a loop or a read past the buffer.
pub fn configuration(bytes: &[u8]) -> Option<Configuration> {
    let total = configuration_length(bytes)?;
    let bytes = &bytes[..usize::from(total).min(bytes.len())];
    let mut config = Configuration { value: bytes[5], total, interfaces: [None; 8] };
    let mut current: Option<usize> = None;
    let mut count = 0;
    let mut at = 0;
    while at + 2 <= bytes.len() {
        let length = usize::from(bytes[at]);
        if length < 2 || at + length > bytes.len() {
            break;
        }
        let d = &bytes[at..at + length];
        match d[1] {
            INTERFACE if length >= 9 && count < config.interfaces.len() => {
                // An alternate setting (byte 3 non-zero) is not the
                // interface as configured: skipped.
                if d[3] == 0 {
                    config.interfaces[count] =
                        Some(Interface { number: d[2], class: d[5], subclass: d[6], protocol: d[7], interrupt_in: None });
                    current = Some(count);
                    count += 1;
                } else {
                    current = None;
                }
            }
            ENDPOINT if length >= 7 => {
                let interrupt = d[3] & 0x3 == 3;
                let inward = d[2] & 0x80 != 0;
                if let Some(Some(interface)) = current.map(|n| &mut config.interfaces[n]) {
                    if interrupt && inward && interface.interrupt_in.is_none() {
                        interface.interrupt_in = Some((d[2], u16::from_le_bytes([d[4], d[5]]), d[6]));
                    }
                }
            }
            _ => {}
        }
        at += length;
    }
    Some(config)
}
