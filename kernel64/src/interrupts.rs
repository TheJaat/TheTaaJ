//! What interrupts do, independent of the hardware that raises them.
//!
//! Handlers here run with interrupts disabled, on whatever stack was
//! interrupted, so they do the minimum: count a tick, or note that a
//! device interrupted and wake its driver. Device drivers run in user
//! mode; the kernel never touches their hardware.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::threads::WaitQueue;

/// Timer interrupts per second.
pub const HZ: u64 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn timer_tick() {
    let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::threads::on_tick(now);
}

/// The last thing every interrupt does, after acknowledging it: if the
/// interrupt made a thread ready that should run now - a sleeper's time
/// came, a slice ran out, a driver has work - switch.
pub fn interrupt_done() {
    crate::threads::preempt_if_needed();
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// A device's interrupt, as a kernel object a driver holds a handle to.
///
/// The handler only counts; the driver waits for the count to become
/// non-zero, then talks to its device. The count is what makes this
/// race-free: an interrupt that arrives while the driver is still busy
/// with the last one is not lost - the next wait returns at once.
///
/// This suits edge-triggered lines like the ISA keyboard's, where one
/// interrupt is one event. A level-triggered line (most PCI devices)
/// keeps interrupting until the device is serviced, so it will need to
/// be masked here and unmasked by the driver's acknowledgement.
pub struct InterruptLine {
    pending: AtomicU64,
    waiters: WaitQueue,
}

impl InterruptLine {
    const fn new() -> InterruptLine {
        InterruptLine { pending: AtomicU64::new(0), waiters: WaitQueue::new() }
    }

    /// Fired and not yet waited for?
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) > 0
    }

    pub fn waiters(&self) -> &WaitQueue {
        &self.waiters
    }

    /// From the interrupt handler.
    pub fn fire(&self) {
        self.pending.fetch_add(1, Ordering::Relaxed);
        self.waiters.wake_all();
    }

    /// Waits until the line has fired; returns how many times it did
    /// since the last wait.
    pub fn wait(&self) -> u64 {
        loop {
            self.waiters.wait_while(|| self.pending.load(Ordering::Relaxed) == 0);
            let count = self.pending.swap(0, Ordering::Relaxed);
            if count > 0 {
                return count; // else another holder took it; wait again
            }
        }
    }
}

/// ISA IRQ 1.
pub static KEYBOARD: InterruptLine = InterruptLine::new();
