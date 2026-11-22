//! The C library for TheTaaJ - its beginning.
//!
//! A C program expects a world: `main` is called with its arguments,
//! `write` puts bytes somewhere, `exit` ends it. None of that is C; it
//! is a library every C program is linked with, written for one
//! operating system. This is that library for this one.
//!
//! It is written in Rust, and that is less odd than it sounds. What a
//! C library *is*, to the linker, is a file of machine code with
//! functions in it under agreed names, following the C calling
//! convention. `#[no_mangle] pub extern "C" fn write(...)` makes exactly
//! such a function. The C compiler never knows what language it was.
//!
//! The reason to do it this way: everything a C program needs done -
//! finding its arguments, writing to standard output whether that is a
//! screen, a file or a pipe, and later asking the registry for the file
//! service and speaking its protocol - the `taaj` runtime already does,
//! for the Rust programs. A libc in C would be all of that a second
//! time. Here it is the same code, with C names on it.
//!
//! So far:
//!   _start   where the kernel starts the program: gets the arguments,
//!            runs the constructors, calls main, exits with what main
//!            returned
//!   exit, _exit
//!   open, close, read, write, lseek, unlink,
//!   rename, isatty                                 (descriptors.rs)
//!   socket, connect, bind, listen, accept, send, recv,
//!   shutdown, setsockopt, getsockname              (sockets.rs)
//!   dup, dup2, fsync, fcntl                        (descriptors.rs)
//!   stat, fstat, mkdir, rmdir, access, chdir,
//!   opendir, readdir, truncate and the rest of
//!   what a file *is*                               (status.rs)
//!   nanosleep, getpid, sysconf, getrlimit, and
//!   fork, exec and wait, which say they cannot     (system.rs)
//!   errno, strerror, strerror_r                    (errno.rs)
//!   malloc, free, calloc, realloc, strdup          (memory.rs)
//!   strcmp, strcpy, strchr, strstr and the others  (string.rs)
//!   memcpy, memmove, memset, memcmp, strlen: these five come with
//!            Rust's own support library, which is inside libc.a
//!
//! One part of the library is written in C, in `c/`: printf and its
//! relations, which are built around the one thing C does that stable
//! Rust will not define - a function taking any number of arguments -
//! and with them the rest of stdio: streams, their buffers, fopen and
//! fgets. And arpa/inet.h's arithmetic on addresses (inet.c). The Makefile compiles those files and adds them to this
//! archive. The two halves call each other: C's fopen calls `open`
//! here, and `exit` here calls C to write out what is still waiting in
//! the streams' buffers.
//!
//! Since C++ came (journal entry 072) the C half also has the rest of
//! what libc++ asks of a C library: math.c, scan.c (sscanf), locale.c
//! (only the "C" locale), and atexit.c, which runs the constructors
//! before `main` and, from `exit`, the atexit handlers and destructors.
//!
//! And since LLVM (journal entry 073), what LLVM's Support library asks
//! of a POSIX system: pthread.c (a process has one thread, so its locks
//! only count and catch a thread waiting on itself), emutls.c (C++'s
//! `thread_local` without a thread pointer), signal.c (signals a process
//! sends itself), mmap.c (a file "mapped" by reading it into memory) and
//! users.c (there are no users: everyone is 0).
//!
//! And since LLVM's code generators (journal entry 074), the rest of
//! math.h - sin, exp, log, pow, fma and their kind - as musl writes them
//! (c/libm), and `uname` here, for the name of the system.
//!
//! The headers a C program includes for these are in `include/`.
//!
//! Built as a static library (libc.a). The C program's object file and
//! this are joined by the linker with the same linker script the Rust
//! programs use (user/user.ld), into the same kind of ELF file.

#![no_std]

extern crate alloc;

pub mod descriptors;
pub mod errno;
pub mod memory;
pub mod number;
pub mod sockets;
pub mod status;
pub mod string;
pub mod system;

use alloc::vec::Vec;
use core::ffi::{c_char, c_int};

extern "C" {
    /// The C program's own `main`. Declared here, defined there: the
    /// linker joins the two.
    fn main(argc: c_int, argv: *mut *mut c_char) -> c_int;

    /// Writes out what every stream still holds (libc/c/stdio.c).
    fn __taaj_flush_streams();

    /// Calls the constructors: C++'s objects outside functions, and C
    /// functions marked `constructor` (libc/c/atexit.c).
    fn __taaj_run_constructors();

    /// Calls what atexit and __cxa_atexit registered, newest first -
    /// C++'s destructors among them - then the `destructor` functions
    /// (libc/c/atexit.c).
    fn __taaj_run_destructors();
}

/// What a C program is told its own name is - `argv[0]`. The shell
/// does not tell a program its name yet, so this stands in.
const NAME: &[u8] = b"program";

/// Where the kernel starts the program - what C calls crt0, "C runtime,
/// part zero": the code that runs before `main` and makes the world
/// `main` expects.
///
/// The kernel enters with the number of startup handles and the
/// parent's flags as the two arguments (see taaj::program!). C's `main`
/// wants something else: a count, and an array of pointers to strings
/// that each end in a zero byte, with a null pointer after the last.
#[no_mangle]
pub extern "C" fn _start(startup_handles: u64, argument: u64) -> ! {
    taaj::set_startup(startup_handles, argument);
    // The strings: the name, then the arguments as the shell divided
    // them - each given the zero byte that ends a C string. (None can
    // have one inside: they travelled here divided by zero bytes.)
    let mut strings: Vec<Vec<u8>> = Vec::new();
    strings.push(NAME.to_vec());
    strings.extend(taaj::arguments().into_iter().map(|argument| argument.into_bytes()));
    for string in &mut strings {
        string.push(0);
    }
    // And the array: a pointer to each, then null.
    let mut argv: Vec<*mut c_char> = strings.iter_mut().map(|string| string.as_mut_ptr().cast()).collect();
    argv.push(core::ptr::null_mut());
    // SAFETY: a C function taking and returning nothing; what it calls
    // is the program's own code, compiled to be called exactly here.
    unsafe { __taaj_run_constructors() };
    // SAFETY: `main` is the C program's, with the signature C gives it.
    // `argv` holds `strings.len()` pointers to zero-ended strings and a
    // null, all of which live until the process ends: `exit` below
    // never returns, so neither vector is ever dropped.
    let code = unsafe { main(strings.len() as c_int, argv.as_mut_ptr()) };
    exit(code)
}

/// `exit(code)`: ends the program. Returning from `main` does the same.
///
/// First what was registered to run at the end (atexit, and C++'s
/// destructors), which may still print; then it writes out what the
/// streams still hold: text a program printed may be waiting in a
/// buffer (libc/c/stdio.c), and a program that ends by returning from
/// `main` has no other moment to send it.
#[no_mangle]
pub extern "C" fn exit(code: c_int) -> ! {
    // SAFETY: C functions taking and returning nothing. The first calls
    // the program's own handlers; the second touches only the library's
    // streams.
    unsafe {
        __taaj_run_destructors();
        __taaj_flush_streams();
    }
    taaj::exit(i64::from(code))
}

/// `_exit(code)`: ends the program at once - nothing is flushed, so
/// text still in a buffer is lost. For when the program's own state
/// can no longer be trusted.
#[no_mangle]
pub extern "C" fn _exit(code: c_int) -> ! {
    taaj::exit(i64::from(code))
}

/// `_Exit(code)`: C99's name for `_exit`.
#[no_mangle]
pub extern "C" fn _Exit(code: c_int) -> ! {
    taaj::exit(i64::from(code))
}
