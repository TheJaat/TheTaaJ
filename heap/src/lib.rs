//! A heap: the memory behind `Box`, `Vec` and `String`.
//!
//! The same allocator serves the kernel and every user program. It does
//! not know where its memory comes from - it is handed regions with
//! `add_region` and carves them up:
//!
//!   - the kernel maps fresh frames at the end of its heap area (mm.rs)
//!   - a user program creates a memory object and maps it (the `taaj`
//!     runtime)
//!
//! Pure logic over addresses, so it is tested here, on the host.
//!
//! A linked-list allocator. The free memory is a list of blocks, each
//! storing its own size and a pointer to the next one *inside the free
//! memory itself*, so the list costs nothing beyond the memory it
//! describes:
//!
//!   free block:  [ size | next | ...unused... ]
//!
//! - allocate: walk the list, take the first block big enough (first
//!   fit), and give back whatever is left over at either end.
//! - deallocate: put the block back in address order, and merge it with
//!   the blocks just before and after it if they touch. Without merging,
//!   the heap would end up as many small blocks no larger request fits.
//!
//! Everything is in 16-byte units, and no block is smaller than 16
//! bytes, the size of the header. So any leftover is 0 bytes or big
//! enough to be a block of its own - there are no unusable slivers.
//!
//! Simple and fully understandable, at the cost of O(n) operations. A
//! slab allocator for the kernel's common fixed-size objects can sit on
//! top later; this remains the general-purpose fallback.

#![cfg_attr(not(test), no_std)]

use core::alloc::Layout;
use core::ptr;

/// Allocation granularity, and the smallest block.
pub const UNIT: usize = 16;

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

const _: () = assert!(core::mem::size_of::<FreeBlock>() <= UNIT);

const fn round_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// The size a request really takes, so `allocate` and `deallocate`
/// always agree on it.
fn block_size(layout: &Layout) -> usize {
    round_up(layout.size().max(UNIT), UNIT)
}

/// The whole pages of a free block that a freed range lets go of:
/// those inside `merged` (the free block `deallocate` returned) less its
/// first UNIT bytes - the free list's header, which stays - that the
/// freed range `freed` covers at least partly. (start, length), or None
/// if there is not one whole page. Only what was just freed is looked
/// at: the rest of the block was given back when it was freed, so a big
/// free block is not walked again on every free beside it.
pub fn pages_to_give_back(merged: (usize, usize), freed: (usize, usize), page: usize) -> Option<(usize, usize)> {
    let inside_start = round_up(merged.0 + UNIT, page);
    let inside_end = (merged.0 + merged.1) & !(page - 1);
    let start = inside_start.max(freed.0 & !(page - 1));
    let end = inside_end.min(round_up(freed.0 + freed.1, page));
    (end > start).then(|| (start, end - start))
}

pub struct Heap {
    /// The lowest free block; the list is sorted by address.
    head: *mut FreeBlock,
    used: usize,
    total: usize,
}

// SAFETY: the raw pointers are into memory the Heap owns outright; it is
// sound to move it to another CPU (it will always be behind a lock).
unsafe impl Send for Heap {}

impl Heap {
    pub const fn empty() -> Heap {
        Heap { head: ptr::null_mut(), used: 0, total: 0 }
    }

    /// Gives the heap the memory `start..start + size` to allocate from.
    ///
    /// # Safety
    /// The range must be valid, writable, unused by anything else, and
    /// stay so for as long as the heap exists.
    pub unsafe fn add_region(&mut self, start: usize, size: usize) {
        let aligned = round_up(start, UNIT);
        let end = (start + size) & !(UNIT - 1);
        if end <= aligned {
            return;
        }
        self.total += end - aligned;
        self.insert(aligned, end - aligned);
    }

    /// Bytes handed out, in whole units.
    pub fn used(&self) -> usize {
        self.used
    }

    /// Bytes the heap manages, used or free.
    pub fn total(&self) -> usize {
        self.total
    }

