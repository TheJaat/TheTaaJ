//! ramfs: files kept in memory, and the messages that reach them.
//!
//! Two parts, both pure logic, tested here on the host:
//!
//!   - the protocol: how a request and its reply are laid out in a
//!     channel message. Clients and the `fs` service both use it, so
//!     they cannot disagree about it.
//!   - `Files`: the file table the `fs` service keeps - names to
//!     contents - and `serve`, which turns one request message into one
//!     reply message.
//!
//! The service itself (user/src/bin/fs.rs) only moves messages between
//! a channel and `serve`.
//!
//! The namespace is flat for now - names, no directories - and nothing
//! survives a reboot or a restart of the service: it is a RAM disk. It
//! is the shape of the file service that matters at this stage. A disk
//! driver and a file system on the disk come next, behind the same
//! protocol.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

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
    }

    impl Status {
        pub fn from_u8(byte: u8) -> Status {
            match byte {
                0 => Status::Ok,
                1 => Status::NotFound,
                2 => Status::BadName,
                4 => Status::TooBig,
                5 => Status::Full,
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

/// Limits, so that one client cannot take all of the service's memory.
pub const FILE_MAX: usize = 1 << 20;
pub const FILES_MAX: usize = 256;
pub const BYTES_MAX: usize = 8 << 20;

#[derive(Default)]
pub struct Files {
    files: BTreeMap<String, Vec<u8>>,
    /// The sum of every file's size.
    bytes: usize,
}

/// Whether `name` may name a file.
pub fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c == '/' || c.is_control())
}

impl Files {
    pub fn new() -> Files {
        Files::default()
    }

    pub fn count(&self) -> usize {
        self.files.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn size(&self, name: &str) -> Result<u64, Status> {
        self.files.get(name).map(|f| f.len() as u64).ok_or(Status::NotFound)
    }

    pub fn read(&self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let file = self.files.get(name).ok_or(Status::NotFound)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(file.len());
        let n = out.len().min(file.len() - start);
        out[..n].copy_from_slice(&file[start..start + n]);
        Ok(n)
    }

    /// Sets the size of `name` - creating it if needed, growing it with
    /// zeros - after checking every limit, so a refused request changes
    /// nothing. A failed allocation is reported, not a crash of the
    /// service.
    fn resize(&mut self, name: &str, size: usize) -> Result<(), Status> {
        if !valid_name(name) {
            return Err(Status::BadName);
        }
        if size > FILE_MAX {
            return Err(Status::TooBig);
        }
        let old = self.files.get(name).map(Vec::len);
        if old.is_none() && self.files.len() == FILES_MAX {
            return Err(Status::Full);
        }
        let total = self.bytes - old.unwrap_or(0) + size;
        if total > BYTES_MAX {
            return Err(Status::Full);
        }
        let file = match self.files.get_mut(name) {
            Some(file) => file,
            None => self.files.entry(String::from(name)).or_default(),
        };
        if size > file.len() && file.try_reserve(size - file.len()).is_err() {
            if old.is_none() {
                self.files.remove(name);
            }
            return Err(Status::Full);
        }
        file.resize(size, 0);
        self.bytes = total;
        Ok(())
    }

    pub fn write(&mut self, name: &str, offset: u64, data: &[u8]) -> Result<u64, Status> {
        let length = self.files.get(name).map_or(0, Vec::len);
        let start = if offset == protocol::APPEND { length } else { usize::try_from(offset).map_err(|_| Status::TooBig)? };
        let end = start.checked_add(data.len()).ok_or(Status::TooBig)?;
        self.resize(name, end.max(length))?;
        let file = self.files.get_mut(name).expect("made by resize");
        file[start..end].copy_from_slice(data);
        Ok(file.len() as u64)
    }

    pub fn truncate(&mut self, name: &str, length: u64) -> Result<(), Status> {
        self.resize(name, usize::try_from(length).map_err(|_| Status::TooBig)?)
    }

    pub fn remove(&mut self, name: &str) -> Result<(), Status> {
        let file = self.files.remove(name).ok_or(Status::NotFound)?;
        self.bytes -= file.len();
        Ok(())
    }

    /// Answers one request message: writes the reply into `reply` and
    /// returns its length. Never fails - a bad request gets a reply
    /// saying so.
    pub fn serve(&mut self, message: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> usize {
        let (status, length) = match self.answer(message, &mut reply[1..]) {
            Ok(length) => (Status::Ok, length),
            Err(status) => (status, 0),
        };
        reply[0] = status as u8;
        1 + length
    }

    /// The payload of the reply, written into `out`.
    fn answer(&mut self, message: &[u8], out: &mut [u8]) -> Result<usize, Status> {
        let request = Request::decode(message).ok_or(Status::BadRequest)?;
        let name = request.name;
        let put_number = |out: &mut [u8], n: u64| {
            out[..8].copy_from_slice(&n.to_le_bytes());
            8
        };
        match request.op {
            protocol::LIST => {
                let mut at = 0;
                let from = usize::try_from(request.number).unwrap_or(usize::MAX);
                for (name, file) in self.files.iter().skip(from) {
                    let entry = 1 + name.len() + 8;
                    if at + entry > out.len() {
                        break;
                    }
                    out[at] = name.len() as u8;
                    out[at + 1..at + 1 + name.len()].copy_from_slice(name.as_bytes());
                    out[at + 1 + name.len()..at + entry].copy_from_slice(&(file.len() as u64).to_le_bytes());
                    at += entry;
                }
                Ok(at)
            }
            protocol::READ => self.read(name, request.number, out),
            protocol::WRITE => Ok(put_number(out, self.write(name, request.number, request.data)?)),
            protocol::TRUNCATE => self.truncate(name, request.number).map(|()| 0),
            protocol::REMOVE => self.remove(name).map(|()| 0),
            protocol::STAT => Ok(put_number(out, self.size(name)?)),
            _ => Err(Status::BadRequest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::protocol::*;
    use super::*;

    /// Sends a request through encode, serve and the reply's layout -
    /// the whole path a message takes, minus the channel.
    fn call(files: &mut Files, request: Request) -> (Status, Vec<u8>) {
        let mut message = [0u8; MESSAGE_MAX];
        let length = request.encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let n = files.serve(&message[..length], &mut reply);
        (Status::from_u8(reply[0]), reply[1..n].to_vec())
    }

    fn write(files: &mut Files, name: &str, offset: u64, data: &[u8]) -> (Status, Vec<u8>) {
        call(files, Request { op: WRITE, name, number: offset, data })
    }

    #[test]
    fn requests_round_trip() {
        let request = Request { op: WRITE, name: "notes", number: 77, data: b"hello" };
        let mut buffer = [0u8; MESSAGE_MAX];
        let n = request.encode(&mut buffer).unwrap();
        assert_eq!(n, 10 + 5 + 5);
        assert_eq!(Request::decode(&buffer[..n]), Some(request));
        // Every truncation of it is malformed, except where the data
        // simply gets shorter.
        for cut in 0..2 + 5 + 8 {
            assert_eq!(Request::decode(&buffer[..cut]), None, "cut at {cut}");
        }
        let too_much = [0u8; MESSAGE_MAX];
        assert_eq!(Request { op: WRITE, name: "a", number: 0, data: &too_much }.encode(&mut buffer), None);
        assert_eq!(write_max("a"), MESSAGE_MAX - 11);
    }

    #[test]
    fn write_read_append_and_stat() {
        let mut files = Files::new();
        assert_eq!(write(&mut files, "notes", 0, b"hello"), (Status::Ok, 5u64.to_le_bytes().to_vec()));
        assert_eq!(write(&mut files, "notes", APPEND, b" world").0, Status::Ok);
        assert_eq!(write(&mut files, "notes", 0, b"J").0, Status::Ok, "overwrites in place");
        assert_eq!(call(&mut files, Request::new(READ, "notes", 0)), (Status::Ok, b"Jello world".to_vec()));
        assert_eq!(call(&mut files, Request::new(READ, "notes", 6)).1, b"world");
        assert_eq!(call(&mut files, Request::new(READ, "notes", 99)), (Status::Ok, vec![]), "past the end: nothing");
        assert_eq!(call(&mut files, Request::new(STAT, "notes", 0)).1, 11u64.to_le_bytes());
        assert_eq!(call(&mut files, Request::new(READ, "other", 0)).0, Status::NotFound);
        assert_eq!(files.bytes(), 11);
    }

    #[test]
    fn writing_past_the_end_fills_with_zeros_and_truncate_shrinks() {
        let mut files = Files::new();
        write(&mut files, "f", 4, b"x");
        assert_eq!(call(&mut files, Request::new(READ, "f", 0)).1, b"\0\0\0\0x");
        assert_eq!(call(&mut files, Request::new(TRUNCATE, "f", 2)).0, Status::Ok);
        assert_eq!(files.size("f"), Ok(2));
        assert_eq!(call(&mut files, Request::new(TRUNCATE, "new", 0)).0, Status::Ok, "creates");
        assert_eq!(files.count(), 2);
        assert_eq!(files.bytes(), 2);
    }

    #[test]
    fn list_pages_through_every_file() {
        let mut files = Files::new();
        for n in 0..200 {
            write(&mut files, &format!("file-with-a-long-name-{n:03}"), 0, &vec![1; n]);
        }
        // One reply cannot hold 200 entries: ask again from where it
        // stopped until a reply is empty.
        let mut seen = Vec::new();
        loop {
            let (status, payload) = call(&mut files, Request::new(LIST, "", seen.len() as u64));
            assert_eq!(status, Status::Ok);
            let page: Vec<(String, u64)> = entries(&payload).map(|(n, s)| (n.to_string(), s)).collect();
            if page.is_empty() {
                break;
            }
            assert!(page.len() < 200, "it really did page");
            seen.extend(page);
        }
        assert_eq!(seen.len(), 200);
        assert_eq!(seen[7], ("file-with-a-long-name-007".to_string(), 7), "in name order, with sizes");
    }

    #[test]
    fn remove() {
        let mut files = Files::new();
        write(&mut files, "a", 0, b"123");
        assert_eq!(call(&mut files, Request::new(REMOVE, "a", 0)).0, Status::Ok);
        assert_eq!(call(&mut files, Request::new(REMOVE, "a", 0)).0, Status::NotFound);
        assert_eq!((files.count(), files.bytes()), (0, 0));
    }

    #[test]
    fn bad_names_and_requests_are_refused() {
        let mut files = Files::new();
        let long = "n".repeat(NAME_MAX + 1);
        for name in ["", ".", "..", "a/b", "tab\there", &long[..]] {
            let mut message = [0u8; MESSAGE_MAX];
            // Encode refuses an over-long name; build that one by hand.
            let n = Request { op: WRITE, name, number: 0, data: b"x" }.encode(&mut message).unwrap_or_else(|| {
                message[0] = WRITE;
                message[1] = name.len() as u8;
                message[2..2 + name.len()].copy_from_slice(name.as_bytes());
                2 + name.len() + 8
            });
            let mut reply = [0u8; MESSAGE_MAX];
            files.serve(&message[..n], &mut reply);
            assert_eq!(Status::from_u8(reply[0]), Status::BadName, "{name:?}");
        }
        assert_eq!(files.count(), 0);
        let mut reply = [0u8; MESSAGE_MAX];
        assert_eq!(files.serve(&[WRITE], &mut reply), 1);
        assert_eq!(Status::from_u8(reply[0]), Status::BadRequest, "truncated");
        assert_eq!(call(&mut files, Request::new(99, "a", 0)).0, Status::BadRequest, "unknown op");
        let invalid_utf8 = [READ, 1, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0];
        files.serve(&invalid_utf8, &mut reply);
        assert_eq!(Status::from_u8(reply[0]), Status::BadRequest);
    }

    #[test]
    fn limits_hold() {
        let mut files = Files::new();
        assert_eq!(files.truncate("big", FILE_MAX as u64 + 1), Err(Status::TooBig));
        assert_eq!(files.write("big", FILE_MAX as u64, b"x"), Err(Status::TooBig));
        assert_eq!(files.write("big", u64::MAX - 1, b"xy"), Err(Status::TooBig), "no overflow");
        assert_eq!(files.count(), 0, "a refused write creates nothing");
        // Exactly BYTES_MAX in all fits; one byte more does not.
        for n in 0..BYTES_MAX / FILE_MAX {
            assert_eq!(files.truncate(&format!("f{n}"), FILE_MAX as u64), Ok(()));
        }
        assert_eq!(files.write("one-more", 0, b"x"), Err(Status::Full));
        assert_eq!(files.bytes(), BYTES_MAX);
        files.remove("f0").unwrap();
        for n in 0..FILES_MAX - files.count() {
            files.write(&format!("small{n}"), 0, b"x").unwrap();
        }
        assert_eq!(files.write("too-many", 0, b"x"), Err(Status::Full), "no room for another file");
        assert_eq!(files.write("small0", APPEND, b"y"), Ok(2), "existing files still grow");
    }
}
