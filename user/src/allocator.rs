//! The heap of a user program: `Box`, `Vec`, `String`, `BTreeMap`.
//!
//! The allocator is the kernel's own (the `heap` crate); only where its
//! memory comes from differs. The kernel maps frames into its heap area
//! itself. A user program asks for memory the way it asks for anything:
//! a system call. It reserves address space (MEMORY_RESERVE) and hands
//! it to the heap as a new region; the kernel gives each page a frame
//! when it is first touched, so a region costs what is used of it.
//!
//! Each region is a separate reservation, and the kernel puts a guard
//! page after every one, so regions never touch and never merge. To
//! keep their number small, each one is at least as big as the heap
//! already is (doubling it), between 1 MiB and 4 GiB.
//!
//! Since pages are taken only when touched, running out shows then, not
//! here: a program past its memory limit is ended at the page that
//! would pass it (with "out of memory" in the log), as Linux does with
//! overcommitted memory. What fails here is only address space.
//! Freeing 64 KiB or more gives the whole pages it frees back to the
//! kernel (MEMORY_DISCARD, entry 102): they stop counting against the
//! program's limit, and the address space stays the heap's, each page
//! zeroed again when next used. Smaller frees keep their pages - one
//! system call per free would cost more than the memory is worth - but
//! libc's small blocks come back in 64 KiB pieces when a piece empties
//! (libc/src/memory.rs), and so are given back then. Everything goes
//! back when the program exits, with its address space.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use crate::sync::Lock;

use heap::Heap;

const REGION_MIN: u64 = 1 << 20;
const REGION_MAX: u64 = 4 << 30;
const PAGE: u64 = 4096;
/// The smallest free whose pages go back to the kernel.
const GIVE_BACK_MIN: usize = 64 << 10;

struct UserHeap {
    /// One thread at a time. A futex lock (sync.rs): taking it when
    /// nobody else wants it is one atomic instruction; a thread that
    /// finds it taken sleeps instead of spinning.
    locked: Lock,
    heap: core::cell::UnsafeCell<Heap>,
}

// SAFETY: `heap` is reached only while `locked` is held.
unsafe impl Sync for UserHeap {}

#[global_allocator]
static HEAP: UserHeap = UserHeap { locked: Lock::new(), heap: core::cell::UnsafeCell::new(Heap::empty()) };

impl UserHeap {
    fn with<R>(&self, f: impl FnOnce(&mut Heap) -> R) -> R {
        self.locked.lock();
        // SAFETY: the lock is held, so this is the only reference.
        let result = f(unsafe { &mut *self.heap.get() });
        self.locked.unlock();
        result
    }
}

/// Reserves a new region of at least `bytes` and gives it to `heap`.
fn grow(heap: &mut Heap, bytes: u64) -> bool {
    let Some(rounded) = bytes.checked_next_multiple_of(PAGE) else { return false };
    let size = rounded.max(heap.total() as u64).clamp(REGION_MIN, REGION_MAX).max(rounded);
    if size > abi::RESERVE_MAX {
        return false; // bigger than any reservation can be
    }
    let Ok(start) = crate::memory_reserve(size, true) else { return false };
    // SAFETY: just reserved, writable, `size` bytes, used by nothing else.
    unsafe { heap.add_region(start as usize, size as usize) };
    true
}

// SAFETY: `alloc` returns null or a block of the heap of at least
// `layout`'s size and alignment, handed to no one else until `dealloc`
// gives it back (heap crate, tested on the host); `dealloc` is given only
// what `alloc` returned, with its layout - GlobalAlloc's contract on the
// caller. `with` keeps two threads out of the heap at once.
unsafe impl GlobalAlloc for UserHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with(|heap| loop {
            if let Some(pointer) = heap.allocate(layout) {
                return pointer;
            }
            // Room for this request even at its worst alignment.
            let needed = layout.size() as u64 + layout.align() as u64;
            if !grow(heap, needed) {
                return ptr::null_mut(); // `alloc` turns this into a panic
            }
        })
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        self.with(|heap| {
            // SAFETY: the caller's promise - `pointer` came from `alloc`
            // with this layout - is the heap's requirement too.
            let merged = unsafe { heap.deallocate(pointer, layout) };
            // Still under the lock: once it is let go, another thread may
            // be handed these pages, and what it wrote would be lost.
            if layout.size() >= GIVE_BACK_MIN {
                let freed = (pointer as usize, layout.size());
                if let Some((start, length)) = heap::pages_to_give_back(merged, freed, PAGE as usize) {
                    let _ = crate::memory_discard(start as *mut u8, length as u64);
                }
            }
        });
    }
}

/// (bytes in use, bytes the heap holds), for programs that report it.
pub fn heap_usage() -> (usize, usize) {
    HEAP.with(|heap| (heap.used(), heap.total()))
}
