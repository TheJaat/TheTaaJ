//! Numbers from text, the time, and the parts of a Unix process that
//! TheTaaJ does not have.
//!
//!   __taaj_parse_double, __taaj_parse_float
//!            the work of strtod and strtof (libc/c/stdlib.c)
//!   time, gettimeofday
//!   getenv, execvp
//!
//! Turning "0.1" into the nearest double is harder than it looks: the
//! decimal 0.1 has no exact binary value, and finding the closest one
//! takes care (and, for some inputs, arithmetic on numbers far longer
//! than 64 bits). Rust's `core` does it - correctly rounded, always -
//! and needs no operating system to do it. So the C library borrows it.
//!
//! How the answer gets back to C needs care too. This half of the
//! library is built for a target without floating-point registers, and
//! so returns an f64 the way that target does: in a general register.
//! A C caller, built with them, looks for it in XMM0 (or v0). The two
//! would never meet. So these functions return the number's bits as an
//! integer - that every target returns the same way - and C turns the
//! bits back into a double (memcpy, in stdlib.c).

use core::ffi::{c_char, c_int, c_long, CStr};
use core::ptr;
use core::sync::atomic::Ordering;

use crate::errno::{errno, ENOSYS, ERANGE};
use crate::number::{number_length, out_of_range};

/// Reads the number at `text` (after any white space): its value, and
/// where it ended - or None, if no number starts there.
///
/// # Safety
/// `text` must point to a string that ends in a zero byte.
unsafe fn parse<T: core::str::FromStr>(text: *const c_char) -> Option<(T, *const c_char)> {
    // SAFETY: the caller's promise.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    let start = bytes.iter().take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)).count();
    let length = number_length(&bytes[start..]);
    if length == 0 {
        return None;
    }
    // Only ASCII was matched, so this is text; and everything matched
    // is something Rust's parser reads too.
    let value = core::str::from_utf8(&bytes[start..start + length]).ok()?.parse().ok()?;
    // SAFETY: start + length is within the string.
    Some((value, unsafe { text.add(start + length) }))
}

/// Says where the number ended, if the caller asked.
///
/// # Safety
/// `end` must be null or valid for writing.
unsafe fn tell_end(end: *mut *mut c_char, at: *const c_char) {
    if !end.is_null() {
        // SAFETY: the caller's promise.
        unsafe { end.write(at.cast_mut()) };
    }
}

/// strtod's work: the double's bits.
///
/// # Safety
/// `text` must point to a string that ends in a zero byte; `end` null
/// or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn __taaj_parse_double(text: *const c_char, end: *mut *mut c_char) -> u64 {
    // SAFETY: the caller's promises.
    match unsafe { parse::<f64>(text) } {
        Some((value, at)) => {
            // SAFETY: both point into the same string, at before end.
            let read = unsafe { core::slice::from_raw_parts(text.cast::<u8>(), at.offset_from(text) as usize) };
            if out_of_range(value.is_infinite(), value == 0.0, read) {
                errno.store(ERANGE, Ordering::Relaxed);
            }
            // SAFETY: the caller's promise.
            unsafe { tell_end(end, at) };
            value.to_bits()
        }
        None => {
            // No number: 0, and the end is the start.
            // SAFETY: the caller's promise.
            unsafe { tell_end(end, text) };
            0
        }
    }
}

/// strtof's work: the float's bits. Read straight to a float, not via
/// a double: rounding twice can give a different, wrong, answer.
///
/// # Safety
/// As for `__taaj_parse_double`.
#[no_mangle]
pub unsafe extern "C" fn __taaj_parse_float(text: *const c_char, end: *mut *mut c_char) -> u32 {
    // SAFETY: the caller's promises.
    match unsafe { parse::<f32>(text) } {
        Some((value, at)) => {
            // SAFETY: both point into the same string, at before end.
            let read = unsafe { core::slice::from_raw_parts(text.cast::<u8>(), at.offset_from(text) as usize) };
            if out_of_range(value.is_infinite(), value == 0.0, read) {
                errno.store(ERANGE, Ordering::Relaxed);
            }
            // SAFETY: the caller's promise.
            unsafe { tell_end(end, at) };
            value.to_bits()
        }
        None => {
            // SAFETY: the caller's promise.
            unsafe { tell_end(end, text) };
            0
        }
    }
}

