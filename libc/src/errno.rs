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
use core::sync::atomic::Ordering;

/// errno itself. Each thread has its own (threads.rs keeps it in the
/// thread's record), and C reaches it through `__errno_location()`, as
/// errno.h's `#define errno (*__errno_location())` says. Rust code here
/// says `errno.store(code, ...)` and `errno.load(...)`, as when errno
/// was one atomic int: the two methods find the running thread's.
#[allow(non_upper_case_globals)]
pub static errno: Errno = Errno;

pub struct Errno;

#[cfg(not(test))]
extern "C" {
    fn __errno_location() -> *mut c_int;
}

/// Compiled alone, on the host, there are no threads: one errno.
#[cfg(test)]
unsafe fn __errno_location() -> *mut c_int {
    static mut ERRNO: c_int = 0;
    &raw mut ERRNO
}

impl Errno {
    pub fn store(&self, code: c_int, _order: Ordering) {
        // SAFETY: the running thread's errno, which only it touches.
        unsafe { *__errno_location() = code };
    }

    pub fn load(&self, _order: Ordering) -> c_int {
        // SAFETY: as above.
        unsafe { *__errno_location() }
    }
}

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
pub const ERANGE: c_int = 34;
pub const ENOSYS: c_int = 38;
pub const ENOTEMPTY: c_int = 39;
pub const EPIPE: c_int = 32;
pub const ENOTSOCK: c_int = 88;
pub const EDESTADDRREQ: c_int = 89;
pub const ENOPROTOOPT: c_int = 92;
pub const EPROTONOSUPPORT: c_int = 93;
pub const EOPNOTSUPP: c_int = 95;
pub const EAFNOSUPPORT: c_int = 97;
pub const EADDRINUSE: c_int = 98;
pub const ENETUNREACH: c_int = 101;
pub const ECONNRESET: c_int = 104;
pub const ENOBUFS: c_int = 105;
pub const EISCONN: c_int = 106;
pub const ENOTCONN: c_int = 107;
pub const ETIMEDOUT: c_int = 110;
pub const ECONNREFUSED: c_int = 111;
pub const EHOSTUNREACH: c_int = 113;

