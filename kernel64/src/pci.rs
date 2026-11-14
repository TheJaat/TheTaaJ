//! PCI: finding the devices on the bus, and what each one needs.
//!
//! Every PCI function has a 256-byte configuration space: who made it
//! (vendor, device), what it is (class), where its registers are (the
//! BARs - base address registers), and a linked list of capabilities.
//! The firmware has already assigned every BAR an address; the kernel
//! reads them, sizes them, and turns them into capabilities of its own:
//! an I/O port range, a memory object, an interrupt.
//!
//! How configuration space is reached is the platform's business (on a
//! PC, ports 0xCF8 and 0xCFC); this module only needs `ConfigSpace`, so
//! it is tested on the host against a simulated bus.
//!
//! Interrupts: a PCI device's legacy interrupt pin is wired, through the
//! chipset, to an I/O APIC input that only the ACPI tables' AML code can
//! name, and it is level-triggered - it stays asserted until the driver
//! services the device. MSI-X avoids both problems: the device raises an
//! interrupt by writing a message - a vector - straight to a CPU's local
//! APIC, like a memory write. No routing, no shared lines, and each
//! message is one event (edge-like). It is what modern devices use, and
//! the only kind this kernel sets up.

use alloc::vec::Vec;

/// Bus, device (0-31) and function (0-7): where a function is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}

/// Reads and writes of configuration space, in aligned 32-bit words.
pub trait ConfigSpace {
    fn read32(&mut self, address: Address, offset: u8) -> u32;
    fn write32(&mut self, address: Address, offset: u8, value: u32);

    fn read16(&mut self, address: Address, offset: u8) -> u16 {
        (self.read32(address, offset & !3) >> ((offset & 2) * 8)) as u16
    }

    fn read8(&mut self, address: Address, offset: u8) -> u8 {
        (self.read32(address, offset & !3) >> ((offset & 3) * 8)) as u8
    }
}

const VENDOR: u8 = 0x00;
const COMMAND: u8 = 0x04;
const STATUS: u8 = 0x06;
const CLASS: u8 = 0x08;
const HEADER_TYPE: u8 = 0x0E;
const BAR0: u8 = 0x10;
const CAPABILITIES: u8 = 0x34;

/// Command register bits.
pub const COMMAND_IO: u16 = 1 << 0;
pub const COMMAND_MEMORY: u16 = 1 << 1;
pub const COMMAND_BUS_MASTER: u16 = 1 << 2;
/// Legacy (pin) interrupts off: MSI-X only.
pub const COMMAND_INTX_DISABLE: u16 = 1 << 10;

const STATUS_CAPABILITIES: u16 = 1 << 4;
pub const CAPABILITY_MSIX: u8 = 0x11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub address: Address,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

impl Device {
    /// A short name for the boot log.
    pub fn kind(&self) -> &'static str {
        match (self.class, self.subclass) {
            (0x01, 0x00) => "SCSI storage",
            (0x01, 0x01) => "IDE controller",
            (0x01, 0x06) => "SATA controller",
            (0x01, 0x08) => "NVMe controller",
            (0x01, _) => "storage",
            (0x02, _) => "network",
            (0x03, _) => "display",
            (0x04, _) => "multimedia",
            (0x06, 0x00) => "host bridge",
            (0x06, 0x01) => "ISA bridge",
            (0x06, 0x04) => "PCI bridge",
            (0x06, _) => "bridge",
            (0x0C, 0x03) => "USB controller",
            (0x0C, 0x05) => "SMBus",
            (0x0C, _) => "serial bus",
            _ => "other",
        }
    }
}

/// Every function on every bus. Brute force - 256 buses x 32 devices -
/// rather than following bridges: simpler, and still only a few thousand
/// reads, once, at boot.
pub fn scan(config: &mut impl ConfigSpace) -> Vec<Device> {
    let mut found = Vec::new();
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            let first = Address { bus, device, function: 0 };
            if config.read16(first, VENDOR) == 0xFFFF {
                continue; // nothing in this slot
            }
            // Bit 7 of the header type: more than one function.
            let functions = if config.read8(first, HEADER_TYPE) & 0x80 != 0 { 8 } else { 1 };
            for function in 0..functions {
                let address = Address { bus, device, function };
                let id = config.read32(address, VENDOR);
                if id & 0xFFFF == 0xFFFF {
                    continue;
                }
                let class = config.read32(address, CLASS);
                found.push(Device {
                    address,
                    vendor: id as u16,
                    device: (id >> 16) as u16,
                    class: (class >> 24) as u8,
                    subclass: (class >> 16) as u8,
                    prog_if: (class >> 8) as u8,
                    header_type: config.read8(address, HEADER_TYPE) & 0x7F,
                });
            }
        }
    }
    found
}

