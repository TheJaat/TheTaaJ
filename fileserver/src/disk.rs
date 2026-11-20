//! Files on a disk: a TaajFS volume, through any block device.
//!
//! Each request becomes one TaajFS change, and every TaajFS change is
//! copy-on-write and committed by a single superblock write before it
//! returns. So when the client hears "ok", the change is on the disk;
//! and if the power fails at any moment, the disk holds either the state
//! before the request or the state after it - never half of it.
//!
//! The protocol asks for a little more than TaajFS offers directly, and
//! each request still has to be one commit, so that a refused or
//! interrupted request changes nothing:
//!
//!   - writing a file that does not exist yet: `put` of its whole
//!     contents (zeros, then the data) - create and fill in one commit,
//!     instead of create, then write
//!   - truncate: `put` of the shortened (or zero-extended) contents
//!
//! Each name in a path is shorter on TaajFS than the protocol allows:
//! 27 bytes. Directories are TaajFS's own; a path is handed to it whole.

use alloc::vec;
use alloc::vec::Vec;

use taajfs::format::{MAX_FILE_SIZE, NAME_MAX as TAAJFS_NAME_MAX};
use taajfs::{BlockDevice, Error, Volume};

use crate::protocol::{self, Kind, Status};
use crate::{names, valid_directory, valid_path, Store};

pub struct DiskStore<D: BlockDevice> {
    volume: Volume<D>,
}

/// What a TaajFS error means to a client.
fn status(error: Error) -> Status {
    match error {
        Error::NotFound => Status::NotFound,
        Error::Name => Status::BadName,
        Error::Full => Status::Full,
        Error::TooBig => Status::TooBig,
        Error::Exists => Status::Exists,
        Error::NotDirectory => Status::NotDirectory,
        Error::IsDirectory => Status::IsDirectory,
        Error::NotEmpty => Status::NotEmpty,
        Error::IntoItself => Status::IntoItself,
        Error::BadSize => Status::BadRequest,
        Error::Device(_) | Error::Magic | Error::NoSuper | Error::Corrupt(_) => Status::DiskError,
    }
}

/// A protocol path, to a file, that TaajFS can also store.
fn check(path: &str) -> Result<&str, Status> {
    if valid_path(path) { check_directory(path) } else { Err(Status::BadName) }
}

/// The same, for a path that may also be the root.
fn check_directory(path: &str) -> Result<&str, Status> {
    if valid_directory(path) && names(path).all(|name| name.len() < TAAJFS_NAME_MAX) {
        Ok(path)
    } else {
        Err(Status::BadName)
    }
}

fn kind(file: &taajfs::FileInfo) -> Kind {
    if file.kind == taajfs::format::TYPE_DIR { Kind::Directory } else { Kind::File }
}

impl<D: BlockDevice> DiskStore<D> {
    /// Mounts the volume on `device`. The error says why not - most
    /// often, that there is no TaajFS volume there.
    pub fn mount(device: D) -> Result<DiskStore<D>, Error> {
        Ok(DiskStore { volume: Volume::mount(device)? })
    }

    pub fn free_blocks(&self) -> u32 {
        self.volume.free_blocks()
    }

    pub fn total_blocks(&self) -> u32 {
        self.volume.superblock().total_blocks
    }

    /// Unmounts, handing the device back.
    pub fn into_device(self) -> D {
        self.volume.into_device()
    }

    /// The size of the file `name`, or None if there is nothing there.
    fn existing(&mut self, name: &str) -> Result<Option<u64>, Status> {
        match self.volume.lookup(name) {
            Ok(file) if kind(&file) == Kind::Directory => Err(Status::IsDirectory),
            Ok(file) => Ok(Some(u64::from(file.size))),
            Err(Error::NotFound) => Ok(None),
            Err(error) => Err(status(error)),
        }
    }
}

impl<D: BlockDevice> Store for DiskStore<D> {
    fn list(
        &mut self,
        directory: &str,
        from: usize,
        each: &mut dyn FnMut(&str, Kind, u64) -> bool,
    ) -> Result<(), Status> {
        // On disk, a directory is in the order things were made; the
        // protocol promises name order, so that paging is stable.
        let mut files = self.volume.list_in(check_directory(directory)?).map_err(status)?;
        files.sort_by(|a, b| a.name.cmp(&b.name));
        for file in files.iter().skip(from) {
            if !each(&file.name, kind(file), u64::from(file.size)) {
                break;
            }
        }
        Ok(())
    }

    fn stat(&mut self, path: &str) -> Result<(Kind, u64), Status> {
        let file = self.volume.lookup(check_directory(path)?).map_err(status)?;
        Ok((kind(&file), u64::from(file.size)))
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Status> {
        self.volume.make_directory(check(path)?).map_err(status)
    }

    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let file = self.volume.lookup(check(name)?).map_err(status)?;
        let Ok(offset) = u32::try_from(offset) else { return Ok(0) }; // far past the end
        self.volume.read_at(&file, offset, out).map_err(status)
    }

