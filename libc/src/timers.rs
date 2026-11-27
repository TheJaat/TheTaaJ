//! Timers: `clock_nanosleep`, `setitimer`/`getitimer` and the POSIX
//! `timer_*` calls (MaK-WSL/sched's K24, journal entry NNN), over the
//! kernel's SLEEP_UNTIL and ALARM - both exact to the nanosecond the
//! one-shot timer gives, not to the 10 ms tick.
//!
//! A process has one alarm in the kernel, and every timer here is it:
//! `alarm`, `setitimer(ITIMER_REAL)` and the one timer `timer_create`
//! makes all set the same SIGALRM. (Linux keeps them apart; a program
//! that uses two of them at once would see one replace the other.) Only
//! ITIMER_REAL exists - the CPU-time timers ITIMER_VIRTUAL and ITIMER_PROF
//! are EINVAL - and a timer signals with SIGALRM or not at all.

use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::descriptors::fail;
use crate::errno::{EAGAIN, EINTR, EINVAL};
use crate::system::{timespec, Timespec, Timeval};

const CLOCK_REALTIME: c_int = 0;
const CLOCK_MONOTONIC: c_int = 1;
const TIMER_ABSTIME: c_int = 1;
const ITIMER_REAL: c_int = 0;
const SIGEV_SIGNAL: c_int = 0;
const SIGALRM: c_int = 14;
const NANOS: u64 = 1_000_000_000;

/// The interval of the alarm set last, which the kernel does not say
/// back (it says what is left): for getitimer and timer_gettime.
static INTERVAL: AtomicU64 = AtomicU64::new(0);
/// Whether timer_create has made the one timer there can be.
static TIMER: AtomicBool = AtomicBool::new(false);
/// The one timer's clock (for TIMER_ABSTIME).
static TIMER_CLOCK: AtomicU64 = AtomicU64::new(0);

/// `alarm` set an alarm that rings once.
pub(crate) fn forget_interval() {
    INTERVAL.store(0, Ordering::Relaxed);
}

/// A timespec as nanoseconds; None if it is negative or its nanoseconds
/// are not below a second.
fn nanoseconds(t: &Timespec) -> Option<u64> {
    if t.tv_sec < 0 || !(0..1_000_000_000).contains(&t.tv_nsec) {
        return None;
    }
    Some((t.tv_sec as u64).saturating_mul(NANOS).saturating_add(t.tv_nsec as u64))
}

/// A time on `clock`, made a time on the monotonic clock - the one the
/// kernel sleeps by. None for a clock that cannot be slept by.
fn monotonic_of(clock: c_int, at: u64) -> Option<u64> {
    match clock {
        CLOCK_MONOTONIC => Some(at),
        // The time of day is the monotonic time plus a fixed offset (the
        // real-time clock at boot), so a time of day converts exactly.
        CLOCK_REALTIME => Some(at.saturating_sub(taaj::realtime_ns().saturating_sub(taaj::monotonic_ns()))),
        _ => None,
    }
}

/// `clock_nanosleep(clock, flags, wanted, left)`: nanosleep on `clock`,
/// until an absolute time with TIMER_ABSTIME. An error number, not
/// errno: EINTR (with the time not slept in `*left`, when relative) if
/// a signal handler ran; EINVAL for a bad time or a clock other than
/// CLOCK_MONOTONIC and CLOCK_REALTIME.
///
/// # Safety
/// `wanted` must be valid for reading, `left` null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn clock_nanosleep(clock: c_int, flags: c_int, wanted: *const Timespec, left: *mut Timespec) -> c_int {
    // SAFETY: the caller's promise.
    let Some(length) = nanoseconds(unsafe { &*wanted }) else { return EINVAL };
    let absolute = flags & TIMER_ABSTIME != 0;
    let end = if absolute { monotonic_of(clock, length) } else { monotonic_of(clock, 0).map(|_| taaj::monotonic_ns().saturating_add(length)) };
    let Some(end) = end else { return EINVAL };
    if taaj::sleep_until_ns(end) {
        return 0;
    }
    if !absolute && !left.is_null() {
        // SAFETY: the caller's promise, and it is not null.
        unsafe { left.write(timespec(end.saturating_sub(taaj::monotonic_ns()))) };
    }
    EINTR
}