/// Where a BAR points, and how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bar {
    Io { base: u32, size: u32 },
    Memory { base: u64, size: u64, prefetchable: bool },
}

pub fn command(config: &mut impl ConfigSpace, address: Address) -> u16 {
    config.read16(address, COMMAND)
}

/// Writes the command register. The status register shares its word,
/// and its bits are cleared by writing 1 - so the word is written with
/// status 0, which changes nothing there.
pub fn set_command(config: &mut impl ConfigSpace, address: Address, value: u16) {
    config.write32(address, COMMAND, u32::from(value));
}

/// BAR `index` (0-5) of an ordinary device (header type 0): None if it
/// is unused. A 64-bit memory BAR takes two slots; `index` must be the
/// first.
///
/// The size is found the standard way: write all ones, read back which
/// bits stuck - the device hard-wires the low bits of its size to zero -
/// and put the original back. Decoding is off meanwhile, so the device
/// never answers at the all-ones address.
pub fn bar(config: &mut impl ConfigSpace, address: Address, index: u8) -> Option<Bar> {
    if index > 5 {
        return None;
    }
    let offset = BAR0 + 4 * index;
    let saved_command = command(config, address);
    set_command(config, address, saved_command & !(COMMAND_IO | COMMAND_MEMORY));

    let probe = |config: &mut dyn FnMut(u8, Option<u32>) -> u32, offset: u8| {
        let original = config(offset, None);
        config(offset, Some(u32::MAX));
        let mask = config(offset, None);
        config(offset, Some(original));
        (original, mask)
    };
    let mut access = |offset: u8, value: Option<u32>| match value {
        Some(value) => {
            config.write32(address, offset, value);
            0
        }
        None => config.read32(address, offset),
    };

    let (original, mask) = probe(&mut access, offset);
    let result = if mask == 0 {
        None
    } else if original & 1 == 1 {
        // I/O: bits 1:0 are flags; ports are 16-bit, whatever the BAR.
        let size = (!(mask & !0x3) & 0xFFFF).wrapping_add(1);
        Some(Bar::Io { base: original & !0x3, size })
    } else {
        let prefetchable = original & 0x8 != 0;
        let low_base = u64::from(original & !0xF);
        let low_mask = u64::from(mask & !0xF);
        if (original >> 1) & 0x3 == 0x2 && index < 5 {
            // 64-bit: the next BAR is the high half.
            let (high, high_mask) = probe(&mut access, offset + 4);
            let base = low_base | u64::from(high) << 32;
            let mask = low_mask | u64::from(high_mask) << 32;
            Some(Bar::Memory { base, size: (!mask).wrapping_add(1), prefetchable })
        } else {
            let size = (!(low_mask as u32)).wrapping_add(1);
            Some(Bar::Memory { base: low_base, size: u64::from(size), prefetchable })
        }
    };
    set_command(config, address, saved_command);
    result.filter(|bar| !matches!(bar, Bar::Io { size: 0, .. } | Bar::Memory { size: 0, .. }))
}

/// The capability list: (id, offset) of each. At most 48 entries - the
/// most 192 bytes of capabilities can hold - so a corrupt list that
/// loops cannot hang the kernel.
pub fn capabilities(config: &mut impl ConfigSpace, address: Address) -> Vec<(u8, u8)> {
    let mut list = Vec::new();
    if config.read16(address, STATUS) & STATUS_CAPABILITIES == 0 {
        return list;
    }
    let mut offset = config.read8(address, CAPABILITIES) & 0xFC;
    while offset >= 0x40 && list.len() < 48 {
        let word = config.read32(address, offset);
        list.push((word as u8, offset));
        offset = (word >> 8) as u8 & 0xFC;
    }
    list
}

