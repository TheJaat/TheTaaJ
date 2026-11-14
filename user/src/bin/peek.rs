//! Tries to read kernel memory directly. The kernel's pages lack the
//! USER bit, so the CPU refuses with a page fault - and the kernel must
//! kill this process, not fall over.

#![no_std]
#![no_main]

taaj::program!(main);

fn main() -> i64 {
    taaj::println!("[peek] reading the kernel's code at 0xffffffff80100000...");
    // SAFETY: none - this is meant to fault.
    let value = unsafe { (0xFFFF_FFFF_8010_0000 as *const u64).read_volatile() };
    taaj::println!("[peek] FAILED: read {value:#x}");
    0
}
