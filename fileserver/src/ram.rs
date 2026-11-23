//! Files in memory: a map from paths to what is there.
//!
//! Gone when the service ends. It is what `fs` uses when there is no
//! disk, and what the kernel's self-test runs the protocol against.
//!
//! There is no tree here. Every file and directory is one entry in one
//! sorted map, under its whole path - "docs", "docs/notes",
//! "docs/notes/today" - and what is in a directory is the entries that
//! start with its path and a '/', and have no further '/'. Sorted order
//! puts those next to each other. The rules a tree would enforce by its
//! shape (a file's directory must exist; a directory with something in
//! it stays) are checked instead.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::protocol::{self, Kind, Status, DEPTH_MAX};
use crate::{names, valid_directory, valid_path, Store};

/// Limits, so that one client cannot take all of the service's memory.
pub const FILE_MAX: usize = 1 << 20;
/// Files and directories together.
pub const FILES_MAX: usize = 256;
pub const BYTES_MAX: usize = 8 << 20;

enum Node {
    File(Vec<u8>),
    Directory,
}

#[derive(Default)]
pub struct RamStore {
    nodes: BTreeMap<String, Node>,
    /// The sum of every file's size.
    bytes: usize,
    /// Each file's change stamp (Store::stamp), and the last one given.
    stamps: BTreeMap<String, u64>,
    clock: u64,
}

/// The one spelling of a path that the map uses: its names, joined by
/// single slashes.
fn key(path: &str) -> String {
    let mut key = String::new();
    for name in names(path) {
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(name);
    }
    key
}

impl RamStore {
    pub fn new() -> RamStore {
        RamStore::default()
    }

    /// How many files and directories there are.
    pub fn count(&self) -> usize {
        self.nodes.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Where `path` would be in the map - after checking that it is a
    /// good path, and that every name before the last is a directory.
    fn place(&self, path: &str) -> Result<String, Status> {
        if !valid_path(path) {
            return Err(Status::BadName);
        }
        let key = key(path);
        for (at, _) in key.match_indices('/') {
            match self.nodes.get(&key[..at]) {
                None => return Err(Status::NotFound),
                Some(Node::File(_)) => return Err(Status::NotDirectory),
                Some(Node::Directory) => {}
            }
        }
        Ok(key)
    }

    fn file(&self, path: &str) -> Result<&Vec<u8>, Status> {
        match self.nodes.get(&self.place(path)?) {
            None => Err(Status::NotFound),
            Some(Node::Directory) => Err(Status::IsDirectory),
            Some(Node::File(file)) => Ok(file),
        }
    }

    /// Whether anything is in the directory at `key`.
    fn has_children(&self, key: &str) -> bool {
        let mut prefix = String::from(key);
        prefix.push('/');
        self.nodes.range::<String, _>(&prefix..).next().is_some_and(|(k, _)| k.starts_with(&prefix))
    }

    /// Sets the size of the file at `path` - creating it if needed,
    /// growing it with zeros - after checking every limit, so a refused
    /// request changes nothing. A failed allocation is reported, not a
    /// crash of the service.
    fn resize(&mut self, path: &str, size: usize) -> Result<(), Status> {
        let key = self.place(path)?;
        if size > FILE_MAX {
            return Err(Status::TooBig);
        }
        let old = match self.nodes.get(&key) {
            Some(Node::File(file)) => Some(file.len()),
            Some(Node::Directory) => return Err(Status::IsDirectory),
            None => None,
        };
        if old.is_none() && self.nodes.len() == FILES_MAX {
            return Err(Status::Full);
        }
        let total = self.bytes - old.unwrap_or(0) + size;
        if total > BYTES_MAX {
            return Err(Status::Full);
        }
        let node = self.nodes.entry(key.clone()).or_insert_with(|| Node::File(Vec::new()));
        let Node::File(file) = node else { return Err(Status::IsDirectory) };
        if size > file.len() && file.try_reserve(size - file.len()).is_err() {
            if old.is_none() {
                self.nodes.remove(&key);
            }
            return Err(Status::Full);
        }
        file.resize(size, 0);
        self.bytes = total;
        self.changed(key);
        Ok(())
    }

    /// The file at `key` changed: a new stamp.
    fn changed(&mut self, key: String) {
        self.clock += 1;
        self.stamps.insert(key, self.clock);
    }
}

impl Store for RamStore {
    fn list(
        &mut self,
        directory: &str,
        from: usize,
        each: &mut dyn FnMut(&str, Kind, u64) -> bool,
    ) -> Result<(), Status> {
        if !valid_directory(directory) {
            return Err(Status::BadName);
        }
        let mut prefix = key(directory);
        if !prefix.is_empty() {
            match self.nodes.get(&self.place(directory)?) {
                None => return Err(Status::NotFound),
                Some(Node::File(_)) => return Err(Status::NotDirectory),
                Some(Node::Directory) => {}
            }
            prefix.push('/');
        }
        // Everything under the directory sorts together, starting at the
        // prefix; what is directly in it has no further slash. Those are
        // in name order, because they differ only after the prefix.
        let inside = self
            .nodes
            .range::<String, _>(&prefix..)
            .take_while(|(path, _)| path.starts_with(&prefix))
            .filter(|(path, _)| !path[prefix.len()..].contains('/'));
        for (path, node) in inside.skip(from) {
            let (kind, size) = match node {
                Node::File(file) => (Kind::File, file.len() as u64),
                Node::Directory => (Kind::Directory, 0),
            };
            if !each(&path[prefix.len()..], kind, size) {
                break;
            }
        }
        Ok(())
    }

