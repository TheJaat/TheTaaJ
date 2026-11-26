//! malloc, free, calloc, realloc: memory a C program asks for while it
//! runs - and strdup, the one string function that needs it.
//!
//! The heap is not new. Every Rust program on this system already has
//! one (user/src/allocator.rs, over the `heap` crate): `Vec` and
//! `String` use it. These functions are that same heap under C's names.
//!
//! One thing does not fit, and it is the interesting part. Rust's
//! allocator must be told the size of a block when the block is given
//! back - the type knows it, so it costs nothing. C's `free(p)` is told
//! only the address. So somebody has to remember the size, and here it
//! is this file: every block is asked for 16 bytes bigger than the
//! program wanted, the size is written in those first 16 bytes, and the
//! program is given the address just after them.
//!
//!     what the heap handed over
//!     |
//!     v
//!     +----------------+--------------------------------+
//!     | size (8) + pad |  the bytes the program asked for |
//!     +----------------+--------------------------------+
//!                      ^
//!                      |
//!                      what malloc returns, and free is given
//!
//! `free` steps back 16 bytes, reads the size, and can then give the
//! block back as Rust requires.
//!
//! And aligned_alloc, for a block on a stricter boundary than 16 (C++
//! asks for one for `new` of an over-aligned type). Its header is the
//! alignment's size, not 16, so that what follows it is aligned too;
//! the size and the alignment are in its last 16 bytes, just in front
//! of the address returned, where free always looks:
//!
//!     +---------------- ... --+------+-------+---------------------+
//!     |  padding              | size | align |  the program's bytes |
//!     +---------------- ... --+------+-------+---------------------+
//!     ^ the heap's block      ^ -16    ^ -8  ^ returned
//!
//! malloc's blocks keep 16 in the "align" word, so free can tell how
//! far back their start is - 16 bytes - the same way.
//!
//! Small blocks - 1 KiB or less, header included - do not go back to
//! the heap when they are freed. The heap is a list of free blocks in
//! address order (the `heap` crate), simple and O(n): every allocation
//! walks it for the first block big enough, every free walks it for the
//! place to put the block back. A C or C++ program makes and frees
//! small blocks by the million - LLVM's TableGen took three minutes
//! over work the host does in a second - and the list grows with them.
//! So each size of small block, in steps of 16 bytes, has a list of its
//! own: free puts a block on the front of its size's list, malloc takes
//! it off again - one step each way, however many there are. New small
//! blocks are cut from 64 KiB pieces of the heap. What is on these
//! lists stays there for blocks of that size: it is never given back to
//! the heap for others.

use alloc::alloc::{alloc, dealloc};
use core::alloc::Layout;
use core::ffi::{c_char, c_void};
use core::ptr;

/// `__taaj_reserve(size)` (for mmap.c): `size` bytes of reserved memory
/// (MEMORY_RESERVE), readable and writable, each page zeroed when first
/// touched; null if the address space would not have it.
#[no_mangle]
pub extern "C" fn __taaj_reserve(size: usize) -> *mut c_void {
    taaj::memory_reserve(size as u64, true).map_or(ptr::null_mut(), |start| start.cast())
}

/// `__taaj_unmap(start)` (for mmap.c): gives reserved memory back,
/// every page it took. 0, or -1 if nothing was reserved there.
#[no_mangle]
pub extern "C" fn __taaj_unmap(start: *mut c_void) -> core::ffi::c_int {
    if taaj::memory_unmap(start.cast()).is_ok() { 0 } else { -1 }
}

/// `__taaj_protect(start, length, access)` (for mmap.c): MEMORY_PROTECT
/// - `access` 0 for none, 1 read, 2 read and write. 0, or -1 with
/// errno: EINVAL for a range not of whole pages, ENOMEM for one not all
/// in one piece of reserved memory (Linux's answer for "not mapped").
#[no_mangle]
pub extern "C" fn __taaj_protect(start: *mut c_void, length: usize, access: core::ffi::c_int) -> core::ffi::c_int {
    match taaj::memory_protect(start.cast(), length as u64, access as u64) {
        Ok(()) => 0,
        Err(error) => memory_failure(error),
    }
}

