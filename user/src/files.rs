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
//!
//! An open file (`File`, from `Files::open`) is a channel of its own to
//! the service, which knows which file it is: its requests carry no
//! name, and it stays the same file when the file is renamed, or even
//! removed (fileserver's `nodes`). Its handle can be handed to another
//! program, which then has that one file - and no other.
//!
//! A `Files` is also the program's **namespace** (J62): directories
//! mounted at paths of its own (`mounted`). A name under a mount goes
//! to that directory's channel, with the rest of the path; the deepest
//! mount wins, and anything under none goes to the service's root. The
//! service never sees the program's paths, only names inside the
//! directory it was given, so ".." is worked out here, in the namespace
//! - it can go up past a mount, and back down somewhere else - and a
//! rename between two mounts is refused here (CrossMount, C's EXDEV),
//! as Unix refuses one between two file systems. A program hands its
//! mounts down to the programs it starts (libc's posix_spawn), so a
//! namespace is the parent's to give. No kernel is involved: a mount is
//! a handle and a path, in the program's own memory.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub use fileserver::path;
pub use fileserver::protocol::{open, Kind, Status, COMPONENT_MAX};
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
    /// This program's namespace (J62): directories mounted at paths of
    /// its own. A name under one goes to that directory's channel, with
    /// the rest of the path; any other name goes to `requests`.
    mounts: Vec<Mount>,
}

/// A directory, seen at `at` in this program's namespace.
#[derive(Clone)]
pub struct Mount {
    /// Where: a resolved path from the root ("" is the root itself: the
    /// whole namespace is that directory).
    pub at: String,
    /// What: an open directory (`Files::open` with open::DIRECTORY),
    /// shared by every `Files` that has the mount - the channel closes
    /// when the last one lets it go.
    pub directory: Arc<File>,
}

/// What `whole` (a resolved path) is called under `at`: the rest of the
/// path, "" for `at` itself, or None if it is not under it.
fn under<'a>(at: &str, whole: &'a str) -> Option<&'a str> {
    if at.is_empty() {
        return Some(whole);
    }
    match whole.strip_prefix(at)? {
        "" => Some(""),
        rest => rest.strip_prefix('/'),
    }
}

impl Files {
    pub fn new(requests: Handle) -> Files {
        Files { requests, patience: crate::ANSWER_WAIT, directory: String::new(), mounts: Vec::new() }
    }

    /// The same, for someone in `directory`: names are taken from there.
    pub fn in_directory(requests: Handle, directory: &str) -> Files {
        Files { directory: path::resolve("", directory), ..Files::new(requests) }
    }

    /// The same, giving up on an answer after `ticks`.
    pub fn with_patience(requests: Handle, ticks: u64) -> Files {
        Files { patience: ticks, ..Files::new(requests) }
    }

    /// The same files - the same mounts - with names taken from
    /// `directory` (a name, resolved from where names are taken now):
    /// chdir.
    pub fn moved_to(&self, directory: &str) -> Files {
        Files { requests: self.requests, patience: self.patience, directory: self.path(directory), mounts: self.mounts.clone() }
    }

    /// The same, with `directory` - an open directory - seen at `at`
    /// from now on, in place of whatever was there: what is at `at` and
    /// under it before is out of sight until it is unmounted. NotDirectory
    /// if `directory` is a file.
    pub fn mounted(&self, at: &str, directory: File) -> Result<Files, FileError> {
        if directory.kind() != Kind::Directory {
            return Err(FileError::Refused(Status::NotDirectory));
        }
        let at = self.path(at);
        let mut mounts: Vec<Mount> = self.mounts.iter().filter(|mount| mount.at != at).cloned().collect();
        mounts.push(Mount { at, directory: Arc::new(directory) });
        Ok(Files { mounts, ..self.moved_to(".") })
    }

