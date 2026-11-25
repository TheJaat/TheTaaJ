//! Random bytes: `getrandom`, `getentropy` and BSD's `arc4random`
//! family, all from the kernel's generator (RANDOM; the kernel's
//! random.rs and chacha.rs say how it is made).
//!
//! None of these blocks or fails for want of entropy: the kernel seeds
//! its generator before the first program starts.

use core::ffi::{c_int, c_uint, c_void};

use crate::errno::{EFAULT, EINVAL, EIO};

/// getrandom's flags (sys/random.h). Both are accepted and change
/// nothing: there is one generator, and it never blocks.
const GRND_NONBLOCK: c_uint = 1;
const GRND_RANDOM: c_uint = 2;
const GRND_INSECURE: c_uint = 4;

/// Fills `length` bytes at `buffer`, or says why not.
///
/// # Safety
/// `buffer` must be valid for writing `length` bytes.
unsafe fn fill(buffer: *mut c_void, length: usize) -> Result<(), c_int> {
    if length == 0 {
        return Ok(());
    }
    if buffer.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: the caller's promise.
    let bytes = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), length) };
    taaj::random(bytes).map_err(|_| EFAULT)
}

/// `getrandom(buffer, length, flags)`: `length` random bytes; returns
/// `length`. -1 with EINVAL for an unknown flag, EFAULT for memory that
/// is not the program's.
///
/// # Safety
/// `buffer` must be valid for writing `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn getrandom(buffer: *mut c_void, length: usize, flags: c_uint) -> isize {
    if flags & !(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE) != 0 {
        return crate::descriptors::fail::<isize>(EINVAL);
    }
    // SAFETY: the caller's promise.
    match unsafe { fill(buffer, length) } {
        Ok(()) => length as isize,
        Err(error) => crate::descriptors::fail::<isize>(error),
    }
}

/// `getentropy(buffer, length)`: at most 256 random bytes (EIO for more,
/// as OpenBSD and glibc say). 0, or -1.
///
/// # Safety
/// `buffer` must be valid for writing `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn getentropy(buffer: *mut c_void, length: usize) -> c_int {
    if length > 256 {
        return crate::descriptors::fail(EIO);
    }
    // SAFETY: the caller's promise.
    match unsafe { fill(buffer, length) } {
        Ok(()) => 0,
        Err(error) => crate::descriptors::fail(error),
    }
}

/// `arc4random_buf(buffer, length)`: `length` random bytes. Cannot fail.
///
/// # Safety
/// `buffer` must be valid for writing `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn arc4random_buf(buffer: *mut c_void, length: usize) {
    // SAFETY: the caller's promise. (The kernel refuses only memory that
    // is not the program's, which the promise rules out.)
    let _ = unsafe { fill(buffer, length) };
}

/// `arc4random()`: a random 32-bit number.
#[no_mangle]
pub extern "C" fn arc4random() -> u32 {
    let mut bytes = [0u8; 4];
    let _ = taaj::random(&mut bytes);
    u32::from_le_bytes(bytes)
}

/// `arc4random_uniform(bound)`: a random number below `bound`, every one
/// equally likely (0 for a bound below 2). Plain `arc4random() % bound`
/// would favour the small numbers whenever 2^32 is not a multiple of
/// `bound`; the numbers below 2^32 mod bound are thrown away instead.
#[no_mangle]
pub extern "C" fn arc4random_uniform(bound: u32) -> u32 {
    if bound < 2 {
        return 0;
    }
    let skip = bound.wrapping_neg() % bound; // 2^32 mod bound
    loop {
        let value = arc4random();
        if value >= skip {
            return value % bound;
        }
    }
}
