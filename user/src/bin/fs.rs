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

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use fileserver::protocol::{self, Request, Status, MESSAGE_MAX};
use fileserver::{Access, DiskStore, RamStore, Store, Stream};
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

/// A READ_INTO: the file's bytes, straight into the memory object the
/// client sent. The reply, or None if the message was not a READ_INTO
/// or the object cannot be mapped for writing (then there is no answer
/// worth sending: the client sees its reply channel closed).
fn read_into(store: &mut impl Store, message: &[u8], memory: Handle, reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
    let size = usize::try_from(taaj::memory_size(memory).ok()?).ok()?;
    let base = taaj::memory_map(memory, true).ok()?;
    // SAFETY: the mapping is `size` bytes long, writable, and ours until
    // it is unmapped below; nothing else in this program touches it.
    let into = unsafe { core::slice::from_raw_parts_mut(base, size) };
    let length = fileserver::serve_into(store, message, into, reply);
    let _ = taaj::memory_unmap(base);
    length
}

/// A WRITE_FROM: the bytes, straight from the memory object the client
/// sent. The reply, or None if the object cannot be mapped.
fn write_from(store: &mut impl Store, access: Access, message: &[u8], memory: Handle, reply: &mut [u8; MESSAGE_MAX]) -> Option<usize> {
    let size = usize::try_from(taaj::memory_size(memory).ok()?).ok()?;
    let base = taaj::memory_map(memory, false).ok()?;
    // SAFETY: the mapping is `size` bytes long and ours until it is
    // unmapped below; it is only read.
    let from = unsafe { core::slice::from_raw_parts(base, size) };
    let length = fileserver::serve_from(store, access, message, from, reply);
    let _ = taaj::memory_unmap(base);
    length
}

/// A stream this service keeps: the channel it travels on, and what it
/// is.
struct Open {
    channel: Handle,
    stream: Stream,
    /// An input stream whose next message found no room in the channel.
    full: bool,
}

/// Most messages taken from or sent on one stream at a time, so that
/// one busy stream does not keep the requests waiting.
const STREAM_TURN: usize = 64;

