//! The kernel's half of signals: what signal.c (the C half - handlers,
//! masks, sigaction) needs the kernel for, and the calls that are only
//! a system call: alarm and pause.
//!
//! How a signal reaches a C program (abi::signal has the kernel's
//! side): signal.c tells the kernel, with `__taaj_signal_setup`, where
//! to deliver the signals it catches - `__taaj_signal_entry`, a few
//! instructions of assembly in signal.c - and which those are. The
//! kernel interrupts a thread on its way back to the program, and
//! starts it at the entry instead, on its own stack, with the signal
//! and the registers it had. The entry saves the floating-point
//! registers, calls signal.c's `__taaj_signal_dispatch` - which runs
//! the handler, or makes the signal wait if it is blocked - and then
//! asks the kernel to put everything back (signal_return).

use core::ffi::{c_int, c_uint};

use crate::errno::{errno, EINTR, EINVAL};
use core::sync::atomic::Ordering;

/// `__taaj_signal_setup(entry, caught, ignored)`: tells the kernel how
/// this program takes signals (abi::call::SIGNAL_SETUP). 0, or -1.
#[no_mangle]
pub extern "C" fn __taaj_signal_setup(entry: u64, caught: u64, ignored: u64) -> c_int {
    if taaj::signal_setup(entry, caught, ignored).is_ok() { 0 } else { -1 }
}

/// `__taaj_signal_self(signal)`: sends `signal` to this program, through
/// the kernel - which delivers it, if it is caught, before this
/// returns; ends the program, if that is what it does. 0, or -1 with
/// EINVAL for a number that is no signal.
#[no_mangle]
pub extern "C" fn __taaj_signal_self(signal: c_int) -> c_int {
    match taaj::signal_send(abi::signal::SELF, signal as u64) {
        Ok(()) => 0,
        Err(_) => crate::descriptors::fail(EINVAL),
    }
}

/// `alarm(seconds)`: SIGALRM in that many seconds, in place of any alarm
/// set before; 0 only cancels. Returns the seconds the one before had
/// left, rounded up - or 0 if there was none.
#[no_mangle]
pub extern "C" fn alarm(seconds: c_uint) -> c_uint {
    let left = taaj::alarm(u64::from(seconds) * 1_000_000_000).unwrap_or(0);
    left.div_ceil(1_000_000_000) as c_uint
}

/// `pause()`: waits for a signal - one that runs a handler, that is: one
/// that ends the program never comes back here. Always -1 with EINTR.
#[no_mangle]
pub extern "C" fn pause() -> c_int {
    // Five minutes at a time - the kernel's longest sleep is ten - for
    // as long as nothing comes.
    while taaj::sleep_unless_signalled(5 * 60 * abi::TICKS_PER_SECOND) {}
    errno.store(EINTR, Ordering::Relaxed);
    -1
}
