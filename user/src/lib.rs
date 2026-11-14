//! `taaj` - the runtime every TheTaaJ user program links: system call
//! wrappers, `print!`, the panic handler and the program entry.
//!
//! A user program has no C library and no Rust `std` underneath it
//! (yet): only `core`, and the system calls in `abi`. This is the
//! thinnest layer that makes writing one pleasant.

#![no_std]

use core::arch::asm;
use core::fmt::{self, Write};

pub use abi::{call, Error};

/// The raw system call: number in RAX, arguments in RDI, RSI, RDX. The
/// `syscall` instruction overwrites RCX and R11, so they are clobbers.
#[inline(always)]
pub fn syscall(number: u64, a: u64, b: u64, c: u64) -> u64 {
    let result: u64;
    // SAFETY: the kernel validates every argument; the worst a bad one
    // does is return an error.
    unsafe {
        asm!("syscall",
             inlateout("rax") number => result,
             in("rdi") a, in("rsi") b, in("rdx") c,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    result
}

pub fn exit(code: i64) -> ! {
    syscall(call::EXIT, code as u64, 0, 0);
    unreachable!("exit returned")
}

/// Writes raw bytes to the console; returns how many, or why not.
pub fn write(bytes: &[u8]) -> Result<u64, Error> {
    abi::decode(syscall(call::DEBUG_WRITE, bytes.as_ptr() as u64, bytes.len() as u64, 0))
}

/// The same call with an arbitrary pointer - for testing that the kernel
/// refuses bad ones.
pub fn write_raw(pointer: u64, length: u64) -> Result<u64, Error> {
    abi::decode(syscall(call::DEBUG_WRITE, pointer, length, 0))
}

pub fn yield_now() {
    syscall(call::YIELD, 0, 0, 0);
}

pub fn sleep(ticks: u64) {
    syscall(call::SLEEP, ticks, 0, 0);
}

pub fn process_id() -> u64 {
    syscall(call::PROCESS_ID, 0, 0, 0)
}

pub fn ticks() -> u64 {
    syscall(call::TICKS, 0, 0, 0)
}

pub struct Console;

impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for chunk in text.as_bytes().chunks(abi::DEBUG_WRITE_MAX as usize) {
            write(chunk).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn print_args(args: fmt::Arguments) {
    let _ = Console.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::print_args(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}

/// Declares the program's `main`: `taaj::program!(main);` with
/// `fn main() -> i64`, whose return value becomes the exit code.
///
/// It defines `_start`, where the kernel starts the program. The kernel
/// enters it with RSP set as if `_start` had just been called, so it is
/// an ordinary function.
#[macro_export]
macro_rules! program {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn _start() -> ! {
            $crate::exit($main())
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {info}");
    exit(-2)
}