/// `struct timeval`, as sys/time.h declares it.
#[repr(C)]
pub struct Timeval {
    tv_sec: c_long,
    tv_usec: c_long,
}

/// Seconds and microseconds since TheTaaJ started. There is no clock
/// that knows the date yet - no driver for the PC's real-time clock or
/// AArch64's - so "the epoch" here is the moment the system booted, not
/// 1 January 1970, and a program that prints the date gets a day in
/// January 1970.
fn since_boot() -> (u64, u64) {
    let ticks = taaj::ticks();
    let per_second = abi::TICKS_PER_SECOND;
    (ticks / per_second, ticks % per_second * (1_000_000 / per_second))
}

/// `time(t)`: seconds since the epoch (see `since_boot`), also stored in
/// `*t` if `t` is not null.
///
/// # Safety
/// `t` must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn time(t: *mut c_long) -> c_long {
    let seconds = since_boot().0 as c_long;
    if !t.is_null() {
        // SAFETY: the caller's promise.
        unsafe { t.write(seconds) };
    }
    seconds
}

/// `gettimeofday(tv, tz)`: the time to the tick (10 ms), as seconds and
/// microseconds. The time zone, an argument that POSIX itself says to
/// pass as null, is ignored.
///
/// # Safety
/// `tv` must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn gettimeofday(tv: *mut Timeval, _tz: *mut u8) -> c_int {
    let (seconds, microseconds) = since_boot();
    if !tv.is_null() {
        // SAFETY: the caller's promise.
        unsafe { tv.write(Timeval { tv_sec: seconds as c_long, tv_usec: microseconds as c_long }) };
    }
    0
}

/// `struct timespec`, as time.h declares it.
#[repr(C)]
pub struct Timespec {
    tv_sec: c_long,
    tv_nsec: c_long,
}

/// The two clocks time.h names; both are the time since boot.
const CLOCK_REALTIME: c_int = 0;
const CLOCK_MONOTONIC: c_int = 1;
/// timespec_get's one time base.
const TIME_UTC: c_int = 1;

/// The time since boot as a timespec.
fn now() -> Timespec {
    let (seconds, microseconds) = since_boot();
    Timespec { tv_sec: seconds as c_long, tv_nsec: (microseconds * 1000) as c_long }
}

/// `clock_gettime(clock, t)`: the time on CLOCK_REALTIME or
/// CLOCK_MONOTONIC, which here are the same clock - since boot, never
/// set back. 0, or -1 with errno EINVAL for any other clock.
///
/// # Safety
/// `t` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn clock_gettime(clock: c_int, t: *mut Timespec) -> c_int {
    if clock != CLOCK_REALTIME && clock != CLOCK_MONOTONIC {
        crate::errno::errno.store(crate::errno::EINVAL, core::sync::atomic::Ordering::Relaxed);
        return -1;
    }
    // SAFETY: the caller's promise.
    unsafe { t.write(now()) };
    0
}

/// `timespec_get(t, base)`: C11's clock_gettime(CLOCK_REALTIME, t).
/// Returns `base` if it is TIME_UTC, the only one; else 0.
///
/// # Safety
/// `t` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn timespec_get(t: *mut Timespec, base: c_int) -> c_int {
    if base != TIME_UTC {
        return 0;
    }
    // SAFETY: the caller's promise.
    unsafe { t.write(now()) };
    base
}

/// `clock()`: the processor time used, in microseconds
/// (CLOCKS_PER_SEC). TheTaaJ does not tell a program its own share of
/// the CPU yet, so this is the time since boot - which, measured twice
/// and subtracted, is at least an upper bound on the time used between.
#[no_mangle]
pub extern "C" fn clock() -> c_long {
    let (seconds, microseconds) = since_boot();
    (seconds * 1_000_000 + microseconds) as c_long
}

/// `getenv(name)`: a TheTaaJ program has no environment - what it is
/// given comes as handles and arguments - so no variable is ever set.
#[no_mangle]
pub extern "C" fn getenv(_name: *const c_char) -> *mut c_char {
    ptr::null_mut()
}

/// `execvp(file, argv)`: replacing the running program with another is
/// not something TheTaaJ does - a program asks for a new process
/// instead - so this always fails, with ENOSYS.
#[no_mangle]
pub extern "C" fn execvp(_file: *const c_char, _argv: *const *const c_char) -> c_int {
    crate::descriptors::fail(ENOSYS)
}