    /// The number of free blocks - a measure of fragmentation.
    pub fn free_blocks(&self) -> usize {
        let mut count = 0;
        let mut block = self.head;
        while !block.is_null() {
            count += 1;
            // SAFETY: every block in the list is a valid FreeBlock.
            block = unsafe { (*block).next };
        }
        count
    }

    /// Memory for `layout`, or None if no free block fits.
    pub fn allocate(&mut self, layout: Layout) -> Option<*mut u8> {
        let size = block_size(&layout);
        let align = layout.align().max(UNIT);

        // `link` is the pointer that points at `block`: the head, or the
        // previous block's `next`. Rewriting it is how a block leaves the
        // list.
        let mut link: *mut *mut FreeBlock = &mut self.head;
        // SAFETY: `link` always points at a live list pointer, and every
        // block in the list is a valid FreeBlock inside memory the heap
        // owns, as add_region and deallocate promise.
        unsafe {
            while !(*link).is_null() {
                let block = *link;
                let block_start = block as usize;
                let block_end = block_start + (*block).size;

                let start = round_up(block_start, align);
                let end = start + size;
                if end > block_end {
                    link = &mut (*block).next;
                    continue;
                }

                // Fits. Up to two leftovers, each 0 or >= UNIT bytes:
                //   front: block_start..start (alignment padding)
                //   tail:  end..block_end
                let next = (*block).next;
                let tail = block_end - end;
                let after = if tail > 0 {
                    let rest = end as *mut FreeBlock;
                    rest.write(FreeBlock { size: tail, next });
                    rest
                } else {
                    next
                };
                if start > block_start {
                    // The block stays in the list, shortened to the front.
                    (*block).size = start - block_start;
                    (*block).next = after;
                } else {
                    *link = after;
                }
                self.used += size;
                return Some(start as *mut u8);
            }
        }
        None
    }

    /// Returns memory from `allocate`. Returns the free block it is now
    /// part of, merged with its neighbours: (start, size). A user
    /// program's heap gives the whole pages inside it back to the kernel
    /// (MEMORY_DISCARD) - all but the block's first UNIT bytes, where
    /// the free list's header lives.
    ///
    /// # Safety
    /// `pointer` must have come from `allocate` on this heap with the same
    /// `layout`, and not been freed since.
    pub unsafe fn deallocate(&mut self, pointer: *mut u8, layout: Layout) -> (usize, usize) {
        let size = block_size(&layout);
        self.used -= size;
        self.insert(pointer as usize, size)
    }