/// `__taaj_discard(start, length)` (for mmap.c): MEMORY_DISCARD - the
/// pages' memory given back, each zeroed again when next touched. 0, or
/// -1 with errno as for __taaj_protect.
#[no_mangle]
pub extern "C" fn __taaj_discard(start: *mut c_void, length: usize) -> core::ffi::c_int {
    match taaj::memory_discard(start.cast(), length as u64) {
        Ok(()) => 0,
        Err(error) => memory_failure(error),
    }
}

fn memory_failure(error: taaj::Error) -> core::ffi::c_int {
    let code = match error {
        taaj::Error::InvalidArgument => crate::errno::EINVAL,
        _ => crate::errno::ENOMEM,
    };
    crate::errno::errno.store(code, core::sync::atomic::Ordering::Relaxed);
    -1
}

/// `__taaj_memory_used()`: the bytes the kernel counts against this
/// program now (MEMORY_USED) - for tests and reports; getrlimit says
/// what it may.
#[no_mangle]
pub extern "C" fn __taaj_memory_used() -> u64 {
    taaj::memory_used()
}

/// The bytes in front of every block of malloc's, where its size and
/// alignment are kept.
const HEADER: usize = 16;

/// Every block starts on a multiple of this. C promises that what
/// malloc returns is aligned for any ordinary type; on x86-64 the
/// strictest of them (long double) wants 16. The header is the same
/// size, so the address after it is as well aligned as the block.
const ALIGNMENT: usize = 16;

/// The largest block, header included, kept on a list of its size.
const SMALL_MAX: usize = 1024;

/// How much is taken from the heap at a time to cut small blocks of one
/// size from: a piece, on a multiple of its own size, so the piece a
/// block is in is its address with the low bits cleared.
const PIECE: usize = 64 * 1024;

/// The start of every piece: which size its blocks are, how many are in
/// use, its own list of free blocks, where uncut room begins, and its
/// neighbours on its size's list of pieces with a block to spare.
#[repr(C)]
struct Piece {
    class: usize,
    in_use: usize,
    free: *mut u8,
    uncut: *mut u8,
    previous: *mut Piece,
    next: *mut Piece,
}

/// Room for the Piece at the start of each, kept a multiple of 16.
const PIECE_HEADER: usize = 64;
const _: () = assert!(core::mem::size_of::<Piece>() <= PIECE_HEADER);

/// The small blocks: for each size (16, 32, ..., SMALL_MAX, at index
/// size / 16), the pieces with a block to give - one free, or room not
/// yet cut. A piece whose blocks are all taken is on no list, and comes
/// back onto its list when one is freed. A piece whose blocks are all
/// free goes back to the heap (whose memory goes back to the kernel:
/// user/src/allocator.rs) - unless it is the only one its size has, so
/// a program that takes and frees one block over and over does not
/// make and break a piece each time.
struct Small {
    pieces: [*mut Piece; SMALL_MAX / ALIGNMENT + 1],
}

/// The one Small, behind a futex lock (taaj::sync): the program's
/// threads take turns, and one that finds it taken sleeps.
struct SmallLock {
    locked: taaj::sync::Lock,
    small: core::cell::UnsafeCell<Small>,
}

// SAFETY: `small` is reached only while `locked` is held.
unsafe impl Sync for SmallLock {}

static SMALL: SmallLock = SmallLock {
    locked: taaj::sync::Lock::new(),
    small: core::cell::UnsafeCell::new(Small { pieces: [ptr::null_mut(); SMALL_MAX / ALIGNMENT + 1] }),
};

fn with_small<R>(f: impl FnOnce(&mut Small) -> R) -> R {
    SMALL.locked.lock();
    // SAFETY: the lock is held, so this is the only reference.
    let result = f(unsafe { &mut *SMALL.small.get() });
    SMALL.locked.unlock();
    result
}

