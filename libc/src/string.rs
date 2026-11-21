//! string.h: C's strings, and the functions that walk them.
//!
//! A C string is not a type. It is a promise: "at this address there
//! are bytes, and somewhere after them a byte that is zero". Nothing
//! records how long it is; every function here finds out by walking to
//! the zero. That is why all of them are `unsafe`: Rust cannot check
//! the promise, only the caller can keep it.
//!
//! Five of the functions string.h names are not in this file, because
//! they are already in libc.a: `memcpy`, `memmove`, `memset`, `memcmp`
//! and `strlen` come with Rust's own support library
//! (compiler_builtins), which needs them itself.
//!
//! Nothing here asks the system for anything, so this file can also be
//! compiled on its own for the machine it is written on, and tested
//! there: `make libc-test`. In that build the functions keep their Rust
//! names (the `cfg_attr`s), or they would take the place of the host's
//! own `strcmp` in the test program itself.

#![cfg_attr(test, allow(dead_code))]

use core::cmp::Ordering;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use core::slice;

/// How many bytes come before the zero.
///
/// # Safety
/// `s` must point to a string that ends in a zero byte.
unsafe fn length(s: *const c_char) -> usize {
    let mut n = 0;
    // SAFETY: every byte up to and including the zero is the caller's
    // to read, and the loop stops at the zero.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// The same, but looking at no more than `max` bytes - for the
/// functions that are told how far they may go.
///
/// # Safety
/// `s` must point to `max` readable bytes, or to a string that ends
/// sooner.
unsafe fn length_within(s: *const c_char, max: usize) -> usize {
    let mut n = 0;
    // SAFETY: as above, and never past `max`.
    while n < max && unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// A C string as a Rust slice: its bytes, without the zero.
///
/// # Safety
/// `s` must point to a string that ends in a zero byte, which is not
/// changed while the slice is in use.
unsafe fn bytes<'a>(s: *const c_char) -> &'a [u8] {
    // SAFETY: `length` bytes are readable: it has just walked them.
    unsafe { slice::from_raw_parts(s.cast::<u8>(), length(s)) }
}

/// `strnlen(s, max)`: the length of `s`, or `max` if there is no zero
/// in the first `max` bytes.
///
/// # Safety
/// `s` must point to `max` readable bytes or a string that ends sooner.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strnlen(s: *const c_char, max: usize) -> usize {
    // SAFETY: the caller's promise is `length_within`'s requirement.
    unsafe { length_within(s, max) }
}

/// `strcmp(a, b)`: negative, zero or positive as `a` sorts before, the
/// same as, or after `b` - comparing bytes as numbers from 0 to 255.
///
/// # Safety
/// Both must point to strings that end in a zero byte.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strcmp(a: *const c_char, b: *const c_char) -> c_int {
    // SAFETY: the caller's promise. Comparing the slices is comparing
    // the strings: a string that is the start of another is the lesser
    // in both, since the zero that ends it is less than any byte.
    match unsafe { bytes(a).cmp(bytes(b)) } {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// `strncmp(a, b, n)`: `strcmp`, looking at no more than `n` bytes.
///
/// # Safety
/// Each must point to `n` readable bytes or a string that ends sooner.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int {
    for at in 0..n {
        // As bytes whatever `char` is: it is signed on x86_64 and
        // unsigned on AArch64, and C compares strings as unsigned.
        // SAFETY: neither string has ended before `at` (the loop would
        // have returned), and `at` is less than `n`.
        let (x, y) = unsafe { (*a.add(at).cast::<u8>(), *b.add(at).cast::<u8>()) };
        if x != y {
            return if x < y { -1 } else { 1 };
        }
        if x == 0 {
            break;
        }
    }
    0
}

/// `strcpy(to, from)`: copies the string and its zero. Returns `to`.
///
/// # Safety
/// `from` must be a string; `to` must have room for all of it and the
/// zero; the two must not overlap. Nothing checks the room - this is
/// the function behind a great many security holes.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strcpy(to: *mut c_char, from: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise: the length and one more byte are
    // readable at `from` and writable at `to`, and do not overlap.
    unsafe { ptr::copy_nonoverlapping(from, to, length(from) + 1) };
    to
}