/// The rest of POSIX's: no part of TheTaaJ sets these yet, but C++'s
/// std::errc names every one, and a program may set or test them.
pub const EPERM: c_int = 1;
pub const ESRCH: c_int = 3;
pub const EINTR: c_int = 4;
pub const ENXIO: c_int = 6;
pub const E2BIG: c_int = 7;
pub const ENOEXEC: c_int = 8;
pub const ECHILD: c_int = 10;
pub const EAGAIN: c_int = 11;
pub const EBUSY: c_int = 16;
pub const EXDEV: c_int = 18;
pub const ENODEV: c_int = 19;
pub const ENFILE: c_int = 23;
pub const ETXTBSY: c_int = 26;
pub const EMLINK: c_int = 31;
pub const EDOM: c_int = 33;
pub const EDEADLK: c_int = 35;
pub const ENAMETOOLONG: c_int = 36;
pub const ENOLCK: c_int = 37;
pub const ELOOP: c_int = 40;
pub const ENOMSG: c_int = 42;
pub const EIDRM: c_int = 43;
pub const ENOSTR: c_int = 60;
pub const ENODATA: c_int = 61;
pub const ETIME: c_int = 62;
pub const ENOSR: c_int = 63;
pub const ENOLINK: c_int = 67;
pub const EPROTO: c_int = 71;
pub const EMULTIHOP: c_int = 72;
pub const EBADMSG: c_int = 74;
pub const EOVERFLOW: c_int = 75;
pub const EILSEQ: c_int = 84;
pub const EMSGSIZE: c_int = 90;
pub const EPROTOTYPE: c_int = 91;
pub const EADDRNOTAVAIL: c_int = 99;
pub const ENETDOWN: c_int = 100;
pub const ENETRESET: c_int = 102;
pub const ECONNABORTED: c_int = 103;
pub const EALREADY: c_int = 114;
pub const EINPROGRESS: c_int = 115;
pub const ESTALE: c_int = 116;
pub const EDQUOT: c_int = 122;
pub const ECANCELED: c_int = 125;
pub const EOWNERDEAD: c_int = 130;
pub const ENOTRECOVERABLE: c_int = 131;

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
    (ERANGE, "a number too big to fit\0"),
    (ENOSYS, "TheTaaJ does not do that\0"),
    (ENOTEMPTY, "the directory is not empty\0"),
    (EPIPE, "the connection is closed: nothing more can be written\0"),
    (ENOTSOCK, "not a socket\0"),
    (EDESTADDRREQ, "the socket is not connected to anything\0"),
    (ENOPROTOOPT, "no such option\0"),
    (EPROTONOSUPPORT, "only TCP is spoken from C so far\0"),
    (EOPNOTSUPP, "a socket cannot do that\0"),
    (EAFNOSUPPORT, "only IPv4 addresses are known here\0"),
    (EADDRINUSE, "another program has that port\0"),
    (ENETUNREACH, "that address is not on this network, and there is no way out of it\0"),
    (ECONNRESET, "the other side cut the connection off\0"),
    (ENOBUFS, "the network service has no room for more\0"),
    (EISCONN, "the socket is connected already\0"),
    (ENOTCONN, "the socket is not connected\0"),
    (ETIMEDOUT, "the service, or the other side, is not answering\0"),
    (ECONNREFUSED, "nothing is listening on that port there\0"),
    (EHOSTUNREACH, "nobody on this network has that address\0"),
    (EPERM, "not permitted\0"),
    (ESRCH, "no such process\0"),
    (EINTR, "interrupted\0"),
    (ENXIO, "no such device or address\0"),
    (E2BIG, "the arguments are too long\0"),
    (ENOEXEC, "not a program that can be run\0"),
    (ECHILD, "no child process\0"),
    (EAGAIN, "not now: try again\0"),
    (EBUSY, "in use\0"),
    (EXDEV, "not on the same file system\0"),
    (ENODEV, "no such device\0"),
    (ENFILE, "too many files open in the system\0"),
    (ETXTBSY, "a program that is running\0"),
    (EMLINK, "too many links\0"),
    (EDOM, "outside the function's domain\0"),
    (EDEADLK, "it would wait for itself forever\0"),
    (ENAMETOOLONG, "the name is too long\0"),
    (ENOLCK, "no lock to be had\0"),
    (ELOOP, "too many links followed\0"),
    (ENOMSG, "no message of that kind\0"),
    (EIDRM, "that identifier was removed\0"),
    (ENOSTR, "not a stream device\0"),
    (ENODATA, "no data\0"),
    (ETIME, "the timer ran out\0"),
    (ENOSR, "no stream resources\0"),
    (ENOLINK, "the link is gone\0"),
    (EPROTO, "the other side broke the protocol\0"),
    (EMULTIHOP, "too many hops\0"),
    (EBADMSG, "a message that makes no sense\0"),
    (EOVERFLOW, "too big for the type it goes in\0"),
    (EILSEQ, "not a valid character\0"),
    (EMSGSIZE, "the message is too long\0"),
    (EPROTOTYPE, "the wrong kind of socket for that protocol\0"),
    (EADDRNOTAVAIL, "that address is not this machine's\0"),
    (ENETDOWN, "the network is down\0"),
    (ENETRESET, "the network dropped the connection\0"),
    (ECONNABORTED, "the connection was given up\0"),
    (EALREADY, "already under way\0"),
    (EINPROGRESS, "under way, not finished\0"),
    (ESTALE, "a file that is no longer there\0"),
    (EDQUOT, "over the disk quota\0"),
    (ECANCELED, "cancelled\0"),
    (EOWNERDEAD, "its owner died holding it\0"),
    (ENOTRECOVERABLE, "broken past repair\0"),
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

/// `strerror_r(code, buffer, size)`: POSIX's strerror into the
/// caller's own buffer of `size` bytes - the form of it meant for
/// programs with threads, which this library's strerror is safe for
/// anyway, since its strings never change. 0; or ERANGE if the message
/// does not fit (as much as fits is still there, ending in a zero).
///
/// # Safety
/// `buffer` must be valid for writing `size` bytes.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn strerror_r(code: c_int, buffer: *mut c_char, size: usize) -> c_int {
    let text = message(code).as_bytes();
    if size == 0 {
        return ERANGE;
    }
    let fits = text.len() <= size; // the zero included
    let n = text.len().min(size);
    // SAFETY: the caller's promise: `size` writable bytes, and n <= size.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr().cast::<c_char>(), buffer, n);
        if !fits {
            buffer.add(size - 1).write(0);
        }
    }
    if fits {
        0
    } else {
        ERANGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn strerror_r_copies_and_says_when_it_does_not_fit() {
        let mut room = [1 as c_char; 64];
        // SAFETY: 64 writable bytes.
        assert_eq!(unsafe { strerror_r(ENOENT, room.as_mut_ptr(), room.len()) }, 0);
        // SAFETY: strerror_r ended it with a zero.
        let text = unsafe { CStr::from_ptr(room.as_ptr()) };
        assert_eq!(text.to_bytes_with_nul(), message(ENOENT).as_bytes());
        let mut small = [1 as c_char; 4];
        // SAFETY: 4 writable bytes.
        assert_eq!(unsafe { strerror_r(ENOENT, small.as_mut_ptr(), small.len()) }, ERANGE);
        assert_eq!(small[3], 0);
        // SAFETY: nothing is written when the size is 0.
        assert_eq!(unsafe { strerror_r(ENOENT, core::ptr::null_mut(), 0) }, ERANGE);
    }

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
        assert_eq!((EPIPE, ENOTSOCK, EADDRINUSE, ECONNRESET, ENOTCONN, ETIMEDOUT, ECONNREFUSED, EHOSTUNREACH), (32, 88, 98, 104, 107, 110, 111, 113));
    }
}
