//! The PIT (8253/8254 Programmable Interval Timer) - only as a ruler.
//!
//! The local APIC timer counts at the CPU's bus clock, which differs
//! from machine to machine and is not reported anywhere reliable. The
//! PIT's input clock is the same on every PC since 1981: 1,193,182 Hz.
//! So: start both, wait for the PIT to measure off 10 ms, and see how
//! far the APIC timer (and the TSC) got. That gives their frequencies.
//!
//! Channel 2 is used because its output can be read back through port
//! 0x61 (it was the PC speaker's tone generator) - so this needs no
//! interrupt, and works before interrupts are set up.

use super::port::{inb, outb};

pub const FREQUENCY: u64 = 1_193_182;

const CHANNEL_2: u16 = 0x42;
const COMMAND: u16 = 0x43;
const SPEAKER: u16 = 0x61; // bit 0: channel 2 gate; bit 1: speaker on; bit 5: channel 2 output

/// Starts a one-shot countdown of `microseconds` on channel 2.
///
/// # Safety
/// Programs timer hardware; nothing else may be using channel 2.
pub unsafe fn start(microseconds: u64) {
    let count = (FREQUENCY * microseconds / 1_000_000).min(0xFFFF) as u16;
    // Gate off and speaker off while programming.
    let control = inb(SPEAKER) & !0b11;
    outb(SPEAKER, control);
    // Channel 2, low byte then high byte, mode 0 ("interrupt on terminal
    // count": output goes high when the count reaches 0), binary.
    outb(COMMAND, 0b1011_0000);
    outb(CHANNEL_2, count as u8);
    outb(CHANNEL_2, (count >> 8) as u8);
    // Raising the gate starts the count.
    outb(SPEAKER, control | 1);
}

/// Waits until the countdown started by `start` reaches zero.
///
/// # Safety
/// `start` must have been called.
pub unsafe fn wait() {
    while inb(SPEAKER) & 0x20 == 0 {
        core::hint::spin_loop();
    }
}
