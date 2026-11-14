//! The first user program: says hello, then exercises every system
//! call, including handing the kernel bad pointers, which it must
//! refuse with an error rather than crash or leak kernel memory.

#![no_std]
#![no_main]

use taaj::{println, Error};

taaj::program!(main);

fn main() -> i64 {
    let pid = taaj::process_id();
    println!("[hello] hello from user mode! I am process {pid}, running in ring 3");

    taaj::yield_now();
    let before = taaj::ticks();
    taaj::sleep(3);
    let slept = taaj::ticks() - before;
    println!("[hello] slept {slept} ticks");
    if slept < 3 {
        return 1;
    }

    // Pointers the kernel must refuse. Each would be a security hole if
    // it were accepted: reading kernel memory, or any address at all.
    let refused = [
        ("null", 0, 16),
        ("kernel code", 0xFFFF_FFFF_8010_0000, 16),
        ("unmapped", 0x7000_0000_0000, 16),
        ("wrapping", u64::MAX - 4, 16),
        ("too long", 0x40_0000, abi::DEBUG_WRITE_MAX + 1),
    ];
    for (what, pointer, length) in refused {
        match taaj::write_raw(pointer, length) {
            Err(Error::BadAddress | Error::InvalidArgument) => {}
            other => {
                println!("[hello] FAILED: the kernel accepted a {what} pointer: {other:?}");
                return 2;
            }
        }
    }
    println!("[hello] the kernel refused all {} bad pointers", refused.len());

    if taaj::syscall(9999, 0, 0, 0) != abi::encode(Err(Error::UnknownCall)) {
        return 3;
    }
    42
}
