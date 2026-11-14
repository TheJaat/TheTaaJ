//! The 8259 PICs - the interrupt controllers of the 1981 IBM PC, still
//! emulated by every x86 chipset - switched off.
//!
//! The APICs replace them, but they power up active, and their default
//! vectors (8-15) collide with CPU exceptions: IRQ 0 would look exactly
//! like a double fault. So they are reprogrammed to harmless vectors
//! first, then every input is masked. Even masked, they can raise a
//! "spurious" IRQ 7 or 15, which is why the vectors still matter.

use super::port::{inb, outb};

/// Where the PICs' 16 inputs land after remapping: 0xE0-0xEF.
pub const VECTOR_BASE: u8 = 0xE0;

const MASTER_COMMAND: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_COMMAND: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

/// A write to an unused port: a short delay old PICs need between
/// initialisation words.
unsafe fn wait() {
    outb(0x80, 0);
}

/// # Safety
/// Interrupts must be disabled.
pub unsafe fn disable() {
    // The initialisation sequence: ICW1 (start, expect ICW4), ICW2 (vector
    // base), ICW3 (how the two are chained: slave on master's input 2),
    // ICW4 (8086 mode).
    for (command, data, base, chain) in [
        (MASTER_COMMAND, MASTER_DATA, VECTOR_BASE, 0b100),
        (SLAVE_COMMAND, SLAVE_DATA, VECTOR_BASE + 8, 2),
    ] {
        outb(command, 0x11);
        wait();
        outb(data, base);
        wait();
        outb(data, chain);
        wait();
        outb(data, 0x01);
        wait();
    }
    // Mask everything.
    outb(MASTER_DATA, 0xFF);
    outb(SLAVE_DATA, 0xFF);
    let _ = inb(MASTER_DATA);
}
