//! poll and select: waiting for any of several descriptors at once -
//! until one can be read without waiting, or written.
//!
//! What "ready" means for each kind of descriptor (descriptors.rs):
//!
//! ```text
//!  standard input, a pipe's read end   bytes held or a message come in
//!                                      (POLLIN), or the end (POLLHUP)
//!  a pipe's write end                  POLLOUT; POLLERR once the reader
//!                                      has closed (a write would EPIPE)
//!  standard output and error, a file   always ready, both ways: a file's
//!                                      read and write go to the file
//!                                      service and come back
//!  a socket                            always ready (a read may still
//!                                      wait: what the network service
//!                                      holds is not known here)
//! ```
//!
//! The kernel knows none of this: it knows channels. So poll asks each
//! descriptor whether it is ready, taking in any message already waiting
//! (pipes.rs, taaj::Input), and if none is, it waits on the channels of
//! those that may become so - OBJECT_WAIT, woken by a message or a closed
//! peer - and asks again. A wake is only a hint: a message that is not
//! part of the stream wakes poll without making anything readable, and
//! poll waits again until the deadline.

use core::ffi::{c_int, c_long, c_short, c_ulong};

use crate::descriptors::{fail, input_ready, open_at, Open};
use crate::errno::{EFAULT, EINTR, EINVAL};

pub const POLLIN: c_short = 0x001;
pub const POLLPRI: c_short = 0x002;
pub const POLLOUT: c_short = 0x004;
pub const POLLERR: c_short = 0x008;
pub const POLLHUP: c_short = 0x010;
pub const POLLNVAL: c_short = 0x020;
pub const POLLRDNORM: c_short = 0x040;
pub const POLLWRNORM: c_short = 0x100;

/// poll.h's struct pollfd.
#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

/// What one descriptor is now: its revents (before masking with what
/// was asked), and a channel whose message or closing might change that.
/// `unknown` when another thread holds it mid-read: nothing to wait on,
/// so look again soon.
struct Now {
    revents: c_short,
    wait_on: Option<taaj::Handle>,
    unknown: bool,
}

const READ: c_short = POLLIN | POLLRDNORM;
const WRITE: c_short = POLLOUT | POLLWRNORM;

fn now(fd: c_int) -> Now {
    let readable = |(bytes, ended): (bool, bool)| {
        (if bytes { READ } else { 0 }) | (if ended { POLLHUP } else { 0 })
    };
    match open_at(fd) {
        None => Now { revents: POLLNVAL, wait_on: None, unknown: false },
        Some(Open::Input) => match input_ready() {
            Some((state, handle)) => Now { revents: readable(state), wait_on: handle, unknown: false },
            None => Now { revents: 0, wait_on: None, unknown: true },
        },
        Some(Open::PipeRead(end)) => match end.ready() {
            Some(state) => Now { revents: readable(state), wait_on: Some(end.channel.0), unknown: false },
            // Being read: its channel still says when something comes.
            None => Now { revents: 0, wait_on: Some(end.channel.0), unknown: true },
        },
        Some(Open::PipeWrite(end)) => {
            // Nothing is sent back on a pipe: a write end that is
            // "ready" to receive has a closed peer.
            let closed = matches!(taaj::poll(&[end.0]), Ok(Some(_)));
            Now { revents: if closed { POLLERR } else { WRITE }, wait_on: None, unknown: false }
        }
        Some(Open::Output | Open::Error) => Now { revents: WRITE, wait_on: None, unknown: false },
        Some(Open::File(_) | Open::Socket(_)) => Now { revents: READ | WRITE, wait_on: None, unknown: false },
    }
}

