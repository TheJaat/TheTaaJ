//! Numbers from text, the time, and the parts of a Unix process that
//! TheTaaJ does not have.
//!
//!   __taaj_parse_double, __taaj_parse_float
//!            the work of strtod and strtof (libc/c/stdlib.c)
//!   time, gettimeofday
//!   execvp
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

/// Seconds and microseconds since 1 January 1970, UTC: the kernel's
/// clock, which the machine's real-time clock set at boot (on a machine
/// without one, 1970 is when the system started).
fn since_epoch() -> (u64, u64) {
    let nanoseconds = taaj::realtime_ns();
    (nanoseconds / 1_000_000_000, nanoseconds % 1_000_000_000 / 1000)
}

/// `time(t)`: seconds since the epoch (see `since_epoch`), also stored in
/// `*t` if `t` is not null.
///
/// # Safety
/// `t` must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn time(t: *mut c_long) -> c_long {
    let seconds = since_epoch().0 as c_long;
    if !t.is_null() {
        // SAFETY: the caller's promise.
        unsafe { t.write(seconds) };
    }
    seconds
}

/// `gettimeofday(tv, tz)`: the time of day, as seconds and
/// microseconds since 1970. The time zone, an argument that POSIX itself says to
/// pass as null, is ignored.
///
/// # Safety
/// `tv` must be null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn gettimeofday(tv: *mut Timeval, _tz: *mut u8) -> c_int {
    let (seconds, microseconds) = since_epoch();
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

/// The two clocks time.h names: the time of day, and the time since
/// boot.
const CLOCK_REALTIME: c_int = 0;
const CLOCK_MONOTONIC: c_int = 1;
/// timespec_get's one time base.
const TIME_UTC: c_int = 1;

/// Nanoseconds as a timespec.
fn timespec(nanoseconds: u64) -> Timespec {
    Timespec { tv_sec: (nanoseconds / 1_000_000_000) as c_long, tv_nsec: (nanoseconds % 1_000_000_000) as c_long }
}

/// `clock_gettime(clock, t)`: the time on CLOCK_REALTIME (since 1970)
/// or CLOCK_MONOTONIC (since boot), to the nanosecond. 0, or -1 with
/// errno EINVAL for any other clock.
///
/// # Safety
/// `t` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn clock_gettime(clock: c_int, t: *mut Timespec) -> c_int {
    let nanoseconds = match clock {
        CLOCK_REALTIME => taaj::realtime_ns(),
        CLOCK_MONOTONIC => taaj::monotonic_ns(),
        _ => {
            crate::errno::errno.store(crate::errno::EINVAL, core::sync::atomic::Ordering::Relaxed);
            return -1;
        }
    };
    // SAFETY: the caller's promise.
    unsafe { t.write(timespec(nanoseconds)) };
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
    unsafe { t.write(timespec(taaj::realtime_ns())) };
    base
}

/// `clock()`: the processor time used, in microseconds
/// (CLOCKS_PER_SEC). TheTaaJ does not tell a program its own share of
/// the CPU yet, so this is the time since boot - which, measured twice
/// and subtracted, is at least an upper bound on the time used between.
#[no_mangle]
pub extern "C" fn clock() -> c_long {
    (taaj::monotonic_ns() / 1000) as c_long
}

/// `execvp(file, argv)`: replacing the running program with another is
/// not something TheTaaJ does - a program asks for a new process
/// instead - so this always fails, with ENOSYS.
#[no_mangle]
pub extern "C" fn execvp(_file: *const c_char, _argv: *const *const c_char) -> c_int {
    crate::descriptors::fail(ENOSYS)
}

/// `nanosleep(wanted, left)`: sleeps for at least `*wanted` - unless a
/// signal handler runs meanwhile: then -1 with EINTR, and the time not
/// slept in `*left`. 0, or -1 with EINVAL if the time is negative or its
/// nanoseconds are not below a second.
///
/// The kernel sleeps in ticks, and "n ticks" means until its tick count
/// is n more than it is now - which, called a moment before a tick, is
/// barely n - 1 ticks of time. So the sleep is worked out from the
/// counter instead: the end is now plus `wanted`; the tick to wake at
/// is the first one at or after it (tick k is when k ticks' time has
/// passed since boot, clock.rs); and if the program wakes before the
/// end all the same, it sleeps again.
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
    let length = (wanted.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(wanted.tv_nsec as u64);
    let end = taaj::monotonic_ns().saturating_add(length);
    let whole = sleep_until(end);
    if !left.is_null() {
        let rest = end.saturating_sub(taaj::monotonic_ns());
        let rest = if whole { 0 } else { rest };
        // SAFETY: the caller's promise, and it is not null.
        unsafe { left.write(timespec(rest)) };
    }
    if whole { 0 } else { crate::descriptors::fail(crate::errno::EINTR) }
}

