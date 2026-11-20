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

use alloc::alloc::{alloc, dealloc};
use core::alloc::Layout;
use core::ffi::{c_char, c_void};
use core::ptr;

/// The bytes in front of every block, where its size is kept.
const HEADER: usize = 16;

/// Every block starts on a multiple of this. C promises that what
/// malloc returns is aligned for any ordinary type; on x86-64 the
/// strictest of them (long double) wants 16. The header is the same
/// size, so the address after it is as well aligned as the block.
const ALIGNMENT: usize = 16;

/// What to ask the heap for, for a block of `size` bytes: the header
/// and the bytes. None if that is more than an address can count.
fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HEADER)?, ALIGNMENT).ok()
}

/// The size written in front of a block that malloc returned.
///
/// # Safety
/// `pointer` must have come from `malloc` and not have been freed.
unsafe fn size_of_block(pointer: *mut c_void) -> usize {
    // SAFETY: malloc wrote a usize `HEADER` bytes before the address it
    // returned, at an address aligned for one.
    unsafe { pointer.cast::<u8>().sub(HEADER).cast::<usize>().read() }
}

/// `malloc(size)`: `size` bytes to use until they are freed, or null
/// if there is no memory. What is in them is not defined.
///
/// `malloc(0)` returns an address that is not null, holds no bytes, and
/// can be freed.
#[no_mangle]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    let Some(layout) = layout(size) else {
        return ptr::null_mut();
    };
    // SAFETY: the layout is never of size zero: it includes the header.
    let block = unsafe { alloc(layout) };
    if block.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the block is at least HEADER bytes, aligned to 16: a
    // usize fits at its start, and the address HEADER bytes on is still
    // inside it (or just at its end, when `size` is zero).
    unsafe {
        block.cast::<usize>().write(size);
        block.add(HEADER).cast()
    }
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
    // SAFETY: by the caller's promise this is a live block of malloc's:
    // its size is in front of it, the heap's own block starts HEADER
    // bytes back, and was asked for with exactly this layout - which
    // was valid then, so it is now.
    unsafe {
        let size = size_of_block(pointer);
        let layout = Layout::from_size_align_unchecked(size + HEADER, ALIGNMENT);
        dealloc(pointer.cast::<u8>().sub(HEADER), layout);
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
        return ptr::null_mut();
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