    /// Adds `start..start + size` to the free list, in address order,
    /// merging it with any neighbour it touches; the merged block.
    unsafe fn insert(&mut self, start: usize, size: usize) -> (usize, usize) {
        let end = start + size;

        // Find the last block before `start` (or none).
        let mut previous: *mut FreeBlock = ptr::null_mut();
        let mut next = self.head;
        while !next.is_null() && (next as usize) < start {
            previous = next;
            next = (*next).next;
        }

        // Two blocks overlapping means a double free or a corrupted list;
        // catch it here instead of handing the same memory out twice.
        if !previous.is_null() {
            assert!(previous as usize + (*previous).size <= start, "heap: double free at {start:#x}");
        }
        if !next.is_null() {
            assert!(end <= next as usize, "heap: double free at {start:#x}");
        }

        // Merge with the following block?
        let (size, following) = if !next.is_null() && next as usize == end {
            (size + (*next).size, (*next).next)
        } else {
            (size, next)
        };

        // Merge with the preceding block?
        if !previous.is_null() && previous as usize + (*previous).size == start {
            (*previous).size += size;
            (*previous).next = following;
            return (previous as usize, (*previous).size);
        }

        let block = start as *mut FreeBlock;
        block.write(FreeBlock { size, next: following });
        if previous.is_null() {
            self.head = block;
        } else {
            (*previous).next = block;
        }
        (start, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A heap over `size` bytes of leaked, 4096-aligned host memory.
    /// The memory a test's heap manages, given back when the test ends.
    /// (It was leaked, which Miri reports as an error - K187.) Bound to a
    /// name in each test, not `_`, so that it lives as long as the test.
    struct Backing(*mut u8, Layout);

    impl Drop for Backing {
        fn drop(&mut self) {
            // SAFETY: allocated with this layout by `heap`, given back once.
            unsafe { std::alloc::dealloc(self.0, self.1) };
        }
    }

    fn heap(size: usize) -> (Heap, usize, Backing) {
        let layout = Layout::from_size_align(size, 4096).unwrap();
        // SAFETY: a non-zero size.
        let memory = unsafe { std::alloc::alloc(layout) };
        let mut heap = Heap::empty();
        // SAFETY: `size` bytes, just allocated, used by nothing else.
        unsafe { heap.add_region(memory as usize, size) };
        (heap, memory as usize, Backing(memory, layout))
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn allocates_from_the_start_and_frees_back_to_one_block() {
        let (mut h, base, _memory) = heap(4096);
        let a = h.allocate(layout(100, 8)).unwrap();
        assert_eq!(a as usize, base);
        assert_eq!(h.used(), 112, "100 rounds up to 7 units");

        let b = h.allocate(layout(16, 8)).unwrap();
        assert_eq!(b as usize, base + 112);

        unsafe {
            h.deallocate(a, layout(100, 8));
            h.deallocate(b, layout(16, 8));
        }
        assert_eq!(h.used(), 0);
        assert_eq!(h.free_blocks(), 1, "everything merged back");
    }

    #[test]
    fn honours_large_alignment_and_keeps_the_padding() {
        let (mut h, base, _memory) = heap(8192);
        let small = h.allocate(layout(16, 16)).unwrap();
        let page = h.allocate(layout(64, 4096)).unwrap();
        assert_eq!(page as usize % 4096, 0);
        assert_eq!(page as usize, base + 4096);
        // The padding between them is still free: a small request uses it.
        let filler = h.allocate(layout(32, 16)).unwrap();
        assert_eq!(filler as usize, base + 16);
        unsafe {
            h.deallocate(small, layout(16, 16));
            h.deallocate(page, layout(64, 4096));
            h.deallocate(filler, layout(32, 16));
        }
        assert_eq!(h.free_blocks(), 1);
    }

    #[test]
    fn reports_exhaustion() {
        let (mut h, _, _memory) = heap(4096);
        assert!(h.allocate(layout(4096, 16)).is_some());
        assert!(h.allocate(layout(16, 16)).is_none());
    }

    #[test]
    fn the_pages_a_free_lets_go_of() {
        const P: usize = 4096;
        // A block freed alone, page-aligned: all its pages but the one
        // holding the free list's header.
        assert_eq!(pages_to_give_back((0x10000, 16 * P), (0x10000, 16 * P), P), Some((0x10000 + P, 15 * P)));
        // Freed beside a big free block it joined: only around what was
        // freed, the rest of the block having gone back already.
        assert_eq!(pages_to_give_back((0x10000, 100 * P), (0x10000 + 50 * P, 8 * P), P), Some((0x10000 + 50 * P, 8 * P)));
        // Not page-aligned: the pages it touches that are wholly free.
        assert_eq!(pages_to_give_back((0x10000, 100 * P), (0x10000 + 50 * P + 8, 2 * P), P), Some((0x10000 + 50 * P, 3 * P)));
        // The block ends mid-page: that page stays.
        assert_eq!(pages_to_give_back((0x10000, 3 * P + 100), (0x10000, 3 * P + 100), P), Some((0x10000 + P, 2 * P)));
        // Less than a whole page: nothing.
        assert_eq!(pages_to_give_back((0x10000 + 8, 2 * P), (0x10000 + 8, 2 * P), P), Some((0x10000 + P, P)));
        assert_eq!(pages_to_give_back((0x10000, P + 16), (0x10000, P + 16), P), None);
    }

    #[test]
    fn deallocate_says_which_free_block_the_memory_joined() {
        let (mut heap, base, _memory) = heap(4096);
        let layout = Layout::from_size_align(256, 16).unwrap();
        let a = heap.allocate(layout).unwrap() as usize;
        let b = heap.allocate(layout).unwrap() as usize;
        let c = heap.allocate(layout).unwrap() as usize;
        assert_eq!((a, b, c), (base, base + 256, base + 512));
        // Alone between two blocks in use: itself.
        assert_eq!(unsafe { heap.deallocate(b as *mut u8, layout) }, (b, 256));
        // Then a, before it: the two together, from a.
        assert_eq!(unsafe { heap.deallocate(a as *mut u8, layout) }, (a, 512));
        // Then c: all of the heap, one block.
        assert_eq!(unsafe { heap.deallocate(c as *mut u8, layout) }, (base, 4096));
    }

    #[test]
    fn merges_with_both_neighbours() {
        let (mut h, _, _memory) = heap(4096);
        let l = layout(64, 16);
        let blocks: Vec<_> = (0..4).map(|_| h.allocate(l).unwrap()).collect();
        unsafe {
            h.deallocate(blocks[0], l);
            h.deallocate(blocks[2], l);
            assert_eq!(h.free_blocks(), 3); // [0], [2], and the tail
            h.deallocate(blocks[1], l); // joins [0] and [2]
            assert_eq!(h.free_blocks(), 2);
            h.deallocate(blocks[3], l); // joins that with the tail
        }
        assert_eq!(h.free_blocks(), 1);
    }

    #[test]
    fn separate_regions_stay_separate_until_they_touch() {
        let (mut h, base, _memory) = heap(4096);
        let (_, other, _other_memory) = heap(4096);
        unsafe { h.add_region(other, 4096) };
        assert_eq!(h.total(), 8192);
        // A request bigger than either region cannot be satisfied,
        // unless the regions happen to be adjacent.
        let adjacent = other == base + 4096 || base == other + 4096;
        assert_eq!(h.allocate(layout(6000, 16)).is_some(), adjacent);
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_is_caught() {
        let (mut h, _, _memory) = heap(4096);
        let l = layout(32, 16);
        let a = h.allocate(l).unwrap();
        let _b = h.allocate(l).unwrap();
        unsafe {
            h.deallocate(a, l);
            h.deallocate(a, l);
        }
    }

    /// Thousands of random allocations and frees, checking that no two
    /// live allocations overlap, that each keeps the bytes written to it,
    /// and that freeing everything restores a single block.
    #[test]
    fn randomized_stress() {
        const SIZE: usize = 256 * 1024;
        let (mut h, base, _memory) = heap(SIZE);
        let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
        // xorshift: deterministic, so any failure reproduces.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        // Under Miri, which checks every memory access as it interprets
        // them, 400 rounds already go through every path; 20000 would take
        // an hour (K187).
        let rounds = if cfg!(miri) { 400 } else { 20_000 };
        for round in 0..rounds {
            if live.is_empty() || random() % 3 != 0 {
                let size = 1 + (random() % 2048) as usize;
                let align = 1 << (random() % 13); // 1 .. 4096
                let l = layout(size, align);
                if let Some(p) = h.allocate(l) {
                    let address = p as usize;
                    assert_eq!(address % align, 0);
                    assert!(address >= base && address + size <= base + SIZE);
                    for &(other, ol, _) in &live {
                        let o = other as usize;
                        assert!(address + size <= o || o + ol.size() <= address, "overlap");
                    }
                    let tag = round as u8;
                    unsafe { p.write_bytes(tag, size) };
                    live.push((p, l, tag));
                }
            } else {
                let victim = live.swap_remove((random() as usize) % live.len());
                let (p, l, tag) = victim;
                for i in 0..l.size() {
                    assert_eq!(unsafe { *p.add(i) }, tag, "allocation was overwritten");
                }
                unsafe { h.deallocate(p, l) };
            }
        }

        for (p, l, _) in live.drain(..) {
            unsafe { h.deallocate(p, l) };
        }
        assert_eq!(h.used(), 0);
        assert_eq!(h.free_blocks(), 1);
        assert_eq!(h.allocate(layout(SIZE, 16)).map(|p| p as usize), Some(base));
    }
}