    fn write(&mut self, name: &str, offset: u64, data: &[u8]) -> Result<u64, Status> {
        let name = check(name)?;
        let size = self.existing(name)?;
        let start = if offset == protocol::APPEND { size.unwrap_or(0) } else { offset };
        let end = start.checked_add(data.len() as u64).filter(|&end| end <= MAX_FILE_SIZE as u64);
        let (Some(end), Ok(start32)) = (end, u32::try_from(start)) else {
            return Err(Status::TooBig);
        };
        match size {
            Some(size) => {
                self.volume.write_at(name, start32, data).map_err(status)?;
                Ok(size.max(end))
            }
            None => {
                // New: made and filled in one commit.
                let mut contents = vec![0u8; start32 as usize];
                contents.extend_from_slice(data);
                self.volume.put(name, &contents).map_err(status)?;
                Ok(end)
            }
        }
    }

    fn truncate(&mut self, name: &str, length: u64) -> Result<(), Status> {
        let name = check(name)?;
        let length = usize::try_from(length).ok().filter(|&n| n <= MAX_FILE_SIZE).ok_or(Status::TooBig)?;
        let mut contents = match self.existing(name)? {
            Some(_) if length > 0 => {
                let file = self.volume.lookup(name).map_err(status)?;
                self.volume.read_all(&file).map_err(status)?
            }
            _ => Vec::new(),
        };
        contents.resize(length, 0);
        self.volume.put(name, &contents).map_err(status)
    }