/// `struct itimerval`, as sys/time.h declares it.
#[repr(C)]
pub struct Itimerval {
    it_interval: Timeval,
    it_value: Timeval,
}

fn from_timeval(t: &Timeval) -> Option<u64> {
    if t.tv_sec < 0 || !(0..1_000_000).contains(&t.tv_usec) {
        return None;
    }
    Some((t.tv_sec as u64).saturating_mul(NANOS).saturating_add(t.tv_usec as u64 * 1000))
}

fn timeval(nanoseconds: u64) -> Timeval {
    // Rounded up, as setitimer's old value is: a time left is never 0
    // while the alarm is still to ring.
    let micros = nanoseconds.div_ceil(1000);
    Timeval { tv_sec: (micros / 1_000_000) as c_long, tv_usec: (micros % 1_000_000) as c_long }
}

/// Sets the alarm to ring in `value` and then every `interval` (0: once;
/// `value` 0: none); what the one before had left, and its interval.
fn set_alarm(value: u64, interval: u64) -> Option<(u64, u64)> {
    let left = taaj::alarm_every(value, if value == 0 { 0 } else { interval }).ok()?;
    let before = INTERVAL.swap(if value == 0 { 0 } else { interval }, Ordering::Relaxed);
    Some((left, if left == 0 { 0 } else { before }))
}

/// `setitimer(ITIMER_REAL, new, old)`: SIGALRM after `it_value`, then
/// every `it_interval`; a zero `it_value` stops it. -1 with EINVAL for
/// another timer or a bad time.
///
/// # Safety
/// `new` must be valid for reading, `old` null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn setitimer(which: c_int, new: *const Itimerval, old: *mut Itimerval) -> c_int {
    if which != ITIMER_REAL || new.is_null() {
        return fail(EINVAL);
    }
    // SAFETY: the caller's promise.
    let new = unsafe { &*new };
    let (Some(value), Some(interval)) = (from_timeval(&new.it_value), from_timeval(&new.it_interval)) else {
        return fail(EINVAL);
    };
    let Some((left, before)) = set_alarm(value, interval) else { return fail(EINVAL) };
    if !old.is_null() {
        // SAFETY: the caller's promise, and it is not null.
        unsafe { old.write(Itimerval { it_interval: timeval(before), it_value: timeval(left) }) };
    }
    0
}

/// `getitimer(ITIMER_REAL, now)`: what is left, and the interval.
///
/// # Safety
/// `now` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn getitimer(which: c_int, now: *mut Itimerval) -> c_int {
    if which != ITIMER_REAL || now.is_null() {
        return fail(EINVAL);
    }
    let left = taaj::alarm_left();
    let interval = if left == 0 { 0 } else { INTERVAL.load(Ordering::Relaxed) };
    // SAFETY: the caller's promise, and it is not null.
    unsafe { now.write(Itimerval { it_interval: timeval(interval), it_value: timeval(left) }) };
    0
}

/// `struct sigevent`'s first fields, as signal.h declares it.
#[repr(C)]
pub struct Sigevent {
    sigev_value: usize,
    sigev_signo: c_int,
    sigev_notify: c_int,
}

/// `struct itimerspec`, as time.h declares it.
#[repr(C)]
pub struct Itimerspec {
    it_interval: Timespec,
    it_value: Timespec,
}

/// The one timer's id: not null, and nothing else is one.
const THE_TIMER: *mut c_void = 1 as *mut c_void;

