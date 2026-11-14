//! Maps a memory object read-only, then writes to it. The page tables
//! say no: a page fault, and the kernel kills this process. Rights on a
//! handle become permissions in the hardware.

#![no_std]
#![no_main]

use taaj::{println, rights};

taaj::program!(main);

fn main() -> i64 {
    let Ok(memory) = taaj::memory_create(4096) else { return 1 };
    let Ok(read_only) = taaj::duplicate(memory, rights::READ) else { return 2 };
    if taaj::memory_map(read_only, true).is_ok() {
        println!("[memro] FAILED: mapped a read-only handle writable");
        return 3;
    }
    let Ok(page) = taaj::memory_map(read_only, false) else { return 4 };
    // SAFETY: reading a mapped page.
    let byte = unsafe { page.read_volatile() };
    println!("[memro] mapped read-only at {page:p} (reads {byte}); now writing to it...");
    // SAFETY: none - this is the point.
    unsafe { page.write_volatile(1) };
    println!("[memro] FAILED: wrote to a read-only mapping");
    5
}
