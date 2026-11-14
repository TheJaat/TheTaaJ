//! The portable, testable core of the TheTaaJ kernel.
//!
//! Everything in this library is pure logic: data structures and
//! algorithms that never touch hardware directly. That is what lets it
//! compile for the host too, so `cargo test` runs it under Linux - with a
//! debugger, sanitizers and thousands of iterations - before it ever
//! runs inside the kernel.
//!
//! The kernel binary (main.rs) adds what cannot be tested this way: the
//! architecture layer, the console, and the glue that points these
//! structures at real memory.

// no_std for the kernel; the full std when compiled for host tests.
#![cfg_attr(not(test), no_std)]

pub mod bootinfo;
pub mod memory;
#[cfg(target_arch = "x86_64")]
pub mod paging;
pub mod sync;