    /// The same, without the mount at `at`: what was there before is
    /// seen again. NotFound if nothing is mounted there.
    pub fn unmounted(&self, at: &str) -> Result<Files, FileError> {
        let at = self.path(at);
        if !self.mounts.iter().any(|mount| mount.at == at) {
            return Err(FileError::Refused(Status::NotFound));
        }
        let mounts = self.mounts.iter().filter(|mount| mount.at != at).cloned().collect();
        Ok(Files { mounts, ..self.moved_to(".") })
    }

    /// Every mount, in the order they were made: to hand them down to a
    /// program this one starts.
    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// The directory names are taken from, as a path from the root.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// The whole path, from the root, that `name` means here.
    fn path(&self, name: &str) -> String {
        path::resolve(&self.directory, name)
    }

    /// Where a request about `name` goes: the channel, and the name to
    /// send on it - the whole path on the root's, the rest of it on a
    /// mounted directory's. The deepest mount the path is under wins;
    /// and the third answer is whether `name` is a mount itself.
    fn route(&self, name: &str) -> (Handle, String, bool) {
        let whole = self.path(name);
        let deepest = self
            .mounts
            .iter()
            .filter_map(|mount| under(&mount.at, &whole).map(|rest| (mount, rest)))
            .max_by_key(|(mount, _)| mount.at.len());
        match deepest {
            Some((mount, rest)) => (mount.directory.channel(), String::from(rest), rest.is_empty()),
            None => (self.requests, whole, false),
        }
    }

    /// The request channel - to pass a copy of it on.
    pub fn channel(&self) -> Handle {
        self.requests
    }

    /// One exchange about `name`, with `more` handles (at most one) sent
    /// after the reply channel: the reply's payload length (in `reply`
    /// after the status byte). The handles are the service's once sent;
    /// if sending fails they are closed here.
    fn call(&self, op: u8, name: &str, number: u64, more: &[Handle], reply: &mut [u8; MESSAGE_MAX]) -> Result<usize, FileError> {
        let (channel, name, _) = self.route(name);
        exchange(channel, self.patience, Request::new(op, &name, number), more, reply)
    }

    /// Opens the file or directory `name` names: a channel of its own to
    /// it (see `File`). `flags` are `open`'s: READ, WRITE, DIRECTORY,
    /// CREATE, EXCLUSIVE, TRUNCATE.
    pub fn open(&self, name: &str, flags: u64) -> Result<File, FileError> {
        let (ours, theirs) = crate::channel_create().map_err(FileError::Unreachable)?;
        let mut reply = [0u8; MESSAGE_MAX];
        match self.call(protocol::OPEN, name, flags, &[theirs], &mut reply) {
            Ok(n) => {
                let (kind, _) = protocol::stat(&reply[1..=n]).unwrap_or((Kind::File, 0));
                Ok(File { channel: ours, kind, patience: self.patience })
            }
            Err(error) => {
                let _ = crate::close(ours);
                Err(error)
            }
        }
    }

    /// The name, kind and size of everything in `directory` ("" or "."
    /// is the one names are taken from), in name order. Returns how
    /// many there were. A mount directly in it is listed too, as a
    /// directory, if the directory has nothing of that name - as a
    /// mount point is seen on Unix.
    pub fn list(&self, directory: &str, mut each: impl FnMut(&str, Kind, u64)) -> Result<usize, FileError> {
        let whole = self.path(directory);
        // The mounts one step below it: their names, unless listed.
        let mut extra: Vec<&str> = self
            .mounts
            .iter()
            .filter_map(|mount| under(&whole, &mount.at))
            .filter(|rest| !rest.is_empty() && !rest.contains('/'))
            .collect();
        let mut reply = [0u8; MESSAGE_MAX];
        let mut count = 0;
        loop {
            let n = self.call(protocol::LIST, directory, count as u64, &[], &mut reply)?;
            let before = count;
            for (name, kind, size) in protocol::entries(&reply[1..=n]) {
                extra.retain(|mounted| *mounted != name);
                each(name, kind, size);
                count += 1;
            }
            if count == before {
                break;
            }
        }
        extra.sort_unstable();
        extra.dedup();
        for name in &extra {
            each(name, Kind::Directory, 0);
        }
        Ok(count + extra.len())
    }

