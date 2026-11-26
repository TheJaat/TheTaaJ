//! Open files and directories: what a file service keeps for each
//! channel it serves, and the requests that come on them.
//!
//! Until J60 the service kept nothing about its clients. Every request
//! named a whole path from the root, so an open file lived only in the
//! client - libc kept a path and an offset - and it followed the *name*:
//! rename a file a program had open, and its next read found whatever
//! had that name now, or nothing; remove it, and its next write made a
//! new one (J36). And an open file could not be handed to another
//! program, only its name could.
//!
//! Now a client can OPEN a file or a directory. It sends one end of a
//! new channel with the request; the service keeps that end, and here
//! is what it stands for: a `Node`. Every request that later comes on
//! that channel is about that node:
//!
//!   - a **file**'s requests carry no name: READ, WRITE, STAT, ... of
//!     the file itself;
//!   - a **directory**'s carry names, taken from that directory - a
//!     channel rooted there. The service's own two request channels are
//!     simply the root directory, open for changing and open for reading
//!     (ROOT, ROOT_READ_ONLY): one protocol, whichever channel a request
//!     comes on.
//!
//! What a channel allows is fixed when it is opened: a node opened from
//! a read-only channel, or without `open::WRITE`, refuses every change
//! (ReadOnly) - so a program can be handed one file to read, and nothing
//! else. One exception, POSIX's: a file opened to read from a channel
//! that may change files may still be marked changed now (TOUCH), as
//! anyone who may write a file may stamp it (futimens on O_RDONLY).
//!
//! The service stays the only one who knows the node's path, and keeps
//! it right as names change:
//!
//!   - a RENAME moves the paths of the nodes open on what was renamed,
//!     or on anything inside a directory that was;
//!   - a REMOVE of a file that is open does not remove it yet: it is
//!     moved into REMOVED, a directory clients cannot name, and removed
//!     when its last node closes - as Unix keeps a file that was
//!     unlinked while open. A RENAME that replaces an open file does the
//!     same with the file it replaces. A directory can only be removed
//!     when it is empty; its nodes then answer NotFound.
//!
//! If the service stops (or the power does) while REMOVED holds files,
//! they are removed when it starts again (`Nodes::new`), as a Unix file
//! system empties its orphan list at mount.
//!
//! Nothing of this is in the kernel: a node is a row in this table, and
//! the channel is the only thing the kernel sees.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::protocol::{self, open, Kind, Request, Status, MESSAGE_MAX};
use crate::{names, Access, Store, Stream};

/// The directory removed-but-open files wait in, at the root of the
/// volume. No client can name it (`Nodes::resolve`), and LIST leaves it
/// out; it exists only while it holds something.
pub const REMOVED: &str = ".taaj-removed";

/// Which node a channel stands for: an index into the table.
pub type NodeId = usize;

/// The service's channel for changing files: the root, open to change.
pub const ROOT: NodeId = 0;
/// Its channel for reading only: the root, open to read.
pub const ROOT_READ_ONLY: NodeId = 1;

/// One open file or directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Where it is now, from the root of the volume ("" is the root).
    pub path: String,
    pub kind: Kind,
    /// What its channel allows.
    pub access: Access,
    /// Whether it was opened from a channel that may change files: then
    /// it may be marked changed now (TOUCH) even if it was opened only to
    /// read, as POSIX lets anyone who may write a file stamp it
    /// (futimens on a descriptor opened O_RDONLY).
    pub may_touch: bool,
    /// A file removed while open: it waits in REMOVED.
    removed: bool,
    /// A directory that was removed: there is nothing here any more.
    gone: bool,
}

/// Every node the service has open.
pub struct Nodes {
    slots: Vec<Option<Node>>,
    /// The name the next file moved into REMOVED takes.
    next_removed: u64,
}

