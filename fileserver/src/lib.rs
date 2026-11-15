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
//! a channel and `serve`. The namespace is flat for now: names, no
//! directories.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod disk;
pub mod ram;

pub use disk::DiskStore;
pub use ram::RamStore;

/// The protocol: one request per message, one reply per request.
///
/// A request:   [op: u8] [name length: u8] [name] [number: u64 LE] [data]
/// A reply:     [status: u8] [payload]
///
/// `number` is an offset, a length or an index, depending on `op`. Each
/// request carries a channel end for its reply as its one handle, so a
/// client waiting for an answer from a service that crashed hears
/// PeerClosed - the reply end died with the service - and does not wait
/// forever.
pub mod protocol {
    pub const MESSAGE_MAX: usize = abi::MESSAGE_MAX as usize;

    /// Names files from `number` on (an index): payload is entries of
    /// [name length: u8] [name] [size: u64 LE], as many as fit. An empty
    /// payload means there are no more.
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
    /// Payload: the size, u64 LE.
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

    /// The longest STALL: 30 seconds of 100 Hz ticks.
    pub const STALL_MAX: u64 = 3000;

    /// As a WRITE offset: the end of the file.
    pub const APPEND: u64 = u64::MAX;

    pub const NAME_MAX: usize = 64;
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
        /// Empty, too long, or with '/' or control characters.
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
                _ => Status::BadRequest,
            }
        }

        pub fn describe(self) -> &'static str {
            match self {
                Status::Ok => "ok",
                Status::NotFound => "no such file",
                Status::BadName => "not a valid file name",
                Status::BadRequest => "the file service did not understand",
                Status::TooBig => "too big for a file",
                Status::Full => "the file system is full",
                Status::DiskError => "the disk failed, or the file system on it is damaged",
                Status::ReadOnly => "the files are read-only here",
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
    pub fn entries(payload: &[u8]) -> impl Iterator<Item = (&str, u64)> {
        let mut rest = payload;
        core::iter::from_fn(move || {
            let (&length, tail) = rest.split_first()?;
            let (name, tail) = tail.split_at_checked(length as usize)?;
            let (size, tail) = tail.split_at_checked(8)?;
            rest = tail;
            Some((core::str::from_utf8(name).unwrap_or("?"), u64::from_le_bytes(size.try_into().ok()?)))
        })
    }

    /// A u64 payload (WRITE, STAT).
    pub fn number(payload: &[u8]) -> Option<u64> {
        Some(u64::from_le_bytes(payload.get(..8)?.try_into().ok()?))
    }
}


use protocol::{Request, Status, MESSAGE_MAX, NAME_MAX};

/// Whether `name` may name a file, in any store.
pub fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c == '/' || c.is_control())
}

/// Wherever files are kept. Each method is one request's worth of work;
/// a refused request must change nothing.
pub trait Store {
    /// Calls `each` with the name and size of every file from the
    /// `from`th on, in name order, until it returns false.
    fn list(&mut self, from: usize, each: &mut dyn FnMut(&str, u64) -> bool) -> Result<(), Status>;
    fn size(&mut self, name: &str) -> Result<u64, Status>;
    /// Bytes from `offset` into `out`; how many. Past the end: none.
    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status>;
    /// Writes at `offset` (or protocol::APPEND), creating the file if
    /// needed and filling any gap with zeros; returns the new size.
    fn write(&mut self, name: &str, offset: u64, data: &[u8]) -> Result<u64, Status>;
    /// Sets the size, creating the file if needed.
    fn truncate(&mut self, name: &str, length: u64) -> Result<(), Status>;
    fn remove(&mut self, name: &str) -> Result<(), Status>;
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
    let changes = matches!(request.op, protocol::WRITE | protocol::TRUNCATE | protocol::REMOVE | protocol::STALL);
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
            store.list(from, &mut |name, size| {
                let entry = 1 + name.len() + 8;
                if at + entry > out.len() {
                    return false;
                }
                out[at] = name.len() as u8;
                out[at + 1..at + 1 + name.len()].copy_from_slice(name.as_bytes());
                out[at + 1 + name.len()..at + entry].copy_from_slice(&size.to_le_bytes());
                at += entry;
                true
            })?;
            Ok(at)
        }
        protocol::READ => store.read(name, request.number, out),
        protocol::WRITE => Ok(put_number(out, store.write(name, request.number, request.data)?)),
        protocol::TRUNCATE => store.truncate(name, request.number).map(|()| 0),
        protocol::REMOVE => store.remove(name).map(|()| 0),
        protocol::STAT => Ok(put_number(out, store.size(name)?)),
        protocol::STALL if request.number <= protocol::STALL_MAX => Ok(0),
        protocol::ACCESS => {
            out[0] = u8::from(access == Access::ReadOnly);
            Ok(1)
        }
        _ => Err(Status::BadRequest),
    }
}
