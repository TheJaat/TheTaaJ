//! `kprint!` and `kprintln!`: formatted output from anywhere in the
//! kernel.
//!
//! Portable: the actual bytes go to `arch::console_write`, which on
//! x86_64 is the COM1 serial port and on another architecture will be
//! whatever early console it has.
//!
//! A lock keeps each message whole: without it, a message printed by an
//! interrupt handler could land in the middle of another one's line.

use core::fmt::{self, Write};

use crate::irqlock::IrqLock;

static LOCK: IrqLock<()> = IrqLock::new(());

struct Console;

/// Implementing `fmt::Write` is all it takes to get `{}` formatting:
/// `write_fmt` comes free with the trait and calls `write_str` with each
/// formatted piece.
impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        crate::arch::console_write(text);
        Ok(())
    }
}

/// Not for direct use - the macros call it. `pub` only because macros
/// expand at the call site, which may be in another module.
pub fn print(args: fmt::Arguments) {
    let _guard = LOCK.lock();
    // Writing to the console cannot fail, so the Result carries nothing.
    let _ = Console.write_fmt(args);
}

/// Frees the console lock, for a panic or fatal exception that must be
/// reported even if it struck while something was printing.
///
/// # Safety
/// Only on the way to halting: whoever held the lock never runs again.
pub unsafe fn break_lock() {
    LOCK.force_unlock();
}

/// Like `print!`, for the kernel.
#[allow(unused_macros)] // part of the console's interface; not every build uses it
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::console::print(format_args!($($arg)*))
    };
}

/// Like `println!`, for the kernel.
///
/// `#[macro_use]` on `mod console` in main.rs makes both macros usable by
/// name in every module declared after it.
macro_rules! kprintln {
    () => {
        $crate::console::print(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::console::print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
