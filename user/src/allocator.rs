//! The heap of a user program: `Box`, `Vec`, `String`, `BTreeMap`.
//!
//! The allocator is the kernel's own (the `heap` crate); only where its
//! memory comes from differs. The kernel maps frames into its heap area
//! itself. A user program asks for memory the way it asks for anything:
//! a system call. It creates a memory object and maps it, and hands the
//! mapping to the heap as a new region.
//!
//! Each region is a separate mapping, and the kernel puts a guard page
//! after every mapping, so regions never touch and never merge. To keep
//! their number small, each one is at least as big as the heap already
//! is (doubling it), between 64 KiB and the 128 MiB a memory object may
//! be - or, when that much is not free, just what was asked for.
//!
//! The memory object's handle is closed straight after mapping: the
//! mapping keeps the object alive, and the handle would only take a
//! slot in the handle table. Memory is not given back to the kernel
//! while the program runs - like most allocators - but all of it is when
//! the program exits, with its address space.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use crate::sync::Lock;

use heap::Heap;

const REGION_MIN: u64 = 64 * 1024;
const REGION_MAX: u64 = abi::MEMORY_MAX;
const PAGE: u64 = 4096;

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

/// Maps a new region of at least `bytes` and gives it to `heap`.
fn grow(heap: &mut Heap, bytes: u64) -> bool {
    let size = bytes.max(heap.total() as u64).clamp(REGION_MIN, REGION_MAX).next_multiple_of(PAGE);
    if bytes > size {
        return false; // bigger than any one memory object can be
    }
    // Doubling can ask for far more than this request needs: 128 MiB
    // more for a heap of 128 MiB. When that much is not there, what the
    // request needs may still be.
    let exact = bytes.max(REGION_MIN).next_multiple_of(PAGE);
    let (memory, size) = match crate::memory_create(size) {
        Ok(memory) => (memory, size),
        Err(_) if exact < size => match crate::memory_create(exact) {
            Ok(memory) => (memory, exact),
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    let mapped = crate::memory_map(memory, true);
    let _ = crate::close(memory); // the mapping keeps it alive
    let Ok(start) = mapped else { return false };
    // SAFETY: just mapped, writable, `size` bytes, used by nothing else.
    unsafe { heap.add_region(start as usize, size as usize) };
    true
}

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
        // SAFETY: the caller's promise - `pointer` came from `alloc` with
        // this layout - is the heap's requirement too.
        self.with(|heap| unsafe { heap.deallocate(pointer, layout) });
    }
}

/// (bytes in use, bytes the heap holds), for programs that report it.
pub fn heap_usage() -> (usize, usize) {
    HEAP.with(|heap| (heap.used(), heap.total()))
}