/// Which list a block for `size` bytes is on, if it is small: its whole
/// size, header included, in 16-byte steps.
fn small_class(size: usize) -> Option<usize> {
    let whole = size.checked_add(HEADER)?;
    (whole <= SMALL_MAX).then(|| whole.div_ceil(ALIGNMENT))
}

fn piece_layout() -> Layout {
    // A power of two, so this cannot fail.
    Layout::from_size_align(PIECE, PIECE).unwrap()
}

impl Small {
    /// Puts `piece` at the front of its size's list.
    ///
    /// # Safety
    /// `piece` is a live piece of this Small's, on no list.
    unsafe fn push(&mut self, piece: *mut Piece) {
        unsafe {
            let class = (*piece).class;
            (*piece).previous = ptr::null_mut();
            (*piece).next = self.pieces[class];
            if !self.pieces[class].is_null() {
                (*self.pieces[class]).previous = piece;
            }
            self.pieces[class] = piece;
        }
    }

    /// Takes `piece` off its size's list.
    ///
    /// # Safety
    /// `piece` is a live piece of this Small's, on its list.
    unsafe fn unlink(&mut self, piece: *mut Piece) {
        unsafe {
            let (previous, next) = ((*piece).previous, (*piece).next);
            if previous.is_null() {
                self.pieces[(*piece).class] = next;
            } else {
                (*previous).next = next;
            }
            if !next.is_null() {
                (*next).previous = previous;
            }
            (*piece).previous = ptr::null_mut();
            (*piece).next = ptr::null_mut();
        }
    }
}

/// Whether `piece` has a block to give: one free, or room for one more.
///
/// # Safety
/// `piece` is a live piece.
unsafe fn has_room(piece: *mut Piece) -> bool {
    unsafe { !(*piece).free.is_null() || (*piece).uncut as usize + (*piece).class * ALIGNMENT <= piece as usize + PIECE }
}

/// A block of `class` * 16 bytes: from the first piece of its size with
/// one to give, or from a new piece.
fn take_small(class: usize) -> *mut u8 {
    with_small(|small| {
        let mut piece = small.pieces[class];
        if piece.is_null() {
            // SAFETY: the layout is of nonzero size.
            let memory = unsafe { alloc(piece_layout()) };
            if memory.is_null() {
                return ptr::null_mut();
            }
            piece = memory.cast::<Piece>();
            // SAFETY: PIECE bytes, aligned for a Piece, ours alone.
            unsafe {
                piece.write(Piece {
                    class,
                    in_use: 0,
                    free: ptr::null_mut(),
                    uncut: memory.add(PIECE_HEADER),
                    previous: ptr::null_mut(),
                    next: ptr::null_mut(),
                });
                small.push(piece);
            }
        }
        // SAFETY: a live piece on its list, so it has room (has_room).
        unsafe {
            let block = if (*piece).free.is_null() {
                let block = (*piece).uncut;
                (*piece).uncut = block.add(class * ALIGNMENT);
                block
            } else {
                // A free block holds the next one's address in its first
                // word (put there by give_small).
                let block = (*piece).free;
                (*piece).free = block.cast::<*mut u8>().read();
                block
            };
            (*piece).in_use += 1;
            if !has_room(piece) {
                small.unlink(piece);
            }
            block
        }
    })
}

/// Gives a small block back to its piece - and the piece back to the
/// heap, if that was its last block in use and its size has another.
///
/// # Safety
/// `block` must be a block of `class` taken by take_small, not in use.
unsafe fn give_small(block: *mut u8, class: usize) {
    let empty = with_small(|small| {
        // SAFETY: the block is in a live piece (pieces are aligned to
        // their size), at least 16 bytes and aligned for a pointer.
        unsafe {
            let piece = (block as usize & !(PIECE - 1)) as *mut Piece;
            debug_assert_eq!((*piece).class, class);
            let was_full = !has_room(piece);
            block.cast::<*mut u8>().write((*piece).free);
            (*piece).free = block;
            (*piece).in_use -= 1;
            if was_full {
                small.push(piece);
            }
            let alone = small.pieces[class] == piece && (*piece).next.is_null();
            if (*piece).in_use == 0 && !alone {
                small.unlink(piece);
                return piece;
            }
            ptr::null_mut()
        }
    });
    if !empty.is_null() {
        // Outside the small lock: the heap has its own.
        // SAFETY: a piece from `alloc` with this layout, on no list, no
        // block of it in use.
        unsafe { dealloc(empty.cast(), piece_layout()) };
    }
}

