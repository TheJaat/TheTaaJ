//! The file service's logic: the protocol, and the stores behind it.
//!
//! All pure logic, tested here on the host:
//!
//!   - the protocol: how a request and its reply are laid out in a
//!     channel message. Clients and the `fs` service both use it, so
//!     they cannot disagree about it.
//!   - `Store`: what the service needs from wherever files are kept, and
//!     `serve`, which turns one request message into one reply message
//!     by calling it.
//!   - two stores: `RamStore`, files in memory that vanish with the
//!     service, and `DiskStore`, a TaajFS volume on a block device,
//!     where every change is committed - copy-on-write - before the
//!     reply goes out.
//!
//! The service itself (user/src/bin/fs.rs) only moves messages between
//! a channel and `serve`.
//!
//! What a request names is a path: names separated by '/', from the
//! directory the channel it came on stands for. On the service's own
//! request channels that is the root - "docs/notes/today". A client can
//! also OPEN a file or a directory: it gets a channel of its own to it
//! (`nodes`), which speaks this same protocol - a file's with no name
//! at all, a directory's with names taken from there - and which follows
//! the file through a rename or a removal. There is still no "current
//! directory" here: that belongs to whoever types paths.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dev;
pub mod disk;
pub mod nodes;
pub mod path;
pub mod proc;
pub mod ram;

pub use dev::DevStore;
pub use disk::DiskStore;
pub use proc::ProcStore;
pub use nodes::{NodeId, Nodes};
pub use ram::RamStore;

/// The protocol: one request per message, one reply per request.
///
/// A request:   [op: u8] [path length: u8] [path] [number: u64 LE] [data]
/// A reply:     [status: u8] [payload]
///
/// `number` is an offset, a length or an index, depending on `op`. Each
/// request carries a channel end for its reply as its one handle, so a
/// client waiting for an answer from a service that crashed hears
/// PeerClosed - the reply end died with the service - and does not wait
/// forever.
pub mod protocol {
    pub const MESSAGE_MAX: usize = abi::MESSAGE_MAX as usize;

    /// What is in the directory the path names ("" is the root), from
    /// the `number`th entry on: payload is entries of
    /// [name length: u8] [name] [kind: u8] [size: u64 LE], as many as
    /// fit. An empty payload means there are no more.
    pub const LIST: u8 = 1;
    /// Up to READ_MAX bytes from `number` (an offset). Fewer than asked
    /// for means the end of the file.
    pub const READ: u8 = 2;
    /// Writes the data at `number` (an offset, or APPEND), creating the
    /// file if needed. Payload: the new size, u64 LE.
    pub const WRITE: u8 = 3;
    /// Sets the size to `number`, creating the file if needed; growing
    /// fills with zeros.
    pub const TRUNCATE: u8 = 4;
    pub const REMOVE: u8 = 5;
    /// Payload: the size, u64 LE, then the kind, u8, then the change
    /// stamp, u64 LE: of two files, the one changed later has the higher
    /// stamp (0: not since stamping began, and always for a directory);
    /// then the time of that change, u64 LE, in nanoseconds since 1970
    /// (0: not known - a directory, a file not changed since times
    /// began (TaajFS version 6), the memory store). 25 bytes in all; an
    /// older service sends the first 17.
    pub const STAT: u8 = 6;
    /// A test of what happens when the service stops answering: it
    /// answers this, then does nothing for `number` ticks (at most
    /// STALL_MAX). Only for clients that may change files - they can
    /// keep the service busy as long by other means. The service loop
    /// acts on it; `serve` only checks it.
    pub const STALL: u8 = 7;
    /// What this channel allows. Payload: one byte, 1 if it is
    /// read-only, 0 if files can be changed through it.
    pub const ACCESS: u8 = 8;

    /// Makes an empty directory. The directory it goes in must exist.
    pub const MKDIR: u8 = 9;

    /// Gives what the path names a new path instead: the data is the
    /// new path. A new name, a new directory, or both; a file, or a
    /// directory with everything in it. Nothing is copied. What the new
    /// path names already is replaced, as POSIX's rename does - a file
    /// by a file, an empty directory by a directory - in one step; other
    /// pairs are refused (IsDirectory, NotDirectory, NotEmpty).
    pub const RENAME: u8 = 10;
    /// Fills the memory object that comes with the request - its second
    /// handle, after the reply channel - with the file's bytes from
    /// `number` (an offset) on, at the same offset in the object: up to
    /// READ_INTO_MAX of them, the end of the object or the end of the
    /// file. Payload: how many, u64 LE. For reading a big file - a
    /// program - in a few exchanges instead of one per READ_MAX bytes.
    pub const READ_INTO: u8 = 11;
    /// The most one READ_INTO reads: 4 MiB. The service does nothing
    /// else meanwhile, and init stops a service that does not answer it
    /// for 4 seconds: llc's 34 MB at once took longer than that, in
    /// QEMU without hardware help. 4 MiB takes about half a second.
    pub const READ_INTO_MAX: usize = 4 << 20;

