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
//! root down - "docs/notes/today". Every request carries the whole path;
//! the service keeps nothing about a client between requests, so there
//! is no "current directory" here. That belongs to whoever types paths.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod disk;
pub mod path;
pub mod ram;

pub use disk::DiskStore;
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
    /// Payload: the size, u64 LE, then the kind, u8.
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
    /// directory with everything in it. Nothing is copied, and nothing
    /// is replaced: a new path that already names something is refused.
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
    // The window: from the offset, at most READ_INTO_MAX, and none of
    // it past the object's end.
    let start = usize::try_from(request.number).unwrap_or(usize::MAX).min(into.len());
    let end = start + (into.len() - start).min(protocol::READ_INTO_MAX);
    match store.read(request.name, request.number, &mut into[start..end]) {
        Ok(n) => {
            reply[0] = Status::Ok as u8;
            reply[1..9].copy_from_slice(&(n as u64).to_le_bytes());
            Some(9)
        }
        Err(status) => {
            reply[0] = status as u8;
            Some(1)
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
    // Checked here, once, before anything is done - not in each
    // operation, where a new one could forget.
    let changes = matches!(
        request.op,
        protocol::WRITE | protocol::TRUNCATE | protocol::REMOVE | protocol::STALL | protocol::MKDIR | protocol::RENAME
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
            let from = usize::try_from(request.number).unwrap_or(usize::MAX);
            store.list(name, from, &mut |name, kind, size| {
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
            out[8] = kind as u8;
            Ok(put_number(out, size) + 1)
        }
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