impl Nodes {
    /// The table, with the root open twice (ROOT, ROOT_READ_ONLY); and
    /// whatever a service that stopped left in REMOVED, removed.
    pub fn new(store: &mut impl Store) -> Nodes {
        let root = |access| Some(Node { path: String::new(), kind: Kind::Directory, access, may_touch: access == Access::ReadWrite, removed: false, gone: false });
        let nodes = Nodes { slots: alloc::vec![root(Access::ReadWrite), root(Access::ReadOnly)], next_removed: 0 };
        let mut left: Vec<String> = Vec::new();
        let _ = store.list(REMOVED, 0, &mut |name, _, _| {
            left.push(format!("{REMOVED}/{name}"));
            true
        });
        for path in &left {
            let _ = store.remove(path);
        }
        let _ = store.remove(REMOVED);
        nodes
    }

    /// The node `id` stands for, if it is open.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.slots.get(id).and_then(Option::as_ref)
    }

    /// How many files and directories are open, the root's two not
    /// counted.
    pub fn open_count(&self) -> usize {
        self.slots.iter().skip(2).filter(|slot| slot.is_some()).count()
    }

    /// The path from the root of the volume that `name`, sent on node
    /// `id`'s channel, means; and what that channel allows.
    pub fn resolve(&self, id: NodeId, name: &str) -> Result<(String, Access), Status> {
        let node = self.get(id).ok_or(Status::BadRequest)?;
        if node.gone {
            return Err(Status::NotFound);
        }
        if names(name).next().is_none() {
            return Ok((node.path.clone(), node.access));
        }
        if node.kind == Kind::File {
            return Err(Status::NotDirectory);
        }
        let path = if node.path.is_empty() { names(name).collect::<Vec<_>>().join("/") } else { format!("{}/{}", node.path, names(name).collect::<Vec<_>>().join("/")) };
        // Reached through a name: REMOVED is the service's own.
        if names(&path).next() == Some(REMOVED) {
            return Err(Status::NotFound);
        }
        if path.len() > protocol::NAME_MAX {
            return Err(Status::BadName);
        }
        Ok((path, node.access))
    }

    /// Answers one request that came on node `id`'s channel with only a
    /// reply channel: writes the reply, returns its length.
    pub fn serve(&mut self, store: &mut impl Store, id: NodeId, message: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> usize {
        let (status, length) = match self.answer(store, id, message, &mut reply[1..]) {
            Ok(length) => (Status::Ok, length),
            Err(status) => (status, 0),
        };
        reply[0] = status as u8;
        1 + length
    }

    fn answer(&mut self, store: &mut impl Store, id: NodeId, message: &[u8], out: &mut [u8]) -> Result<usize, Status> {
        let request = Request::decode(message).ok_or(Status::BadRequest)?;
        let (path, mut access) = self.resolve(id, request.name)?;
        // A file's own channel, opened to read from one that may change
        // files: it may still stamp the file.
        if request.op == protocol::TOUCH && request.name.is_empty() && self.get(id).is_some_and(|node| node.kind == Kind::File && node.may_touch) {
            access = Access::ReadWrite;
        }
        let changes = matches!(request.op, protocol::REMOVE | protocol::RENAME);
        if changes && access == Access::ReadOnly {
            return Err(Status::ReadOnly);
        }
        match request.op {
            protocol::REMOVE => self.remove(store, &path).map(|()| 0),
            protocol::RENAME => {
                let to = core::str::from_utf8(request.data).map_err(|_| Status::BadRequest)?;
                let (to, _) = self.resolve(id, to)?;
                self.rename(store, &path, &to).map(|()| 0)
            }
            // A file's removal or replacement is the service's business
            // alone: the bytes of REMOVED itself are not for listing.
            protocol::LIST if path.is_empty() => {
                let hidden = self.hidden_place(store);
                crate::answer_request(store, access, &Request { name: &path, ..request }, out, hidden.map(|place| (REMOVED, place)))
            }
            // Needs the handle that comes with it: not a request for a
            // reply channel alone.
            protocol::OPEN => Err(Status::BadRequest),
            _ => crate::answer_request(store, access, &Request { name: &path, ..request }, out, None),
        }
    }

    /// Where REMOVED is among the root's names, if it is there: how many
    /// come before it.
    fn hidden_place(&self, store: &mut impl Store) -> Option<usize> {
        store.stat(REMOVED).ok()?;
        let mut place = 0;
        let mut found = false;
        let _ = store.list("", 0, &mut |name, _, _| {
            if name == REMOVED {
                found = true;
                return false;
            }
            place += 1;
            true
        });
        found.then_some(place)
    }

    /// An OPEN that came on node `id`'s channel: the node made, if it
    /// may be. Writes the reply (STAT's payload, of what was opened) and
    /// returns its length, and the new node - whose channel end the
    /// request brought; the service keeps it as that node's.
    pub fn open(&mut self, store: &mut impl Store, id: NodeId, message: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> (usize, Option<NodeId>) {
        match self.opened(store, id, message, &mut reply[1..]) {
            Ok((length, node)) => {
                reply[0] = Status::Ok as u8;
                (1 + length, Some(node))
            }
            Err(status) => {
                reply[0] = status as u8;
                (1, None)
            }
        }
    }

    fn opened(&mut self, store: &mut impl Store, id: NodeId, message: &[u8], out: &mut [u8]) -> Result<(usize, NodeId), Status> {
        let request = Request::decode(message).filter(|request| request.op == protocol::OPEN).ok_or(Status::BadRequest)?;
        let flags = request.number;
        if flags & !open::ALL != 0 {
            return Err(Status::BadRequest);
        }
        let (path, access) = self.resolve(id, request.name)?;
        let writes = flags & open::WRITE != 0;
        if (writes || flags & open::CREATE != 0) && access == Access::ReadOnly {
            return Err(Status::ReadOnly);
        }
        if self.open_count() >= protocol::OPEN_MAX {
            return Err(Status::Busy);
        }
        let exclusive = flags & (open::CREATE | open::EXCLUSIVE) == open::CREATE | open::EXCLUSIVE;
        let kind = match store.stat(&path) {
            Ok(_) if exclusive => return Err(Status::Exists),
            Ok((Kind::Directory, _)) if flags & open::DIRECTORY == 0 => return Err(Status::IsDirectory),
            Ok((Kind::Directory, _)) if flags & open::TRUNCATE != 0 => return Err(Status::IsDirectory),
            Ok((Kind::File, _)) if flags & open::DIRECTORY != 0 => return Err(Status::NotDirectory),
            Ok((Kind::File, _)) if writes && flags & open::TRUNCATE != 0 => {
                store.truncate(&path, 0)?;
                Kind::File
            }
            Ok((kind, _)) => kind,
            Err(Status::NotFound) if flags & open::CREATE != 0 && flags & open::DIRECTORY == 0 => {
                // Adding nothing to a file that is not there makes it.
                store.write(&path, protocol::APPEND, &[])?;
                Kind::File
            }
            Err(status) => return Err(status),
        };
        // The new channel allows no more than the one it was asked on.
        let may_touch = access == Access::ReadWrite;
        let access = if writes { access } else { Access::ReadOnly };
        let node = Node { path, kind, access, may_touch, removed: false, gone: false };
        let length = crate::answer_request(store, access, &Request { op: protocol::STAT, name: &node.path, number: 0, data: &[] }, out, None)?;
        let id = match self.slots.iter().skip(2).position(Option::is_none) {
            Some(free) => {
                self.slots[free + 2] = Some(node);
                free + 2
            }
            None => {
                self.slots.push(Some(node));
                self.slots.len() - 1
            }
        };
        Ok((length, id))
    }

    /// Node `id`'s channel was closed: the node is let go - and if it was
    /// the last on a file removed while open, the file goes now.
    pub fn close(&mut self, store: &mut impl Store, id: NodeId) {
        if id == ROOT || id == ROOT_READ_ONLY {
            return;
        }
        let Some(node) = self.slots.get_mut(id).and_then(Option::take) else { return };
        if node.removed && !self.slots.iter().flatten().any(|other| other.path == node.path) {
            let _ = store.remove(&node.path);
            // Empty now? Then it goes too (a directory with files in it
            // is refused, and stays).
            let _ = store.remove(REMOVED);
        }
    }

    /// A READ_INTO on node `id`'s channel: as `crate::serve_into`.
    pub fn serve_into(&self, store: &mut impl Store, id: NodeId, message: &[u8], into: &mut [u8], reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
        let request = Request::decode(message).filter(|request| request.op == protocol::READ_INTO)?;
        Some(match self.resolve(id, request.name) {
            Ok((path, _)) => crate::serve_into_request(store, &Request { name: &path, ..request }, into, reply),
            Err(status) => {
                reply[0] = status as u8;
                1
            }
        })
    }

    /// A WRITE_FROM on node `id`'s channel: as `crate::serve_from`.
    pub fn serve_from(&self, store: &mut impl Store, id: NodeId, message: &[u8], from: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
        let request = Request::decode(message).filter(|request| request.op == protocol::WRITE_FROM)?;
        Some(match self.resolve(id, request.name) {
            Ok((path, access)) => crate::serve_from_request(store, access, &Request { name: &path, ..request }, from, reply),
            Err(status) => {
                reply[0] = status as u8;
                1
            }
        })
    }

    /// A STREAM_OUT or STREAM_IN on node `id`'s channel: as
    /// `crate::open_stream`.
    pub fn open_stream(&self, store: &mut impl Store, id: NodeId, message: &[u8], reply: &mut [u8; MESSAGE_MAX]) -> (usize, Option<Stream>) {
        let resolved = Request::decode(message)
            .ok_or(Status::BadRequest)
            .and_then(|request| self.resolve(id, request.name).map(|(path, access)| (request, path, access)));
        match resolved {
            Ok((request, path, access)) => crate::open_stream_request(store, access, &Request { name: &path, ..request }, reply),
            Err(status) => {
                reply[0] = status as u8;
                (1, None)
            }
        }
    }

    /// Whether a file node is open on `path`.
    fn file_open(&self, path: &str) -> bool {
        self.slots.iter().flatten().any(|node| node.kind == Kind::File && !node.removed && node.path == path)
    }

    /// Moves the file at `path`, which is open, into REMOVED; its nodes
    /// go with it.
    fn set_aside(&mut self, store: &mut impl Store, path: &str) -> Result<(), Status> {
        match store.make_directory(REMOVED) {
            Ok(()) | Err(Status::Exists) => {}
            Err(status) => return Err(status),
        }
        let aside = format!("{REMOVED}/{}", self.next_removed);
        self.next_removed += 1;
        if let Err(status) = store.rename(path, &aside) {
            let _ = store.remove(REMOVED); // made for nothing, if empty
            return Err(status);
        }
        for node in self.slots.iter_mut().flatten() {
            if node.kind == Kind::File && !node.removed && node.path == path {
                node.path = aside.clone();
                node.removed = true;
            }
        }
        Ok(())
    }

    /// REMOVE `path`: a file nobody has open, or an empty directory, at
    /// once; a file that is open, once its last node closes.
    fn remove(&mut self, store: &mut impl Store, path: &str) -> Result<(), Status> {
        if store.stat(path)?.0 == Kind::File && self.file_open(path) {
            // If it cannot be set aside (no room for REMOVED), it is
            // removed anyway - its nodes then find nothing - rather than
            // refusing a removal for something the caller cannot see.
            if self.set_aside(store, path).is_ok() {
                return Ok(());
            }
            store.remove(path)?;
            self.forget(path);
            return Ok(());
        }
        store.remove(path)?;
        self.forget(path);
        Ok(())
    }

    /// Nodes on `path`, which is not there any more, find nothing.
    fn forget(&mut self, path: &str) {
        for node in self.slots.iter_mut().flatten() {
            if !node.removed && node.path == path {
                node.gone = true;
            }
        }
    }

    /// RENAME `from` to `to`, and the open nodes' paths moved with it.
    fn rename(&mut self, store: &mut impl Store, from: &str, to: &str) -> Result<(), Status> {
        // A file replaced by a file: if the one replaced is open, it is
        // set aside first, so that its readers keep it.
        let replaces_open_file = from != to
            && matches!(store.stat(from), Ok((Kind::File, _)))
            && matches!(store.stat(to), Ok((Kind::File, _)))
            && self.file_open(to);
        let replaces_directory = from != to && matches!(store.stat(to), Ok((Kind::Directory, _)));
        if replaces_open_file {
            self.set_aside(store, to)?;
        }
        if let Err(status) = store.rename(from, to) {
            if replaces_open_file {
                self.put_back(store, to);
            }
            return Err(status);
        }
        if replaces_directory {
            self.forget(to);
        }
        if from == to {
            return Ok(());
        }
        let inside = format!("{from}/");
        for node in self.slots.iter_mut().flatten() {
            if node.removed || node.gone {
                continue;
            }
            if node.path == from {
                node.path = String::from(to);
            } else if let Some(rest) = node.path.strip_prefix(&inside) {
                node.path = format!("{to}/{rest}");
            }
        }
        Ok(())
    }

    /// Undoes `set_aside` of the file that was at `path`: the rename it
    /// was for failed.
    fn put_back(&mut self, store: &mut impl Store, path: &str) {
        let aside = format!("{REMOVED}/{}", self.next_removed - 1);
        if store.rename(&aside, path).is_ok() {
            for node in self.slots.iter_mut().flatten() {
                if node.removed && node.path == aside {
                    node.path = String::from(path);
                    node.removed = false;
                }
            }
            let _ = store.remove(REMOVED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiskStore, RamStore};
    use alloc::vec;
    use taajfs::{format_volume, MemDevice};

    /// A request on node `id`'s channel: the status and the payload.
    fn ask(nodes: &mut Nodes, store: &mut impl Store, id: NodeId, op: u8, name: &str, number: u64, data: &[u8]) -> (Status, Vec<u8>) {
        let mut message = [0u8; MESSAGE_MAX];
        let length = Request { op, name, number, data }.encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let n = nodes.serve(store, id, &message[..length], &mut reply);
        (Status::from_u8(reply[0]), reply[1..n].to_vec())
    }

    fn open_node(nodes: &mut Nodes, store: &mut impl Store, id: NodeId, name: &str, flags: u64) -> Result<NodeId, Status> {
        let mut message = [0u8; MESSAGE_MAX];
        let length = Request::new(protocol::OPEN, name, flags).encode(&mut message).expect("fits");
        let mut reply = [0u8; MESSAGE_MAX];
        let (n, node) = nodes.open(store, id, &message[..length], &mut reply);
        match node {
            Some(node) => {
                assert_eq!(n, 1 + 25, "STAT's payload");
                Ok(node)
            }
            None => Err(Status::from_u8(reply[0])),
        }
    }

    fn read(nodes: &mut Nodes, store: &mut impl Store, id: NodeId, name: &str) -> Result<Vec<u8>, Status> {
        match ask(nodes, store, id, protocol::READ, name, 0, &[]) {
            (Status::Ok, bytes) => Ok(bytes),
            (status, _) => Err(status),
        }
    }

    fn write(nodes: &mut Nodes, store: &mut impl Store, id: NodeId, name: &str, data: &[u8]) -> Status {
        ask(nodes, store, id, protocol::WRITE, name, 0, data).0
    }

    fn listed(nodes: &mut Nodes, store: &mut impl Store, id: NodeId, name: &str) -> Vec<String> {
        // One entry a request, as a client with a small reply would
        // page through: the numbering must not skip or repeat.
        let mut all = Vec::new();
        let mut from = 0;
        loop {
            let (status, payload) = ask(nodes, store, id, protocol::LIST, name, from, &[]);
            assert_eq!(status, Status::Ok);
            let got: Vec<String> = protocol::entries(&payload).map(|(name, _, _)| String::from(name)).collect();
            if got.is_empty() {
                return all;
            }
            // Take only the first, to test the paging.
            all.push(got[0].clone());
            from += 1;
        }
    }

    /// The same for every store.
    fn open_files_follow_their_file(store: &mut impl Store) {
        let mut nodes = Nodes::new(store);
        let nodes = &mut nodes;
        assert_eq!(write(nodes, store, ROOT, "a", b"first"), Status::Ok);
        assert_eq!(ask(nodes, store, ROOT, protocol::MKDIR, "d", 0, &[]).0, Status::Ok);

        // A file opened: its requests carry no name.
        let file = open_node(nodes, store, ROOT, "a", open::READ | open::WRITE).unwrap();
        assert_eq!(read(nodes, store, file, ""), Ok(b"first".to_vec()));
        assert_eq!(read(nodes, store, file, "x"), Err(Status::NotDirectory), "a file has no names in it");

        // Renamed - even into another directory - it is still the same file.
        assert_eq!(ask(nodes, store, ROOT, protocol::RENAME, "a", 0, b"d/b").0, Status::Ok);
        assert_eq!(read(nodes, store, file, ""), Ok(b"first".to_vec()), "it follows the rename");
        assert_eq!(write(nodes, store, file, "", b"FIRST"), Status::Ok);
        assert_eq!(read(nodes, store, ROOT, "d/b"), Ok(b"FIRST".to_vec()), "and writes land in it");
        assert_eq!(read(nodes, store, ROOT, "a"), Err(Status::NotFound), "nothing was made at the old name");

        // A directory opened: names from there. Its rename moves what is open inside it.
        let dir = open_node(nodes, store, ROOT, "d", open::READ | open::DIRECTORY | open::WRITE).unwrap();
        assert_eq!(read(nodes, store, dir, "b"), Ok(b"FIRST".to_vec()));
        assert_eq!(ask(nodes, store, ROOT, protocol::RENAME, "d", 0, b"e").0, Status::Ok);
        assert_eq!(read(nodes, store, dir, "b"), Ok(b"FIRST".to_vec()), "the directory moved, and its node with it");
        assert_eq!(read(nodes, store, file, ""), Ok(b"FIRST".to_vec()), "and the file inside it");
        assert_eq!(nodes.get(file).unwrap().path, "e/b");

        // Removed while open: still readable and writable; the name is free.
        assert_eq!(ask(nodes, store, ROOT, protocol::REMOVE, "e/b", 0, &[]).0, Status::Ok);
        assert_eq!(read(nodes, store, ROOT, "e/b"), Err(Status::NotFound), "the name is gone");
        assert_eq!(read(nodes, store, file, ""), Ok(b"FIRST".to_vec()), "the open file is not");
        assert_eq!(write(nodes, store, file, "", b"still"), Status::Ok);
        assert_eq!(listed(nodes, store, ROOT, ""), vec!["e"], "REMOVED is not listed");
        assert_eq!(read(nodes, store, ROOT, &format!("{REMOVED}/0")), Err(Status::NotFound), "nor reachable by name");
        assert_eq!(write(nodes, store, ROOT, "e/b", b"new"), Status::Ok, "a new file can take the name");
        nodes.close(store, file);
        assert!(store.stat(REMOVED).is_err(), "the last close removes it, and REMOVED with it");
        assert_eq!(read(nodes, store, ROOT, "e/b"), Ok(b"new".to_vec()), "the new file is untouched");

        // Replaced by a rename while open: the reader keeps the old one.
        assert_eq!(write(nodes, store, ROOT, "e/c", b"replacement"), Status::Ok);
        let old = open_node(nodes, store, ROOT, "e/b", open::READ).unwrap();
        assert_eq!(ask(nodes, store, ROOT, protocol::RENAME, "e/c", 0, b"e/b").0, Status::Ok);
        assert_eq!(read(nodes, store, old, ""), Ok(b"new".to_vec()), "the replaced file, still");
        assert_eq!(read(nodes, store, ROOT, "e/b"), Ok(b"replacement".to_vec()));
        nodes.close(store, old);
        assert!(store.stat(REMOVED).is_err());

        // What a channel allows.
        let reading = open_node(nodes, store, ROOT, "e/b", open::READ).unwrap();
        assert_eq!(write(nodes, store, reading, "", b"x"), Status::ReadOnly, "opened to read");
        assert_eq!(ask(nodes, store, reading, protocol::TOUCH, "", 0, &[]).0, Status::Ok, "but it may be stamped: it could have been opened to write");
        let only_reading = open_node(nodes, store, ROOT_READ_ONLY, "e/b", open::READ).unwrap();
        assert_eq!(ask(nodes, store, only_reading, protocol::TOUCH, "", 0, &[]).0, Status::ReadOnly, "from a read-only channel, not even that");
        nodes.close(store, only_reading);
        let from_read_only = open_node(nodes, store, ROOT_READ_ONLY, "e", open::DIRECTORY | open::WRITE);
        assert_eq!(from_read_only, Err(Status::ReadOnly), "a read-only channel gives no more");
        let readonly_dir = open_node(nodes, store, ROOT, "e", open::DIRECTORY).unwrap();
        assert_eq!(ask(nodes, store, readonly_dir, protocol::REMOVE, "b", 0, &[]).0, Status::ReadOnly);
        assert_eq!(read(nodes, store, readonly_dir, "../e/b"), Err(Status::BadName), "no way up and out");

        // The flags.
        assert_eq!(open_node(nodes, store, ROOT, "e", open::READ), Err(Status::IsDirectory));
        assert_eq!(open_node(nodes, store, ROOT, "e/b", open::DIRECTORY), Err(Status::NotDirectory));
        assert_eq!(open_node(nodes, store, ROOT, "e/b", open::CREATE | open::EXCLUSIVE | open::WRITE), Err(Status::Exists));
        assert_eq!(open_node(nodes, store, ROOT, "nothing", open::READ), Err(Status::NotFound));
        assert_eq!(open_node(nodes, store, ROOT_READ_ONLY, "made", open::CREATE), Err(Status::ReadOnly));
        let made = open_node(nodes, store, ROOT, "made", open::CREATE | open::WRITE).unwrap();
        assert_eq!(read(nodes, store, made, ""), Ok(vec![]));
        let emptied = open_node(nodes, store, ROOT, "e/b", open::WRITE | open::TRUNCATE).unwrap();
        assert_eq!(read(nodes, store, emptied, ""), Ok(vec![]));
        assert_eq!(open_node(nodes, store, ROOT, "a", 1 << 20), Err(Status::BadRequest), "an unknown flag");

        // An empty directory removed while open: its node finds nothing.
        assert_eq!(ask(nodes, store, ROOT, protocol::MKDIR, "empty", 0, &[]).0, Status::Ok);
        let empty = open_node(nodes, store, ROOT, "empty", open::DIRECTORY).unwrap();
        assert_eq!(ask(nodes, store, ROOT, protocol::REMOVE, "empty", 0, &[]).0, Status::Ok);
        assert_eq!(ask(nodes, store, empty, protocol::LIST, "", 0, &[]).0, Status::NotFound);
        assert_eq!(ask(nodes, store, ROOT, protocol::MKDIR, "empty", 0, &[]).0, Status::Ok);
        assert_eq!(ask(nodes, store, empty, protocol::LIST, "", 0, &[]).0, Status::NotFound, "a new one of that name is not it");
        for id in [reading, readonly_dir, made, emptied, empty, dir] {
            nodes.close(store, id);
        }
        assert_eq!(nodes.open_count(), 0);
    }

    #[test]
    fn open_files_follow_their_file_in_memory() {
        open_files_follow_their_file(&mut RamStore::new());
    }

    #[test]
    fn open_files_follow_their_file_on_disk() {
        let mut device = MemDevice::new(512);
        format_volume(&mut device, 512).unwrap();
        open_files_follow_their_file(&mut DiskStore::mount(device).unwrap());
    }

    #[test]
    fn a_removed_open_file_left_by_a_stop_is_removed_at_the_start() {
        let mut device = MemDevice::new(256);
        format_volume(&mut device, 256).unwrap();
        let mut store = DiskStore::mount(device).unwrap();
        let mut nodes = Nodes::new(&mut store);
        assert_eq!(write(&mut nodes, &mut store, ROOT, "tmp", b"scratch"), Status::Ok);
        let _open = open_node(&mut nodes, &mut store, ROOT, "tmp", open::READ).unwrap();
        assert_eq!(ask(&mut nodes, &mut store, ROOT, protocol::REMOVE, "tmp", 0, &[]).0, Status::Ok);
        let free = store.free_blocks();
        // The service stops here, its node never closed; the volume is
        // mounted again.
        let mut store = DiskStore::mount(store.into_device()).unwrap();
        assert!(store.stat(REMOVED).is_ok(), "it is on the disk");
        let _nodes = Nodes::new(&mut store);
        assert!(store.stat(REMOVED).is_err(), "and gone once a service starts");
        assert!(store.free_blocks() > free, "its blocks free again");
    }

    #[test]
    fn listing_the_root_pages_past_the_hidden_directory() {
        let mut store = RamStore::new();
        let mut nodes = Nodes::new(&mut store);
        for name in ["a", "m", "z"] {
            assert_eq!(write(&mut nodes, &mut store, ROOT, name, b"."), Status::Ok);
        }
        let open = open_node(&mut nodes, &mut store, ROOT, "m", open::READ).unwrap();
        assert_eq!(ask(&mut nodes, &mut store, ROOT, protocol::REMOVE, "m", 0, &[]).0, Status::Ok);
        assert_eq!(listed(&mut nodes, &mut store, ROOT, ""), vec!["a", "z"], "one at a time");
        let (_, payload) = ask(&mut nodes, &mut store, ROOT, protocol::LIST, "", 0, &[]);
        let all: Vec<&str> = protocol::entries(&payload).map(|(name, _, _)| name).collect();
        assert_eq!(all, vec!["a", "z"], "all at once");
        nodes.close(&mut store, open);
    }

    #[test]
    fn open_files_are_counted() {
        let mut store = RamStore::new();
        let mut nodes = Nodes::new(&mut store);
        assert_eq!(write(&mut nodes, &mut store, ROOT, "f", b"."), Status::Ok);
        let ids: Vec<NodeId> = (0..protocol::OPEN_MAX).map(|_| open_node(&mut nodes, &mut store, ROOT, "f", open::READ).unwrap()).collect();
        assert_eq!(open_node(&mut nodes, &mut store, ROOT, "f", open::READ), Err(Status::Busy));
        nodes.close(&mut store, ids[7]);
        let again = open_node(&mut nodes, &mut store, ROOT, "f", open::READ).unwrap();
        assert_eq!(again, ids[7], "a free slot is used again");
        nodes.close(&mut store, ROOT);
        assert!(nodes.get(ROOT).is_some(), "the root channels are never closed");
    }
}