/// `strncpy(to, from, n)`: writes exactly `n` bytes: the string, then
/// zeros to make up the count. If the string has `n` bytes or more, no
/// zero is written, and `to` is then not a string.
///
/// # Safety
/// `to` must have room for `n` bytes; `from` must be `n` readable
/// bytes or a string that ends sooner; the two must not overlap.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strncpy(to: *mut c_char, from: *const c_char, n: usize) -> *mut c_char {
    // SAFETY: `copied` is at most `n`, so both the copy and the zeros
    // after it stay inside the `n` bytes the caller promised.
    unsafe {
        let copied = length_within(from, n);
        ptr::copy_nonoverlapping(from, to, copied);
        ptr::write_bytes(to.add(copied), 0, n - copied);
    }
    to
}

/// `strcat(to, from)`: copies `from` onto the end of `to`. Returns `to`.
///
/// # Safety
/// Both must be strings; `to` must have room after its end for all of
/// `from` and the zero; the two must not overlap.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strcat(to: *mut c_char, from: *const c_char) -> *mut c_char {
    // SAFETY: the end of `to` is inside it, and what follows is
    // `strcpy`'s requirement, which the caller has promised.
    unsafe { strcpy(to.add(length(to)), from) };
    to
}

/// `strncat(to, from, n)`: copies at most `n` bytes of `from` onto the
/// end of `to`, and always a zero after them.
///
/// # Safety
/// `to` must be a string with room after its end for `n` bytes and a
/// zero; `from` must be `n` readable bytes or a string that ends
/// sooner; the two must not overlap.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strncat(to: *mut c_char, from: *const c_char, n: usize) -> *mut c_char {
    // SAFETY: `copied` is at most `n`; the caller promised room for
    // that many and one more after the end of `to`.
    unsafe {
        let end = to.add(length(to));
        let copied = length_within(from, n);
        ptr::copy_nonoverlapping(from, end, copied);
        *end.add(copied) = 0;
    }
    to
}

/// `strchr(s, c)`: where the byte `c` first is in `s`, or null. The
/// zero at the end counts as part of the string, so `strchr(s, 0)`
/// finds the end.
///
/// # Safety
/// `s` must point to a string that ends in a zero byte.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strchr(s: *const c_char, c: c_int) -> *mut c_char {
    let wanted = c as u8;
    let mut at = s;
    loop {
        // SAFETY: `at` has not passed the zero: the loop returns there.
        let byte = unsafe { *at.cast::<u8>() };
        if byte == wanted {
            return at.cast_mut();
        }
        if byte == 0 {
            return ptr::null_mut();
        }
        // SAFETY: the byte was not the zero, so the next is the string's.
        at = unsafe { at.add(1) };
    }
}

/// `strrchr(s, c)`: where the byte `c` last is in `s`, or null.
///
/// # Safety
/// `s` must point to a string that ends in a zero byte.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strrchr(s: *const c_char, c: c_int) -> *mut c_char {
    let wanted = c as u8;
    // SAFETY: the caller's promise; the zero itself is looked at too.
    let with_zero = unsafe { slice::from_raw_parts(s.cast::<u8>(), length(s) + 1) };
    match with_zero.iter().rposition(|&byte| byte == wanted) {
        // SAFETY: `at` is inside the string.
        Some(at) => unsafe { s.add(at).cast_mut() },
        None => ptr::null_mut(),
    }
}

/// `strstr(in, find)`: where the string `find` first is inside `in`, or
/// null. An empty `find` is found at the start.
///
/// # Safety
/// Both must point to strings that end in a zero byte.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strstr(within: *const c_char, find: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let (within_bytes, find) = unsafe { (bytes(within), bytes(find)) };
    if find.is_empty() {
        return within.cast_mut();
    }
    match within_bytes.windows(find.len()).position(|window| window == find) {
        // SAFETY: `at` is inside the string.
        Some(at) => unsafe { within.add(at).cast_mut() },
        None => ptr::null_mut(),
    }
}

/// `strspn(s, set)`: how many bytes at the start of `s` are all among
/// the bytes of `set`.
///
/// # Safety
/// Both must point to strings that end in a zero byte.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strspn(s: *const c_char, set: *const c_char) -> usize {
    // SAFETY: the caller's promise.
    let (s, set) = unsafe { (bytes(s), bytes(set)) };
    s.iter().take_while(|byte| set.contains(byte)).count()
}

