//! Tries to switch interrupts off. That is privileged on every CPU:
//! `cli` in ring 3 is a general protection fault on x86, and writing
//! the interrupt masks (`msr daifset`) at exception level 0 is a
//! trapped instruction on AArch64. Either way the process is killed.
//! If user code could do this, any program could freeze the machine.

#![no_std]
#![no_main]

taaj::program!(main);

fn main() -> i64 {
    taaj::println!("[cli] trying to disable interrupts...");
    // SAFETY: none - this is meant to fault.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("cli")
    };
    // SAFETY: none - this is meant to fault.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #2")
    };
    taaj::println!("[cli] FAILED: interrupts are off");
    0
}