fn serve(store: &mut impl Store, requests: Handle, look: Option<Handle>) -> i64 {
    let mut message = [0u8; MESSAGE_MAX];
    let mut reply = [0u8; MESSAGE_MAX];
    // The channels watched: requests that may change files, then -
    // while anyone holds its other end - requests that may only look,
    // then the streams.
    let mut look = look;
    let mut streams: Vec<Open> = Vec::new();
    let mut watched: Vec<Handle> = Vec::new();
    loop {
        send_inputs(store, &mut streams, &mut message);
        watched.clear();
        watched.push(requests);
        watched.extend(look);
        let first_stream = watched.len();
        watched.extend(streams.iter().map(|open| open.channel));
        // An input stream waiting for room is tried again a tick later:
        // the kernel has no "wake me when there is room".
        let ready = if streams.iter().any(|open| open.full) {
            taaj::object_wait_until(&watched, taaj::ticks() + 1)
        } else {
            taaj::object_wait(&watched).map(Some)
        };
        let from = match ready {
            Ok(Some(from)) => from,
            Ok(None) => continue,
            Err(error) => {
                println!("[fs] cannot wait: {error:?}");
                return 2;
            }
        };
        if from >= first_stream {
            take_from(store, &mut streams, from - first_stream, &mut message);
            continue;
        }
        empty_outputs(store, &mut streams, &mut message);
        let access = if from == 0 { Access::ReadWrite } else { Access::ReadOnly };
        // Room for the largest message and the most handles one can
        // carry: a message that did not fit would stay at the head of
        // the queue, and be refused again, forever.
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match taaj::receive(watched[from], &mut message, &mut handles) {
            Ok(received) => received,
            // The service lives as long as its first channel has a
            // sender; the second going only means fewer to watch.
            Err(Error::PeerClosed) if from == 0 => return 0,
            Err(Error::PeerClosed) => {
                look = None;
                continue;
            }
            Err(error) => {
                println!("[fs] cannot receive: {error:?}");
                return 2;
            }
        };
        // Two handles: a stream to open, with its channel; or a
        // READ_INTO, with the memory object to fill. The object is let
        // go before the answer goes: a client that starts a program
        // from it at once must find it its own alone, or the kernel
        // copies it first (ipc::spawn).
        if count == 2 && Stream::is_opened_by(&message[..n]) {
            let (length, stream) = if streams.len() >= protocol::STREAMS_MAX {
                reply[0] = Status::Busy as u8;
                (1, None)
            } else {
                fileserver::open_stream(store, access, &message[..n], &mut reply)
            };
            match stream {
                Some(stream) => streams.push(Open { channel: handles[1], stream, full: false }),
                None => {
                    let _ = taaj::close(handles[1]);
                }
            }
            let _ = taaj::send(handles[0], &reply[..length], &[]);
            let _ = taaj::close(handles[0]);
            continue;
        }
        if count == 2 && message[0] == protocol::WRITE_FROM {
            let length = write_from(store, access, &message[..n], handles[1], &mut reply);
            let _ = taaj::close(handles[1]);
            if let Some(length) = length {
                let _ = taaj::send(handles[0], &reply[..length], &[]);
            }
            let _ = taaj::close(handles[0]);
            continue;
        }
        if count == 2 {
            let length = read_into(store, &message[..n], handles[1], &mut reply);
            let _ = taaj::close(handles[1]);
            if let Some(length) = length {
                let _ = taaj::send(handles[0], &reply[..length], &[]);
            }
            let _ = taaj::close(handles[0]);
            continue;
        }
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

/// Lets stream `index` go: its channel closed, and no longer watched.
/// For an output stream the program writing finds nobody reading; for
/// an input stream the program reading finds the end.
fn close_stream(streams: &mut Vec<Open>, index: usize) {
    let open = streams.remove(index);
    let _ = taaj::close(open.channel);
}

/// Stream `index`'s channel is ready: messages for an output stream,
/// or the reader of an input stream gone. Every message already waiting
/// (up to STREAM_TURN) is taken, and their bytes added to the file
/// together: one commit. Returns whether the stream is still open.
fn take_from(store: &mut impl Store, streams: &mut Vec<Open>, index: usize, message: &mut [u8; MESSAGE_MAX]) -> bool {
    let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
    let mut gathered: Vec<u8> = Vec::new();
    let mut open = true;
    for taken in 0..STREAM_TURN {
        // The first was said to be there; the rest only if they are.
        if taken > 0 && !matches!(taaj::poll(&[streams[index].channel]), Ok(Some(_))) {
            break;
        }
        match taaj::receive(streams[index].channel, message, &mut handles) {
            Ok((n, count)) => {
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle); // not part of a stream
                }
                if let Some(bytes) = fileserver::stream_data(&message[..n]) {
                    gathered.extend_from_slice(bytes);
                }
            }
            Err(_) => {
                // The writer is gone: what it sent first still counts.
                open = false;
                break;
            }
        }
    }
    // A file that cannot take more: the writer is told the only way a
    // stream can tell it, by nobody being there.
    if streams[index].stream.add(store, &gathered).is_err() || !open {
        close_stream(streams, index);
        return false;
    }
    true
}

/// Everything already sent on the output streams, into their files.
fn empty_outputs(store: &mut impl Store, streams: &mut Vec<Open>, message: &mut [u8; MESSAGE_MAX]) {
    let mut index = 0;
    while index < streams.len() {
        if !matches!(streams[index].stream, Stream::Out { .. }) {
            index += 1;
            continue;
        }
        let mut open = true;
        let mut turns = 0;
        // A turn takes up to STREAM_TURN messages; a few turns, so that
        // a writer that keeps the channel full cannot hold the requests
        // back for ever.
        while open && turns < 4 && matches!(taaj::poll(&[streams[index].channel]), Ok(Some(_))) {
            open = take_from(store, streams, index, message);
            turns += 1;
        }
        // A stream closed is out of the list: the next is at `index`.
        if open {
            index += 1;
        }
    }
}

/// The input streams, sent on as far as their channels have room; one
/// at its end is closed.
fn send_inputs(store: &mut impl Store, streams: &mut Vec<Open>, message: &mut [u8; MESSAGE_MAX]) {
    let mut index = 0;
    while index < streams.len() {
        let mut open = true;
        streams[index].full = false;
        for _ in 0..STREAM_TURN {
            let Open { channel, stream, full } = &mut streams[index];
            if !matches!(stream, Stream::In { .. }) {
                break;
            }
            let length = match stream.next(store, message) {
                Ok(0) | Err(_) => 0,
                Ok(length) => length,
            };
            if length == 0 {
                open = false;
                break;
            }
            match taaj::send(*channel, &message[..length], &[]) {
                Ok(()) => stream.sent(length),
                Err(Error::QueueFull) => {
                    *full = true;
                    break;
                }
                Err(_) => {
                    open = false;
                    break;
                }
            }
        }
        // Sent its turn and more to come: tried again soon.
        if open && matches!(streams[index].stream, Stream::In { .. }) && !streams[index].full {
            streams[index].full = true;
        }
        if open {
            index += 1;
        } else {
            close_stream(streams, index);
        }
    }
}