/// `strcspn(s, set)`: how many bytes at the start of `s` are *not*
/// among the bytes of `set` - where the first one that is stands, or
/// the length of `s` if none is. `line[strcspn(line, "\n")] = 0` cuts
/// a line at its new line, and does nothing wrong if it has none.
///
/// # Safety
/// Both must point to strings that end in a zero byte.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strcspn(s: *const c_char, set: *const c_char) -> usize {
    // SAFETY: the caller's promise.
    let (s, set) = unsafe { (bytes(s), bytes(set)) };
    s.iter().take_while(|byte| !set.contains(byte)).count()
}

/// `memchr(s, c, n)`: where the byte `c` first is in the `n` bytes at
/// `s`, or null. Memory, not a string: a zero does not stop it.
///
/// # Safety
/// `s` must point to `n` readable bytes.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn memchr(s: *const c_void, c: c_int, n: usize) -> *mut c_void {
    if n == 0 {
        return ptr::null_mut(); // and `s` may then be anything, even null
    }
    let wanted = c as u8;
    // SAFETY: the caller's promise, and `n` is not zero.
    let memory = unsafe { slice::from_raw_parts(s.cast::<u8>(), n) };
    match memory.iter().position(|&byte| byte == wanted) {
        // SAFETY: `at` is inside the `n` bytes.
        Some(at) => unsafe { s.add(at).cast_mut() },
        None => ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zero-ended literal as C sees it.
    fn c(text: &'static [u8]) -> *const c_char {
        assert_eq!(text.last(), Some(&0), "the test's string needs its zero");
        text.as_ptr().cast()
    }

    /// What is `offset` bytes into `start`, for comparing found places.
    fn at(start: *const c_char, offset: usize) -> *mut c_char {
        unsafe { start.add(offset).cast_mut() }
    }

    #[test]
    fn strnlen_stops_at_the_zero_or_the_limit() {
        unsafe {
            assert_eq!(strnlen(c(b"hello\0"), 10), 5);
            assert_eq!(strnlen(c(b"hello\0"), 3), 3);
            assert_eq!(strnlen(c(b"\0"), 10), 0);
            assert_eq!(strnlen(c(b"hello\0"), 0), 0);
        }
    }

    #[test]
    fn strcmp_orders_as_a_dictionary_does() {
        unsafe {
            assert_eq!(strcmp(c(b"apple\0"), c(b"apple\0")), 0);
            assert!(strcmp(c(b"apple\0"), c(b"fig\0")) < 0);
            assert!(strcmp(c(b"fig\0"), c(b"apple\0")) > 0);
            // The start of a string sorts before the whole of it.
            assert!(strcmp(c(b"app\0"), c(b"apple\0")) < 0);
            assert!(strcmp(c(b"apple\0"), c(b"app\0")) > 0);
            assert_eq!(strcmp(c(b"\0"), c(b"\0")), 0);
        }
    }

    #[test]
    fn strcmp_compares_bytes_as_unsigned() {
        // 0xE9 is negative as a signed char; C says it still sorts
        // after every ASCII letter.
        unsafe {
            assert!(strcmp(c(b"\xE9\0"), c(b"z\0")) > 0);
            assert!(strncmp(c(b"\xE9\0"), c(b"z\0"), 1) > 0);
        }
    }

    #[test]
    fn strncmp_looks_no_further_than_it_is_told() {
        unsafe {
            assert_eq!(strncmp(c(b"apple\0"), c(b"apricot\0"), 2), 0);
            assert!(strncmp(c(b"apple\0"), c(b"apricot\0"), 3) < 0);
            assert_eq!(strncmp(c(b"apple\0"), c(b"fig\0"), 0), 0);
            // Past the end of both, nothing more is compared.
            assert_eq!(strncmp(c(b"fig\0"), c(b"fig\0"), 100), 0);
            assert!(strncmp(c(b"fig\0"), c(b"figs\0"), 100) < 0);
        }
    }

    #[test]
    fn strcpy_copies_the_zero_too() {
        let mut to = [b'#' as c_char; 8];
        unsafe {
            assert_eq!(strcpy(to.as_mut_ptr(), c(b"fig\0")), to.as_mut_ptr());
        }
        assert_eq!(to.map(|byte| byte as u8), *b"fig\0####");
    }

    #[test]
    fn strncpy_pads_with_zeros_and_can_leave_none() {
        let mut to = [b'#' as c_char; 8];
        unsafe { strncpy(to.as_mut_ptr(), c(b"fig\0"), 6) };
        assert_eq!(to.map(|byte| byte as u8), *b"fig\0\0\0##");
        // Too long for the count: cut off, and no zero written.
        let mut to = [b'#' as c_char; 8];
        unsafe { strncpy(to.as_mut_ptr(), c(b"apricot\0"), 4) };
        assert_eq!(to.map(|byte| byte as u8), *b"apri####");
    }

    #[test]
    fn strcat_and_strncat_add_to_the_end() {
        let mut to = [b'#' as c_char; 12];
        unsafe {
            strcpy(to.as_mut_ptr(), c(b"ab\0"));
            strcat(to.as_mut_ptr(), c(b"cd\0"));
            assert_eq!(bytes(to.as_ptr()), b"abcd");
            strncat(to.as_mut_ptr(), c(b"efgh\0"), 2);
            assert_eq!(bytes(to.as_ptr()), b"abcdef");
            strncat(to.as_mut_ptr(), c(b"g\0"), 5);
            assert_eq!(bytes(to.as_ptr()), b"abcdefg");
            strcat(to.as_mut_ptr(), c(b"\0"));
            assert_eq!(bytes(to.as_ptr()), b"abcdefg");
        }
        // Nothing was written past the last zero.
        assert_eq!(to[8..].iter().map(|&byte| byte as u8).collect::<Vec<_>>(), b"####");
    }

    #[test]
    fn strchr_finds_the_first_and_strrchr_the_last() {
        let s = c(b"a/b/c\0");
        unsafe {
            assert_eq!(strchr(s, b'/' as c_int), at(s, 1));
            assert_eq!(strrchr(s, b'/' as c_int), at(s, 3));
            assert_eq!(strchr(s, b'a' as c_int), at(s, 0));
            assert!(strchr(s, b'x' as c_int).is_null());
            assert!(strrchr(s, b'x' as c_int).is_null());
            // The zero is part of the string.
            assert_eq!(strchr(s, 0), at(s, 5));
            assert_eq!(strrchr(s, 0), at(s, 5));
        }
    }

    #[test]
    fn strspn_and_strcspn_count_a_run_at_the_start() {
        unsafe {
            assert_eq!(strspn(c(b"  \t word\0"), c(b" \t\0")), 4);
            assert_eq!(strspn(c(b"word\0"), c(b" \t\0")), 0);
            assert_eq!(strspn(c(b"aaa\0"), c(b"a\0")), 3);
            assert_eq!(strspn(c(b"abc\0"), c(b"\0")), 0);
            assert_eq!(strspn(c(b"\0"), c(b"abc\0")), 0);
            assert_eq!(strcspn(c(b"a line\n\0"), c(b"\n\0")), 6);
            assert_eq!(strcspn(c(b"no end\0"), c(b"\n\0")), 6);
            assert_eq!(strcspn(c(b"key=value\0"), c(b":=\0")), 3);
            assert_eq!(strcspn(c(b"\n\0"), c(b"\n\0")), 0);
            assert_eq!(strcspn(c(b"abc\0"), c(b"\0")), 3);
            assert_eq!(strcspn(c(b"\0"), c(b"abc\0")), 0);
        }
    }

    #[test]
    fn strstr_finds_a_string_inside_another() {
        let s = c(b"one two three\0");
        unsafe {
            assert_eq!(strstr(s, c(b"two\0")), at(s, 4));
            assert_eq!(strstr(s, c(b"one\0")), at(s, 0));
            assert_eq!(strstr(s, c(b"three\0")), at(s, 8));
            assert_eq!(strstr(s, c(b"t\0")), at(s, 4));
            assert!(strstr(s, c(b"four\0")).is_null());
            assert!(strstr(s, c(b"three!\0")).is_null());
            assert_eq!(strstr(s, c(b"\0")), at(s, 0));
            assert!(strstr(c(b"\0"), c(b"a\0")).is_null());
        }
    }

    #[test]
    fn memchr_does_not_stop_at_a_zero() {
        let memory = b"ab\0cd";
        let start: *const c_void = memory.as_ptr().cast();
        unsafe {
            assert_eq!(memchr(start, b'c' as c_int, 5), start.add(3).cast_mut());
            assert_eq!(memchr(start, 0, 5), start.add(2).cast_mut());
            assert!(memchr(start, b'c' as c_int, 3).is_null());
            assert!(memchr(start, b'x' as c_int, 5).is_null());
            assert!(memchr(ptr::null(), b'x' as c_int, 0).is_null());
        }
    }
}