/// Sleeps until the time since boot is at least `end` nanoseconds.
/// False if a signal's handler ran first, and the sleep was cut short.
pub(crate) fn sleep_until(end: u64) -> bool {
    let tick = 1_000_000_000 / abi::TICKS_PER_SECOND;
    // The kernel's longest sleep, ten minutes; a longer one is a few.
    let longest = 10 * 60 * abi::TICKS_PER_SECOND;
    while taaj::monotonic_ns() < end {
        let wake_at = end.div_ceil(tick);
        if !taaj::sleep_unless_signalled(wake_at.saturating_sub(taaj::ticks()).clamp(1, longest)) {
            return false;
        }
    }
    true
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

/// `fork()`: copying a running program is not how TheTaaJ starts
/// programs: a new one is started from its file, as a new process
/// (posix_spawn, in spawn.rs). -1, ENOSYS.
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

/// sys/utsname.h's `struct utsname`: five strings of 65 bytes.
#[repr(C)]
pub struct Utsname {
    sysname: [c_char; 65],
    nodename: [c_char; 65],
    release: [c_char; 65],
    version: [c_char; 65],
    machine: [c_char; 65],
}

/// The CPU's name, as `uname -m` says it elsewhere.
#[cfg(target_arch = "aarch64")]
const MACHINE: &[u8] = b"aarch64";
#[cfg(not(target_arch = "aarch64"))]
const MACHINE: &[u8] = b"x86_64";

/// `uname(out)`: what system this is (see sys/utsname.h). LLVM asks it
/// for the version of the system it runs on.
///
/// # Safety
/// `out` must be null or point to a writable `struct utsname`.
#[no_mangle]
pub unsafe extern "C" fn uname(out: *mut Utsname) -> c_int {
    if out.is_null() {
        return crate::descriptors::fail(crate::errno::EFAULT);
    }
    fn field(text: &[u8]) -> [c_char; 65] {
        let mut field = [0; 65];
        for (to, &from) in field.iter_mut().zip(text) {
            *to = from as c_char;
        }
        field
    }
    let name = Utsname {
        sysname: field(b"TheTaaJ"),
        nodename: field(b"taaj"),
        release: field(b"0.6"),
        version: field(b"TheTaaJ"),
        machine: field(MACHINE),
    };
    // SAFETY: the caller's promise: `out` points to a writable utsname.
    unsafe { out.write(name) };
    0
}

/// sys/resource.h's `struct rlimit`.
#[repr(C)]
pub struct Rlimit {
    current: u64,
    maximum: u64,
}

const RLIM_INFINITY: u64 = u64::MAX;
const RLIMIT_DATA: c_int = 2;
const RLIMIT_NOFILE: c_int = 7;
const RLIMIT_AS: c_int = 9;

/// `getrlimit(resource, limit)`: how much of something this program may
/// use. Descriptors: the library's table of 32. Memory (RLIMIT_AS and
/// RLIMIT_DATA, the same thing here): the kernel's memory limit for this
/// process (MEMORY_LIMIT). Nothing else has a limit: the stack is the
/// size the program asked for when it was linked (the kernel reads it
/// from the ELF file). Those are "infinite" here, as Linux says of a
/// limit nobody set.
///
/// # Safety
/// `limit` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn getrlimit(resource: c_int, limit: *mut Rlimit) -> c_int {
    let (current, maximum) = match resource {
        RLIMIT_NOFILE => (32, 32),
        RLIMIT_AS | RLIMIT_DATA => {
            let bytes = taaj::memory_limit(0);
            (bytes, bytes)
        }
        0..=15 => (RLIM_INFINITY, RLIM_INFINITY),
        _ => return crate::descriptors::fail(crate::errno::EINVAL),
    };
    // SAFETY: the caller's promise.
    unsafe { limit.write(Rlimit { current, maximum }) };
    0
}

/// `setrlimit(resource, limit)`: memory's limit can be lowered (and
/// then programs this one starts get it too), never raised - EPERM.
/// The other limits cannot be changed: asking for what they already are
/// succeeds, anything else is EPERM.
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
    if resource == RLIMIT_AS || resource == RLIMIT_DATA {
        if wanted.current > wanted.maximum {
            return crate::descriptors::fail(crate::errno::EINVAL);
        }
        if wanted.current > now.current {
            return crate::descriptors::fail(crate::errno::EPERM);
        }
        // Rounded down to whole pages, at least one.
        taaj::memory_limit(wanted.current.max(4096));
        return 0;
    }
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
