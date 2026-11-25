//! Files: the client side of the file service's protocol.
//!
//! A program that was given a channel to the `fs` service makes a
//! `Files` from it. Each call is one or more request-reply exchanges:
//! a request goes out on the shared request channel together with one
//! end of a fresh channel, and the reply comes back on the other end -
//! so replies cannot get mixed up between clients, and if the service
//! dies before answering, its end of the reply channel dies with it and
//! the wait ends in PeerClosed instead of lasting forever. And if the
//! service is alive but never answers - stuck - the wait ends anyway,
//! after `patience` ticks, in TimedOut: a client is not obliged to hang
//! because its server did.
//!
//! Files bigger than one message are read and written a message's worth
//! at a time.
//!
//! The service takes one kind of name: a path from the root,
//! "docs/notes/today". It remembers nothing about its clients, so "the
//! directory I am in" is not something it could hold. A `Files` holds
//! it instead: the directory the program was started in. Every name
//! given to the calls below is resolved against it here, on the
//! client's side (fileserver::path) - "today", "../plan", "/welcome" -
//! and what goes out in the request is always the whole path.

use alloc::string::String;

pub use fileserver::path;
pub use fileserver::protocol::{Kind, Status};
use fileserver::protocol::{self, Request, MESSAGE_MAX};

use crate::{Error, Handle};

/// Why a file operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    /// The service answered: no.
    Refused(Status),
    /// It could not be asked, or did not answer.
    Unreachable(Error),
}

impl FileError {
    pub fn describe(self) -> &'static str {
        match self {
            FileError::Refused(status) => status.describe(),
            FileError::Unreachable(Error::PeerClosed) => "the file service stopped before answering",
            FileError::Unreachable(Error::TimedOut) => "the file service is not answering",
            FileError::Unreachable(_) => "the file service cannot be reached",
        }
    }
}

pub struct Files {
    requests: Handle,
    /// Ticks to wait for each answer.
    patience: u64,
    /// Where names that do not start with '/' start from: a resolved
    /// path from the root, "" for the root itself.
    directory: String,
}

impl Files {
    pub fn new(requests: Handle) -> Files {
        Files { requests, patience: crate::ANSWER_WAIT, directory: String::new() }
    }

    /// The same, for someone in `directory`: names are taken from there.
    pub fn in_directory(requests: Handle, directory: &str) -> Files {
        Files { directory: path::resolve("", directory), ..Files::new(requests) }
    }

    /// The same, giving up on an answer after `ticks`.
    pub fn with_patience(requests: Handle, ticks: u64) -> Files {
        Files { patience: ticks, ..Files::new(requests) }
    }

    /// The directory names are taken from, as a path from the root.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// The whole path, from the root, that `name` means here.
    fn path(&self, name: &str) -> String {
        path::resolve(&self.directory, name)
    }

    /// The request channel - to pass a copy of it on.
    pub fn channel(&self) -> Handle {
        self.requests
    }

    /// One exchange. Returns the reply's payload length (in `reply`
    /// after the status byte).
    fn call(&self, request: Request, reply: &mut [u8; MESSAGE_MAX]) -> Result<usize, FileError> {
        self.call_with(request, &[], reply)
    }

    /// One exchange, with `more` handles (at most one) sent after the
    /// reply channel. They are the service's once sent; if sending fails
    /// they are closed here.
    fn call_with(&self, request: Request, more: &[Handle], reply: &mut [u8; MESSAGE_MAX]) -> Result<usize, FileError> {
        let mut message = [0u8; MESSAGE_MAX];
        let Some(length) = request.encode(&mut message) else {
            close_all(more);
            return Err(FileError::Refused(Status::BadName));
        };
        let (answer, theirs) = match crate::channel_create() {
            Ok(pair) => pair,
            Err(error) => {
                close_all(more);
                return Err(FileError::Unreachable(error));
            }
        };
        let mut handles = [theirs, 0];
        handles[1..=more.len()].copy_from_slice(more);
        let sent = crate::send(self.requests, &message[..length], &handles[..=more.len()]);
        if sent.is_err() {
            // not sent: still ours
            close_all(&handles[..=more.len()]);
        }
        // Giving up closes our end: an answer that comes later after all
        // finds no one, and the service's send fails - harmlessly.
        let deadline = crate::ticks() + self.patience;
        let received = sent.and_then(|()| crate::receive_until(answer, reply, &mut [], deadline));
        let _ = crate::close(answer);
        match received.map_err(FileError::Unreachable)? {
            (0, _) => Err(FileError::Refused(Status::BadRequest)),
            (n, _) => match Status::from_u8(reply[0]) {
                Status::Ok => Ok(n - 1),
                status => Err(FileError::Refused(status)),
            },
        }
    }

