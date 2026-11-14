//! The TheTaaJ system call interface: the contract between the kernel
//! and every user program.
//!
//! One crate, used by both sides, so the numbers cannot drift apart. It
//! holds only constants and plain types - no code that runs.
//!
//! The calling convention (x86_64), close to Linux's so it is familiar:
//!
//!   `syscall` instruction
//!   RAX        system call number
//!   RDI, RSI, RDX, R10, R8, R9   arguments 1-6 (R10, not RCX: the
//!              instruction itself overwrites RCX with the return address)
//!   RAX        result: >= 0 success, < 0 an `Error`, negated
//!   RCX, R11   destroyed (the CPU uses them); everything else preserved
//!
//! This is the native interface, deliberately small. Capabilities and
//! IPC come next; the POSIX layer will sit on top of them, in user space.

#![no_std]

/// System call numbers.
pub mod call {
    /// exit(code) - ends the process. Never returns.
    pub const EXIT: u64 = 0;
    /// debug_write(pointer, length) - writes text to the kernel console.
    /// A stand-in until there are real I/O channels. Returns the length.
    pub const DEBUG_WRITE: u64 = 1;
    /// yield() - lets other threads run.
    pub const YIELD: u64 = 2;
    /// sleep(ticks) - sleeps for at least that many timer ticks.
    pub const SLEEP: u64 = 3;
    /// process_id() - this process's id.
    pub const PROCESS_ID: u64 = 4;
    /// ticks() - timer ticks since boot.
    pub const TICKS: u64 = 5;
}

/// Why a system call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    /// No system call has that number.
    UnknownCall = 1,
    /// An argument is out of range.
    InvalidArgument = 2,
    /// A pointer argument points at memory the process cannot access.
    BadAddress = 3,
}

impl Error {
    pub fn from_code(code: i64) -> Option<Error> {
        match code {
            1 => Some(Error::UnknownCall),
            2 => Some(Error::InvalidArgument),
            3 => Some(Error::BadAddress),
            _ => None,
        }
    }
}

/// Packs a result into RAX.
pub fn encode(result: Result<u64, Error>) -> u64 {
    match result {
        Ok(value) => value,
        Err(error) => (-(error as i64)) as u64,
    }
}

/// Unpacks RAX.
pub fn decode(raw: u64) -> Result<u64, Error> {
    let signed = raw as i64;
    if signed < 0 {
        Err(Error::from_code(-signed).unwrap_or(Error::InvalidArgument))
    } else {
        Ok(raw)
    }
}

/// Most bytes one debug_write may carry.
pub const DEBUG_WRITE_MAX: u64 = 4096;

/// The highest user address + 1: the lower half of the address space.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// Exit code the kernel records for a process it killed for a fault.
pub const KILLED: i64 = -1000;