    /// `name` as a stream to write to (protocol::STREAM_OUT): a channel
    /// on which every stream DATA message sent is added to the file -
    /// what to give a program as its standard output, to have what it
    /// prints in the file. `keep` false empties the file first (a
    /// shell's `>`), true adds to it (`>>`); either makes it if it is
    /// missing. Closing the channel (every copy of it) ends the stream.
    pub fn stream_out(&self, name: &str, keep: bool) -> Result<Handle, FileError> {
        self.stream(protocol::STREAM_OUT, name, u64::from(keep))
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
        self.stream(protocol::STREAM_IN, name, offset)
    }

    /// A stream opened: our end of a new channel, whose other end the
    /// service keeps.
    fn stream(&self, op: u8, name: &str, number: u64) -> Result<Handle, FileError> {
        let (ours, theirs) = crate::channel_create().map_err(FileError::Unreachable)?;
        let mut reply = [0u8; MESSAGE_MAX];
        match self.call(op, name, number, &[theirs], &mut reply) {
            Ok(_) => Ok(ours),
            Err(error) => {
                let _ = crate::close(ours);
                Err(error)
            }
        }
    }

    /// The whole file, a piece at a time. Returns its size.
    pub fn read(&self, name: &str, mut each: impl FnMut(&[u8])) -> Result<u64, FileError> {
        let (channel, name, _) = self.route(name);
        let mut reply = [0u8; MESSAGE_MAX];
        let mut offset = 0u64;
        loop {
            let n = exchange(channel, self.patience, Request::new(protocol::READ, &name, offset), &[], &mut reply)?;
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
        let (channel, name, _) = self.route(name);
        let size = crate::memory_size(memory).map_err(FileError::Unreachable)?;
        let mut offset = 0u64;
        loop {
            let lent = crate::duplicate(memory, crate::rights::READ | crate::rights::WRITE | crate::rights::TRANSFER)
                .map_err(FileError::Unreachable)?;
            let mut reply = [0u8; MESSAGE_MAX];
            let n = exchange(channel, self.patience, Request::new(protocol::READ_INTO, &name, offset), &[lent], &mut reply)?;
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
        let n = self.call(protocol::READ, name, offset, &[], &mut reply)?;
        piece(&reply[1..=n]);
        Ok(n)
    }

    /// Replaces the file's contents with `data`, creating it if needed.
    pub fn write(&self, name: &str, data: &[u8]) -> Result<u64, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(protocol::TRUNCATE, name, 0, &[], &mut reply)?;
        self.write_from(name, 0, data)
    }

    /// Adds `data` at the end of the file, creating it if needed.
    pub fn append(&self, name: &str, data: &[u8]) -> Result<u64, FileError> {
        self.write_from(name, protocol::APPEND, data)
    }

    /// Writes `data` into the file from `offset` on, over what is
    /// there and past its end, creating it if needed. Returns the
    /// file's size afterwards.
    pub fn write_from(&self, name: &str, offset: u64, data: &[u8]) -> Result<u64, FileError> {
        let (channel, name, _) = self.route(name);
        write_at(channel, self.patience, &name, offset, data)
    }

    /// What `path` names - a file or a directory - and its size.
    pub fn stat(&self, path: &str) -> Result<(Kind, u64), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(protocol::STAT, path, 0, &[], &mut reply)?;
        protocol::stat(&reply[1..=n]).ok_or(FileError::Refused(Status::BadRequest))
    }

