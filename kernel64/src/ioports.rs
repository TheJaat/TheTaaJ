//! I/O port permissions: which ports a process may use directly.
//!
//! x86 has a second address space besides memory: 65536 I/O ports, read
//! and written with `in` and `out`. Legacy devices live there - the PS/2
//! controller at 0x60/0x64, the serial ports at 0x3F8. The kernel itself
//! can always use them; ring 3 can only if the CPU's I/O permission
//! bitmap allows that port.
//!
//! The bitmap lives at the end of the TSS: one bit per port, 0 = allowed,
//! 1 = refused (#GP). The kernel keeps one bitmap per process and loads
//! it into the TSS when switching to it, so a driver can talk to its
//! device without a system call per byte - and only to its own device.
//!
//! A bitmap for all 65536 ports would be 8 KiB, copied on every switch.
//! Ports past the end of the bitmap are refused, so a short one is fine:
//! this covers the first 1024, where the legacy devices are.

use core::ops::Range;

/// Ports covered by the bitmap; anything above is always refused.
pub const PORTS: usize = 1024;
pub const BYTES: usize = PORTS / 8;

#[derive(Clone, PartialEq, Eq)]
pub struct PortBitmap([u8; BYTES]);

impl Default for PortBitmap {
    fn default() -> Self {
        PortBitmap::NONE
    }
}

impl PortBitmap {
    /// Every port refused: what kernel threads and new processes get.
    pub const NONE: PortBitmap = PortBitmap([0xFF; BYTES]);

    /// Allows `ports`. False (and nothing changed) if any is above PORTS.
    pub fn allow(&mut self, ports: Range<u16>) -> bool {
        if ports.end as usize > PORTS {
            return false;
        }
        for port in ports {
            self.0[port as usize / 8] &= !(1 << (port % 8));
        }
        true
    }

    pub fn allows(&self, port: u16) -> bool {
        (port as usize) < PORTS && self.0[port as usize / 8] & (1 << (port % 8)) == 0
    }

    /// The bits as the CPU reads them.
    pub const fn bytes(&self) -> &[u8; BYTES] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_refused_until_allowed() {
        let mut map = PortBitmap::NONE;
        assert!((0..PORTS as u16).all(|p| !map.allows(p)));
        assert!(map.allow(0x60..0x61));
        assert!(map.allow(0x64..0x65));
        let allowed: Vec<u16> = (0..PORTS as u16).filter(|&p| map.allows(p)).collect();
        assert_eq!(allowed, vec![0x60, 0x64], "and not 0x61-0x63 between them");
        assert_eq!(map.bytes()[0x60 / 8], !0b0001_0001);
    }

    #[test]
    fn ports_beyond_the_bitmap_cannot_be_allowed() {
        let mut map = PortBitmap::NONE;
        assert!(!map.allow(0x3F8..0x500));
        assert!(map == PortBitmap::NONE, "nothing half-allowed");
        assert!(!map.allows(0xCF8), "PCI configuration space stays the kernel's");
    }
}
