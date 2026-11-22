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

/// `nanosleep(wanted, left)`: sleeps for at least `*wanted`, in whole
/// ticks of the system's clock (abi::TICKS_PER_SECOND a second),
/// rounded up so that it is never shorter. Nothing can wake the program
/// early - there are no signals - so `*left` is always zero. 0, or -1
/// with EINVAL if the time is negative or its nanoseconds are not
/// below a second.
///
/// # Safety
/// `wanted` must be valid for reading, `left` null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn nanosleep(wanted: *const Timespec, left: *mut Timespec) -> c_int {
    // SAFETY: the caller's promise.
    let wanted = unsafe { wanted.read() };
    if wanted.tv_sec < 0 || !(0..1_000_000_000).contains(&wanted.tv_nsec) {
        return crate::descriptors::fail(crate::errno::EINVAL);
    }
    let tick = 1_000_000_000 / abi::TICKS_PER_SECOND;
    let ticks = (wanted.tv_sec as u64) * abi::TICKS_PER_SECOND + (wanted.tv_nsec as u64).div_ceil(tick);
    if ticks > 0 {
        taaj::sleep(ticks);
    }
    if !left.is_null() {
        // SAFETY: the caller's promise, and it is not null.
        unsafe { left.write(Timespec { tv_sec: 0, tv_nsec: 0 }) };
    }
    0
}

/// `sched_yield()`: lets the kernel run something else now.
#[no_mangle]
pub extern "C" fn sched_yield() -> c_int {
    taaj::yield_now();
    0
}

/// `getpid()`: the kernel's number for this process.
#[no_mangle]
pub extern "C" fn getpid() -> c_int {
    taaj::process_id() as c_int
}

// sysconf's questions, with Linux's numbers (unistd.h says the same).
const SC_ARG_MAX: c_int = 0;
const SC_CLK_TCK: c_int = 2;
const SC_OPEN_MAX: c_int = 4;
const SC_PAGESIZE: c_int = 30;
const SC_NPROCESSORS_CONF: c_int = 83;
const SC_NPROCESSORS_ONLN: c_int = 84;
const SC_GETPW_R_SIZE_MAX: c_int = 70;

/// `sysconf(name)`: a few facts about the system. The page is 4096
/// bytes on both CPUs TheTaaJ runs on. A program has one thread, so as
/// far as it can use them there is one processor. -1 with EINVAL for a
/// question not answered here.
#[no_mangle]
pub extern "C" fn sysconf(name: c_int) -> c_long {
    match name {
        SC_PAGESIZE => 4096,
        // POSIX's least: a C program cannot start another yet anyway.
        SC_ARG_MAX => 4096,
        SC_CLK_TCK => abi::TICKS_PER_SECOND as c_long,
        SC_OPEN_MAX => 32,
        SC_NPROCESSORS_CONF | SC_NPROCESSORS_ONLN => 1,
        // There is no user entry to need room (pwd.h): no limit, which
        // sysconf says by -1 with errno left as it was.
        SC_GETPW_R_SIZE_MAX => -1,
        _ => crate::descriptors::fail(crate::errno::EINVAL),
    }
}

/// `getpagesize()`: sysconf(_SC_PAGESIZE), the old way of asking.
#[no_mangle]
pub extern "C" fn getpagesize() -> c_int {
    4096
}

/// `waitpid(process, status, options)`: a C program here cannot start
/// another (there is no fork, exec or posix_spawn yet), so it has no
/// children to wait for: -1 with ECHILD, as POSIX says.
#[no_mangle]
pub extern "C" fn waitpid(_process: c_int, _status: *mut c_int, _options: c_int) -> c_int {
    crate::descriptors::fail(crate::errno::ECHILD)
}

/// `wait(status)`: waitpid for any child - and there are none.
#[no_mangle]
pub extern "C" fn wait(status: *mut c_int) -> c_int {
    waitpid(-1, status, 0)
}

/// `wait4(process, status, options, usage)`: waitpid that also says how
/// much the child used. There are no children: ECHILD.
#[no_mangle]
pub extern "C" fn wait4(process: c_int, status: *mut c_int, options: c_int, _usage: *mut u8) -> c_int {
    waitpid(process, status, options)
}