/// The size written in front of a block that malloc returned.
///
/// # Safety
/// `pointer` must have come from `malloc` (or aligned_alloc) and not
/// have been freed.
unsafe fn size_of_block(pointer: *mut c_void) -> usize {
    // SAFETY: malloc wrote a usize `HEADER` bytes before the address it
    // returned, at an address aligned for one.
    unsafe { pointer.cast::<u8>().sub(HEADER).cast::<usize>().read() }
}

/// The alignment written in front of a block: 16 for malloc's.
///
/// # Safety
/// As for `size_of_block`.
unsafe fn alignment_of_block(pointer: *mut c_void) -> usize {
    // SAFETY: the word after the size, 8 bytes before the address.
    unsafe { pointer.cast::<u8>().sub(HEADER / 2).cast::<usize>().read() }
}

/// Asks the heap for `size` bytes on a multiple of `alignment` (a power
/// of two, 16 or more), behind a header of `alignment` bytes, and writes
/// the size and alignment at the header's end.
fn allocate(size: usize, alignment: usize) -> *mut c_void {
    if alignment == ALIGNMENT {
        if let Some(class) = small_class(size) {
            let block = take_small(class);
            if block.is_null() {
                return no_memory();
            }
            // SAFETY: the block has class * 16 >= size + HEADER bytes,
            // aligned to 16: the header's two words fit before `start`.
            unsafe {
                let start = block.add(HEADER);
                start.sub(HEADER).cast::<usize>().write(size);
                start.sub(HEADER / 2).cast::<usize>().write(ALIGNMENT);
                return start.cast();
            }
        }
    }
    let Some(layout) = size.checked_add(alignment).and_then(|total| Layout::from_size_align(total, alignment).ok())
    else {
        return no_memory();
    };
    // SAFETY: the layout is never of size zero: it includes the header.
    let block = unsafe { alloc(layout) };
    if block.is_null() {
        return no_memory();
    }
    // SAFETY: the block is at least `alignment` (>= 16) bytes and aligned
    // to it, so the address `alignment` bytes on is aligned too and the
    // two words just before it are inside the block, each aligned for a
    // usize.
    unsafe {
        let start = block.add(alignment);
        start.sub(HEADER).cast::<usize>().write(size);
        start.sub(HEADER / 2).cast::<usize>().write(alignment);
        start.cast()
    }
}

/// The answer when there is no memory: null, and the reason in errno.
fn no_memory() -> *mut c_void {
    crate::errno::errno.store(crate::errno::ENOMEM, core::sync::atomic::Ordering::Relaxed);
    ptr::null_mut()
}

/// `malloc(size)`: `size` bytes to use until they are freed, or null
/// if there is no memory. What is in them is not defined.
///
/// `malloc(0)` returns an address that is not null, holds no bytes, and
/// can be freed.
#[no_mangle]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    allocate(size, ALIGNMENT)
}

/// `aligned_alloc(alignment, size)`: as malloc, on a multiple of
/// `alignment`, which must be a power of two - or null, with errno
/// EINVAL, if it is not. Freed by free.
#[no_mangle]
pub extern "C" fn aligned_alloc(alignment: usize, size: usize) -> *mut c_void {
    if !alignment.is_power_of_two() {
        crate::errno::errno.store(crate::errno::EINVAL, core::sync::atomic::Ordering::Relaxed);
        return ptr::null_mut();
    }
    allocate(size, alignment.max(ALIGNMENT))
}