    fn stat(&mut self, path: &str) -> Result<(Kind, u64), Status> {
        if valid_directory(path) && names(path).next().is_none() {
            return Ok((Kind::Directory, 0)); // the root
        }
        match self.nodes.get(&self.place(path)?) {
            None => Err(Status::NotFound),
            Some(Node::Directory) => Ok((Kind::Directory, 0)),
            Some(Node::File(file)) => Ok((Kind::File, file.len() as u64)),
        }
    }

    fn stamp(&mut self, path: &str) -> Result<u64, Status> {
        match self.stat(path)? {
            (Kind::Directory, _) => Ok(0),
            (Kind::File, _) => Ok(self.stamps.get(&key(path)).copied().unwrap_or(0)),
        }
    }

    fn touch(&mut self, name: &str) -> Result<(), Status> {
        self.file(name)?;
        self.changed(key(name));
        Ok(())
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Status> {
        let key = self.place(path)?;
        if names(path).count() > DEPTH_MAX {
            return Err(Status::BadName); // deeper than a path can reach into
        }
        if self.nodes.contains_key(&key) {
            return Err(Status::Exists);
        }
        if self.nodes.len() == FILES_MAX {
            return Err(Status::Full);
        }
        self.nodes.insert(key, Node::Directory);
        Ok(())
    }

    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let file = self.file(name)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(file.len());
        let n = out.len().min(file.len() - start);
        out[..n].copy_from_slice(&file[start..start + n]);
        Ok(n)
    }

    fn write(&mut self, name: &str, offset: u64, data: &[u8]) -> Result<u64, Status> {
        let length = match self.file(name) {
            Ok(file) => file.len(),
            Err(Status::NotFound) => 0,
            Err(status) => return Err(status),
        };
        let start = if offset == protocol::APPEND { length } else { usize::try_from(offset).map_err(|_| Status::TooBig)? };
        let end = start.checked_add(data.len()).ok_or(Status::TooBig)?;
        self.resize(name, end.max(length))?;
        match self.nodes.get_mut(&key(name)) {
            Some(Node::File(file)) => {
                file[start..end].copy_from_slice(data);
                Ok(file.len() as u64)
            }
            _ => Err(Status::BadRequest), // resize made it: not reached
        }
    }

    fn truncate(&mut self, name: &str, length: u64) -> Result<(), Status> {
        self.resize(name, usize::try_from(length).map_err(|_| Status::TooBig)?)
    }

