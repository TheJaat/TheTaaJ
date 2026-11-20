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
//! All the logic - the protocol, both stores, the mapping onto TaajFS -
//! is in the `fileserver` crate, tested on the host. This program only
//! connects it to channels: requests in, and disk sectors out.

#![no_std]
#![no_main]

use fileserver::protocol::{self, Request, Status, MESSAGE_MAX};
use fileserver::{Access, DiskStore, RamStore, Store};
use taaj::disk::{Disk, Window};
use taaj::services::{ServiceError, Services};
use taaj::{println, Error, Handle};
use taajfs::format::{Block, BLOCK_SIZE};
use taajfs::{BlockDevice, Cache, DeviceError};

taaj::program!(main);

const SECTOR: usize = abi::block::SECTOR;
const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / SECTOR) as u64;
/// Blocks kept in memory: 1 MiB. Plenty for the metadata of a small
/// volume, and for a program read twice.
const CACHE_BLOCKS: usize = 256;

/// The disk as TaajFS sees it: 4 KiB blocks, each one request to the
/// block driver for eight sectors, through a window of shared memory.
struct DiskDevice {
    disk: Disk,
    window: Window,
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
        Ok(first)
    }
}

impl BlockDevice for DiskDevice {
    fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
        let first = self.first_sector(number)?;
        self.disk.read_many(&mut self.window, first, SECTORS_PER_BLOCK as u32).map_err(device_error("disk read"))?;
        buffer.copy_from_slice(&self.window.bytes()[..BLOCK_SIZE]);
        Ok(())
    }

    fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
        let first = self.first_sector(number)?;
        self.window.bytes()[..BLOCK_SIZE].copy_from_slice(buffer);
        self.disk.write_many(&mut self.window, first, SECTORS_PER_BLOCK as u32).map_err(device_error("disk write"))
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
    let window = match Window::new(SECTORS_PER_BLOCK as u32) {
        Ok(window) => window,
        Err(error) => {
            println!("[fs] no memory to share with the disk driver ({error:?}): files in memory only");
            return None;
        }
    };
    match DiskStore::mount(Cache::new(DiskDevice { disk, window, sectors }, CACHE_BLOCKS)) {
        Ok(store) if u64::from(store.total_blocks()) * SECTORS_PER_BLOCK <= sectors => Some(store),
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
            serve(&mut store, requests, look)
        }
        None => {
            println!(
                "[fs] ready: files in memory, up to {} of {} KiB each, {} MiB in all",
                fileserver::ram::FILES_MAX,
                fileserver::ram::FILE_MAX / 1024,
                fileserver::ram::BYTES_MAX >> 20
            );
            serve(&mut RamStore::new(), requests, look)
        }
    }
}

fn serve(store: &mut impl Store, requests: Handle, look: Option<Handle>) -> i64 {
    let mut message = [0u8; MESSAGE_MAX];
    let mut reply = [0u8; MESSAGE_MAX];
    // The channels watched: requests that may change files, then -
    // while anyone holds its other end - requests that may only look.
    let channels = [requests, look.unwrap_or(0)];
    let mut watched = 1 + usize::from(look.is_some());
    loop {
        let from = match taaj::object_wait(&channels[..watched]) {
            Ok(from) => from,
            Err(error) => {
                println!("[fs] cannot wait: {error:?}");
                return 2;
            }
        };
        let access = if from == 0 { Access::ReadWrite } else { Access::ReadOnly };
        // Room for the largest message and the most handles one can
        // carry: a message that did not fit would stay at the head of
        // the queue, and be refused again, forever.
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match taaj::receive(channels[from], &mut message, &mut handles) {
            Ok(received) => received,
            // The service lives as long as its first channel has a
            // sender; the second going only means fewer to watch.
            Err(Error::PeerClosed) if from == 0 => return 0,
            Err(Error::PeerClosed) => {
                watched = 1;
                continue;
            }
            Err(error) => {
                println!("[fs] cannot receive: {error:?}");
                return 2;
            }
        };
        // One handle, the reply channel. Anything else: not a request.
        if count != 1 {
            for &handle in &handles[..count] {
                let _ = taaj::close(handle);
            }
            continue;
        }
        let length = fileserver::serve(store, access, &message[..n], &mut reply);
        // A client that gave up has closed its end: nothing to do.
        let _ = taaj::send(handles[0], &reply[..length], &[]);
        let _ = taaj::close(handles[0]);
        // The test of a service that stops answering: answered - so
        // the one who asked is not the one kept waiting - then silence.
        if reply[0] == Status::Ok as u8 {
            if let Some(Request { op: protocol::STALL, number, .. }) = Request::decode(&message[..n]) {
                println!("[fs] asked to stall: answering nothing for {number} ticks");
                taaj::sleep(number);
                println!("[fs] answering again");
            }
        }
    }
}
