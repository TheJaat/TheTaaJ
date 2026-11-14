//! PCI configuration space on a PC: "configuration mechanism #1".
//!
//! Write the function's address and the register to 0xCF8, then read or
//! write the 32-bit register at 0xCFC. Two accesses per register, so the
//! pair must not be split by another user of the same ports - hence the
//! lock. (PCI Express also maps configuration space into memory - ECAM,
//! found through the ACPI MCFG table - with 4 KiB per function instead
//! of 256 bytes. The kernel needs nothing past the first 256 yet.)
//!
//! Ports 0xCF8-0xCFF stay the kernel's: a process that could write them
//! could reprogram any device's BARs, and with them where it does DMA.

use kernel64::pci::{Address, ConfigSpace};

use super::port::{inl, outl};
use crate::irqlock::IrqLock;

const ADDRESS: u16 = 0xCF8;
const DATA: u16 = 0xCFC;

static LOCK: IrqLock<()> = IrqLock::new(());

/// Configuration space through the two ports.
pub struct Ports;

fn select(address: Address, offset: u8) {
    let value = 1 << 31 // enable
        | u32::from(address.bus) << 16
        | u32::from(address.device & 0x1F) << 11
        | u32::from(address.function & 0x7) << 8
        | u32::from(offset & 0xFC);
    // SAFETY: 0xCF8 is the configuration address register on every PC.
    unsafe { outl(ADDRESS, value) };
}

impl ConfigSpace for Ports {
    fn read32(&mut self, address: Address, offset: u8) -> u32 {
        let _guard = LOCK.lock();
        select(address, offset);
        // SAFETY: reads the register just selected; config reads have no
        // side effects.
        unsafe { inl(DATA) }
    }

    fn write32(&mut self, address: Address, offset: u8, value: u32) {
        let _guard = LOCK.lock();
        select(address, offset);
        // SAFETY: the kernel's own device setup is the only writer.
        unsafe { outl(DATA, value) };
    }
}