/// Fills each revents; how many are non-zero. The channels worth waiting
/// on go to `wait_on` (at most WAIT_HANDLES_MAX), and whether any
/// descriptor could not be looked at.
fn scan(fds: &mut [PollFd], wait_on: &mut alloc::vec::Vec<taaj::Handle>) -> (c_int, bool) {
    let mut ready = 0;
    let mut unknown = false;
    wait_on.clear();
    for entry in fds.iter_mut() {
        entry.revents = 0;
        if entry.fd < 0 {
            continue; // POSIX: ignored, revents 0
        }
        let state = now(entry.fd);
        // POLLERR, POLLHUP and POLLNVAL are reported whether asked for
        // or not.
        entry.revents = state.revents & (entry.events | POLLERR | POLLHUP | POLLNVAL);
        if entry.revents != 0 {
            ready += 1;
        } else {
            unknown |= state.unknown;
            if let Some(handle) = state.wait_on {
                if !wait_on.contains(&handle) && wait_on.len() < abi::WAIT_HANDLES_MAX as usize {
                    wait_on.push(handle);
                }
            }
        }
    }
    (ready, unknown)
}

/// When `milliseconds` from now is, on the CPU's counter (nanoseconds
/// since boot); None for no timeout (negative).
fn deadline(milliseconds: c_long) -> Option<u64> {
    u64::try_from(milliseconds).ok().map(|ms| taaj::monotonic_ns().saturating_add(ms.saturating_mul(1_000_000)))
}

/// The tick to wake at for `end`: the first one at or after it (tick k
/// is when k ticks' time has passed since boot - as nanosleep reckons,
/// system.rs), and at least the next.
fn wake_tick(end: u64) -> u64 {
    let tick = 1_000_000_000 / abi::TICKS_PER_SECOND;
    end.div_ceil(tick).max(taaj::ticks() + 1)
}

/// The wait both poll and select make: until something in `fds` is
/// ready (its count), the deadline (0), or a signal (-1, EINTR).
///
/// The timeout is measured on the counter, not in ticks: under an
/// emulator ticks can come late and then in a burst, and a wait of
/// "three ticks" could end after one tick's time. Waking before the
/// end only means looking again.
fn wait(fds: &mut [PollFd], end: Option<u64>) -> c_int {
    let mut wait_on = alloc::vec::Vec::new();
    loop {
        let (ready, unknown) = scan(fds, &mut wait_on);
        if ready > 0 {
            return ready;
        }
        if end.is_some_and(|end| taaj::monotonic_ns() >= end) {
            return 0;
        }
        // A descriptor another thread is reading is looked at again
        // a tick later.
        let now = taaj::ticks();
        let mut until = end.map(wake_tick);
        if unknown {
            until = Some(until.map_or(now + 1, |until| until.min(now + 1)));
        }
        let waited = if wait_on.is_empty() {
            // Nothing that can change: only the time passes.
            let ticks = until.map_or(5 * 60 * abi::TICKS_PER_SECOND, |until| until.saturating_sub(now).max(1));
            if taaj::sleep_unless_signalled(ticks) { Ok(None) } else { Err(taaj::Error::Interrupted) }
        } else {
            taaj::object_wait_or_signal(&wait_on, until)
        };
        match waited {
            Ok(_) => {}
            Err(taaj::Error::Interrupted) => return fail(EINTR),
            // A handle that went away meanwhile: the next scan says so.
            Err(_) => {}
        }
    }
}

/// `poll(fds, count, timeout)`: waits until one of the `count`
/// descriptors in `fds` is ready for what its `events` ask (each
/// `revents` says what it is ready for), for at most `timeout`
/// milliseconds - none if it is negative, a look without waiting if 0.
/// How many are ready; 0 on the timeout; -1 with EINTR if a signal's
/// handler ran, EINVAL for more than 1024 entries.
///
/// # Safety
/// `fds` must point to `count` pollfds.
#[no_mangle]
pub unsafe extern "C" fn poll(fds: *mut PollFd, count: c_ulong, timeout: c_int) -> c_int {
    if count > 1024 {
        return fail(EINVAL);
    }
    if fds.is_null() && count != 0 {
        return fail(EFAULT);
    }
    let fds = if count == 0 {
        &mut [][..]
    } else {
        // SAFETY: the caller's promise.
        unsafe { core::slice::from_raw_parts_mut(fds, count as usize) }
    };
    wait(fds, deadline(c_long::from(timeout)))
}