    /// As `stat`, and when it last changed: its change stamp, a number
    /// that only grows (fileserver's protocol::STAT). 0 for a directory.
    pub fn stat_stamped(&self, path: &str) -> Result<(Kind, u64, u64), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(protocol::STAT, path, 0, &[], &mut reply)?;
        let (kind, size) = protocol::stat(&reply[1..=n]).ok_or(FileError::Refused(Status::BadRequest))?;
        Ok((kind, size, protocol::stamp(&reply[1..=n])))
    }

    /// As `stat_stamped`, and the time of that change: nanoseconds since
    /// 1970, or 0 if the service does not know it (a directory, a file
    /// not changed since TaajFS version 6, the memory store).
    pub fn stat_timed(&self, path: &str) -> Result<(Kind, u64, u64, u64), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(protocol::STAT, path, 0, &[], &mut reply)?;
        let payload = &reply[1..=n];
        let (kind, size) = protocol::stat(payload).ok_or(FileError::Refused(Status::BadRequest))?;
        Ok((kind, size, protocol::stamp(payload), protocol::time(payload)))
    }

    /// Marks the file `name` changed now, as `touch` does: a new, higher
    /// stamp, and nothing in it changed.
    pub fn touch(&self, name: &str) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(protocol::TOUCH, name, 0, &[], &mut reply).map(|_| ())
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
        let (_, _, mount) = self.route(path);
        if mount {
            return Err(FileError::Refused(Status::Exists));
        }
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(protocol::MKDIR, path, 0, &[], &mut reply).map(|_| ())
    }

    /// Whether this channel is for reading only.
    pub fn read_only(&self) -> Result<bool, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = exchange(self.requests, self.patience, Request::new(protocol::ACCESS, "", 0), &[], &mut reply)?;
        Ok(n == 1 && reply[1] == 1)
    }

    /// Asks the service to answer nothing for `ticks` - a test of what
    /// its clients do meanwhile. This request itself is answered.
    pub fn stall(&self, ticks: u64) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        exchange(self.requests, self.patience, Request::new(protocol::STALL, "", ticks), &[], &mut reply).map(|_| ())
    }

    /// Gives what `from` names the path `to` instead: a new name, a
    /// new directory, or both. A directory moves with all that is in
    /// it. Refused if `to` already names something. Both must be under
    /// the same mount (CrossMount, as Unix's EXDEV), and neither may be
    /// a mount (MountPoint, EBUSY): a request goes to one directory.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), FileError> {
        let (channel, from, from_mount) = self.route(from);
        let (other, to, to_mount) = self.route(to);
        if channel != other {
            return Err(FileError::Refused(Status::CrossMount));
        }
        if from_mount || to_mount {
            return Err(FileError::Refused(Status::MountPoint));
        }
        let mut reply = [0u8; MESSAGE_MAX];
        let request = Request { op: protocol::RENAME, name: &from, number: 0, data: to.as_bytes() };
        exchange(channel, self.patience, request, &[], &mut reply).map(|_| ())
    }

    /// The whole path, from the root, that `name` means here ("" is
    /// the root).
    pub fn resolve(&self, name: &str) -> String {
        self.path(name)
    }

    /// Removes the file, or empty directory, `name`. Not a mount
    /// (MountPoint): unmount it first.
    pub fn remove(&self, name: &str) -> Result<(), FileError> {
        let (_, _, mount) = self.route(name);
        if mount {
            return Err(FileError::Refused(Status::MountPoint));
        }
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(protocol::REMOVE, name, 0, &[], &mut reply).map(|_| ())
    }
}

/// Closes each of `handles`, for when they are not wanted any more and
/// a failure to close would change nothing.
fn close_all(handles: &[Handle]) {
    for &handle in handles {
        let _ = crate::close(handle);
    }
}

/// Writes `data` at `offset` (or protocol::APPEND) in what `name` names
/// on `channel`: a whole path on a service's channel, "" on a file's.
/// Returns the file's size afterwards.
fn write_at(channel: Handle, patience: u64, name: &str, offset: u64, data: &[u8]) -> Result<u64, FileError> {
    if name.len() > protocol::NAME_MAX {
        return Err(FileError::Refused(Status::BadName));
    }
    if data.len() > protocol::write_max(name) {
        return write_through_memory(channel, patience, name, offset, data);
    }
    let mut reply = [0u8; MESSAGE_MAX];
    let mut size = None;
    let mut offset = offset;
    // At least one request, so an empty write still creates the file.
    let mut chunks = data.chunks(protocol::write_max(name).max(1));
    let first: &[u8] = chunks.next().unwrap_or(&[]);
    for chunk in core::iter::once(first).chain(chunks) {
        let request = Request { op: protocol::WRITE, name, number: offset, data: chunk };
        let n = exchange(channel, patience, request, &[], &mut reply)?;
        size = protocol::number(&reply[1..=n]);
        if offset != protocol::APPEND {
            offset += chunk.len() as u64;
        }
    }
    size.ok_or(FileError::Refused(Status::BadRequest))
}

