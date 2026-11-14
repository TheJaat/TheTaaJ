//! What interrupts do, independent of the hardware that raises them.
//!
//! Handlers here run with interrupts disabled, on whatever stack was
//! interrupted, so they do the minimum: count a tick, queue a byte. The
//! real work happens later, in normal code. (Once there are user-mode
//! drivers, an interrupt will instead wake the driver's thread.)

use core::sync::atomic::{AtomicU64, Ordering};

use kernel64::queue::Queue;

/// Timer interrupts per second.
pub const HZ: u64 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// Raw bytes from the keyboard, waiting to be decoded. The interrupt
/// handler is the only producer, the main loop the only consumer.
pub static KEYBOARD: Queue<256> = Queue::new();

pub fn timer_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn keyboard_byte(byte: u8) {
    // A full queue drops the byte: losing a keystroke is better than
    // blocking in an interrupt handler.
    let _ = KEYBOARD.push(byte);
}