    /// Marks the file changed now - a new, higher stamp (STAT) - and
    /// changes nothing in it, as `touch` does.
    pub const TOUCH: u8 = 12;
    /// STREAM_OUT name, number: a file as a program's output. The
    /// request carries, after its reply channel, one end of a channel:
    /// every stream DATA message that arrives on it is added to the end
    /// of the file, until the other end is closed. `number` 0 empties
    /// the file first (`>`), 1 keeps it (`>>`); either makes it if it is
    /// not there. A change, so refused on the read-only channel.
    pub const STREAM_OUT: u8 = 13;
    /// STREAM_IN name, number: a file as a program's input. As
    /// STREAM_OUT, but the service sends the file's bytes, from offset
    /// `number`, as DATA messages on the channel, and closes it at the
    /// end (`<`). Allowed on either channel.
    pub const STREAM_IN: u8 = 14;
    /// Most streams the service keeps open at once.
    pub const STREAMS_MAX: usize = 16;
    /// WRITE_FROM name, number: writes bytes that come in the memory
    /// object with the request - its second handle, after the reply
    /// channel - at `number` (an offset, or APPEND), creating the file
    /// if needed: one change, one commit, for up to WRITE_FROM_MAX bytes,
    /// where WRITE carries at most a message's worth. The data field is
    /// where they are in the object and how many: [from: u64 LE]
    /// [length: u64 LE]. Payload: the new size, u64 LE, as WRITE's.
    pub const WRITE_FROM: u8 = 15;
    /// The most one WRITE_FROM writes: 1 MiB. As READ_INTO_MAX, small
    /// enough that the service answers everyone else soon enough; and
    /// a write is slower than a read - every block goes to the disk.
    pub const WRITE_FROM_MAX: usize = 1 << 20;

    /// OPEN name, number: the file or directory `name` names, as a
    /// channel of its own. The request carries, after its reply channel,
    /// one end of a new channel: the service keeps it, and every request
    /// that comes on it is about what was opened (`nodes`). `number` is
    /// a set of `open` flags. Payload: STAT's, of what was opened.
    pub const OPEN: u8 = 16;
    /// What OPEN's `number` may say.
    pub mod open {
        /// To read. Every open allows that; the flag is for clarity.
        pub const READ: u64 = 1;
        /// To change: a file's bytes, or what is in a directory. Refused
        /// on a channel that is for reading only.
        pub const WRITE: u64 = 2;
        /// It must be a directory; without this, a directory is refused
        /// (IsDirectory), as C's open does.
        pub const DIRECTORY: u64 = 4;
        /// Made if it is not there: an empty file. Needs a channel that
        /// may change files.
        pub const CREATE: u64 = 8;
        /// With CREATE: refused (Exists) if it is there already.
        pub const EXCLUSIVE: u64 = 16;
        /// With WRITE: a file is emptied.
        pub const TRUNCATE: u64 = 32;
        /// Every flag there is; others are a bad request.
        pub const ALL: u64 = READ | WRITE | DIRECTORY | CREATE | EXCLUSIVE | TRUNCATE;
    }
    /// Most files and directories one service keeps open at once, for all
    /// its clients together. Each is a channel the service waits on, with
    /// its two request channels and its streams, in one object_wait.
    pub const OPEN_MAX: usize = 200;

    /// The longest STALL: 30 seconds of 100 Hz ticks.
    pub const STALL_MAX: u64 = 3000;

    /// As a WRITE offset: the end of the file.
    pub const APPEND: u64 = u64::MAX;

    /// The longest path, in bytes. (Its length travels in one byte.)
    pub const NAME_MAX: usize = 200;
    /// The longest single name in a path.
    pub const COMPONENT_MAX: usize = 64;
    /// How many directories a path may go through.
    pub const DEPTH_MAX: usize = 16;

