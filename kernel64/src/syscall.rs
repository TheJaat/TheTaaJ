//! System calls: what a user program may ask of the kernel.
//!
//! The numbers and errors are in the `abi` crate, shared with user
//! programs. Every argument is untrusted: numbers are range-checked and
//! pointers go through `process::copy_from_user` and `copy_to_user`.

use alloc::string::String;

use abi::{call, Error};

use crate::{arch, interrupts, ipc, process, threads};

/// Longest sleep accepted, in ticks: ten minutes.
const MAX_SLEEP: u64 = 10 * 60 * interrupts::HZ;

/// Runs system call `number`; returns the value for RAX.
pub fn dispatch(number: u64, args: [u64; 6]) -> u64 {
    // The entry stub runs with interrupts off. A system call may take a
    // while (or sleep), so let the timer in - the thread is on its own
    // kernel stack and can be preempted like any other.
    arch::enable_interrupts();
    let result = match number {
        call::EXIT => process::exit_current(args[0] as i64),
        call::DEBUG_WRITE => debug_write(args[0], args[1]),
        call::YIELD => {
            threads::yield_now();
            Ok(0)
        }
        call::SLEEP if args[0] <= MAX_SLEEP => {
            threads::sleep(args[0]);
            Ok(0)
        }
        call::SLEEP => Err(Error::InvalidArgument),
        call::PROCESS_ID => Ok(process::current().map_or(0, |p| p.pid)),
        call::TICKS => Ok(interrupts::ticks()),
        call::CHANNEL_CREATE => ipc::create(args[0]),
        call::CHANNEL_SEND => ipc::send(args[0], args[1], args[2], args[3], args[4]),
        call::CHANNEL_RECEIVE => ipc::receive(args[0], args[1], args[2], args[3], args[4]),
        call::HANDLE_CLOSE => ipc::close(args[0]),
        call::HANDLE_DUPLICATE => ipc::duplicate(args[0], args[1]),
        _ => Err(Error::UnknownCall),
    };
    arch::disable_interrupts();
    abi::encode(result)
}

fn debug_write(pointer: u64, length: u64) -> Result<u64, Error> {
    if length > abi::DEBUG_WRITE_MAX {
        return Err(Error::InvalidArgument);
    }
    let bytes = process::copy_from_user(pointer, length)?;
    // Not trusted to be UTF-8 either: invalid bytes print as U+FFFD.
    kprint!("{}", String::from_utf8_lossy(&bytes));
    Ok(length)
}