/// A function's MSI-X capability: where its table of messages is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsiX {
    /// The capability's offset in configuration space.
    pub capability: u8,
    /// Which BAR the table is in, and where in it.
    pub table_bar: u8,
    pub table_offset: u32,
    pub entries: u16,
}

/// Each table entry: address (low, high), data, vector control.
pub const MSIX_ENTRY_SIZE: u64 = 16;
const MSIX_ENABLE: u32 = 1 << 15;
const MSIX_FUNCTION_MASK: u32 = 1 << 14;

pub fn msix(config: &mut impl ConfigSpace, address: Address) -> Option<MsiX> {
    let (_, capability) = capabilities(config, address).into_iter().find(|&(id, _)| id == CAPABILITY_MSIX)?;
    let control = config.read32(address, capability) >> 16;
    let table = config.read32(address, capability + 4);
    Some(MsiX {
        capability,
        table_bar: (table & 0x7) as u8,
        table_offset: table & !0x7,
        entries: (control & 0x7FF) as u16 + 1,
    })
}

/// Turns MSI-X on (and the whole-function mask off). Program the table
/// entries first: from now on the device may send them.
pub fn enable_msix(config: &mut impl ConfigSpace, address: Address, msix: &MsiX) {
    let word = config.read32(address, msix.capability);
    config.write32(address, msix.capability, (word | MSIX_ENABLE << 16) & !(MSIX_FUNCTION_MASK << 16));
}

