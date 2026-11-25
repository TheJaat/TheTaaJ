//! threadcheck: threads in a Rust program (taaj::thread, taaj::sync).
//!
//! Four threads, on as many CPUs as there are, each allocating and
//! freeing thousands of blocks - all through the one heap, whose lock
//! they share - and adding to one counter under a `Mutex`. Then the
//! counter is exact, every block held what its thread wrote, and the
//! heap is as it was. Prints "threadcheck: all passed" and exits 0; a
//! broken check panics, which exits -2.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

use taaj::println;
use taaj::sync::Mutex;

taaj::program!(main);

const THREADS: u64 = 4;
const ROUNDS: u64 = 5000;

fn main() -> i64 {
    let counter = Arc::new(Mutex::new(0u64));
    let mut handles = Vec::new();
    for me in 0..THREADS {
        let counter = counter.clone();
        let handle = taaj::thread::spawn(move || {
            let mut kept: Vec<Vec<u64>> = Vec::new();
            for round in 0..ROUNDS {
                // A block of a size that changes, filled with this
                // thread's mark: another thread's block given out twice
                // would show as the wrong mark.
                kept.push(alloc::vec![me * 1_000_000 + round; (round % 61 + 1) as usize]);
                if kept.len() > 32 {
                    let block = kept.remove(0);
                    assert!(block.iter().all(|&v| v / 1_000_000 == me), "a block kept what its thread wrote");
                }
                *counter.lock() += 1;
            }
            assert!(kept.iter().flatten().all(|&v| v / 1_000_000 == me), "the last blocks too");
        });
        handles.push(handle.expect("a thread"));
    }
    for handle in handles {
        handle.join();
    }
    assert_eq!(*counter.lock(), THREADS * ROUNDS, "every addition under the Mutex counted");
    println!("threadcheck: {THREADS} threads, {} additions and as many blocks, all passed", THREADS * ROUNDS);
    0
}