    fn remove(&mut self, name: &str) -> Result<(), Status> {
        self.volume.delete(check(name)?).map_err(status)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Status> {
        // TaajFS does it in one commit: see Volume::rename.
        self.volume.rename(check(from)?, check(to)?).map_err(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Request, APPEND, LIST, MESSAGE_MAX, READ};
    use crate::{serve, Access};
    use taajfs::format::BLOCK_SIZE;
    use taajfs::{format_volume, MemDevice};

    const BLOCKS: u32 = 256;

    fn fresh() -> DiskStore<MemDevice> {
        let mut device = MemDevice::new(BLOCKS);
        format_volume(&mut device, BLOCKS).unwrap();
        DiskStore::mount(device).unwrap()
    }

    /// As if the machine rebooted: only what reached the disk is left.
    fn remount(store: DiskStore<MemDevice>) -> DiskStore<MemDevice> {
        DiskStore::mount(store.into_device()).unwrap()
    }

    fn contents(store: &mut DiskStore<MemDevice>, name: &str) -> Vec<u8> {
        let mut data = vec![0u8; store.size(name).unwrap() as usize];
        let n = store.read(name, 0, &mut data).unwrap();
        data.truncate(n);
        data
    }

    #[test]
    fn files_survive_a_remount() {
        let mut s = fresh();
        assert_eq!(s.write("notes", 0, b"hello\n"), Ok(6));
        assert_eq!(s.write("notes", APPEND, b"world\n"), Ok(12));
        assert_eq!(s.write("big", 0, &vec![7; 3 * BLOCK_SIZE + 5]), Ok(3 * BLOCK_SIZE as u64 + 5));
        let mut s = remount(s);
        assert_eq!(contents(&mut s, "notes"), b"hello\nworld\n");
        assert_eq!(contents(&mut s, "big"), vec![7; 3 * BLOCK_SIZE + 5]);
        assert_eq!(s.size("notes"), Ok(12));
    }

    #[test]
    fn the_protocol_works_over_a_disk() {
        // The same requests the shell sends, through serve.
        let mut s = fresh();
        let mut message = [0u8; MESSAGE_MAX];
        let mut reply = [0u8; MESSAGE_MAX];
        let mut call = |s: &mut DiskStore<MemDevice>, request: Request| {
            let n = request.encode(&mut message).unwrap();
            let length = serve(s, Access::ReadWrite, &message[..n], &mut reply);
            (Status::from_u8(reply[0]), reply[1..length].to_vec())
        };
        for name in ["zebra", "apple", "mango"] {
            assert_eq!(call(&mut s, Request { op: protocol::WRITE, name, number: 0, data: name.as_bytes() }).0, Status::Ok);
        }
        let (status, payload) = call(&mut s, Request::new(LIST, "", 0));
        let names: Vec<&str> = protocol::entries(&payload).map(|(n, _, _)| n).collect();
        assert_eq!((status, names), (Status::Ok, vec!["apple", "mango", "zebra"]), "name order, not creation order");
        let (_, payload) = call(&mut s, Request::new(LIST, "", 2));
        assert_eq!(protocol::entries(&payload).count(), 1, "paging from an index");
        assert_eq!(call(&mut s, Request::new(READ, "mango", 2)), (Status::Ok, b"ngo".to_vec()));
        assert_eq!(call(&mut s, Request::new(READ, "kiwi", 0)).0, Status::NotFound);
    }

    #[test]
    fn writes_past_the_end_and_truncate() {
        let mut s = fresh();
        assert_eq!(s.write("gap", 5, b"x"), Ok(6), "a new file with a gap");
        assert_eq!(contents(&mut s, "gap"), b"\0\0\0\0\0x");
        s.write("gap", 8, b"yz").unwrap();
        assert_eq!(contents(&mut s, "gap"), b"\0\0\0\0\0x\0\0yz");
        s.truncate("gap", 6).unwrap();
        assert_eq!(contents(&mut s, "gap"), b"\0\0\0\0\0x");
        s.truncate("gap", 0).unwrap();
        assert_eq!(s.size("gap"), Ok(0));
        s.truncate("new", 3).unwrap();
        assert_eq!(contents(&mut remount(s), "new"), b"\0\0\0");
    }

    #[test]
    fn refusals_change_nothing() {
        let mut s = fresh();
        s.write("keep", 0, b"safe").unwrap();
        let free = s.free_blocks();
        assert_eq!(s.write(&"n".repeat(27), 0, b"x").map(|_| ()), Ok(()), "27 bytes fit");
        assert_eq!(s.write(&"n".repeat(28), 0, b"x"), Err(Status::BadName), "28 do not, on TaajFS");
        assert_eq!(s.write("a/b", 0, b"x"), Err(Status::NotFound), "no directory a");
        assert_eq!(s.write("keep/b", 0, b"x"), Err(Status::NotDirectory));
        assert_eq!(s.write("a/..", 0, b"x"), Err(Status::BadName));
        assert_eq!(s.write("big", 0, &vec![0; MAX_FILE_SIZE + 1]), Err(Status::TooBig));
        assert_eq!(s.write("big", u64::from(u32::MAX) + 1, b"x"), Err(Status::TooBig));
        // A disk of 256 blocks cannot hold a megabyte: full, and the
        // half-written file is not there.
        assert_eq!(s.write("huge", 0, &vec![1; 1 << 20]), Err(Status::Full));
        assert_eq!(s.size("huge"), Err(Status::NotFound));
        assert_eq!(s.remove("nothing"), Err(Status::NotFound));
        s.remove(&"n".repeat(27)).unwrap();
        assert_eq!(s.free_blocks(), free, "every block of the refused writes came back");
        assert_eq!(contents(&mut remount(s), "keep"), b"safe");
    }

    #[test]
    fn renaming_on_a_disk() {
        let mut s = fresh();
        s.make_directory("docs").unwrap();
        s.make_directory("docs/notes").unwrap();
        s.write("docs/notes/today", 0, b"deep").unwrap();
        s.write("top", 0, b"t").unwrap();
        let free = s.free_blocks();
        let call = |s: &mut DiskStore<MemDevice>, access: Access, from: &str, to: &str| {
            let mut message = [0u8; MESSAGE_MAX];
            let mut reply = [0u8; MESSAGE_MAX];
            let request = Request { op: protocol::RENAME, name: from, number: 0, data: to.as_bytes() };
            let n = request.encode(&mut message).unwrap();
            assert_eq!(serve(s, access, &message[..n], &mut reply), 1, "a status and nothing more");
            Status::from_u8(reply[0])
        };
        let rw = Access::ReadWrite;
        assert_eq!(call(&mut s, Access::ReadOnly, "top", "bottom"), Status::ReadOnly);
        assert_eq!(call(&mut s, rw, "top", "bottom"), Status::Ok);
        assert_eq!(call(&mut s, rw, "bottom", "docs/notes/bottom"), Status::Ok);
        assert_eq!(call(&mut s, rw, "docs/notes", "notes"), Status::Ok);
        assert_eq!(call(&mut s, rw, "notes", "docs"), Status::Exists);
        assert_eq!(call(&mut s, rw, "notes", "notes/inside"), Status::IntoItself);
        assert_eq!(call(&mut s, rw, "nothing", "x"), Status::NotFound);
        assert_eq!(call(&mut s, rw, "notes", "nowhere/x"), Status::NotFound);
        assert_eq!(call(&mut s, rw, "docs", "notes/today/x"), Status::NotDirectory);
        assert_eq!(call(&mut s, rw, "notes", "a/../b"), Status::BadName);
        assert_eq!(call(&mut s, rw, "notes", &"n".repeat(28)), Status::BadName, "too long for TaajFS");
        assert_eq!(call(&mut s, rw, "notes", ""), Status::BadName);
        assert_eq!(s.free_blocks(), free, "a move keeps no blocks, and a refused one takes none");

        let mut s = remount(s);
        assert_eq!(contents(&mut s, "notes/today"), b"deep");
        assert_eq!(contents(&mut s, "notes/bottom"), b"t");
        assert_eq!(s.stat("top"), Err(Status::NotFound));
        assert_eq!(s.stat("docs/notes"), Err(Status::NotFound));
        assert_eq!(s.stat("docs"), Ok((Kind::Directory, 0)));
    }

    #[test]
    fn directories_on_a_disk() {
        let mut s = fresh();
        let free = s.free_blocks();
        assert_eq!(s.make_directory("docs"), Ok(()));
        assert_eq!(s.make_directory("docs/notes"), Ok(()));
        assert_eq!(s.write("docs/notes/today", 0, b"deep"), Ok(4));
        assert_eq!(s.write("docs/notes/today", APPEND, b"er"), Ok(6));
        assert_eq!(s.truncate("docs/plan", 3), Ok(()));
        assert_eq!(s.write("top", 0, b"t"), Ok(1));

        let mut s = remount(s);
        let call = |s: &mut DiskStore<MemDevice>, request: Request| {
            let mut message = [0u8; MESSAGE_MAX];
            let mut reply = [0u8; MESSAGE_MAX];
            let n = request.encode(&mut message).unwrap();
            let length = serve(s, Access::ReadWrite, &message[..n], &mut reply);
            (Status::from_u8(reply[0]), reply[1..length].to_vec())
        };
        let listing = |payload: &[u8]| -> Vec<(String, Kind, u64)> {
            protocol::entries(payload).map(|(n, k, s)| (n.to_string(), k, s)).collect()
        };
        let (status, payload) = call(&mut s, Request::new(LIST, "", 0));
        assert_eq!(status, Status::Ok);
        assert_eq!(listing(&payload), [("docs".to_string(), Kind::Directory, 0), ("top".to_string(), Kind::File, 1)]);
        let (_, payload) = call(&mut s, Request::new(LIST, "/docs/", 0));
        assert_eq!(listing(&payload), [("notes".to_string(), Kind::Directory, 0), ("plan".to_string(), Kind::File, 3)]);
        assert_eq!(call(&mut s, Request::new(LIST, "top", 0)).0, Status::NotDirectory);
        assert_eq!(call(&mut s, Request::new(LIST, "nowhere", 0)).0, Status::NotFound);
        assert_eq!(call(&mut s, Request::new(READ, "docs/notes/today", 0)), (Status::Ok, b"deeper".to_vec()));
        assert_eq!(protocol::stat(&call(&mut s, Request::new(protocol::STAT, "docs", 0)).1), Some((Kind::Directory, 0)));
        assert_eq!(protocol::stat(&call(&mut s, Request::new(protocol::STAT, "", 0)).1), Some((Kind::Directory, 0)));
        assert_eq!(call(&mut s, Request::new(protocol::MKDIR, "docs", 0)).0, Status::Exists);
        assert_eq!(call(&mut s, Request::new(READ, "docs", 0)).0, Status::IsDirectory);
        assert_eq!(call(&mut s, Request::new(protocol::TRUNCATE, "docs", 0)).0, Status::IsDirectory);
        assert_eq!(call(&mut s, Request { op: protocol::WRITE, name: "docs", number: 0, data: b"x" }).0, Status::IsDirectory);
        assert_eq!(call(&mut s, Request::new(protocol::REMOVE, "docs", 0)).0, Status::NotEmpty);

        for path in ["docs/notes/today", "docs/notes", "docs/plan", "docs", "top"] {
            assert_eq!(s.remove(path), Ok(()), "{path}");
        }
        assert_eq!(s.free_blocks(), free, "every block came back");
        assert_eq!(remount(s).free_blocks(), free);
    }

    #[test]
    fn a_power_cut_during_any_request_leaves_old_or_new() {
        // The copy-on-write promise, through the protocol's operations:
        // cut the power after every possible number of writes.
        let mut base = fresh();
        base.write("log", 0, b"one\n").unwrap();
        let base = base.into_device();
        for allowed in 0.. {
            let device = MemDevice { blocks: base.blocks.clone(), writes_left: Some(allowed) };
            let mut s = DiskStore::mount(device).unwrap();
            let done = s.write("log", APPEND, b"two\n").is_ok() && s.truncate("log", 4).is_ok();
            let mut device = s.into_device();
            device.writes_left = None;
            let mut s = DiskStore::mount(device).expect("always mountable");
            let log = contents(&mut s, "log");
            assert!(
                log == b"one\n" || log == b"one\ntwo\n",
                "after {allowed} writes: {:?}",
                String::from_utf8_lossy(&log)
            );
            if done {
                assert_eq!(log, b"one\n", "both requests done: appended, then cut back");
                break;
            }
        }
    }
}