/// The message that raises `vector` on the CPU whose local APIC id is
/// `apic`: an address in the APIC's 0xFEE window naming the CPU, and
/// data naming the vector (fixed delivery, edge).
pub fn msi_message(apic: u8, vector: u8) -> (u64, u32) {
    (0xFEE0_0000 | u64::from(apic) << 12, u32::from(vector))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A simulated function: 64 words of configuration space, and BAR
    /// sizes, so sizing behaves as real hardware does.
    #[derive(Clone)]
    struct Function {
        words: [u32; 64],
        /// Size of each BAR slot's region (0 = unused); for a 64-bit BAR,
        /// slot n holds the size and slot n+1 the marker u64::MAX.
        sizes: [u64; 6],
    }

    #[derive(Default)]
    struct Bus(BTreeMap<Address, Function>);

    impl ConfigSpace for Bus {
        fn read32(&mut self, address: Address, offset: u8) -> u32 {
            self.0.get(&address).map_or(u32::MAX, |f| f.words[offset as usize / 4])
        }

        fn write32(&mut self, address: Address, offset: u8, value: u32) {
            let Some(f) = self.0.get_mut(&address) else { return };
            let word = offset as usize / 4;
            match word {
                1 => f.words[1] = (f.words[1] & 0xFFFF_0000) | (value & 0xFFFF), // status read-only here
                4..=9 => {
                    let slot = word - 4;
                    let (size, flags_mask, high) = match f.sizes[slot] {
                        u64::MAX => (f.sizes[slot - 1], 0, true),
                        size => (size, if f.words[word] & 1 == 1 { 0x3 } else { 0xF }, false),
                    };
                    let mask = (!(size.max(1) - 1)) >> if high { 32 } else { 0 };
                    let flags = f.words[word] & flags_mask;
                    f.words[word] = if size == 0 { 0 } else { (value & mask as u32) | flags };
                }
                _ => f.words[word] = value,
            }
        }
    }

    fn function(vendor: u16, device: u16, class: u32, multi: bool) -> Function {
        let mut words = [0u32; 64];
        words[0] = u32::from(device) << 16 | u32::from(vendor);
        words[2] = class << 8;
        words[3] = if multi { 0x80 << 16 } else { 0 };
        Function { words, sizes: [0; 6] }
    }

    fn at(bus: u8, device: u8, function: u8) -> Address {
        Address { bus, device, function }
    }

    #[test]
    fn scan_finds_every_function() {
        let mut bus = Bus::default();
        bus.0.insert(at(0, 0, 0), function(0x8086, 0x1237, 0x06_00_00, false));
        bus.0.insert(at(0, 1, 0), function(0x8086, 0x7000, 0x06_01_00, true));
        bus.0.insert(at(0, 1, 1), function(0x8086, 0x7010, 0x01_01_80, false));
        bus.0.insert(at(0, 1, 3), function(0x8086, 0x7113, 0x06_80_00, false));
        bus.0.insert(at(0, 4, 0), function(0x1AF4, 0x1001, 0x01_00_00, false));
        // A function 2 behind a single-function device is not looked at.
        bus.0.insert(at(0, 4, 2), function(0xDEAD, 0xBEEF, 0, false));
        bus.0.insert(at(3, 31, 0), function(0x1234, 0x5678, 0x02_00_00, false));
        let found = scan(&mut bus);
        let addresses: Vec<Address> = found.iter().map(|d| d.address).collect();
        assert_eq!(addresses, vec![at(0, 0, 0), at(0, 1, 0), at(0, 1, 1), at(0, 1, 3), at(0, 4, 0), at(3, 31, 0)]);
        let virtio = found[4];
        assert_eq!((virtio.vendor, virtio.device, virtio.class, virtio.subclass), (0x1AF4, 0x1001, 1, 0));
        assert_eq!(found[2].prog_if, 0x80);
        assert_eq!(found[2].kind(), "IDE controller");
    }

    #[test]
    fn bars_are_decoded_and_sized() {
        let mut bus = Bus::default();
        let mut f = function(0x1AF4, 0x1001, 0x01_00_00, false);
        f.words[1] = 0x0010_0007; // capabilities; IO, memory, bus master on
        f.words[4] = 0xC041; // I/O at 0xC040
        f.sizes[0] = 0x40;
        f.words[5] = 0xFEBD_1000; // 32-bit memory
        f.sizes[1] = 0x1000;
        f.words[8] = 0xFE00_000C; // 64-bit prefetchable...
        f.words[9] = 0x0000_0001; // ...at 0x1_FE00_0000
        f.sizes[4] = 0x4000;
        f.sizes[5] = u64::MAX;
        let a = at(0, 4, 0);
        bus.0.insert(a, f);
        assert_eq!(bar(&mut bus, a, 0), Some(Bar::Io { base: 0xC040, size: 0x40 }));
        assert_eq!(bar(&mut bus, a, 1), Some(Bar::Memory { base: 0xFEBD_1000, size: 0x1000, prefetchable: false }));
        assert_eq!(bar(&mut bus, a, 2), None, "unused");
        assert_eq!(bar(&mut bus, a, 4), Some(Bar::Memory { base: 0x1_FE00_0000, size: 0x4000, prefetchable: true }));
        assert_eq!(bar(&mut bus, a, 6), None);
        // Sizing put everything back, decoding included.
        assert_eq!(bus.read32(a, BAR0), 0xC041);
        assert_eq!(bus.read32(a, BAR0 + 0x14), 1);
        assert_eq!(command(&mut bus, a), 0x0007);
    }

    #[test]
    fn capabilities_and_msix() {
        let mut bus = Bus::default();
        let mut f = function(0x1AF4, 0x1001, 0x01_00_00, false);
        f.words[1] = 0x0010_0000;
        f.words[CAPABILITIES as usize / 4] = 0x40;
        f.words[0x40 / 4] = 0x0000_5009; // vendor-specific (9), next 0x50
        f.words[0x50 / 4] = 0x0001_0011; // MSI-X, last; 2 entries
        f.words[0x54 / 4] = 0x0000_0001; // table in BAR1 at offset 0
        let a = at(0, 4, 0);
        bus.0.insert(a, f);
        assert_eq!(capabilities(&mut bus, a), vec![(0x09, 0x40), (0x11, 0x50)]);
        let m = msix(&mut bus, a).unwrap();
        assert_eq!(m, MsiX { capability: 0x50, table_bar: 1, table_offset: 0, entries: 2 });
        enable_msix(&mut bus, a, &m);
        assert_eq!(bus.read32(a, 0x50) >> 16, 0x8001, "enabled, entry count kept");
        assert_eq!(msi_message(3, 0x30), (0xFEE0_3000, 0x30));
    }

    #[test]
    fn a_looping_capability_list_ends() {
        let mut bus = Bus::default();
        let mut f = function(1, 2, 0, false);
        f.words[1] = 0x0010_0000;
        f.words[CAPABILITIES as usize / 4] = 0x40;
        f.words[0x40 / 4] = 0x0000_4005; // points at itself
        let a = at(0, 0, 0);
        bus.0.insert(a, f);
        assert_eq!(capabilities(&mut bus, a).len(), 48);
        assert_eq!(msix(&mut bus, a), None);
    }
}