/// More than one message holds: the bytes copied into a memory
/// object, and written from there (protocol::WRITE_FROM) - one
/// request, and one commit, for each WRITE_FROM_MAX of them, where
/// WRITEs would make one a message (about 1 KB).
fn write_through_memory(channel: Handle, patience: u64, name: &str, offset: u64, data: &[u8]) -> Result<u64, FileError> {
    let memory = crate::memory_create(data.len() as u64).map_err(FileError::Unreachable)?;
    let filled = crate::memory_map(memory, true).map(|base| {
        // SAFETY: the object is at least data.len() bytes, mapped
        // writable, and ours alone until unmapped.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), base, data.len()) };
        let _ = crate::memory_unmap(base);
    });
    if let Err(error) = filled {
        let _ = crate::close(memory);
        return Err(FileError::Unreachable(error));
    }
    let mut written = Ok(0);
    let mut offset = offset;
    for start in (0..data.len()).step_by(protocol::WRITE_FROM_MAX) {
        let length = (data.len() - start).min(protocol::WRITE_FROM_MAX);
        let place = [(start as u64).to_le_bytes(), (length as u64).to_le_bytes()].concat();
        let request = Request { op: protocol::WRITE_FROM, name, number: offset, data: &place };
        let mut reply = [0u8; MESSAGE_MAX];
        written = crate::duplicate(memory, crate::rights::READ | crate::rights::TRANSFER)
            .map_err(FileError::Unreachable)
            .and_then(|lent| exchange(channel, patience, request, &[lent], &mut reply))
            .and_then(|n| protocol::number(&reply[1..=n]).ok_or(FileError::Refused(Status::BadRequest)));
        if written.is_err() {
            break;
        }
        if offset != protocol::APPEND {
            offset += length as u64;
        }
    }
    let _ = crate::close(memory);
    written
}

