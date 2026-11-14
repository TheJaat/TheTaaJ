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
//! A bitmap for all 65536 ports is 8 KiB. Copying that on every switch
//! would cost more than the switch itself, so a process's bitmap keeps
//! only the bytes up to its highest allowed port - a keyboard driver's is
//! 13 bytes, most processes' none at all - and the TSS rewrites only the
//! bytes that differ between the old process's extent and the new one's.

use alloc::vec::Vec;
use core::ops::Range;

/// Every port there is.
pub const PORTS: usize = 65536;
pub const BYTES: usize = PORTS / 8;

/// The bytes up to the last allowed port; every port past them refused.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct PortBitmap(Vec<u8>);

impl PortBitmap {
    /// Every port refused: what kernel threads and new processes get.
    pub const NONE: PortBitmap = PortBitmap(Vec::new());

    /// Allows `ports`. False (and nothing changed) if the range is empty
    /// or runs past the last port.
    pub fn allow(&mut self, ports: Range<u32>) -> bool {
        if ports.is_empty() || ports.end as usize > PORTS {
            return false;
        }
        let needed = (ports.end as usize).div_ceil(8);
        if self.0.len() < needed {
            self.0.resize(needed, 0xFF);
        }
        for port in ports {
            self.0[port as usize / 8] &= !(1 << (port % 8));
        }
        true
    }

    pub fn allows(&self, port: u16) -> bool {
        self.0.get(port as usize / 8).is_some_and(|byte| byte & (1 << (port % 8)) == 0)
    }

    /// The bits as the CPU reads them, as far as any port is allowed.
    /// Every byte after these is 0xFF.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_refused_until_allowed() {
        let mut map = PortBitmap::NONE;
        assert!((0..=u16::MAX).all(|p| !map.allows(p)));
        assert!(map.allow(0x60..0x61));
        assert!(map.allow(0x64..0x65));
        let allowed: Vec<u16> = (0..=u16::MAX).filter(|&p| map.allows(p)).collect();
        assert_eq!(allowed, vec![0x60, 0x64], "and not 0x61-0x63 between them");
        assert_eq!(map.bytes()[0x60 / 8], !0b0001_0001);
        assert_eq!(map.bytes().len(), 13, "only as far as the last allowed port");
    }

    #[test]
    fn high_ports_too() {
        let mut map = PortBitmap::NONE;
        assert!(map.allow(0xC040..0xC080));
        assert!(map.allows(0xC040) && map.allows(0xC07F));
        assert!(!map.allows(0xC03F) && !map.allows(0xC080));
        assert!(!map.allows(0xCF8), "PCI configuration space stays the kernel's");
        assert!(map.allow(0xFFFF..0x10000), "the very last port");
        assert_eq!(map.bytes().len(), BYTES);
    }

    #[test]
    fn silly_ranges_are_refused() {
        let mut map = PortBitmap::NONE;
        assert!(!map.allow(0x100..0x100), "empty");
        assert!(!map.allow(0xFFF0..0x10001), "past the last port");
        assert!(map == PortBitmap::NONE, "nothing half-allowed");
    }
}
