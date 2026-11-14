//! Tries to switch interrupts off. `cli` is privileged: in ring 3 it is
//! a general protection fault, and the process is killed. If user code
//! could do this, any program could freeze the machine.

#![no_std]
#![no_main]

taaj::program!(main);

fn main() -> i64 {
    taaj::println!("[cli] trying to disable interrupts...");
    // SAFETY: none - this is meant to fault.
    unsafe { core::arch::asm!("cli") };
    taaj::println!("[cli] FAILED: interrupts are off");
    0
}
