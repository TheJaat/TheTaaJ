//! Threads for Rust programs: `spawn` a closure, `join` it.
//!
//! A thread is the kernel's (THREAD_CREATE); everything else is here.
//! Its stack is a memory object mapped for it. It starts in
//! `trampoline`, which runs the closure and ends the thread with
//! THREAD_EXIT, naming a word the kernel sets to 0 and wakes once the
//! thread runs no more: `join` sleeps on that word, then unmaps the
//! stack - which nothing uses any more.
//!
//! (C programs have pthreads, in the C library, built the same way.)

use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, Ordering};

use abi::Error;

/// The stack a thread gets unless asked otherwise: 256 KiB.
pub const STACK_DEFAULT: usize = 256 * 1024;

struct Start {
    /// 1 while the thread runs; the kernel sets it to 0 at its end.
    running: AtomicU32,
    work: Option<Box<dyn FnOnce() + Send>>,
}

/// A thread started by `spawn`. Dropping it without `join` leaves the
/// thread running, and its stack and record in memory for good.
pub struct JoinHandle {
    start: *mut Start,
    stack: *mut u8,
}

/// Runs `work` on a new thread with a stack of `stack_size` bytes.
pub fn spawn_with(stack_size: usize, work: impl FnOnce() + Send + 'static) -> Result<JoinHandle, Error> {
    let size = stack_size.max(16 * 1024).next_multiple_of(4096) as u64;
    let memory = crate::memory_create(size)?;
    let mapped = crate::memory_map(memory, true);
    let _ = crate::close(memory); // the mapping keeps it
    let stack = mapped?;
    let start = Box::into_raw(Box::new(Start { running: AtomicU32::new(1), work: Some(Box::new(work)) }));
    let top = stack as u64 + size; // a whole number of pages: 16-byte aligned
    match crate::thread_create(trampoline as *const () as u64, top, start as u64, 0) {
        Ok(_) => Ok(JoinHandle { start, stack }),
        Err(error) => {
            // SAFETY: never handed to a thread.
            drop(unsafe { Box::from_raw(start) });
            let _ = crate::memory_unmap(stack);
            Err(error)
        }
    }
}

/// Runs `work` on a new thread, with the default stack.
pub fn spawn(work: impl FnOnce() + Send + 'static) -> Result<JoinHandle, Error> {
    spawn_with(STACK_DEFAULT, work)
}

extern "C" fn trampoline(start: *mut Start) -> ! {
    // SAFETY: `spawn` made it and keeps it until `join` has seen
    // `running` cleared, which happens only after this thread is done
    // with it.
    let start = unsafe { &mut *start };
    if let Some(work) = start.work.take() {
        work();
    }
    crate::thread_exit(&start.running)
}

impl JoinHandle {
    /// Waits for the thread to end, then frees its stack.
    pub fn join(self) {
        // SAFETY: as in `trampoline`.
        let running = unsafe { &(*self.start).running };
        while running.load(Ordering::Acquire) != 0 {
            let _ = crate::futex_wait(running, 1, 0);
        }
        // SAFETY: the thread has ended: nothing uses either any more.
        drop(unsafe { Box::from_raw(self.start) });
        let _ = crate::memory_unmap(self.stack);
    }
}
