//! The PS/2 controller (the "8042"): where the keyboard's bytes arrive.
//!
//! Two ports: 0x64 for status and commands to the controller itself,
//! 0x60 for data. Status bit 0 says a byte is waiting to be read; bit 1
//! says the controller has not yet taken the last byte written.
//!
//! USB keyboards on real PCs usually appear here too, through the
//! firmware's "legacy USB" emulation - until the OS has a USB driver.

use super::port::{inb, outb};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;

const OUTPUT_FULL: u8 = 1 << 0;
const INPUT_FULL: u8 = 1 << 1;

const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const DISABLE_PORT_2: u8 = 0xA7;
const DISABLE_PORT_1: u8 = 0xAD;
const ENABLE_PORT_1: u8 = 0xAE;

const CONFIG_PORT_1_INTERRUPT: u8 = 1 << 0;
const CONFIG_PORT_2_INTERRUPT: u8 = 1 << 1;
const CONFIG_PORT_1_CLOCK_OFF: u8 = 1 << 4;
const CONFIG_TRANSLATION: u8 = 1 << 6;

/// A missing controller never sets its status bits; give up after this
/// many polls instead of hanging.
const PATIENCE: u32 = 100_000;

unsafe fn wait_writable() -> bool {
    (0..PATIENCE).any(|_| inb(STATUS) & INPUT_FULL == 0)
}

unsafe fn wait_readable() -> bool {
    (0..PATIENCE).any(|_| inb(STATUS) & OUTPUT_FULL != 0)
}

unsafe fn command(byte: u8) -> bool {
    if !wait_writable() {
        return false;
    }
    outb(COMMAND, byte);
    true
}

/// Sets the controller up for keyboard interrupts in scancode set 1.
/// False if no controller answers.
///
/// # Safety
/// Interrupts disabled.
pub unsafe fn init() -> bool {
    // Quiet both ports while reconfiguring, and drop any stale byte.
    if !command(DISABLE_PORT_1) || !command(DISABLE_PORT_2) {
        return false;
    }
    while inb(STATUS) & OUTPUT_FULL != 0 {
        let _ = inb(DATA);
    }

    if !command(READ_CONFIG) || !wait_readable() {
        return false;
    }
    let mut config = inb(DATA);
    config |= CONFIG_PORT_1_INTERRUPT | CONFIG_TRANSLATION;
    config &= !(CONFIG_PORT_2_INTERRUPT | CONFIG_PORT_1_CLOCK_OFF);
    if !command(WRITE_CONFIG) || !wait_writable() {
        return false;
    }
    outb(DATA, config);

    command(ENABLE_PORT_1)
}

/// The waiting byte, if there is one.
pub fn read() -> Option<u8> {
    // SAFETY: reading the 8042's status and data ports has no effect
    // beyond taking the byte.
    unsafe { (inb(STATUS) & OUTPUT_FULL != 0).then(|| inb(DATA)) }
}
