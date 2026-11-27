//! fs - the file service: files on the disk, for whoever holds a
//! channel to it.
//!
//! Startup handles, from init:
//!   1  the request channel (receive only). Every client sends on the
//!      other end; each request carries the end of a channel to reply on.
//!   2  a second request channel, for reading only: the same requests,
//!      but any that would change a file is refused. Which channel a
//!      client holds is its permission - there is nothing to check but
//!      where the request came from.
//!   3  the channel to the registry, on which fs asks for the one
//!      service it uses: "disk", the block driver's request channel
//!
//! With a disk that holds a TaajFS volume, files live there: each
//! request is one copy-on-write commit, on the disk before the reply goes
//! out, and they survive restarts of this service and reboots of the
//! machine. With no disk - or one with no volume on it - files live in
//! this process's memory, and go with it.
//!
//! It never formats a disk: a disk with something else on it is not
//! this service's to erase. Volumes are made with tfstool, on the host.
//!
//! Between TaajFS and the disk sits a block cache (taajfs::Cache): the
//! blocks every request needs again - the directory, the tree above it -
//! are read from the disk once. Writes are not held back: each goes to
//! the disk before the request is answered, as before.
//!
//! A commit costs the same whether it adds a byte or a megabyte - the
//! new inode, directories and superblock are written either way - so
//! this service makes as few as it can: a WRITE_FROM brings up to 1 MiB
//! in a memory object, one commit; and the messages waiting on an
//! output stream are gathered and added to the file together, one
//! commit for all of them, not one each.
//!
//! All the logic - the protocol, both stores, the mapping onto TaajFS -
//! is in the `fileserver` crate, tested on the host. This program only
//! connects it to channels: requests in, and disk sectors out.
//!
//! Streams (protocol::STREAM_OUT, STREAM_IN): a file as a program's
//! output or input. The request brings one end of a channel, which this
//! service then watches with its request channels: what arrives on it is
//! added to the file, or the file's bytes are sent on it, until it
//! closes. Before any request is answered, every output stream is
//! emptied into its file: a program that waited for another to end, and
//! then reads what it wrote, finds all of it there.
//!
//! Open files (protocol::OPEN, J60): a client may open a file or a
//! directory, and gets a channel of its own to it - this service keeps
//! the other end, and watches it with the rest. Which node each channel
//! stands for, and the paths that follow renames and removals, are the
//! `fileserver` crate's (`Nodes`); here a channel is only matched to its
//! node, and a node let go when its channel closes. The two request
//! channels above are the root, open to change and open to read: every
//! channel speaks the same protocol.

#![no_std]
#![no_main]

extern crate alloc;

use fileserver::{DiskStore, RamStore};
use taaj::disk::{Disk, Window};
use taaj::services::{ServiceError, Services};
use taaj::{println, Handle};
use taajfs::format::{Block, BLOCK_SIZE};
use taajfs::{BlockDevice, Cache, DeviceError};

// The request loop, shared with dev (J65).
#[path = "common/serve.rs"]
mod serve;

taaj::program!(main);

const SECTOR: usize = abi::block::SECTOR;
const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / SECTOR) as u64;
/// Blocks kept in memory: 1 MiB. Plenty for the metadata of a small
/// volume, and for a program read twice.
const CACHE_BLOCKS: usize = 256;

/// The disk as TaajFS sees it: 4 KiB blocks, each one request to the
/// block driver for eight sectors, through a window of shared memory -
/// or a run of neighbouring blocks, a file's data, as few requests as
/// the window allows (32 blocks each).
struct DiskDevice {
    disk: Disk,
    window: Window,
    /// Where the volume is: from sector `start`, `sectors` long - the
    /// whole disk, or a partition (a USB stick made to boot from).
    start: u64,
    sectors: u64,
}

fn device_error(what: &'static str) -> impl Fn(taaj::disk::DiskError) -> DeviceError {
    move |error| DeviceError {
        what,
        code: match error {
            taaj::disk::DiskError::Refused(status) => Some(i64::from(status)),
            taaj::disk::DiskError::Unreachable(_) => None,
        },
    }
}

impl DiskDevice {
    fn first_sector(&self, number: u32) -> Result<u64, DeviceError> {
        let first = u64::from(number) * SECTORS_PER_BLOCK;
        if first + SECTORS_PER_BLOCK > self.sectors {
            return Err(DeviceError::new("block past the end of the disk"));
        }
        Ok(self.start + first)
    }
}

