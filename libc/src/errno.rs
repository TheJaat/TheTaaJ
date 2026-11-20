//! errno: how a C function says why it failed.
//!
//! A C function has one return value, and for most it is already
//! spoken for: `read` returns how many bytes, `open` a descriptor. So
//! failure is a value that could not be an answer (-1), and the reason
//! is left in a variable everybody agrees on, `errno`, for the caller
//! to look at afterwards. Nothing sets it back to 0 on success: it
//! means something only straight after a call that said it failed.
//!
//! The numbers are the ones Linux uses - they have to be some numbers,
//! and a program that prints one is then understood by anyone.
//!
//! The messages and the numbers ask the system for nothing, so this
//! file is also compiled on its own and tested on the host
//! (`make libc-test`), like string.rs.

#![cfg_attr(test, allow(dead_code))]

use core::ffi::{c_char, c_int};
use core::sync::atomic::AtomicI32;

/// The variable itself. An atomic integer is, in memory, exactly a C
/// `int`, so C reads and writes it as one - and Rust can change it
/// without `unsafe`, which a plain `static mut` would need.
#[cfg_attr(not(test), no_mangle)]
#[allow(non_upper_case_globals)]
pub static errno: AtomicI32 = AtomicI32::new(0);

pub const ENOENT: c_int = 2;
pub const EIO: c_int = 5;
pub const EBADF: c_int = 9;
pub const ENOMEM: c_int = 12;
pub const EACCES: c_int = 13;
pub const EFAULT: c_int = 14;
pub const EEXIST: c_int = 17;
pub const ENOTDIR: c_int = 20;
pub const EISDIR: c_int = 21;
pub const EINVAL: c_int = 22;
pub const EMFILE: c_int = 24;
pub const ENOTTY: c_int = 25;
pub const EFBIG: c_int = 27;
pub const ENOSPC: c_int = 28;
pub const ESPIPE: c_int = 29;
pub const EROFS: c_int = 30;
pub const ENOTEMPTY: c_int = 39;
pub const ETIMEDOUT: c_int = 110;

/// Every number this library sets, with what it means - each message
/// ending in the zero byte that makes it a C string. Where the system
/// already has words for the thing (the file service's, which `cat`
/// prints), these are the same words.
pub const MESSAGES: &[(c_int, &str)] = &[
    (0, "no error\0"),
    (ENOENT, "no such file or directory\0"),
    (EIO, "the file service or the disk failed\0"),
    (EBADF, "not an open descriptor, or not open for that\0"),
    (ENOMEM, "out of memory\0"),
    (EACCES, "this program was not given the files\0"),
    (EFAULT, "a null pointer where an address was needed\0"),
    (EEXIST, "there is already something with that name\0"),
    (ENOTDIR, "not a directory\0"),
    (EISDIR, "is a directory\0"),
    (EINVAL, "not a valid argument\0"),
    (EMFILE, "too many open descriptors\0"),
    (ENOTTY, "not the screen\0"),
    (EFBIG, "too big for a file\0"),
    (ENOSPC, "the file system is full\0"),
    (ESPIPE, "a stream has no position to move\0"),
    (EROFS, "the files are read-only here\0"),
    (ENOTEMPTY, "the directory is not empty\0"),
    (ETIMEDOUT, "the file service is not answering\0"),
];

/// What `code` means, as text ending in a zero byte.
pub fn message(code: c_int) -> &'static str {
    MESSAGES.iter().find(|(number, _)| *number == code).map_or("unknown error\0", |(_, text)| text)
}

/// `strerror(code)`: what an errno value means, as a string. The
/// string is the library's: not to be changed or freed.
#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn strerror(code: c_int) -> *mut c_char {
    message(code).as_ptr().cast::<c_char>().cast_mut()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn every_message_is_a_c_string_with_one_zero_at_its_end() {
        for (code, text) in MESSAGES {
            assert_eq!(text.bytes().filter(|&byte| byte == 0).count(), 1, "errno {code}");
            assert!(text.ends_with('\0'), "errno {code}");
            assert!(text.len() > 1, "errno {code} says nothing");
        }
        assert!(message(9999).ends_with('\0'));
    }

    #[test]
    fn no_number_is_listed_twice_and_no_two_say_the_same() {
        for (at, (code, text)) in MESSAGES.iter().enumerate() {
            for (other_code, other_text) in &MESSAGES[at + 1..] {
                assert_ne!(code, other_code, "listed twice");
                assert_ne!(text, other_text, "errno {code} and {other_code} cannot be told apart");
            }
        }
    }

    #[test]
    fn strerror_gives_the_message_as_c_reads_it() {
        let read = |code| unsafe { CStr::from_ptr(strerror(code)) }.to_str().unwrap();
        assert_eq!(read(ENOENT), "no such file or directory");
        assert_eq!(read(EROFS), "the files are read-only here");
        assert_eq!(read(0), "no error");
        assert_eq!(read(-1), "unknown error");
        assert_eq!(read(9999), "unknown error");
    }

    #[test]
    fn the_numbers_are_the_ones_linux_uses() {
        // Spot checks against the values every errno.h on Linux has.
        assert_eq!((ENOENT, EBADF, EACCES, EEXIST, EINVAL, ENOSPC, EROFS), (2, 9, 13, 17, 22, 28, 30));
    }
}