/// `timer_create(clock, event, id)`: the one timer this process may have,
/// on CLOCK_MONOTONIC or CLOCK_REALTIME, signalling SIGALRM (`event` null,
/// or SIGEV_SIGNAL with SIGALRM). -1 with EAGAIN for a second timer,
/// EINVAL for anything else.
///
/// # Safety
/// `event` null or valid for reading; `id` valid for writing.
#[no_mangle]
pub unsafe extern "C" fn timer_create(clock: c_int, event: *const Sigevent, id: *mut *mut c_void) -> c_int {
    if clock != CLOCK_MONOTONIC && clock != CLOCK_REALTIME || id.is_null() {
        return fail(EINVAL);
    }
    if !event.is_null() {
        // SAFETY: the caller's promise.
        let event = unsafe { &*event };
        if event.sigev_notify != SIGEV_SIGNAL || event.sigev_signo != SIGALRM {
            return fail(EINVAL);
        }
    }
    if TIMER.swap(true, Ordering::AcqRel) {
        return fail(EAGAIN);
    }
    TIMER_CLOCK.store(clock as u64, Ordering::Relaxed);
    // SAFETY: the caller's promise.
    unsafe { id.write(THE_TIMER) };
    0
}

fn is_the_timer(id: *mut c_void) -> bool {
    id == THE_TIMER && TIMER.load(Ordering::Acquire)
}

/// `timer_settime(id, flags, new, old)`: as setitimer, in nanoseconds; with
/// TIMER_ABSTIME `it_value` is a time on the timer's clock.
///
/// # Safety
/// `new` valid for reading; `old` null or valid for writing.
#[no_mangle]
pub unsafe extern "C" fn timer_settime(id: *mut c_void, flags: c_int, new: *const Itimerspec, old: *mut Itimerspec) -> c_int {
    if !is_the_timer(id) || new.is_null() {
        return fail(EINVAL);
    }
    // SAFETY: the caller's promise.
    let new = unsafe { &*new };
    let (Some(mut value), Some(interval)) = (nanoseconds(&new.it_value), nanoseconds(&new.it_interval)) else {
        return fail(EINVAL);
    };
    if value != 0 && flags & TIMER_ABSTIME != 0 {
        let clock = TIMER_CLOCK.load(Ordering::Relaxed) as c_int;
        let Some(at) = monotonic_of(clock, value) else { return fail(EINVAL) };
        // A time already past rings at once (the kernel's least is 1 ns).
        value = at.saturating_sub(taaj::monotonic_ns()).max(1);
    }
    let Some((left, before)) = set_alarm(value, interval) else { return fail(EINVAL) };
    if !old.is_null() {
        // SAFETY: the caller's promise, and it is not null.
        unsafe { old.write(Itimerspec { it_interval: timespec(before), it_value: timespec(left) }) };
    }
    0
}

/// `timer_gettime(id, now)`.
///
/// # Safety
/// `now` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn timer_gettime(id: *mut c_void, now: *mut Itimerspec) -> c_int {
    if !is_the_timer(id) || now.is_null() {
        return fail(EINVAL);
    }
    let left = taaj::alarm_left();
    let interval = if left == 0 { 0 } else { INTERVAL.load(Ordering::Relaxed) };
    // SAFETY: the caller's promise, and it is not null.
    unsafe { now.write(Itimerspec { it_interval: timespec(interval), it_value: timespec(left) }) };
    0
}

/// `timer_getoverrun(id)`: 0. Intervals missed while a SIGALRM was still
/// pending ring once, and are not counted.
#[no_mangle]
pub extern "C" fn timer_getoverrun(id: *mut c_void) -> c_int {
    if is_the_timer(id) { 0 } else { fail(EINVAL) }
}

/// `timer_delete(id)`: stops the timer, and another may be made.
#[no_mangle]
pub extern "C" fn timer_delete(id: *mut c_void) -> c_int {
    if !is_the_timer(id) {
        return fail(EINVAL);
    }
    let _ = set_alarm(0, 0);
    TIMER.store(false, Ordering::Release);
    0
}