impl BlockDevice for DiskDevice {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
        let first = self.first_sector(number)?;
        self.disk.read_many(&mut self.window, first, SECTORS_PER_BLOCK as u32).map_err(device_error("disk read"))?;
        buffer.copy_from_slice(&self.window.bytes()[..BLOCK_SIZE]);
        Ok(())
    }

    fn read_blocks(&mut self, first: u32, buffer: &mut [u8]) -> Result<(), DeviceError> {
        let per_request = (self.window.sectors() as usize * SECTOR / BLOCK_SIZE).max(1);
        for (n, part) in buffer.chunks_mut(per_request * BLOCK_SIZE).enumerate() {
            let blocks = (part.len() / BLOCK_SIZE) as u32;
            let start = self.first_sector(first + (n * per_request) as u32)?;
            if start + u64::from(blocks) * SECTORS_PER_BLOCK > self.start + self.sectors {
                return Err(DeviceError::new("block past the end of the disk"));
            }
            let sectors = blocks * SECTORS_PER_BLOCK as u32;
            self.disk.read_many(&mut self.window, start, sectors).map_err(device_error("disk read"))?;
            part.copy_from_slice(&self.window.bytes()[..part.len()]);
        }
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
        let first = self.first_sector(number)?;
        self.window.bytes()[..BLOCK_SIZE].copy_from_slice(buffer);
        self.disk.write_many(&mut self.window, first, SECTORS_PER_BLOCK as u32).map_err(device_error("disk write"))
    }

    fn flush(&mut self) -> Result<(), DeviceError> {
        self.disk.flush().map_err(device_error("disk flush"))
    }
}

/// The volume on the disk, if there is one this service can use.
fn mount(disk: Handle) -> Option<DiskStore<Cache<DiskDevice>>> {
    let disk = Disk::new(disk);
    let sectors = match disk.info() {
        Ok((sectors, false)) => sectors,
        Ok((_, true)) => {
            println!("[fs] the disk is read-only: files in memory only");
            return None;
        }
        Err(error) => {
            println!("[fs] no disk ({}): files in memory only", error.describe());
            return None;
        }
    };
    // The volume: in a TaajFS partition if sector 0 is a partition table
    // that lists one (taajfs::partition), else the whole disk.
    let mut sector0 = [0u8; abi::block::SECTOR];
    let (start, sectors) = match disk.read(0, &mut sector0) {
        Ok(()) => match taajfs::partition::find_taajfs(&sector0, sectors) {
            Some((first, count)) => {
                println!("[fs] TaajFS partition: sectors {first} to {}", first + count - 1);
                (first, count)
            }
            None => (0, sectors),
        },
        Err(error) => {
            println!("[fs] cannot read the disk ({}): files in memory only", error.describe());
            return None;
        }
    };
    let window = match Window::new(abi::block::SECTORS_MAX) {
        Ok(window) => window,
        Err(error) => {
            println!("[fs] no memory to share with the disk driver ({error:?}): files in memory only");
            return None;
        }
    };
    match DiskStore::mount(Cache::new(DiskDevice { disk, window, start, sectors }, CACHE_BLOCKS)) {
        Ok(mut store) if u64::from(store.total_blocks()) * SECTORS_PER_BLOCK <= sectors => {
            // A file's time is the machine's (TaajFS version 6): the
            // real-time clock at boot, and the counter since.
            store.set_clock(taaj::realtime_ns);
            Some(store)
        }
        Ok(store) => {
            println!("[fs] the volume says it has {} blocks; the disk is smaller - not using it", store.total_blocks());
            None
        }
        Err(error) => {
            println!("[fs] no TaajFS volume on the disk ({error}): files in memory only");
            None
        }
    }
}

fn main() -> i64 {
    let Some(requests) = taaj::startup_handle(0) else {
        println!("[fs] started without a request channel");
        return 1;
    };
    let look = taaj::startup_handle(1);
    // The disk driver is asked for by name, on the channel to the
    // registry - if there is one: a test may start this with none.
    let disk = match taaj::startup_handle(2).map(|registry| Services::new(registry).lookup("disk")) {
        Some(Ok(disk)) => Some(disk),
        Some(Err(ServiceError::NotFound)) | None => None,
        // Not knowing is not the same as there being no disk: to go on
        // with files in memory would hide every file there is. End,
        // and be started again.
        Some(Err(error)) => {
            println!("[fs] cannot ask for the disk: {}", error.describe());
            return 1;
        }
    };
    match disk.and_then(mount) {
        Some(mut store) => {
            println!(
                "[fs] ready: TaajFS on the disk, {} of {} blocks free; a cache of {} blocks",
                store.free_blocks(),
                store.total_blocks(),
                CACHE_BLOCKS
            );
            serve::serve("fs", &mut store, requests, look)
        }
        None => {
            println!(
                "[fs] ready: files in memory, up to {} of {} KiB each, {} MiB in all",
                fileserver::ram::FILES_MAX,
                fileserver::ram::FILE_MAX / 1024,
                fileserver::ram::BYTES_MAX >> 20
            );
            serve::serve("fs", &mut RamStore::new(), requests, look)
        }
    }
}
