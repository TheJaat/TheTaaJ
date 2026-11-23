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
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ffi::c_int;
use taaj::{Error, Handle};

/// A channel end, closed when the last copy goes.
pub(crate) struct Channel(pub(crate) Handle);

impl Drop for Channel {
    fn drop(&mut self) {
        let _ = taaj::close(self.0);
    }
}

/// The end of a stream that is read: what a message brought, kept
/// until it has all been asked for.
pub(crate) struct Reader {
    pub(crate) channel: Channel,
    held: Vec<u8>,
    taken: usize,
    ended: bool,
}

impl Reader {
    pub(crate) fn new(channel: Handle) -> Reader {
        Reader { channel: Channel(channel), held: Vec::new(), taken: 0, ended: false }
    }

    /// Up to `out.len()` bytes, waiting until there are some; 0 at the
    /// end, when every writer has closed its end and all was read.
    pub(crate) fn read(&mut self, out: &mut [u8]) -> usize {
        loop {
            if self.taken < self.held.len() {
                let n = out.len().min(self.held.len() - self.taken);
                out[..n].copy_from_slice(&self.held[self.taken..self.taken + n]);
                self.taken += n;
                return n;
            }
            if self.ended {
                return 0;
            }
            let mut message = [0u8; abi::MESSAGE_MAX as usize];
            match taaj::receive(self.channel.0, &mut message, &mut []) {
                Ok((length, _)) if length > 1 && message[0] == abi::stream::DATA => {
                    self.held.clear();
                    self.held.extend_from_slice(&message[1..length]);
                    self.taken = 0;
                }
                Ok(_) => {} // empty, or not part of the stream
                Err(_) => self.ended = true,
            }
        }
    }
}

/// A pipe's read end, shared by its descriptors.
pub(crate) type ReadEnd = Rc<RefCell<Reader>>;
/// A pipe's write end, shared by its descriptors.
pub(crate) type WriteEnd = Rc<Channel>;

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
        state.table[read] = Some(Open::PipeRead(Rc::new(RefCell::new(Reader::new(read_end)))));
        let Some(write) = state.free_slot() else {
            state.table[read] = None; // closes read_end
            return None;
        };
        state.table[write] = Some(Open::PipeWrite(Rc::new(Channel(write_end))));
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

/// `pipe2(fds, flags)`: as pipe. O_CLOEXEC means nothing here (no
/// exec); O_NONBLOCK is not kept - the ends wait.
///
/// # Safety
/// As pipe.
#[no_mangle]
pub unsafe extern "C" fn pipe2(fds: *mut c_int, _flags: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { pipe(fds) }
}