    /// The name, kind and size of everything in `directory` ("" or "."
    /// is the one names are taken from), in name order. Returns how
    /// many there were.
    pub fn list(&self, directory: &str, mut each: impl FnMut(&str, Kind, u64)) -> Result<usize, FileError> {
        let directory = &self.path(directory);
        let mut reply = [0u8; MESSAGE_MAX];
        let mut count = 0;
        loop {
            let n = self.call(Request::new(protocol::LIST, directory, count as u64), &mut reply)?;
            let before = count;
            for (name, kind, size) in protocol::entries(&reply[1..=n]) {
                each(name, kind, size);
                count += 1;
            }
            if count == before {
                return Ok(count);
            }
        }
    }

    /// `name` as a stream to write to (protocol::STREAM_OUT): a channel
    /// on which every stream DATA message sent is added to the file -
    /// what to give a program as its standard output, to have what it
    /// prints in the file. `keep` false empties the file first (a
    /// shell's `>`), true adds to it (`>>`); either makes it if it is
    /// missing. Closing the channel (every copy of it) ends the stream.
    pub fn stream_out(&self, name: &str, keep: bool) -> Result<Handle, FileError> {
        self.stream(Request::new(protocol::STREAM_OUT, &self.path(name), u64::from(keep)))
    }

    /// `name` as a stream to read from (protocol::STREAM_IN): a channel
    /// on which the service sends the file's bytes, from the start, as
    /// stream DATA messages, and which ends with the file - what to give
    /// a program as its standard input (a shell's `<`).
    pub fn stream_in(&self, name: &str) -> Result<Handle, FileError> {
        self.stream_in_from(name, 0)
    }

    /// The same, from byte `offset` on: a descriptor that had read part
    /// of the file, handed to a program as its input, goes on from where
    /// it was.
    pub fn stream_in_from(&self, name: &str, offset: u64) -> Result<Handle, FileError> {
        self.stream(Request::new(protocol::STREAM_IN, &self.path(name), offset))
    }

    /// A stream opened: our end of a new channel, whose other end the
    /// service keeps.
    fn stream(&self, request: Request) -> Result<Handle, FileError> {
        let (ours, theirs) = crate::channel_create().map_err(FileError::Unreachable)?;
        let mut reply = [0u8; MESSAGE_MAX];
        match self.call_with(request, &[theirs], &mut reply) {
            Ok(_) => Ok(ours),
            Err(error) => {
                let _ = crate::close(ours);
                Err(error)
            }
        }
    }

    /// The whole file, a piece at a time. Returns its size.
    pub fn read(&self, name: &str, mut each: impl FnMut(&[u8])) -> Result<u64, FileError> {
        let name = &self.path(name);
        let mut reply = [0u8; MESSAGE_MAX];
        let mut offset = 0u64;
        loop {
            let n = self.call(Request::new(protocol::READ, name, offset), &mut reply)?;
            if n > 0 {
                each(&reply[1..=n]);
            }
            offset += n as u64;
            if n < protocol::READ_MAX {
                return Ok(offset);
            }
        }
    }

    /// The file, into the memory object `memory`, each byte at its own
    /// offset: as much as fits, protocol::READ_INTO_MAX bytes an
    /// exchange (protocol::READ_INTO). The object is lent to the service
    /// for each: a duplicate goes, and the service closes it. Returns
    /// how many bytes were read - the file's size, if it fits.
    pub fn read_into(&self, name: &str, memory: Handle) -> Result<u64, FileError> {
        let name = &self.path(name);
        let size = crate::memory_size(memory).map_err(FileError::Unreachable)?;
        let mut offset = 0u64;
        loop {
            let lent = crate::duplicate(memory, crate::rights::READ | crate::rights::WRITE | crate::rights::TRANSFER)
                .map_err(FileError::Unreachable)?;
            let mut reply = [0u8; MESSAGE_MAX];
            let n = self.call_with(Request::new(protocol::READ_INTO, name, offset), &[lent], &mut reply)?;
            if n < 8 {
                return Err(FileError::Refused(Status::BadRequest));
            }
            let got = u64::from_le_bytes(reply[1..9].try_into().expect("8 bytes"));
            offset += got;
            // Less than a whole piece: the file (or the object) ends.
            if got < protocol::READ_INTO_MAX as u64 || offset >= size {
                return Ok(offset);
            }
        }
    }

