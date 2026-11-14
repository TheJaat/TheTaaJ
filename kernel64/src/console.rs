//! `kprint!` and `kprintln!`: formatted output from anywhere in the
//! kernel.
//!
//! Portable: the actual bytes go to `arch::console_write`, which on
//! x86_64 is the COM1 serial port and on another architecture will be
//! whatever early console it has.
//!
//! No lock yet. That is correct for now - one CPU, interrupts off - and
//! stops being correct the moment either changes.

use core::fmt::{self, Write};

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
    // Writing to the console cannot fail, so the Result carries nothing.
    let _ = Console.write_fmt(args);
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
