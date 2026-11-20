//! Looks for the kernel through the GS register.
//!
//! The kernel keeps each CPU's private data at the address in GS, and
//! swaps that address for the program's own (`swapgs`) every time it is
//! entered and left: at a system call, at an interrupt, at a fault. If
//! one of those swaps were missing or doubled, the program would be
//! running with the kernel's GS - and the kernel with the program's.
//!
//! So: enter the kernel every way there is, many times, and then read
//! through GS. A program's GS points at address 0, where nothing is
//! mapped, so the read must fault at exactly the offset asked for -
//! 0x5a5, a number chosen to be found in the log - and not somewhere
//! in the kernel's half.

#![no_std]
#![no_main]

use core::arch::asm;

taaj::program!(main);

fn main() -> i64 {
    taaj::println!("[gs] system calls, timer interrupts, then a read through GS...");
    // Timer interrupts arriving in ring 3, and a system call a round.
    let start = taaj::ticks();
    while taaj::ticks() < start + 5 {
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
    }
    // Loading GS with a segment is allowed in ring 3 (this is the user
    // data segment), and resets what GS points at. The kernel must not
    // care.
    // SAFETY: changes only GS, which nothing in this program uses.
    unsafe { asm!("mov gs, {0:x}", in(reg) 0x1Bu64, options(nomem, nostack, preserves_flags)) };
    taaj::sleep(1);
    let value: u64;
    // SAFETY: none - this is meant to fault.
    unsafe { asm!("mov {}, gs:[0x5a5]", out(reg) value, options(nostack, preserves_flags)) };
    taaj::println!("[gs] FAILED: read {value:#x} through GS");
    0
}
