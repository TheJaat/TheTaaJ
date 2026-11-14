//! I/O ports: x86's second address space, reached only with the `in`
//! and `out` instructions. Older devices - the serial port, the PIC,
//! the PIT, the PS/2 controller - live here instead of in memory.
//!
//! This is exactly the part other architectures do not have. AArch64
//! and RISC-V reach every device through memory, which is why drivers
//! will go through an abstraction rather than calling these directly.

use core::arch::asm;

/// Writes a byte to an I/O port.
///
/// # Safety
/// Writing to a port can do anything the device on it does - including
/// reset the machine. The caller must know what lives at `port`.
pub unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

/// Reads a byte from an I/O port.
///
/// # Safety
/// As for `outb`: reading can have side effects (it often acknowledges
/// something).
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}
