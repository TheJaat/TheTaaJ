//! Files in memory: a map from names to contents.
//!
//! Gone when the service ends. It is what `fs` uses when there is no
//! disk, and what the kernel's self-test runs the protocol against.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::protocol::{self, Status};
use crate::{valid_name, Store};

/// Limits, so that one client cannot take all of the service's memory.
pub const FILE_MAX: usize = 1 << 20;
pub const FILES_MAX: usize = 256;
pub const BYTES_MAX: usize = 8 << 20;

#[derive(Default)]
pub struct RamStore {
    files: BTreeMap<String, Vec<u8>>,
    /// The sum of every file's size.
    bytes: usize,
}

impl RamStore {
    pub fn new() -> RamStore {
        RamStore::default()
    }

    pub fn count(&self) -> usize {
        self.files.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
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
}

impl Store for RamStore {
    fn list(&mut self, from: usize, each: &mut dyn FnMut(&str, u64) -> bool) -> Result<(), Status> {
        for (name, file) in self.files.iter().skip(from) {
            if !each(name, file.len() as u64) {
                break;
            }
        }
        Ok(())
    }

    fn size(&mut self, name: &str) -> Result<u64, Status> {
        self.files.get(name).map(|f| f.len() as u64).ok_or(Status::NotFound)
    }

    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let file = self.files.get(name).ok_or(Status::NotFound)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(file.len());
        let n = out.len().min(file.len() - start);
        out[..n].copy_from_slice(&file[start..start + n]);
        Ok(n)
    }

    fn write(&mut self, name: &str, offset: u64, data: &[u8]) -> Result<u64, Status> {
        let length = self.files.get(name).map_or(0, Vec::len);
        let start = if offset == protocol::APPEND { length } else { usize::try_from(offset).map_err(|_| Status::TooBig)? };
        let end = start.checked_add(data.len()).ok_or(Status::TooBig)?;
        self.resize(name, end.max(length))?;
        let file = self.files.get_mut(name).expect("made by resize");
        file[start..end].copy_from_slice(data);
        Ok(file.len() as u64)
    }

    fn truncate(&mut self, name: &str, length: u64) -> Result<(), Status> {
        self.resize(name, usize::try_from(length).map_err(|_| Status::TooBig)?)
    }

    fn remove(&mut self, name: &str) -> Result<(), Status> {
        let file = self.files.remove(name).ok_or(Status::NotFound)?;
        self.bytes -= file.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;
    use crate::{serve, Access, Store};

    /// Sends a request through encode, serve and the reply's layout -
    /// the whole path a message takes, minus the channel.
    fn call(files: &mut RamStore, request: Request) -> (Status, Vec<u8>) {
        let mut message = [0u8; MESSAGE_MAX];
        let length = request.encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let n = serve(files, Access::ReadWrite, &message[..length], &mut reply);
        (Status::from_u8(reply[0]), reply[1..n].to_vec())
    }

    /// The same, on the read-only channel.
    fn look(files: &mut RamStore, request: Request) -> (Status, Vec<u8>) {
        let mut message = [0u8; MESSAGE_MAX];
        let length = request.encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let n = serve(files, Access::ReadOnly, &message[..length], &mut reply);
        (Status::from_u8(reply[0]), reply[1..n].to_vec())
    }

    #[test]
    fn a_read_only_channel_reads_and_changes_nothing() {
        let mut files = RamStore::new();
        assert_eq!(write(&mut files, "a", 0, b"kept").0, Status::Ok);
        // Everything that looks, works.
        assert_eq!(look(&mut files, Request::new(READ, "a", 0)), (Status::Ok, b"kept".to_vec()));
        assert_eq!(look(&mut files, Request::new(STAT, "a", 0)).0, Status::Ok);
        assert_eq!(look(&mut files, Request::new(LIST, "", 0)).0, Status::Ok);
        // Everything that changes is refused - also what would create.
        for request in [
            Request { op: WRITE, name: "a", number: 0, data: b"lost" },
            Request { op: WRITE, name: "new", number: 0, data: b"x" },
            Request::new(TRUNCATE, "a", 0),
            Request::new(TRUNCATE, "new", 0),
            Request::new(REMOVE, "a", 0),
            Request::new(STALL, "", 1),
        ] {
            assert_eq!(look(&mut files, request), (Status::ReadOnly, Vec::new()), "{request:?}");
        }
        // ... before the name is even looked at: no telling which exist.
        assert_eq!(look(&mut files, Request::new(REMOVE, "missing", 0)).0, Status::ReadOnly);
        assert_eq!(call(&mut files, Request::new(READ, "a", 0)), (Status::Ok, b"kept".to_vec()));
        assert_eq!(call(&mut files, Request::new(STAT, "new", 0)).0, Status::NotFound);
    }

    #[test]
    fn a_channel_says_what_it_allows_and_a_stall_has_a_limit() {
        let mut files = RamStore::new();
        assert_eq!(call(&mut files, Request::new(ACCESS, "", 0)), (Status::Ok, vec![0]));
        assert_eq!(look(&mut files, Request::new(ACCESS, "", 0)), (Status::Ok, vec![1]));
        assert_eq!(call(&mut files, Request::new(STALL, "", STALL_MAX)).0, Status::Ok);
        assert_eq!(call(&mut files, Request::new(STALL, "", STALL_MAX + 1)).0, Status::BadRequest);
    }

    fn write(files: &mut RamStore, name: &str, offset: u64, data: &[u8]) -> (Status, Vec<u8>) {
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
        let mut files = RamStore::new();
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
        let mut files = RamStore::new();
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
        let mut files = RamStore::new();
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
        let mut files = RamStore::new();
        write(&mut files, "a", 0, b"123");
        assert_eq!(call(&mut files, Request::new(REMOVE, "a", 0)).0, Status::Ok);
        assert_eq!(call(&mut files, Request::new(REMOVE, "a", 0)).0, Status::NotFound);
        assert_eq!((files.count(), files.bytes()), (0, 0));
    }

    #[test]
    fn bad_names_and_requests_are_refused() {
        let mut files = RamStore::new();
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
            serve(&mut files, Access::ReadWrite, &message[..n], &mut reply);
            assert_eq!(Status::from_u8(reply[0]), Status::BadName, "{name:?}");
        }
        assert_eq!(files.count(), 0);
        let mut reply = [0u8; MESSAGE_MAX];
        assert_eq!(serve(&mut files, Access::ReadWrite, &[WRITE], &mut reply), 1);
        assert_eq!(Status::from_u8(reply[0]), Status::BadRequest, "truncated");
        assert_eq!(call(&mut files, Request::new(99, "a", 0)).0, Status::BadRequest, "unknown op");
        let invalid_utf8 = [READ, 1, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0];
        serve(&mut files, Access::ReadWrite, &invalid_utf8, &mut reply);
        assert_eq!(Status::from_u8(reply[0]), Status::BadRequest);
    }

    #[test]
    fn limits_hold() {
        let mut files = RamStore::new();
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
