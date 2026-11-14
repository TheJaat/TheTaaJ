//! Spins in user mode, never yielding, for a while. The timer interrupt
//! must still take the CPU away from it - it arrives in ring 3 and
//! switches to the kernel stack (TSS.RSP0) - or the rest of the system
//! would stop.

#![no_std]
#![no_main]

taaj::program!(main);

fn main() -> i64 {
    let start = taaj::ticks();
    let mut rounds = 0u64;
    while taaj::ticks() < start + 20 {
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        rounds += 1;
    }
    taaj::println!("[spin] spun {rounds} rounds over 20 ticks without yielding");
    0
}