    /// One piece of the file, from `offset`: up to protocol::READ_MAX
    /// bytes, given to `piece`. Returns how many; fewer than READ_MAX
    /// means the file ends there. For reading a file a little at a
    /// time, with other work in between.
    pub fn read_from(&self, name: &str, offset: u64, piece: impl FnOnce(&[u8])) -> Result<usize, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::READ, &self.path(name), offset), &mut reply)?;
        piece(&reply[1..=n]);
        Ok(n)
    }

    /// Replaces the file's contents with `data`, creating it if needed.
    pub fn write(&self, name: &str, data: &[u8]) -> Result<u64, FileError> {
        let name = &self.path(name);
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::TRUNCATE, name, 0), &mut reply)?;
        self.write_at(name, 0, data)
    }

    /// Adds `data` at the end of the file, creating it if needed.
    pub fn append(&self, name: &str, data: &[u8]) -> Result<u64, FileError> {
        self.write_at(&self.path(name), protocol::APPEND, data)
    }

    /// Writes `data` into the file from `offset` on, over what is
    /// there and past its end, creating it if needed. Returns the
    /// file's size afterwards.
    pub fn write_from(&self, name: &str, offset: u64, data: &[u8]) -> Result<u64, FileError> {
        self.write_at(&self.path(name), offset, data)
    }

    /// `name` is a whole path here: already resolved.
    fn write_at(&self, name: &str, offset: u64, data: &[u8]) -> Result<u64, FileError> {
        if name.len() > protocol::NAME_MAX {
            return Err(FileError::Refused(Status::BadName));
        }
        let mut reply = [0u8; MESSAGE_MAX];
        let mut size = None;
        let mut offset = offset;
        // At least one request, so an empty write still creates the file.
        let mut chunks = data.chunks(protocol::write_max(name).max(1));
        let first: &[u8] = chunks.next().unwrap_or(&[]);
        for chunk in core::iter::once(first).chain(chunks) {
            let request = Request { op: protocol::WRITE, name, number: offset, data: chunk };
            let n = self.call(request, &mut reply)?;
            size = protocol::number(&reply[1..=n]);
            if offset != protocol::APPEND {
                offset += chunk.len() as u64;
            }
        }
        size.ok_or(FileError::Refused(Status::BadRequest))
    }

    /// What `path` names - a file or a directory - and its size.
    pub fn stat(&self, path: &str) -> Result<(Kind, u64), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::STAT, &self.path(path), 0), &mut reply)?;
        protocol::stat(&reply[1..=n]).ok_or(FileError::Refused(Status::BadRequest))
    }

    /// As `stat`, and when it last changed: its change stamp, a number
    /// that only grows (fileserver's protocol::STAT). 0 for a directory.
    pub fn stat_stamped(&self, path: &str) -> Result<(Kind, u64, u64), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::STAT, &self.path(path), 0), &mut reply)?;
        let (kind, size) = protocol::stat(&reply[1..=n]).ok_or(FileError::Refused(Status::BadRequest))?;
        Ok((kind, size, protocol::stamp(&reply[1..=n])))
    }

    /// Marks the file `name` changed now, as `touch` does: a new, higher
    /// stamp, and nothing in it changed.
    pub fn touch(&self, name: &str) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::TOUCH, &self.path(name), 0), &mut reply).map(|_| ())
    }

    /// The size of the file `name`. A directory has none.
    pub fn size(&self, name: &str) -> Result<u64, FileError> {
        match self.stat(name)? {
            (Kind::File, size) => Ok(size),
            (Kind::Directory, _) => Err(FileError::Refused(Status::IsDirectory)),
        }
    }

    /// Makes an empty directory, in a directory that is already there.
    pub fn make_directory(&self, path: &str) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::MKDIR, &self.path(path), 0), &mut reply).map(|_| ())
    }

    /// Whether this channel is for reading only.
    pub fn read_only(&self) -> Result<bool, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::ACCESS, "", 0), &mut reply)?;
        Ok(n == 1 && reply[1] == 1)
    }

    /// Asks the service to answer nothing for `ticks` - a test of what
    /// its clients do meanwhile. This request itself is answered.
    pub fn stall(&self, ticks: u64) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::STALL, "", ticks), &mut reply).map(|_| ())
    }

    /// Gives what `from` names the path `to` instead: a new name, a
    /// new directory, or both. A directory moves with all that is in
    /// it. Refused if `to` already names something.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let (from, to) = (self.path(from), self.path(to));
        let request = Request { op: protocol::RENAME, name: &from, number: 0, data: to.as_bytes() };
        self.call(request, &mut reply).map(|_| ())
    }

    /// The whole path, from the root, that `name` means here ("" is
    /// the root).
    pub fn resolve(&self, name: &str) -> String {
        self.path(name)
    }

    pub fn remove(&self, name: &str) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::REMOVE, &self.path(name), 0), &mut reply).map(|_| ())
    }
}

/// Closes each of `handles`, for when they are not wanted any more and
/// a failure to close would change nothing.
fn close_all(handles: &[Handle]) {
    for &handle in handles {
        let _ = crate::close(handle);
    }
}
