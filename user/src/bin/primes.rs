//! primes: a program that is not part of the kernel image.
//!
//! Every other program here is built into the kernel and started by
//! name. This one is only ever a file: the boot test puts it on the
//! disk with tfstool, the shell reads it from the file service, and the
//! kernel loads it from the memory the shell hands over - the first
//! program TheTaaJ runs that it did not carry with it.
//!
//! It also uses the heap, to show a program from the disk is a program
//! like any other.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    // The sieve of Eratosthenes: cross out every multiple.
    const LIMIT: usize = 50;
    let mut composite = Vec::new();
    composite.resize(LIMIT, false);
    let mut line = String::new();
    for n in 2..LIMIT {
        if !composite[n] {
            let _ = write!(line, " {n}");
            for multiple in (n * n..LIMIT).step_by(n) {
                composite[multiple] = true;
            }
        }
    }
    println!("primes below {LIMIT}:{line}");
    0
}