    /// What a name in a directory is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Kind {
        File = 0,
        Directory = 1,
    }

    impl Kind {
        pub fn from_u8(byte: u8) -> Kind {
            if byte == 1 { Kind::Directory } else { Kind::File }
        }
    }

    const HEADER: usize = 1 + 1 + 8;
    /// Most bytes one READ returns.
    pub const READ_MAX: usize = MESSAGE_MAX - 1;

    /// Most data one WRITE to `name` can carry.
    pub const fn write_max(name: &str) -> usize {
        MESSAGE_MAX - HEADER - name.len()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Status {
        Ok = 0,
        NotFound = 1,
        /// A name in the path is empty, too long, "." or "..", or has
        /// control characters; or the path is too long or too deep.
        BadName = 2,
        /// Not a request this service understands.
        BadRequest = 3,
        /// Bigger than a file may be.
        TooBig = 4,
        /// No room for another file, or another byte.
        Full = 5,
        /// The disk failed, or what is on it is damaged.
        DiskError = 6,
        /// The request would change something, and came on a channel
        /// that is for reading.
        ReadOnly = 7,
        /// A name on the way to the last one is a file.
        NotDirectory = 8,
        /// A directory, where only a file will do.
        IsDirectory = 9,
        /// Only an empty directory can be removed.
        NotEmpty = 10,
        /// A directory cannot be made where something already is.
        Exists = 11,
        /// A directory cannot be moved to a place inside itself.
        IntoItself = 12,
        /// The service has as many streams (STREAMS_MAX) or open files
        /// (OPEN_MAX) as it keeps.
        Busy = 13,
        /// Never sent by a service: a client's namespace (J62) says it.
        /// The two names are under different mounts - different
        /// directories, perhaps of different services - and one
        /// request can't reach both (C's EXDEV).
        CrossMount = 14,
        /// Never sent by a service either: the name is where something
        /// is mounted in the client's namespace, and can't be removed
        /// or renamed (C's EBUSY).
        MountPoint = 15,
    }

    impl Status {
        pub fn from_u8(byte: u8) -> Status {
            match byte {
                0 => Status::Ok,
                1 => Status::NotFound,
                2 => Status::BadName,
                4 => Status::TooBig,
                5 => Status::Full,
                6 => Status::DiskError,
                7 => Status::ReadOnly,
                8 => Status::NotDirectory,
                9 => Status::IsDirectory,
                10 => Status::NotEmpty,
                11 => Status::Exists,
                12 => Status::IntoItself,
                13 => Status::Busy,
                14 => Status::CrossMount,
                15 => Status::MountPoint,
                _ => Status::BadRequest,
            }
        }

        pub fn describe(self) -> &'static str {
            match self {
                Status::Ok => "ok",
                Status::NotFound => "no such file or directory",
                Status::BadName => "not a valid name",
                Status::BadRequest => "the file service did not understand",
                Status::TooBig => "too big for a file",
                Status::Full => "the file system is full",
                Status::DiskError => "the disk failed, or the file system on it is damaged",
                Status::ReadOnly => "the files are read-only here",
                Status::NotDirectory => "not a directory",
                Status::IsDirectory => "is a directory",
                Status::NotEmpty => "the directory is not empty",
                Status::Exists => "there is already something with that name",
                Status::IntoItself => "a directory cannot be moved into itself",
                Status::Busy => "the file service has too many streams or files open",
                Status::CrossMount => "not under the same mount",
                Status::MountPoint => "something is mounted there",
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Request<'a> {
        pub op: u8,
        pub name: &'a str,
        pub number: u64,
        pub data: &'a [u8],
    }

    impl<'a> Request<'a> {
        pub fn new(op: u8, name: &'a str, number: u64) -> Request<'a> {
            Request { op, name, number, data: &[] }
        }

        /// Lays the request out in `out`; returns its length, or None if
        /// it does not fit in a message.
        pub fn encode(&self, out: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
            let length = HEADER + self.name.len() + self.data.len();
            if self.name.len() > NAME_MAX || length > MESSAGE_MAX {
                return None;
            }
            out[0] = self.op;
            out[1] = self.name.len() as u8;
            let mut at = 2;
            for part in [self.name.as_bytes(), &self.number.to_le_bytes(), self.data] {
                out[at..at + part.len()].copy_from_slice(part);
                at += part.len();
            }
            Some(length)
        }

        /// Reads a request; None if it is malformed. The name is only
        /// checked to be UTF-8 - whether it is a good name is for the
        /// file table to say.
        pub fn decode(bytes: &'a [u8]) -> Option<Request<'a>> {
            let (&op, rest) = bytes.split_first()?;
            let (&name_length, rest) = rest.split_first()?;
            let (name, rest) = rest.split_at_checked(name_length as usize)?;
            let (number, data) = rest.split_at_checked(8)?;
            Some(Request {
                op,
                name: core::str::from_utf8(name).ok()?,
                number: u64::from_le_bytes(number.try_into().ok()?),
                data,
            })
        }
    }

    /// The entries of a LIST reply's payload.
    pub fn entries(payload: &[u8]) -> impl Iterator<Item = (&str, Kind, u64)> {
        let mut rest = payload;
        core::iter::from_fn(move || {
            let (&length, tail) = rest.split_first()?;
            let (name, tail) = tail.split_at_checked(length as usize)?;
            let (&kind, tail) = tail.split_first()?;
            let (size, tail) = tail.split_at_checked(8)?;
            rest = tail;
            let name = core::str::from_utf8(name).unwrap_or("?");
            Some((name, Kind::from_u8(kind), u64::from_le_bytes(size.try_into().ok()?)))
        })
    }

    /// A u64 payload (WRITE; the size in STAT's).
    pub fn number(payload: &[u8]) -> Option<u64> {
        Some(u64::from_le_bytes(payload.get(..8)?.try_into().ok()?))
    }

    /// A STAT payload: what the path names, and its size.
    pub fn stat(payload: &[u8]) -> Option<(Kind, u64)> {
        Some((Kind::from_u8(*payload.get(8)?), number(payload)?))
    }

    /// A STAT payload's change stamp; 0 from a service that sends none.
    pub fn stamp(payload: &[u8]) -> u64 {
        payload.get(9..17).map_or(0, |bytes| u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    /// A STAT payload's time, in nanoseconds since 1970; 0 if not known
    /// or from a service that sends none.
    pub fn time(payload: &[u8]) -> u64 {
        payload.get(17..25).map_or(0, |bytes| u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }
}


use protocol::{Kind, Request, Status, COMPONENT_MAX, DEPTH_MAX, MESSAGE_MAX, NAME_MAX};

/// Whether `name` may be one name in a path, in any store.
pub fn valid_name(name: &str) -> bool {
    (1..=COMPONENT_MAX).contains(&name.len())
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c == '/' || c.is_control())
}

/// The names in a path, from the root down. Slashes at either end, and
/// doubled ones, mean nothing: "/a/", "a" and "//a" are the same path,
/// and a path with no names in it is the root.
pub fn names(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|name| !name.is_empty())
}

/// Whether `path` may name a directory: good names, not too many, and
/// possibly none at all - the root.
pub fn valid_directory(path: &str) -> bool {
    path.len() <= NAME_MAX && names(path).count() <= DEPTH_MAX + 1 && names(path).all(valid_name)
}

/// Whether `path` may name a file: as a directory, but not the root.
pub fn valid_path(path: &str) -> bool {
    valid_directory(path) && names(path).next().is_some()
}

/// Wherever files are kept. Each method is one request's worth of work;
/// a refused request must change nothing.
pub trait Store {
    /// Calls `each` with the name, kind and size of everything in
    /// `directory` ("" is the root) from the `from`th on, in name order,
    /// until it returns false.
    fn list(
        &mut self,
        directory: &str,
        from: usize,
        each: &mut dyn FnMut(&str, Kind, u64) -> bool,
    ) -> Result<(), Status>;
    /// What `path` names, and its size (0 for a directory).
    fn stat(&mut self, path: &str) -> Result<(Kind, u64), Status>;
    /// When what `path` names last changed: a number that only grows
    /// (protocol::STAT). 0 for a directory.
    fn stamp(&mut self, path: &str) -> Result<u64, Status>;
    /// When it last changed, in nanoseconds since 1970: 0 if the store
    /// does not know (protocol::STAT).
    fn time(&mut self, _path: &str) -> Result<u64, Status> {
        Ok(0)
    }
    /// Marks the file changed now, changing nothing in it.
    fn touch(&mut self, name: &str) -> Result<(), Status>;
    /// The size of the file `path` names.
    fn size(&mut self, path: &str) -> Result<u64, Status> {
        match self.stat(path)? {
            (Kind::File, size) => Ok(size),
            (Kind::Directory, _) => Err(Status::IsDirectory),
        }
    }
    /// Makes an empty directory, in a directory that exists.
    fn make_directory(&mut self, path: &str) -> Result<(), Status>;
    /// Bytes from `offset` into `out`; how many. Past the end: none.
    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status>;
    /// Writes at `offset` (or protocol::APPEND), creating the file if
    /// needed and filling any gap with zeros; returns the new size.
    fn write(&mut self, name: &str, offset: u64, data: &[u8]) -> Result<u64, Status>;
    /// Sets the size, creating the file if needed.
    fn truncate(&mut self, name: &str, length: u64) -> Result<(), Status>;
    /// Removes a file, or a directory with nothing in it.
    fn remove(&mut self, name: &str) -> Result<(), Status>;
    /// Gives what `from` names the path `to` instead - a file, or a
    /// directory and all that is in it. `to` must name nothing yet, in
    /// a directory that exists, and not be inside `from`. All of it
    /// happens or none of it.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), Status>;
}

/// Answers one request message: writes the reply into `reply` and
/// returns its length. Never fails - a bad request gets a reply saying
/// so.
pub fn serve(store: &mut impl Store, access: Access, message: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> usize {
    let (status, length) = match answer(store, access, message, &mut reply[1..]) {
        Ok(length) => (Status::Ok, length),
        Err(status) => (status, 0),
    };
    reply[0] = status as u8;
    1 + length
}

/// The answer to a READ_INTO request, whose memory object the service
/// has mapped as `into` (all of it): the file's bytes, into the part
/// the request names, and the reply, into `reply`. Returns the reply's length; None if the message is not a
/// READ_INTO (and so is no business of this function). Reading needs
/// no permission beyond either channel's.
pub fn serve_into(store: &mut impl Store, message: &[u8], into: &mut [u8], reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
    let request = Request::decode(message).filter(|request| request.op == protocol::READ_INTO)?;
    Some(serve_into_request(store, &request, into, reply))
}

/// `serve_into`, for a request already read, whose name is a path from
/// the root (`nodes` works it out).
pub(crate) fn serve_into_request(store: &mut impl Store, request: &Request, into: &mut [u8], reply: &mut [u8; MESSAGE_MAX]) -> usize {
    // The window: from the offset, at most READ_INTO_MAX, and none of
    // it past the object's end.
    let start = usize::try_from(request.number).unwrap_or(usize::MAX).min(into.len());
    let end = start + (into.len() - start).min(protocol::READ_INTO_MAX);
    match store.read(request.name, request.number, &mut into[start..end]) {
        Ok(n) => {
            reply[0] = Status::Ok as u8;
            reply[1..9].copy_from_slice(&(n as u64).to_le_bytes());
            9
        }
        Err(status) => {
            reply[0] = status as u8;
            1
        }
    }
}

/// The answer to a WRITE_FROM request, whose memory object the service
/// has mapped as `from` (all of it, to read): the bytes the request
/// names, written into the file, and the reply into `reply`. Returns
/// the reply's length; None if the message is not a WRITE_FROM. A
/// change: refused on the read-only channel.
pub fn serve_from(store: &mut impl Store, access: Access, message: &[u8], from: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
    let request = Request::decode(message).filter(|request| request.op == protocol::WRITE_FROM)?;
    Some(serve_from_request(store, access, &request, from, reply))
}

/// `serve_from`, for a request already read, its name a path from the
/// root.
pub(crate) fn serve_from_request(store: &mut impl Store, access: Access, request: &Request, from: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> usize {
    let written = (|| {
        if access == Access::ReadOnly {
            return Err(Status::ReadOnly);
        }
        let start = request.data.get(..8).and_then(protocol::number).ok_or(Status::BadRequest)?;
        let length = request.data.get(8..16).and_then(protocol::number).ok_or(Status::BadRequest)?;
        let start = usize::try_from(start).map_err(|_| Status::BadRequest)?;
        let length = usize::try_from(length).map_err(|_| Status::BadRequest)?;
        if length > protocol::WRITE_FROM_MAX {
            return Err(Status::BadRequest);
        }
        let bytes = start.checked_add(length).and_then(|end| from.get(start..end)).ok_or(Status::BadRequest)?;
        store.write(request.name, request.number, bytes)
    })();
    match written {
        Ok(size) => {
            reply[0] = Status::Ok as u8;
            reply[1..9].copy_from_slice(&size.to_le_bytes());
            9
        }
        Err(status) => {
            reply[0] = status as u8;
            1
        }
    }
}

/// The bytes of a message on an output stream, if it is a DATA message
/// with any: the rest is not part of the stream.
pub fn stream_data(message: &[u8]) -> Option<&[u8]> {
    match message.split_first() {
        Some((&abi::stream::DATA, bytes)) if !bytes.is_empty() => Some(bytes),
        _ => None,
    }
}

/// A file being written or read as a stream (protocol::STREAM_OUT,
/// STREAM_IN): what the service keeps for it between messages. The
/// channel it travels on is the service's business, not this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stream {
    /// Bytes that arrive are added to the end of `path`.
    Out { path: alloc::string::String },
    /// `path`'s bytes go out from `offset` on.
    In { path: alloc::string::String, offset: u64 },
}

/// Opens a stream for a STREAM_OUT or STREAM_IN request: the file made
/// ready (emptied, made), and what to keep. Writes the reply into
/// `reply` and returns its length, with the stream if the reply is Ok.
pub fn open_stream(store: &mut impl Store, access: Access, message: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> (usize, Option<Stream>) {
    match Request::decode(message) {
        Some(request) => open_stream_request(store, access, &request, reply),
        None => {
            reply[0] = Status::BadRequest as u8;
            (1, None)
        }
    }
}

/// `open_stream`, for a request already read, its name a path from the
/// root.
pub(crate) fn open_stream_request(store: &mut impl Store, access: Access, request: &Request, reply: &mut [u8; MESSAGE_MAX]) -> (usize, Option<Stream>) {
    let opened = (|| {
        let path = alloc::string::String::from(request.name);
        match (request.op, request.number) {
            (protocol::STREAM_OUT, _) if access == Access::ReadOnly => Err(Status::ReadOnly),
            (protocol::STREAM_OUT, keep @ (0 | 1)) => {
                match store.stat(request.name) {
                    Ok((Kind::Directory, _)) => return Err(Status::IsDirectory),
                    Ok((Kind::File, _)) if keep == 0 => store.truncate(request.name, 0)?,
                    Ok((Kind::File, _)) => {}
                    Err(Status::NotFound) => {
                        store.write(request.name, protocol::APPEND, &[])?;
                    }
                    Err(status) => return Err(status),
                }
                Ok(Stream::Out { path })
            }
            (protocol::STREAM_IN, offset) => match store.stat(request.name)? {
                (Kind::Directory, _) => Err(Status::IsDirectory),
                (Kind::File, _) => Ok(Stream::In { path, offset }),
            },
            _ => Err(Status::BadRequest),
        }
    })();
    match opened {
        Ok(stream) => {
            reply[0] = Status::Ok as u8;
            (1, Some(stream))
        }
        Err(status) => {
            reply[0] = status as u8;
            (1, None)
        }
    }
}

impl Stream {
    /// Whether a request message is one that opens a stream.
    pub fn is_opened_by(message: &[u8]) -> bool {
        matches!(message.first(), Some(&(protocol::STREAM_OUT | protocol::STREAM_IN)))
    }

    /// A message that arrived on an output stream: a DATA message's
    /// bytes are added to the file; anything else is not part of the
    /// stream and is ignored. An error means the file cannot take
    /// more, and the stream should be closed.
    pub fn take(&mut self, store: &mut impl Store, message: &[u8]) -> Result<(), Status> {
        match stream_data(message) {
            Some(bytes) => self.add(store, bytes),
            None => Ok(()),
        }
    }

    /// Bytes of an output stream - the data of one message or of many
    /// gathered - added to the end of the file: one change.
    pub fn add(&mut self, store: &mut impl Store, bytes: &[u8]) -> Result<(), Status> {
        match self {
            Stream::Out { path } if !bytes.is_empty() => store.write(path, protocol::APPEND, bytes).map(|_| ()),
            _ => Ok(()),
        }
    }

    /// The next DATA message of an input stream, written into `out`:
    /// its length, or 0 at the end of the file. It counts as sent only
    /// once `sent` says so, so that a message the channel had no room
    /// for is made again.
    pub fn next(&mut self, store: &mut impl Store, out: &mut [u8; MESSAGE_MAX]) -> Result<usize, Status> {
        let Stream::In { path, offset } = self else { return Ok(0) };
        out[0] = abi::stream::DATA;
        let n = store.read(path, *offset, &mut out[1..])?;
        Ok(if n == 0 { 0 } else { 1 + n })
    }

    /// The message `next` made was sent.
    pub fn sent(&mut self, length: usize) {
        if let Stream::In { offset, .. } = self {
            *offset += length.saturating_sub(1) as u64;
        }
    }
}

/// What the channel a request came on allows. The service has one
/// channel for each: which one a client holds is its permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadWrite,
    ReadOnly,
}

/// The payload of the reply, written into `out`.
fn answer(store: &mut impl Store, access: Access, message: &[u8], out: &mut [u8]) -> Result<usize, Status> {
    let request = Request::decode(message).ok_or(Status::BadRequest)?;
    answer_request(store, access, &request, out, None)
}

/// The payload of the reply to a request already read, its name a path
/// from the root. `hidden`, if given, is a name LIST leaves out of the
/// directory it lists, with the place it is at: the entry numbers a
/// client gives are counted without it (`nodes`, for its REMOVED).
pub(crate) fn answer_request(store: &mut impl Store, access: Access, request: &Request, out: &mut [u8], hidden: Option<(&str, usize)>) -> Result<usize, Status> {
    let request = *request;
    // Checked here, once, before anything is done - not in each
    // operation, where a new one could forget.
    let changes = matches!(
        request.op,
        protocol::WRITE
            | protocol::TRUNCATE
            | protocol::REMOVE
            | protocol::STALL
            | protocol::MKDIR
            | protocol::RENAME
            | protocol::TOUCH
    );
    if changes && access == Access::ReadOnly {
        return Err(Status::ReadOnly);
    }
    let name = request.name;
    let put_number = |out: &mut [u8], n: u64| {
        out[..8].copy_from_slice(&n.to_le_bytes());
        8
    };
    match request.op {
        protocol::LIST => {
            let mut at = 0;
            let mut from = usize::try_from(request.number).unwrap_or(usize::MAX);
            if let Some((_, place)) = hidden {
                if from >= place {
                    from = from.saturating_add(1);
                }
            }
            store.list(name, from, &mut |name, kind, size| {
                if hidden.is_some_and(|(hidden, _)| hidden == name) {
                    return true;
                }
                let entry = 1 + name.len() + 1 + 8;
                if at + entry > out.len() {
                    return false;
                }
                out[at] = name.len() as u8;
                out[at + 1..at + 1 + name.len()].copy_from_slice(name.as_bytes());
                out[at + 1 + name.len()] = kind as u8;
                out[at + 2 + name.len()..at + entry].copy_from_slice(&size.to_le_bytes());
                at += entry;
                true
            })?;
            Ok(at)
        }
        protocol::READ => store.read(name, request.number, out),
        protocol::WRITE => Ok(put_number(out, store.write(name, request.number, request.data)?)),
        protocol::TRUNCATE => store.truncate(name, request.number).map(|()| 0),
        protocol::REMOVE => store.remove(name).map(|()| 0),
        protocol::STAT => {
            let (kind, size) = store.stat(name)?;
            let stamp = store.stamp(name)?;
            let time = store.time(name)?;
            out[8] = kind as u8;
            out[9..17].copy_from_slice(&stamp.to_le_bytes());
            out[17..25].copy_from_slice(&time.to_le_bytes());
            Ok(put_number(out, size) + 1 + 8 + 8)
        }
        protocol::TOUCH => store.touch(name).map(|()| 0),
        protocol::MKDIR => store.make_directory(name).map(|()| 0),
        protocol::RENAME => {
            // The second path travels where a WRITE's data does.
            let to = core::str::from_utf8(request.data).map_err(|_| Status::BadRequest)?;
            store.rename(name, to).map(|()| 0)
        }
        protocol::STALL if request.number <= protocol::STALL_MAX => Ok(0),
        protocol::ACCESS => {
            out[0] = u8::from(access == Access::ReadOnly);
            Ok(1)
        }
        _ => Err(Status::BadRequest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Request, STAT, TOUCH, WRITE};
    use taajfs::{format_volume, MemDevice};

    fn ask(store: &mut impl Store, access: Access, op: u8, name: &str, data: &[u8]) -> (Status, Vec<u8>) {
        let mut message = [0u8; MESSAGE_MAX];
        let length = Request { op, name, number: 0, data }.encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let n = serve(store, access, &message[..length], &mut reply);
        (Status::from_u8(reply[0]), reply[1..n].to_vec())
    }

    fn stamp(store: &mut impl Store, name: &str) -> u64 {
        let (status, payload) = ask(store, Access::ReadOnly, STAT, name, &[]);
        assert_eq!(status, Status::Ok, "{name}");
        assert_eq!(payload.len(), 25, "size, kind, stamp, time");
        protocol::stamp(&payload)
    }

    /// The same for every store: what changed later has the higher
    /// stamp, a touch is a change, and a read-only channel cannot touch.
    fn stamps_order_changes(store: &mut impl Store) {
        assert_eq!(ask(store, Access::ReadWrite, WRITE, "a.c", b"int a;").0, Status::Ok);
        assert_eq!(ask(store, Access::ReadWrite, WRITE, "a.o", b"object").0, Status::Ok);
        let (c, o) = (stamp(store, "a.c"), stamp(store, "a.o"));
        assert!(c > 0 && o > c, "{c} {o}");
        assert_eq!(ask(store, Access::ReadWrite, TOUCH, "a.c", &[]).0, Status::Ok);
        assert!(stamp(store, "a.c") > o);
        assert_eq!(ask(store, Access::ReadOnly, TOUCH, "a.o", &[]).0, Status::ReadOnly);
        assert_eq!(ask(store, Access::ReadWrite, TOUCH, "none", &[]).0, Status::NotFound);
        assert_eq!(stamp(store, ""), 0, "a directory has none");
        let (_, payload) = ask(store, Access::ReadOnly, STAT, "a.o", &[]);
        assert_eq!(protocol::stat(&payload), Some((Kind::File, 6)), "the old fields stay where they were");
    }

    #[test]
    fn stamps_order_changes_in_memory() {
        stamps_order_changes(&mut RamStore::new());
    }

    #[test]
    fn stamps_order_changes_on_disk_and_survive_a_remount() {
        let mut device = MemDevice::new(256);
        format_volume(&mut device, 256).unwrap();
        let mut store = DiskStore::mount(device).unwrap();
        stamps_order_changes(&mut store);
        let before = stamp(&mut store, "a.c");
        let mut store = DiskStore::mount(store.into_device()).unwrap();
        assert_eq!(stamp(&mut store, "a.c"), before);
    }

    fn open(store: &mut impl Store, access: Access, op: u8, name: &str, number: u64) -> (Status, Option<Stream>) {
        let mut message = [0u8; MESSAGE_MAX];
        let length = Request { op, name, number, data: &[] }.encode(&mut message).expect("fits");
        assert!(Stream::is_opened_by(&message[..length]));
        let mut reply = [0u8; MESSAGE_MAX];
        let (n, stream) = open_stream(store, access, &message[..length], &mut reply);
        assert_eq!(n, 1);
        (Status::from_u8(reply[0]), stream)
    }

    fn contents(store: &mut impl Store, name: &str) -> Vec<u8> {
        let (status, payload) = ask(store, Access::ReadOnly, protocol::READ, name, &[]);
        assert_eq!(status, Status::Ok, "{name}");
        payload
    }

    /// The same for every store: `>` empties and adds, `>>` adds, `<`
    /// gives the bytes back in pieces and then says the end; what is
    /// not allowed is refused at the opening.
    fn streams_write_and_read_files(store: &mut impl Store) {
        use protocol::{STREAM_IN, STREAM_OUT};
        assert_eq!(ask(store, Access::ReadWrite, WRITE, "out", b"old contents").0, Status::Ok);
        let (status, stream) = open(store, Access::ReadWrite, STREAM_OUT, "out", 0);
        assert_eq!(status, Status::Ok);
        let mut stream = stream.expect("a stream");
        assert!(contents(store, "out").is_empty(), "> empties the file at once");
        let data = |bytes: &[u8]| [&[abi::stream::DATA][..], bytes].concat();
        for piece in [data(b"hello "), data(b"world\n"), vec![abi::stream::WANT], data(b""), vec![]] {
            stream.take(store, &piece).unwrap();
        }
        assert_eq!(contents(store, "out"), b"hello world\n", "DATA added; WANT, empty and nothing ignored");
        let (status, stream) = open(store, Access::ReadWrite, STREAM_OUT, "out", 1);
        assert_eq!(status, Status::Ok);
        stream.unwrap().take(store, &data(b"more\n")).unwrap();
        assert_eq!(contents(store, "out"), b"hello world\nmore\n", ">> keeps what was there");
        let (status, stream) = open(store, Access::ReadWrite, STREAM_OUT, "made", 1);
        assert!(status == Status::Ok && stream.is_some());
        assert!(contents(store, "made").is_empty(), "a missing file is made");

        // Reading: big enough for three messages.
        let big: Vec<u8> = (0..2500u32).map(|n| b'a' + (n % 26) as u8).collect();
        store.write("big", 0, &big).unwrap();
        let (status, stream) = open(store, Access::ReadOnly, STREAM_IN, "big", 0);
        assert_eq!(status, Status::Ok, "reading is allowed on the read-only channel");
        let mut stream = stream.unwrap();
        let mut got = Vec::new();
        let mut message = [0u8; MESSAGE_MAX];
        let mut messages = 0;
        loop {
            let n = stream.next(store, &mut message).unwrap();
            if n == 0 {
                break;
            }
            assert_eq!(message[0], abi::stream::DATA);
            // A message not sent is made again, the same.
            let again = stream.next(store, &mut [0u8; MESSAGE_MAX]).unwrap();
            assert_eq!(again, n);
            got.extend_from_slice(&message[1..n]);
            stream.sent(n);
            messages += 1;
        }
        assert_eq!(got, big);
        assert_eq!(messages, 3);
        let (_, stream) = open(store, Access::ReadOnly, STREAM_IN, "big", 2490);
        let n = stream.unwrap().next(store, &mut message).unwrap();
        assert_eq!(&message[1..n], &big[2490..], "from an offset");

        assert_eq!(open(store, Access::ReadOnly, STREAM_OUT, "out", 0).0, Status::ReadOnly);
        assert_eq!(contents(store, "out"), b"hello world\nmore\n", "a refused > empties nothing");
        assert_eq!(open(store, Access::ReadWrite, STREAM_OUT, "out", 2).0, Status::BadRequest);
        assert_eq!(open(store, Access::ReadOnly, STREAM_IN, "none", 0).0, Status::NotFound);
        assert_eq!(ask(store, Access::ReadWrite, protocol::MKDIR, "dir", &[]).0, Status::Ok);
        assert_eq!(open(store, Access::ReadWrite, STREAM_OUT, "dir", 0).0, Status::IsDirectory);
        assert_eq!(open(store, Access::ReadOnly, STREAM_IN, "dir", 0).0, Status::IsDirectory);
        assert_eq!(open(store, Access::ReadWrite, STREAM_OUT, "none/x", 0).0, Status::NotFound);
        // Not a request that opens a stream: serve's business.
        assert_eq!(ask(store, Access::ReadWrite, protocol::STREAM_OUT, "out", &[]).0, Status::BadRequest);
    }

    #[test]
    fn streams_write_and_read_files_in_memory() {
        streams_write_and_read_files(&mut RamStore::new());
    }

    #[test]
    fn streams_write_and_read_files_on_disk() {
        let mut device = MemDevice::new(256);
        format_volume(&mut device, 256).unwrap();
        streams_write_and_read_files(&mut DiskStore::mount(device).unwrap());
    }

    fn write_from(store: &mut impl Store, access: Access, name: &str, number: u64, object: &[u8], start: u64, length: u64) -> (Status, Option<u64>) {
        let mut message = [0u8; MESSAGE_MAX];
        let data = [start.to_le_bytes(), length.to_le_bytes()].concat();
        let n = Request { op: protocol::WRITE_FROM, name, number, data: &data }.encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let length = serve_from(store, access, &message[..n], object, &mut reply).expect("a WRITE_FROM");
        (Status::from_u8(reply[0]), protocol::number(&reply[1..length]))
    }

    /// The same for every store: what the request names of the object
    /// goes into the file, at the offset or the end; anything outside
    /// the object, too much, or on the read-only channel, is refused
    /// and changes nothing.
    fn writes_from_memory(store: &mut impl Store) {
        let object: Vec<u8> = (0..3 * 4096 + 100u32).map(|n| (n % 251) as u8).collect();
        assert_eq!(write_from(store, Access::ReadWrite, "f", 0, &object, 0, object.len() as u64), (Status::Ok, Some(object.len() as u64)));
        assert_eq!(contents_all(store, "f"), object, "one request, many blocks");
        assert_eq!(write_from(store, Access::ReadWrite, "f", protocol::APPEND, &object, 10, 5), (Status::Ok, Some(object.len() as u64 + 5)));
        assert_eq!(write_from(store, Access::ReadWrite, "f", 1, &object, 0, 2), (Status::Ok, Some(object.len() as u64 + 5)));
        let mut expected = object.clone();
        expected.extend_from_slice(&object[10..15]);
        expected[1..3].copy_from_slice(&object[0..2]);
        assert_eq!(contents_all(store, "f"), expected);
        let size = object.len() as u64;
        for (start, length, why) in [(size, 1, "past the object"), (0, size + 1, "longer than it"), (u64::MAX, 2, "wrapping round")] {
            assert_eq!(write_from(store, Access::ReadWrite, "f", 0, &object, start, length).0, Status::BadRequest, "{why}");
        }
        let big = vec![0u8; protocol::WRITE_FROM_MAX + 1];
        assert_eq!(write_from(store, Access::ReadWrite, "g", 0, &big, 0, big.len() as u64).0, Status::BadRequest, "more than WRITE_FROM_MAX");
        assert_eq!(write_from(store, Access::ReadOnly, "f", 0, &object, 0, 4).0, Status::ReadOnly);
        assert_eq!(contents_all(store, "f"), expected, "refused: nothing changed");
        assert_eq!(ask(store, Access::ReadOnly, STAT, "g", &[]).0, Status::NotFound);
        let mut message = [0u8; MESSAGE_MAX];
        let n = Request::new(protocol::READ, "f", 0).encode(&mut message).unwrap();
        assert!(serve_from(store, Access::ReadWrite, &message[..n], &object, &mut [0u8; MESSAGE_MAX]).is_none(), "not a WRITE_FROM");
    }

    fn contents_all(store: &mut impl Store, name: &str) -> Vec<u8> {
        let size = store.size(name).unwrap() as usize;
        let mut out = vec![0u8; size];
        let mut at = 0;
        while at < size {
            at += store.read(name, at as u64, &mut out[at..]).unwrap();
        }
        out
    }

    #[test]
    fn writes_from_memory_in_memory() {
        writes_from_memory(&mut RamStore::new());
    }

    #[test]
    fn writes_from_memory_on_disk() {
        let mut device = MemDevice::new(256);
        format_volume(&mut device, 256).unwrap();
        writes_from_memory(&mut DiskStore::mount(device).unwrap());
    }

    #[test]
    fn many_stream_messages_are_one_change() {
        let mut device = MemDevice::new(256);
        format_volume(&mut device, 256).unwrap();
        let mut store = DiskStore::mount(device).unwrap();
        let (_, stream) = open(&mut store, Access::ReadWrite, protocol::STREAM_OUT, "out", 0);
        let mut stream = stream.unwrap();
        let before = stamp(&mut store, "out");
        let messages: Vec<Vec<u8>> = (0..5).map(|n| [&[abi::stream::DATA][..], format!("line {n}\n").as_bytes()].concat()).collect();
        let gathered: Vec<u8> = messages.iter().filter_map(|m| stream_data(m)).flatten().copied().collect();
        stream.add(&mut store, &gathered).unwrap();
        assert_eq!(contents(&mut store, "out"), b"line 0\nline 1\nline 2\nline 3\nline 4\n");
        assert_eq!(stamp(&mut store, "out"), before + 1, "one commit");
        assert_eq!(stream_data(&[abi::stream::WANT]), None);
        assert_eq!(stream_data(&[abi::stream::DATA]), None);
    }

    #[test]
    fn busy_is_a_status_of_its_own() {
        assert_eq!(Status::from_u8(Status::Busy as u8), Status::Busy);
        assert!(Status::Busy.describe().contains("streams"));
    }
}
