//! Pipes: `pipe`, and what a descriptor is when it is one end of a
//! stream channel - a pipe's, or one handed to a program by whoever
//! started it.
//!
//! A pipe here is a channel, the same kind of thing a program's standard
//! output already is: what is written on one end is stream DATA
//! messages (abi::stream), read from the other. So the write end of a
//! pipe can be given to a program as its standard output, the read end
//! as its standard input (spawn.rs), and the program cannot tell it from
//! any other output or input.
//!
//! An end is shared by every descriptor `dup` made of it, and its handle
//! is closed when the last of them is: the reader sees the end of the
//! stream only once every write end - in this program and in any it
//! gave one to - is closed. That is Unix's rule too, and why a program
//! that hands a pipe's write end to another closes its own.
//!
//! Writing to a pipe nobody reads fails with EPIPE (no SIGPIPE: signals
//! reach a program here only from itself).

use crate::descriptors::{fail, with, Open};
use crate::errno::{EFAULT, EMFILE, EPIPE, EIO, ENFILE};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_int;
use taaj::sync::Mutex;
use taaj::{Error, Handle};

/// A channel end, closed when the last copy goes.
pub(crate) struct Channel(pub(crate) Handle);

impl Drop for Channel {
    fn drop(&mut self) {
        let _ = taaj::close(self.0);
    }
}

/// The end of a stream that is read: the channel, and what a message
/// brought, kept until it has all been asked for. The channel needs no
/// lock (spawn.rs hands its handle on while a thread may be reading);
/// the bytes held have one, so two threads reading the same end take
/// turns, and each byte goes to one of them.
pub(crate) struct Reader {
    pub(crate) channel: Channel,
    held: Mutex<Held>,
}

struct Held {
    bytes: Vec<u8>,
    taken: usize,
    ended: bool,
}

impl Reader {
    pub(crate) fn new(channel: Handle) -> Reader {
        Reader { channel: Channel(channel), held: Mutex::new(Held { bytes: Vec::new(), taken: 0, ended: false }) }
    }

    /// Up to `out.len()` bytes, waiting until there are some; 0 at the
    /// end, when every writer has closed its end and all was read.
    pub(crate) fn read(&self, out: &mut [u8]) -> usize {
        let mut held = self.held.lock();
        loop {
            if held.taken < held.bytes.len() {
                let n = out.len().min(held.bytes.len() - held.taken);
                out[..n].copy_from_slice(&held.bytes[held.taken..held.taken + n]);
                held.taken += n;
                return n;
            }
            if held.ended {
                return 0;
            }
            let mut message = [0u8; abi::MESSAGE_MAX as usize];
            match taaj::receive(self.channel.0, &mut message, &mut []) {
                Ok((length, _)) if length > 1 && message[0] == abi::stream::DATA => {
                    held.bytes.clear();
                    held.bytes.extend_from_slice(&message[1..length]);
                    held.taken = 0;
                }
                Ok(_) => {} // empty, or not part of the stream
                Err(_) => held.ended = true,
            }
        }
    }
}

impl Reader {
    /// Whether a read would not wait, found out without waiting:
    /// `(bytes, ended)`. Messages already there are taken in. None if
    /// another thread is reading now (and so holds the bytes): what it
    /// is waiting for, poll can wait for too, on the channel.
    pub(crate) fn ready(&self) -> Option<(bool, bool)> {
        let mut held = self.held.try_lock()?;
        loop {
            if held.taken < held.bytes.len() {
                return Some((true, false));
            }
            if held.ended {
                return Some((false, true));
            }
            if !matches!(taaj::poll(&[self.channel.0]), Ok(Some(_))) {
                return Some((false, false));
            }
            let mut message = [0u8; abi::MESSAGE_MAX as usize];
            match taaj::receive(self.channel.0, &mut message, &mut []) {
                Ok((length, _)) if length > 1 && message[0] == abi::stream::DATA => {
                    held.bytes.clear();
                    held.bytes.extend_from_slice(&message[1..length]);
                    held.taken = 0;
                }
                Ok(_) => {}
                Err(_) => held.ended = true,
            }
        }
    }
}

/// A pipe's read end, shared by its descriptors (and threads).
pub(crate) type ReadEnd = Arc<Reader>;
/// A pipe's write end, shared by its descriptors.
pub(crate) type WriteEnd = Arc<Channel>;

/// Writes `bytes` on a write end, as stream DATA messages.
pub(crate) fn write(end: &WriteEnd, bytes: &[u8]) -> isize {
    match taaj::write_to_stream(end.0, bytes) {
        Ok(()) => bytes.len() as isize,
        Err(Error::PeerClosed) => fail(EPIPE),
        Err(_) => fail(EIO),
    }
}

/// `pipe(fds)`: a new pipe; `fds[0]` reads what `fds[1]` writes. The
/// two lowest free descriptors.
///
/// # Safety
/// `fds` must be valid for writing two ints.
#[no_mangle]
pub unsafe extern "C" fn pipe(fds: *mut c_int) -> c_int {
    if fds.is_null() {
        return fail(EFAULT);
    }
    let Ok((read_end, write_end)) = taaj::channel_create() else { return fail(ENFILE) };
    let ends = with(|state| {
        let Some(read) = state.free_slot() else {
            let _ = taaj::close(read_end);
            return None;
        };
        state.table[read] = Some(Open::PipeRead(Arc::new(Reader::new(read_end))));
        let Some(write) = state.free_slot() else {
            state.table[read] = None; // closes read_end
            return None;
        };
        state.table[write] = Some(Open::PipeWrite(Arc::new(Channel(write_end))));
        Some((read, write))
    });
    match ends {
        Some((read, write)) => {
            // SAFETY: the caller's promise.
            unsafe {
                *fds = read as c_int;
                *fds.add(1) = write as c_int;
            }
            0
        }
        None => {
            let _ = taaj::close(write_end);
            fail(EMFILE)
        }
    }
}

/// `pipe2(fds, flags)`: as pipe; with O_CLOEXEC both ends are kept from
/// the programs this one starts (spawn.rs). O_NONBLOCK is not kept -
/// the ends wait.
///
/// # Safety
/// As pipe.
#[no_mangle]
pub unsafe extern "C" fn pipe2(fds: *mut c_int, flags: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let made = unsafe { pipe(fds) };
    if made == 0 && flags & O_CLOEXEC != 0 {
        // SAFETY: pipe wrote both.
        let (read, write) = unsafe { (*fds, *fds.add(1)) };
        with(|state| {
            state.set_cloexec(read as usize, true);
            state.set_cloexec(write as usize, true);
        });
    }
    made
}

/// open's O_CLOEXEC (fcntl.h).
pub(crate) const O_CLOEXEC: c_int = 0o2000000;

/// A pipe for this library's own use (the shell's, system's, popen's):
/// both ends close-on-exec, so that a program started while it is open
/// does not get them - and, holding a write end, keep a reader from ever
/// seeing the end. The ends it does mean a program to have it gets by
/// dup2, which clears the mark.
pub(crate) fn private_pipe(fds: &mut [c_int; 2]) -> c_int {
    // SAFETY: room for two.
    unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) }
}