/// One exchange on `channel`: the request, with a fresh channel for the
/// reply and `more` handles (at most one) after it; the reply's payload
/// length (in `reply`, after the status byte). The handles are the
/// service's once sent; if sending fails they are closed here. Giving
/// up after `patience` ticks closes our end of the reply channel: an
/// answer that comes later finds no one, and the service's send fails -
/// harmlessly.
fn exchange(channel: Handle, patience: u64, request: Request, more: &[Handle], reply: &mut [u8; MESSAGE_MAX]) -> Result<usize, FileError> {
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
    let sent = crate::send(channel, &message[..length], &handles[..=more.len()]);
    if sent.is_err() {
        // not sent: still ours
        close_all(&handles[..=more.len()]);
    }
    let deadline = crate::ticks() + patience;
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

/// An open file or directory: a channel of its own to the file service
/// (protocol::OPEN). Its requests carry no name - the service knows
/// which file the channel is for - so it is the same file however it is
/// renamed or moved, and while it is open a removal only takes its name
/// away. Closed when dropped: the service then lets it go.
///
/// What it allows was fixed when it was opened: a file opened without
/// `open::WRITE` refuses every change, whoever holds it.
pub struct File {
    channel: Handle,
    kind: Kind,
    patience: u64,
}

impl File {
    /// A file whose channel was handed to this program (by whoever
    /// started it, say). Its kind is asked for here.
    pub fn from_handle(channel: Handle) -> Result<File, FileError> {
        let mut file = File { channel, kind: Kind::File, patience: crate::ANSWER_WAIT };
        file.kind = file.stat()?.0;
        Ok(file)
    }

    /// The channel: to hand a copy of it on (`share`), or to wait on.
    pub fn channel(&self) -> Handle {
        self.channel
    }

    /// A second handle to it, that can be sent to another program. The
    /// copy allows what this one does: the service decides that, not
    /// the handle.
    pub fn share(&self) -> Result<Handle, FileError> {
        crate::duplicate(self.channel, crate::rights::SEND | crate::rights::DUPLICATE | crate::rights::TRANSFER)
            .map_err(FileError::Unreachable)
    }

    /// Whether it is a file or a directory.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    fn call(&self, request: Request, reply: &mut [u8; MESSAGE_MAX]) -> Result<usize, FileError> {
        exchange(self.channel, self.patience, request, &[], reply)
    }

    /// Up to `out.len()` bytes from `offset` (at most protocol::READ_MAX
    /// in one exchange). Returns how many: 0 at the end of the file.
    pub fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<usize, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::READ, "", offset), &mut reply)?;
        let taken = n.min(out.len());
        out[..taken].copy_from_slice(&reply[1..=taken]);
        Ok(taken)
    }

    /// Writes `data` from `offset` on, over what is there and past its
    /// end. Returns the file's size afterwards.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<u64, FileError> {
        write_at(self.channel, self.patience, "", offset, data)
    }

    /// Adds `data` at the end - where the end is when the service
    /// writes, so two writers appending cannot write over each other.
    /// Returns the file's size afterwards.
    pub fn append(&self, data: &[u8]) -> Result<u64, FileError> {
        write_at(self.channel, self.patience, "", protocol::APPEND, data)
    }

    /// Sets the size; growing fills with zeros.
    pub fn truncate(&self, length: u64) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::TRUNCATE, "", length), &mut reply).map(|_| ())
    }

    /// Its kind and size.
    pub fn stat(&self) -> Result<(Kind, u64), FileError> {
        let (kind, size, _, _) = self.stat_timed()?;
        Ok((kind, size))
    }

    /// Its kind, size, change stamp and time of last change (as
    /// `Files::stat_timed`).
    pub fn stat_timed(&self) -> Result<(Kind, u64, u64, u64), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let n = self.call(Request::new(protocol::STAT, "", 0), &mut reply)?;
        let payload = &reply[1..=n];
        let (kind, size) = protocol::stat(payload).ok_or(FileError::Refused(Status::BadRequest))?;
        Ok((kind, size, protocol::stamp(payload), protocol::time(payload)))
    }

    /// Its size now.
    pub fn size(&self) -> Result<u64, FileError> {
        Ok(self.stat()?.1)
    }

    /// Marks it changed now, changing nothing in it.
    pub fn touch(&self) -> Result<(), FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        self.call(Request::new(protocol::TOUCH, "", 0), &mut reply).map(|_| ())
    }

    /// The file as a stream to read, from `offset` on (as
    /// `Files::stream_in_from`): a program's standard input.
    pub fn stream_in_from(&self, offset: u64) -> Result<Handle, FileError> {
        self.stream(Request::new(protocol::STREAM_IN, "", offset))
    }

    /// The file as a stream to write to its end (as `Files::stream_out`
    /// with `keep`): a program's standard output.
    pub fn stream_out(&self) -> Result<Handle, FileError> {
        self.stream(Request::new(protocol::STREAM_OUT, "", 1))
    }

    fn stream(&self, request: Request) -> Result<Handle, FileError> {
        let (ours, theirs) = crate::channel_create().map_err(FileError::Unreachable)?;
        let mut reply = [0u8; MESSAGE_MAX];
        match exchange(self.channel, self.patience, request, &[theirs], &mut reply) {
            Ok(_) => Ok(ours),
            Err(error) => {
                let _ = crate::close(ours);
                Err(error)
            }
        }
    }

    /// A directory's names, kinds and sizes, in name order (as
    /// `Files::list`). Returns how many there were.
    pub fn list(&self, mut each: impl FnMut(&str, Kind, u64)) -> Result<usize, FileError> {
        let mut reply = [0u8; MESSAGE_MAX];
        let mut count = 0;
        loop {
            let n = self.call(Request::new(protocol::LIST, "", count as u64), &mut reply)?;
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
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = crate::close(self.channel);
    }
}