/// sys/select.h's fd_set: a bit for each of FD_SETSIZE descriptors, as
/// Linux lays it out.
pub const FD_SETSIZE: usize = 1024;
const BITS: usize = c_ulong::BITS as usize;

#[repr(C)]
pub struct FdSet {
    bits: [c_ulong; FD_SETSIZE / BITS],
}

impl FdSet {
    fn has(&self, fd: usize) -> bool {
        self.bits[fd / BITS] & (1 << (fd % BITS)) != 0
    }

    fn put(&mut self, fd: usize) {
        self.bits[fd / BITS] |= 1 << (fd % BITS);
    }
}

#[repr(C)]
pub struct TimeVal {
    tv_sec: c_long,
    tv_usec: c_long,
}

/// `select(count, read, write, except, timeout)`: poll in the older
/// shape. The descriptors below `count` set in `read` are waited for
/// to be readable, those in `write` writable; on return each set has
/// only the ready ones left. `except` (out-of-band data) is never ready
/// here: it is emptied. `timeout` null waits for ever. How many bits
/// are set in all; 0 on the timeout; -1 with EBADF for a descriptor not
/// open, EINTR, or EINVAL for a count outside 0..=FD_SETSIZE.
///
/// # Safety
/// Each set null or valid; `timeout` null or valid.
#[no_mangle]
pub unsafe extern "C" fn select(
    count: c_int,
    read: *mut FdSet,
    write: *mut FdSet,
    except: *mut FdSet,
    timeout: *mut TimeVal,
) -> c_int {
    let Ok(count) = usize::try_from(count) else { return fail(EINVAL) };
    if count > FD_SETSIZE {
        return fail(EINVAL);
    }
    // SAFETY: the caller's promise, for all four.
    let (mut read, mut write, except, timeout) =
        unsafe { (read.as_mut(), write.as_mut(), except.as_mut(), timeout.as_ref()) };
    let milliseconds = match timeout {
        None => -1,
        Some(t) if t.tv_sec < 0 || !(0..1_000_000).contains(&t.tv_usec) => return fail(EINVAL),
        Some(t) => t.tv_sec.saturating_mul(1000).saturating_add((t.tv_usec + 999) / 1000),
    };
    let mut fds = alloc::vec::Vec::new();
    for fd in 0..count {
        let mut events = 0;
        if read.as_ref().is_some_and(|set| set.has(fd)) {
            events |= POLLIN;
        }
        if write.as_ref().is_some_and(|set| set.has(fd)) {
            events |= POLLOUT;
        }
        if events != 0 {
            fds.push(PollFd { fd: fd as c_int, events, revents: 0 });
        }
    }
    // A descriptor not open is an error to select, not an answer.
    if fds.iter().any(|entry| open_at(entry.fd).is_none()) {
        return fail(crate::errno::EBADF);
    }
    let ready = wait(&mut fds, deadline(milliseconds));
    if ready < 0 {
        return ready;
    }
    if let Some(set) = except {
        set.bits = [0; FD_SETSIZE / BITS];
    }
    for set in [read.as_deref_mut(), write.as_deref_mut()].into_iter().flatten() {
        set.bits = [0; FD_SETSIZE / BITS];
    }
    let mut bits = 0;
    for entry in &fds {
        let fd = entry.fd as usize;
        // A hang-up or an error counts as readable (a read says what
        // happened) and as writable (a write does).
        let readable = entry.revents & (POLLIN | POLLHUP | POLLERR) != 0 && entry.events & POLLIN != 0;
        let writable = entry.revents & (POLLOUT | POLLERR) != 0 && entry.events & POLLOUT != 0;
        if readable {
            if let Some(set) = read.as_deref_mut() {
                set.put(fd);
                bits += 1;
            }
        }
        if writable {
            if let Some(set) = write.as_deref_mut() {
                set.put(fd);
                bits += 1;
            }
        }
    }
    bits
}
