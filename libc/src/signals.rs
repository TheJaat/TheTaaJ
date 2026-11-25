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

use core::ffi::{c_int, c_uint, c_void};

use crate::errno::{errno, EFAULT, EINTR, EINVAL, ENOMEM, EPERM};
use core::sync::atomic::Ordering;

/// `__taaj_signal_setup(entry, caught, ignored, on_stack)`: tells the
/// kernel how this program takes signals (abi::call::SIGNAL_SETUP) -
/// `on_stack` the signals whose handler asked for SA_ONSTACK. 0, or -1.
#[no_mangle]
pub extern "C" fn __taaj_signal_setup(entry: u64, caught: u64, ignored: u64, on_stack: u64) -> c_int {
    if taaj::signal_setup(entry, caught, ignored, on_stack).is_ok() { 0 } else { -1 }
}

/// signal.h's stack_t, as Linux lays it out.
#[repr(C)]
pub struct StackT {
    ss_sp: *mut c_void,
    ss_flags: c_int,
    ss_size: usize,
}

const SS_ONSTACK: c_int = 1;
const SS_DISABLE: c_int = 2;
/// signal.h's MINSIGSTKSZ: what the kernel asks for at least.
const MINSIGSTKSZ: usize = abi::SIGNAL_STACK_MIN as usize;

/// The stack pointer now, near enough: the address of a local.
fn stack_pointer() -> usize {
    let here = 0u8;
    core::hint::black_box(&here) as *const u8 as usize
}

/// `sigaltstack(new, old)`: this thread's signal stack - where a signal
/// whose handler was installed with SA_ONSTACK runs. The one place a
/// handler can run when the thread's own stack has overflowed: the
/// kernel has no room there for the signal's context.
///
/// `*old` says what it was: SS_DISABLE if none, SS_ONSTACK if this
/// thread is running on it now. A `new` with SS_DISABLE removes it;
/// otherwise it must be at least MINSIGSTKSZ bytes (ENOMEM), and it
/// can't be changed while running on it (EPERM). EINVAL for other flags.
///
/// # Safety
/// `new` and `old` null or valid; `new`'s memory the thread's to use.
#[no_mangle]
pub unsafe extern "C" fn sigaltstack(new: *const StackT, old: *mut StackT) -> c_int {
    let (base, size) = crate::threads::signal_stack();
    let on_it = size != 0 && (base..base + size).contains(&stack_pointer());
    // SAFETY: the caller's promise.
    if let Some(old) = unsafe { old.as_mut() } {
        old.ss_sp = base as *mut c_void;
        old.ss_size = size;
        old.ss_flags = if size == 0 { SS_DISABLE } else if on_it { SS_ONSTACK } else { 0 };
    }
    // SAFETY: the caller's promise.
    let Some(new) = (unsafe { new.as_ref() }) else { return 0 };
    if new.ss_flags & !(SS_DISABLE | SS_ONSTACK) != 0 {
        return crate::descriptors::fail(EINVAL);
    }
    if on_it {
        return crate::descriptors::fail(EPERM);
    }
    let wanted = if new.ss_flags & SS_DISABLE != 0 {
        (0, 0)
    } else if new.ss_size < MINSIGSTKSZ {
        return crate::descriptors::fail(ENOMEM);
    } else {
        (new.ss_sp as usize, new.ss_size)
    };
    match taaj::signal_stack(wanted.0 as u64, wanted.1 as u64) {
        Ok(()) => {
            crate::threads::set_signal_stack(wanted);
            0
        }
        Err(_) => crate::descriptors::fail(EFAULT),
    }
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