/// `free(pointer)`: gives back what malloc, calloc or realloc returned.
/// `free(NULL)` does nothing.
///
/// # Safety
/// `pointer` must be null, or have come from one of those three and
/// not have been freed already. Freeing twice, or using the memory
/// afterwards, is the program's own bug and nothing here catches it.
#[no_mangle]
pub unsafe extern "C" fn free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: by the caller's promise this is a live block of malloc's
    // or aligned_alloc's: its size and alignment are in front of it, the
    // heap's own block starts `alignment` bytes back, and was asked for
    // with exactly this layout - which was valid then, so it is now.
    unsafe {
        let size = size_of_block(pointer);
        let alignment = alignment_of_block(pointer);
        if alignment == ALIGNMENT {
            if let Some(class) = small_class(size) {
                give_small(pointer.cast::<u8>().sub(HEADER), class);
                return;
            }
        }
        let layout = Layout::from_size_align_unchecked(size + alignment, alignment);
        dealloc(pointer.cast::<u8>().sub(alignment), layout);
    }
}

/// `calloc(count, size)`: room for `count` things of `size` bytes each,
/// all zero - or null if there is no memory, or if the two multiplied
/// are more than a size can hold.
#[no_mangle]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    // The multiplication is the reason calloc exists as well as malloc:
    // done by hand, `malloc(count * size)` can wrap round to a small
    // number, and the program then writes far past a small block.
    let Some(total) = count.checked_mul(size) else {
        return no_memory();
    };
    let block = malloc(total);
    if !block.is_null() {
        // SAFETY: malloc returned `total` writable bytes.
        unsafe { ptr::write_bytes(block.cast::<u8>(), 0, total) };
    }
    block
}

/// `realloc(pointer, size)`: the same bytes in a block of a new size.
/// The block may move: the old address must not be used again, only
/// the one returned. If there is no memory, returns null and the old
/// block is untouched and still the program's.
///
/// `realloc(NULL, size)` is `malloc(size)`. `realloc(pointer, 0)` frees
/// the block and returns null.
///
/// # Safety
/// As for `free`.
#[no_mangle]
pub unsafe extern "C" fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
    if pointer.is_null() {
        return malloc(size);
    }
    if size == 0 {
        // SAFETY: the caller's promise.
        unsafe { free(pointer) };
        return ptr::null_mut();
    }
    // Always a new block, a copy, and the old one freed. A better one
    // would first see whether the block can grow where it is.
    let new = malloc(size);
    if new.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the old block holds `old` bytes and the new one `size`;
    // the smaller of the two is in both. They are different blocks, so
    // they do not overlap. Then the old one is given back.
    unsafe {
        let old = size_of_block(pointer);
        ptr::copy_nonoverlapping(pointer.cast::<u8>(), new.cast::<u8>(), old.min(size));
        free(pointer);
    }
    new
}

/// `strndup(s, n)`: strdup of at most `n` bytes of `s`, and a zero.
///
/// # Safety
/// `s` must be a string, or `n` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn strndup(s: *const c_char, n: usize) -> *mut c_char {
    // SAFETY: the caller's promise.
    let length = unsafe { crate::string::strnlen(s, n) };
    let copy = malloc(length + 1).cast::<c_char>();
    if !copy.is_null() {
        // SAFETY: `length` bytes are readable at `s`; the new block has
        // room for them and the zero.
        unsafe {
            ptr::copy_nonoverlapping(s, copy, length);
            copy.add(length).write(0);
        }
    }
    copy
}

/// `strdup(s)`: a copy of the string in memory from malloc, for the
/// program to free - or null if there is no memory.
///
/// # Safety
/// `s` must point to a string that ends in a zero byte.
#[no_mangle]
pub unsafe extern "C" fn strdup(s: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let length = unsafe { crate::string::strnlen(s, usize::MAX) };
    let copy = malloc(length + 1).cast::<c_char>();
    if !copy.is_null() {
        // SAFETY: `length` bytes and the zero are readable at `s`, and
        // the new block has room for exactly that.
        unsafe { ptr::copy_nonoverlapping(s, copy, length + 1) };
    }
    copy
}