    fn remove(&mut self, name: &str) -> Result<(), Status> {
        let key = self.place(name)?;
        match self.nodes.get(&key) {
            None => return Err(Status::NotFound),
            Some(Node::Directory) if self.has_children(&key) => return Err(Status::NotEmpty),
            Some(_) => {}
        }
        if let Some(Node::File(file)) = self.nodes.remove(&key) {
            self.bytes -= file.len();
        }
        self.stamps.remove(&key);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Status> {
        let old = self.place(from)?;
        if !valid_path(to) {
            return Err(Status::BadName);
        }
        let new = key(to);
        let directory = match self.nodes.get(&old) {
            None => return Err(Status::NotFound),
            Some(Node::Directory) => true,
            Some(Node::File(_)) => false,
        };
        if new == old {
            return Ok(()); // it is there already
        }
        // Everything in a directory has a key that starts with the
        // directory's and a slash. The slash matters: "docs2" starts
        // with "docs" and is not in it.
        let mut inside = old.clone();
        inside.push('/');
        if directory {
            if new.starts_with(&inside) {
                return Err(Status::IntoItself);
            }
            // The directories in it must stay within reach of a path.
            let height = self
                .nodes
                .range::<String, _>(&inside..)
                .take_while(|(path, _)| path.starts_with(&inside))
                .filter(|(_, node)| matches!(node, Node::Directory))
                .map(|(path, _)| names(&path[inside.len()..]).count())
                .max()
                .unwrap_or(0);
            if names(&new).count() + height > DEPTH_MAX {
                return Err(Status::BadName);
            }
        }
        // Now that it is known not to lead through `from`: the
        // directories on the way to the new place must be there.
        self.place(to)?;
        if self.nodes.contains_key(&new) {
            return Err(Status::Exists);
        }
        // There is no tree to re-hang: every key under the old path is
        // taken out and put back under the new one. Nothing can fail
        // from here on, so it is all or nothing.
        //
        // The search for what is under it starts at "old/", not at
        // "old": between the two, in sorted order, come keys like
        // "old.txt" ('.' sorts before '/'), and a walk from "old" that
        // stops at the first key outside would stop there - having
        // moved the directory and left its contents behind.
        let mut moving = alloc::vec![old.clone()];
        if directory {
            moving.extend(
                self.nodes
                    .range::<String, _>(&inside..)
                    .take_while(|(path, _)| path.starts_with(&inside))
                    .map(|(path, _)| path.clone()),
            );
        }
        for path in moving {
            if let Some(node) = self.nodes.remove(&path) {
                let mut moved = new.clone();
                moved.push_str(&path[old.len()..]);
                if let Some(stamp) = self.stamps.remove(&path) {
                    self.stamps.insert(moved.clone(), stamp);
                }
                self.nodes.insert(moved, node);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;
    use crate::{serve, serve_into, Access, Store};

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

    #[test]
    fn a_whole_file_is_read_into_memory_in_one_exchange() {
        let mut files = RamStore::new();
        // Bigger than many READs' worth.
        let data: Vec<u8> = (0..10_000u32).map(|i| (i * 7 + i / 256) as u8).collect();
        for (i, piece) in data.chunks(write_max("big")).enumerate() {
            let at = (i * write_max("big")) as u64;
            assert_eq!(write(&mut files, "big", at, piece).0, Status::Ok);
        }
        let into_with = |files: &mut RamStore, request: Request, into: &mut [u8]| {
            let mut message = [0u8; MESSAGE_MAX];
            let length = request.encode(&mut message).expect("fits");
            let mut reply = [0u8; MESSAGE_MAX];
            serve_into(files, &message[..length], into, &mut reply).map(|n| reply[..n].to_vec())
        };
        // All of it, with room to spare: the count says where it ends.
        let mut memory = vec![0xAAu8; 12_000];
        let reply = into_with(&mut files, Request::new(READ_INTO, "big", 0), &mut memory).unwrap();
        assert_eq!(reply[0], Status::Ok as u8);
        assert_eq!(u64::from_le_bytes(reply[1..9].try_into().unwrap()), 10_000);
        assert_eq!(&memory[..10_000], &data[..]);
        assert!(memory[10_000..].iter().all(|&b| b == 0xAA));
        // From an offset, to the same offset in the object; and no
        // further than the object goes.
        let mut memory = vec![0u8; 8_000];
        let reply = into_with(&mut files, Request::new(READ_INTO, "big", 5_000), &mut memory).unwrap();
        assert_eq!(u64::from_le_bytes(reply[1..9].try_into().unwrap()), 3_000);
        assert_eq!(&memory[5_000..], &data[5_000..8_000]);
        assert!(memory[..5_000].iter().all(|&b| b == 0));
        // An offset past the object reads nothing.
        let reply = into_with(&mut files, Request::new(READ_INTO, "big", 9_000), &mut memory).unwrap();
        assert_eq!(u64::from_le_bytes(reply[1..9].try_into().unwrap()), 0);
        // A file that is not there is said so, with no count.
        let reply = into_with(&mut files, Request::new(READ_INTO, "nothing", 0), &mut memory).unwrap();
        assert_eq!(reply, vec![Status::NotFound as u8]);
        // Only READ_INTO is served this way; and READ_INTO without the
        // memory, on the ordinary path, is refused.
        assert_eq!(into_with(&mut files, Request::new(READ, "big", 0), &mut memory), None);
        assert_eq!(call(&mut files, Request::new(READ_INTO, "big", 0)).0, Status::BadRequest);
    }

    /// A store of one file, "huge", of sevens: bigger than a RamStore
    /// may hold, for the READ_INTO limit.
    struct Sevens(usize);

    impl Store for Sevens {
        fn list(&mut self, _: &str, _: usize, _: &mut dyn FnMut(&str, Kind, u64) -> bool) -> Result<(), Status> {
            Err(Status::BadRequest)
        }
        fn stat(&mut self, _: &str) -> Result<(Kind, u64), Status> {
            Ok((Kind::File, self.0 as u64))
        }
        fn stamp(&mut self, _: &str) -> Result<u64, Status> {
            Ok(0)
        }
        fn touch(&mut self, _: &str) -> Result<(), Status> {
            Err(Status::BadRequest)
        }
        fn make_directory(&mut self, _: &str) -> Result<(), Status> {
            Err(Status::BadRequest)
        }
        fn read(&mut self, _: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
            let n = out.len().min(self.0.saturating_sub(offset as usize));
            out[..n].fill(7);
            Ok(n)
        }
        fn write(&mut self, _: &str, _: u64, _: &[u8]) -> Result<u64, Status> {
            Err(Status::BadRequest)
        }
        fn truncate(&mut self, _: &str, _: u64) -> Result<(), Status> {
            Err(Status::BadRequest)
        }
        fn remove(&mut self, _: &str) -> Result<(), Status> {
            Err(Status::BadRequest)
        }
        fn rename(&mut self, _: &str, _: &str) -> Result<(), Status> {
            Err(Status::BadRequest)
        }
    }

    #[test]
    fn one_read_into_reads_no_more_than_its_limit() {
        let size = READ_INTO_MAX + 100_000;
        let mut files = Sevens(size);
        let mut message = [0u8; MESSAGE_MAX];
        let mut reply = [0u8; MESSAGE_MAX];
        let mut memory = vec![0u8; size];
        let mut count = |offset: u64, memory: &mut [u8]| {
            let length = Request::new(READ_INTO, "huge", offset).encode(&mut message).unwrap();
            serve_into(&mut files, &message[..length], memory, &mut reply).unwrap();
            u64::from_le_bytes(reply[1..9].try_into().unwrap())
        };
        // The first exchange stops at the limit; the next reads the rest.
        assert_eq!(count(0, &mut memory), READ_INTO_MAX as u64);
        assert_eq!(memory[READ_INTO_MAX], 0);
        assert_eq!(count(READ_INTO_MAX as u64, &mut memory), 100_000);
        assert!(memory.iter().all(|&b| b == 7));
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
        assert_eq!(stat(&call(&mut files, Request::new(STAT, "notes", 0)).1), Some((Kind::File, 11)));
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
            let page: Vec<(String, u64)> = entries(&payload).map(|(n, _, s)| (n.to_string(), s)).collect();
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
        let wide = "w".repeat(COMPONENT_MAX + 1);
        let deep = "d/".repeat(DEPTH_MAX + 1) + "leaf";
        for name in ["", "/", ".", "..", "a/..", "./a", "tab\there", &long[..], &wide[..], &deep[..]] {
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
    fn renaming() {
        let mut files = RamStore::new();
        let rename = |files: &mut RamStore, from: &str, to: &str| {
            call(files, Request { op: RENAME, name: from, number: 0, data: to.as_bytes() }).0
        };
        for directory in ["docs", "docs/notes", "docs2"] {
            assert_eq!(call(&mut files, Request::new(MKDIR, directory, 0)).0, Status::Ok);
        }
        files.write("docs/notes/today", 0, b"deep").unwrap();
        files.write("docs.txt", 0, b"beside").unwrap();
        files.write("top", 0, b"t").unwrap();
        let before = (files.count(), files.bytes());

        // A new name; another directory; a directory and what is in it.
        assert_eq!(rename(&mut files, "top", "bottom"), Status::Ok);
        assert_eq!(rename(&mut files, "bottom", "/docs/notes//bottom"), Status::Ok);
        assert_eq!(rename(&mut files, "docs", "papers"), Status::Ok);
        assert_eq!(listing(&mut files, ""), (Status::Ok, vec!["docs.txt".into(), "docs2/".into(), "papers/".into()]));
        assert_eq!(listing(&mut files, "papers/notes"), (Status::Ok, vec!["bottom".into(), "today".into()]));
        assert_eq!(listing(&mut files, "docs2"), (Status::Ok, vec![]), "a neighbour with a longer name stayed");
        assert_eq!(files.read("docs.txt", 0, &mut [0; 8]), Ok(6), "and so did the file beside it");
        assert_eq!(listing(&mut files, "docs").0, Status::NotFound);

        // What is refused, and changes nothing.
        assert_eq!(rename(&mut files, "nothing", "x"), Status::NotFound);
        assert_eq!(rename(&mut files, "papers", "nowhere/x"), Status::NotFound);
        assert_eq!(rename(&mut files, "papers", "docs.txt/x"), Status::NotDirectory);
        assert_eq!(rename(&mut files, "papers", "docs2"), Status::Exists);
        assert_eq!(rename(&mut files, "docs.txt", "papers"), Status::Exists);
        assert_eq!(rename(&mut files, "papers", "papers/in"), Status::IntoItself);
        assert_eq!(rename(&mut files, "papers", "papers/notes/in"), Status::IntoItself);
        assert_eq!(rename(&mut files, "papers", "papers/nowhere/in"), Status::IntoItself);
        assert_eq!(rename(&mut files, "papers", ""), Status::BadName);
        assert_eq!(rename(&mut files, "papers", "a/../b"), Status::BadName);
        assert_eq!(rename(&mut files, "", "x"), Status::BadName);
        assert_eq!(call(&mut files, Request { op: RENAME, name: "papers", number: 0, data: &[0xFF, 0xFE] }).0, Status::BadRequest);
        assert_eq!(look(&mut files, Request { op: RENAME, name: "papers", number: 0, data: b"x" }).0, Status::ReadOnly);
        // To where it is: nothing to do.
        assert_eq!(rename(&mut files, "papers", "/papers/"), Status::Ok);
        // "papers2" is not inside "papers".
        assert_eq!(rename(&mut files, "papers", "docs2/papers"), Status::Ok);
        assert_eq!(listing(&mut files, "docs2/papers/notes"), (Status::Ok, vec!["bottom".into(), "today".into()]));
        assert_eq!((files.count(), files.bytes()), before, "nothing was made or lost");
    }

    #[test]
    fn a_move_cannot_put_directories_out_of_reach() {
        let mut files = RamStore::new();
        let mut path = String::new();
        for level in 0..DEPTH_MAX - 1 {
            path.push_str(&format!("/d{level}"));
            files.make_directory(&path).unwrap();
        }
        files.make_directory("flat").unwrap();
        files.make_directory("tall").unwrap();
        files.make_directory("tall/inner").unwrap();
        assert_eq!(files.rename("tall", &format!("{path}/tall")), Err(Status::BadName));
        assert_eq!(files.rename("flat", &format!("{path}/flat")), Ok(()));
        assert_eq!(files.rename("tall/inner", &format!("{path}/flat/inner")), Err(Status::BadName));
    }

    /// What is in a directory, as "name" or "name/".
    fn listing(files: &mut RamStore, directory: &str) -> (Status, Vec<String>) {
        let (status, payload) = call(files, Request::new(LIST, directory, 0));
        let names = entries(&payload)
            .map(|(name, kind, _)| if kind == Kind::Directory { format!("{name}/") } else { name.to_string() })
            .collect();
        (status, names)
    }

    #[test]
    fn directories() {
        let mut files = RamStore::new();
        assert_eq!(call(&mut files, Request::new(MKDIR, "docs", 0)).0, Status::Ok);
        assert_eq!(call(&mut files, Request::new(MKDIR, "docs/notes", 0)).0, Status::Ok);
        assert_eq!(write(&mut files, "docs/notes/today", 0, b"deep").0, Status::Ok);
        assert_eq!(write(&mut files, "docs/plan", 0, b"p").0, Status::Ok);
        assert_eq!(write(&mut files, "docs.txt", 0, b"beside").0, Status::Ok);
        assert_eq!(write(&mut files, "top", 0, b"t").0, Status::Ok);

        // A listing is of one directory, not of everything under it.
        assert_eq!(listing(&mut files, ""), (Status::Ok, vec!["docs/".into(), "docs.txt".into(), "top".into()]));
        assert_eq!(listing(&mut files, "docs"), (Status::Ok, vec!["notes/".into(), "plan".into()]));
        assert_eq!(listing(&mut files, "/docs//notes/").1, ["today"], "slashes around mean nothing");
        assert_eq!(listing(&mut files, "docs/plan").0, Status::NotDirectory);
        assert_eq!(listing(&mut files, "nowhere").0, Status::NotFound);

        assert_eq!(call(&mut files, Request::new(READ, "/docs/notes/today", 0)).1, b"deep");
        assert_eq!(stat(&call(&mut files, Request::new(STAT, "docs", 0)).1), Some((Kind::Directory, 0)));
        assert_eq!(stat(&call(&mut files, Request::new(STAT, "", 0)).1), Some((Kind::Directory, 0)), "the root");
        assert_eq!(stat(&call(&mut files, Request::new(STAT, "docs/plan", 0)).1), Some((Kind::File, 1)));

        // A path leads through directories only, and they must be there.
        assert_eq!(write(&mut files, "top/x", 0, b"x").0, Status::NotDirectory);
        assert_eq!(write(&mut files, "nowhere/x", 0, b"x").0, Status::NotFound);
        assert_eq!(call(&mut files, Request::new(MKDIR, "nowhere/x", 0)).0, Status::NotFound);
        // A directory is not a file, and one name is one thing.
        assert_eq!(write(&mut files, "docs", 0, b"x").0, Status::IsDirectory);
        assert_eq!(call(&mut files, Request::new(READ, "docs", 0)).0, Status::IsDirectory);
        assert_eq!(call(&mut files, Request::new(TRUNCATE, "docs", 0)).0, Status::IsDirectory);
        assert_eq!(call(&mut files, Request::new(MKDIR, "docs", 0)).0, Status::Exists);
        assert_eq!(call(&mut files, Request::new(MKDIR, "top", 0)).0, Status::Exists);
        // Making a directory is a change.
        assert_eq!(look(&mut files, Request::new(MKDIR, "other", 0)).0, Status::ReadOnly);

        // Only an empty directory goes.
        assert_eq!(call(&mut files, Request::new(REMOVE, "docs", 0)).0, Status::NotEmpty);
        assert_eq!(call(&mut files, Request::new(REMOVE, "docs/notes", 0)).0, Status::NotEmpty);
        for path in ["docs/notes/today", "docs/notes", "docs/plan", "docs"] {
            assert_eq!(call(&mut files, Request::new(REMOVE, path, 0)).0, Status::Ok, "{path}");
        }
        assert_eq!(listing(&mut files, "").1, ["docs.txt", "top"], "the file beside it was not in it");
        assert_eq!((files.count(), files.bytes()), (2, 7));
    }

    #[test]
    fn directories_nest_to_the_limit() {
        let mut files = RamStore::new();
        let mut path = String::new();
        for level in 0..DEPTH_MAX {
            path.push_str(&format!("/d{level}"));
            assert_eq!(files.make_directory(&path), Ok(()));
        }
        assert_eq!(files.write(&format!("{path}/leaf"), 0, b"x"), Ok(1));
        assert_eq!(files.make_directory(&format!("{path}/more")), Err(Status::BadName));
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