/// `fork()`: copying a running program is not how TheTaaJ starts
/// programs (a new one is started from its file, as a new process), and
/// a C program cannot do that yet either: -1, ENOSYS.
#[no_mangle]
pub extern "C" fn fork() -> c_int {
    crate::descriptors::fail(ENOSYS)
}

/// `execv(path, argv)`: as execvp - never replaced: -1, ENOSYS.
#[no_mangle]
pub extern "C" fn execv(_path: *const c_char, _argv: *const *const c_char) -> c_int {
    crate::descriptors::fail(ENOSYS)
}

/// `execve(path, argv, environment)`: the same.
#[no_mangle]
pub extern "C" fn execve(_path: *const c_char, _argv: *const *const c_char, _env: *const *const c_char) -> c_int {
    crate::descriptors::fail(ENOSYS)
}

/// `getsid(process)`: the session a process is in. TheTaaJ has no
/// sessions (or process groups): each process is taken to be its own,
/// so the answer is its number.
#[no_mangle]
pub extern "C" fn getsid(process: c_int) -> c_int {
    if process == 0 {
        getpid()
    } else {
        process
    }
}

/// `gethostname(name, size)`: the machine's name, which is "taaj" -
/// there is no way to name it anything else yet. 0; or -1 with
/// ENAMETOOLONG if it does not fit with its zero.
///
/// # Safety
/// `name` must be valid for writing `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn gethostname(name: *mut c_char, size: usize) -> c_int {
    let host = b"taaj\0";
    if size < host.len() {
        return crate::descriptors::fail(crate::errno::ENAMETOOLONG);
    }
    // SAFETY: the caller's promise: `size` writable bytes, and it fits.
    unsafe { ptr::copy_nonoverlapping(host.as_ptr().cast::<c_char>(), name, host.len()) };
    0
}

/// sys/resource.h's `struct rlimit`.
#[repr(C)]
pub struct Rlimit {
    current: u64,
    maximum: u64,
}

const RLIM_INFINITY: u64 = u64::MAX;
const RLIMIT_NOFILE: c_int = 7;

/// `getrlimit(resource, limit)`: how much of something this program may
/// use. Descriptors: the library's table of 32. Nothing else has a limit
/// that can be raised or lowered: the stack is the size the program
/// asked for when it was linked (the kernel reads it from the ELF
/// file), and memory is what there is. Those are "infinite" here, as
/// Linux says of a limit nobody set.
///
/// # Safety
/// `limit` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn getrlimit(resource: c_int, limit: *mut Rlimit) -> c_int {
    let (current, maximum) = match resource {
        RLIMIT_NOFILE => (32, 32),
        0..=15 => (RLIM_INFINITY, RLIM_INFINITY),
        _ => return crate::descriptors::fail(crate::errno::EINVAL),
    };
    // SAFETY: the caller's promise.
    unsafe { limit.write(Rlimit { current, maximum }) };
    0
}

/// `setrlimit(resource, limit)`: the limits cannot be changed - asking
/// for what they already are succeeds, anything else is EPERM.
///
/// # Safety
/// `limit` must be valid for reading.
#[no_mangle]
pub unsafe extern "C" fn setrlimit(resource: c_int, limit: *const Rlimit) -> c_int {
    let mut now = Rlimit { current: 0, maximum: 0 };
    // SAFETY: `now` is writable.
    if unsafe { getrlimit(resource, &mut now) } != 0 {
        return -1;
    }
    // SAFETY: the caller's promise.
    let wanted = unsafe { limit.read() };
    if wanted.current == now.current && wanted.maximum == now.maximum {
        0
    } else {
        crate::descriptors::fail(crate::errno::EPERM)
    }
}

/// `usleep(microseconds)`: nanosleep, in microseconds.
#[no_mangle]
pub extern "C" fn usleep(microseconds: u32) -> c_int {
    let wanted = Timespec {
        tv_sec: (microseconds / 1_000_000) as c_long,
        tv_nsec: ((microseconds % 1_000_000) * 1000) as c_long,
    };
    // SAFETY: `wanted` is readable, and no time is left to be told.
    unsafe { nanosleep(&wanted, ptr::null_mut()) }
}
