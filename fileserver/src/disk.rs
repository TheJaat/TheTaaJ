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
//! TaajFS names are shorter than the protocol allows: 27 bytes.

use alloc::vec;
use alloc::vec::Vec;

use taajfs::format::{MAX_FILE_SIZE, NAME_MAX as TAAJFS_NAME_MAX};
use taajfs::{BlockDevice, Error, Volume};

use crate::protocol::{self, Status};
use crate::{valid_name, Store};

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
        Error::Exists | Error::BadSize => Status::BadRequest,
        Error::Device(_) | Error::Magic | Error::NoSuper | Error::Corrupt(_) => Status::DiskError,
    }
}

/// A protocol name that TaajFS can also store.
fn check(name: &str) -> Result<&str, Status> {
    if valid_name(name) && name.len() < TAAJFS_NAME_MAX {
        Ok(name)
    } else {
        Err(Status::BadName)
    }
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

    /// The size of `name`, or None if there is no such file.
    fn existing(&mut self, name: &str) -> Result<Option<u64>, Status> {
        match self.volume.lookup(name) {
            Ok(file) => Ok(Some(u64::from(file.size))),
            Err(Error::NotFound) => Ok(None),
            Err(error) => Err(status(error)),
        }
    }
}

impl<D: BlockDevice> Store for DiskStore<D> {
    fn list(&mut self, from: usize, each: &mut dyn FnMut(&str, u64) -> bool) -> Result<(), Status> {
        // On disk, the directory is in the order files were made; the
        // protocol promises name order, so that paging is stable.
        let mut files = self.volume.list().map_err(status)?;
        files.sort_by(|a, b| a.name.cmp(&b.name));
        for file in files.iter().skip(from) {
            if !each(&file.name, u64::from(file.size)) {
                break;
            }
        }
        Ok(())
    }

    fn size(&mut self, name: &str) -> Result<u64, Status> {
        self.existing(name)?.ok_or(Status::NotFound)
    }

    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let file = self.volume.lookup(name).map_err(status)?;
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
        self.volume.delete(name).map_err(status)
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
        let names: Vec<&str> = protocol::entries(&payload).map(|(n, _)| n).collect();
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
        assert_eq!(s.write("a/b", 0, b"x"), Err(Status::BadName));
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
