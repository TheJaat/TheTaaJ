//! heapcheck: proves a user program's heap works.
//!
//! Collections grow past several regions (each memory reserved by a
//! system call), big alignments are honoured, freed memory is
//! reused rather than growing again, and every byte read back is what
//! was written. Exits 0 if all of it held; a broken check panics, which
//! exits -2.
//!
//! The kernel's self-test runs it twice and then checks no frame was
//! lost: the heap's memory must all go back when the program ends.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    // A vector grown one element at a time to 2 MiB: it reallocates
    // again and again, and needs more than the first 64 KiB region.
    let mut numbers: Vec<u64> = Vec::new();
    for n in 0..256 * 1024u64 {
        numbers.push(n * 3);
    }
    assert!(numbers.iter().enumerate().all(|(i, &v)| v == i as u64 * 3), "vector contents");
    let (_, grown) = taaj::heap_usage();
    assert!(grown >= 2 << 20, "the heap grew past 2 MiB");

    // Many small allocations of different sizes, in a map.
    let mut names = BTreeMap::new();
    for n in 0..2000u32 {
        let mut name = String::new();
        let _ = write!(name, "file-{n:04}");
        names.insert(name, alloc::vec![n as u8; (n % 97) as usize]);
    }
    assert_eq!(names.len(), 2000);
    assert_eq!(names.get("file-0123").map(Vec::len), Some(123 % 97));
    assert!(names.values().enumerate().all(|(i, v)| v.iter().all(|&b| b == i as u8)), "map contents");

    // A page-aligned block, as a buffer for a device would need.
    #[repr(align(4096))]
    struct Page([u8; 4096]);
    let page = Box::new(Page([7; 4096]));
    assert_eq!(&*page as *const Page as usize % 4096, 0, "page alignment");
    assert!(page.0.iter().all(|&b| b == 7));

    // Free everything, and allocate it all again: the heap must reuse
    // what it has instead of asking the kernel for more.
    drop(numbers);
    drop(names);
    drop(page);
    let (used, total) = taaj::heap_usage();
    assert_eq!(used, 0, "everything freed");
    let again: Vec<u64> = (0..256 * 1024).collect();
    assert_eq!(again[256 * 1024 - 1], 256 * 1024 - 1);
    assert_eq!(taaj::heap_usage().1, total, "freed memory was reused");

    // Larger than any reservation may be: refused, not a crash.
    let refused = Vec::<u8>::new().try_reserve_exact(abi::RESERVE_MAX as usize + 1).is_err();
    assert!(refused, "an impossible allocation fails cleanly");

    // Reserved memory nobody has touched, filled by the kernel itself:
    // a message received straight into it, across a page boundary. The
    // kernel finds the pages missing as it copies, and fills them as
    // the program's own access would have.
    let (a, b) = taaj::channel_create().expect("a channel");
    let message: Vec<u8> = (0..1000u32).map(|n| (n % 251) as u8 + 1).collect();
    taaj::send(a, &message, &[]).expect("sent");
    let before = taaj::memory_used();
    let reserved = taaj::memory_reserve(2 * 4096, true).expect("two pages reserved");
    assert_eq!(taaj::memory_used(), before, "reserving took nothing");
    // SAFETY: reserved above, writable, 8 KiB; nothing else uses it.
    let into = unsafe { core::slice::from_raw_parts_mut(reserved.add(4096 - 500), 1000) };
    assert_eq!(taaj::receive(b, into, &mut []).map(|(n, _)| n), Ok(1000), "received into untouched pages");
    assert_eq!(into, &message[..], "what the kernel wrote there");
    assert_eq!(taaj::memory_used(), before + 2 * 4096, "the two pages the kernel filled were counted");
    let _ = taaj::close(a);
    let _ = taaj::close(b);
    taaj::memory_unmap(reserved).expect("given back");
    assert_eq!(taaj::memory_used(), before, "and given back");

    println!("[heapcheck] heap of {} KiB: grew, aligned, freed and reused", total / 1024);
    0
}
