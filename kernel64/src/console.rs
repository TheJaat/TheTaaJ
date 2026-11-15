//! `kprint!` and `kprintln!`: formatted output from anywhere in the
//! kernel.
//!
//! Portable: the actual bytes go to `arch::console_write`, which on
//! x86_64 is the COM1 serial port and on another architecture will be
//! whatever early console it has.
//!
//! A lock keeps each message whole: without it, a message printed by an
//! interrupt handler could land in the middle of another one's line.
//!
//! Two kinds of text come through here, treated differently:
//!
//!   - The kernel's own messages (`kprintln!`) are written at once,
//!     while the caller waits. They are few, and some must not be
//!     delayed: a panic is followed by nothing.
//!   - What programs print (the debug_write call) is queued, and
//!     written when the CPU is idle. A program does not wait for the
//!     serial line; and the kernel never keeps interrupts off for the
//!     length of a program's message. If programs print faster than the
//!     line can carry - a flood - messages are dropped, and the log says
//!     how much is missing.
//!
//! Order is kept: before the kernel writes a message of its own it
//! writes out everything queued before it.

use core::fmt::{self, Write};

use kernel64::logring::LogRing;

use crate::irqlock::IrqLock;

/// Room for a few screens of text between two idle moments.
const QUEUE_SIZE: usize = 64 * 1024;
/// Bytes written per turn of the idle loop, with interrupts off: about
/// 1.4 ms at 115200 baud.
const CHUNK: usize = 16;

/// The lock for the serial line, and what it guards: the queue.
static QUEUE: IrqLock<LogRing<QUEUE_SIZE>> = IrqLock::new(LogRing::new());

struct Console;

/// Implementing `fmt::Write` is all it takes to get `{}` formatting:
/// `write_fmt` comes free with the trait and calls `write_str` with each
/// formatted piece.
impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        crate::arch::console_write(text);
        // And on the screen - once the kernel has taken it, to stop.
        crate::lastwords::echo(text);
        Ok(())
    }
}

/// Writes out up to `limit` queued bytes; and, once the queue is empty,
/// a line saying what was dropped, if anything was. Returns whether it
/// wrote anything. The caller holds the lock.
fn flush(queue: &mut LogRing<QUEUE_SIZE>, limit: usize) -> bool {
    let mut chunk = [0u8; CHUNK];
    let mut written = 0;
    while written < limit {
        let n = queue.pop(&mut chunk[..CHUNK.min(limit - written)]);
        if n == 0 {
            break;
        }
        crate::arch::console_write_bytes(&chunk[..n]);
        written += n;
    }
    if queue.is_empty() {
        let dropped = queue.take_dropped();
        if dropped > 0 {
            let _ = writeln!(Console, "\n[log] {dropped} bytes of program output are missing here: the serial line could not keep up");
            return true;
        }
    }
    written > 0
}

/// Not for direct use - the macros call it. `pub` only because macros
/// expand at the call site, which may be in another module.
pub fn print(args: fmt::Arguments) {
    let mut queue = QUEUE.lock();
    // What programs printed before this comes before it.
    flush(&mut queue, usize::MAX);
    // Writing to the console cannot fail, so the Result carries nothing.
    let _ = Console.write_fmt(args);
}

/// Queues a program's output for the log. Never waits: if there is no
/// room the message is dropped, and counted.
pub fn write_later(bytes: &[u8]) {
    QUEUE.lock().push(bytes);
}

/// Writes a little of what is queued; false if there was nothing. For
/// the idle loop, which calls it until then.
pub fn write_queued() -> bool {
    flush(&mut QUEUE.lock(), CHUNK)
}

/// Frees the console lock, for a panic or fatal exception that must be
/// reported even if it struck while something was printing.
///
/// # Safety
/// Only on the way to halting: whoever held the lock never runs again.
pub unsafe fn break_lock() {
    QUEUE.force_unlock();
    // The kernel is stopping: what it says now goes on the screen too.
    crate::lastwords::begin();
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
