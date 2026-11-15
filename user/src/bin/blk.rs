//! blk - the virtio block driver: a disk, from ring 3.
//!
//! Startup handles, from init:
//!   1  the device's I/O ports (its BAR 0)
//!   2  its DMA memory: physically contiguous, its address known
//!   3  the device channel: init sends the PciDeviceInfo first
//!   4  the request channel (abi::block): reads and writes - for the
//!      file service, which owns what is on the disk
//!   5  a second request channel: reads only - for anyone who may look
//!      at the disk but not change it (the shell)
//!   6  the device's interrupt (MSI-X), if it has one - else polled
//!
//! Two channels, two levels of authority: which one a client was given
//! decides what it may do. The kernel knows nothing of it - handle
//! rights say who may send on a channel, and this service decides what
//! a message on each one is allowed to mean.
//!
//! The DMA memory holds everything the device reads or writes:
//!
//!   0x0000  the virtqueue: descriptors, available ring, used ring
//!   0x8000  the request header (16 bytes), then its status byte
//!   0x9000  the data: up to SECTORS_MAX sectors (16 KiB)
//!
//! One request at a time, start to finish: put three buffers - header,
//! data, status - in the queue, notify the device, wait for the
//! interrupt, take them back. Many requests in flight at once is how a
//! disk gets faster still, later.
//!
//! A request for one sector carries its data in the messages. A request
//! for many (READ_MANY, WRITE_MANY) comes with a memory object: the
//! driver maps it, copies between it and the DMA buffer, and unmaps it
//! again before replying. One device request and one copy for 16 KiB,
//! where sector-sized messages would take 32 of each. The driver cannot
//! let the device use the client's memory directly: the device needs
//! physical addresses, and only DMA memory comes with those.
//!
//! A client's memory object is the client's: it may be smaller than the
//! request claims, or not a memory object at all. Its size is asked of
//! the kernel before anything is mapped or touched, so a wrong one
//! costs the client its request and the driver nothing.
//!
//! On a restart init gives the new driver the same memory and ports; it
//! resets the device, which forgets the old queue, and starts over.

#![no_std]
#![no_main]

use abi::{block, PciDeviceInfo};
use taaj::port::{in16, in32, out16, out32, out8};
use taaj::{println, Error, Handle};
use virtio::{legacy, Buffer, Layout, Queue};

taaj::program!(main);

const QUEUE_AREA: usize = 0x8000;
const HEADER: usize = 0x8000;
const STATUS: usize = 0x8000 + virtio::block::HEADER_SIZE;
const DATA: usize = 0x9000;
const DATA_MAX: usize = block::SECTORS_MAX as usize * block::SECTOR;
const DMA_NEEDED: u64 = (DATA + DATA_MAX) as u64;

struct Disk {
    ports: u16,
    queue: Queue,
    /// Where the DMA memory is mapped here, and where it is for the device.
    memory: *mut u8,
    physical: u64,
    interrupt: Option<Handle>,
    sectors: u64,
    read_only: bool,
}

impl Disk {
    /// Sets the device up, following the legacy sequence: reset, say
    /// hello, agree on features, give it the queue, say ready.
    fn start(info: &PciDeviceInfo, memory: *mut u8, interrupt: Option<Handle>) -> Result<Disk, &'static str> {
        let ports = info.io_base;
        let register = |offset: u16| ports + offset;
        // SAFETY (each port access below): the device's own registers,
        // enabled for this process, used as the virtio spec says.
        unsafe {
            out8(register(legacy::STATUS), 0); // reset: forget any old queue
            out8(register(legacy::STATUS), legacy::ACKNOWLEDGE);
            out8(register(legacy::STATUS), legacy::ACKNOWLEDGE | legacy::DRIVER);

            // Features: none are needed - but read-only must be noticed.
            let offered = in32(register(legacy::HOST_FEATURES));
            out32(register(legacy::GUEST_FEATURES), 0);

            out16(register(legacy::QUEUE_SELECT), 0);
            if info.msix {
                // Queue interrupts on MSI-X message 0; none for config
                // changes. The device answers NO_VECTOR if it cannot.
                out16(register(legacy::MSIX_CONFIG_VECTOR), legacy::NO_VECTOR);
                out16(register(legacy::MSIX_QUEUE_VECTOR), 0);
                if in16(register(legacy::MSIX_QUEUE_VECTOR)) != 0 {
                    out8(register(legacy::STATUS), legacy::FAILED);
                    return Err("the device refused the MSI-X vector");
                }
            }
            let size = in16(register(legacy::QUEUE_SIZE));
            let layout = Layout::legacy(size);
            if size == 0 || layout.total > QUEUE_AREA {
                out8(register(legacy::STATUS), legacy::FAILED);
                return Err("no queue, or one too big for the DMA memory");
            }
            // SAFETY: the start of the DMA memory, page-aligned, used for
            // nothing else; QUEUE_AREA bytes, more than layout.total.
            let queue = Queue::new(memory, layout);
            out32(register(legacy::QUEUE_PFN), (info.dma_physical / 4096) as u32);
            out8(register(legacy::STATUS), legacy::ACKNOWLEDGE | legacy::DRIVER | legacy::DRIVER_OK);

            // The device's configuration: first, its size in sectors.
            let config = register(legacy::device_config(info.msix));
            let sectors = u64::from(in32(config)) | u64::from(in32(config + 4)) << 32;
            Ok(Disk {
                ports,
                queue,
                memory,
                physical: info.dma_physical,
                interrupt,
                sectors,
                read_only: offered & virtio::block::FEATURE_READ_ONLY != 0,
            })
        }
    }

    /// `count` sectors (1 to SECTORS_MAX), in or out of the DATA
    /// buffer, as one request to the device. Returns a block status.
    fn transfer(&mut self, write: bool, sector: u64, count: u32) -> u8 {
        if count == 0 || count > block::SECTORS_MAX {
            return block::BAD_REQUEST;
        }
        if !sector.checked_add(u64::from(count)).is_some_and(|end| end <= self.sectors) {
            return block::OUT_OF_RANGE;
        }
        if write && self.read_only {
            return block::READ_ONLY;
        }
        let kind = if write { virtio::block::WRITE } else { virtio::block::READ };
        let header = virtio::block::header(kind, sector);
        // SAFETY: inside the DMA memory, which this process mapped; the
        // device is not using these bytes between requests. Volatile:
        // the device reads them behind the compiler's back.
        unsafe {
            for (n, &byte) in header.iter().enumerate() {
                self.memory.add(HEADER + n).write_volatile(byte);
            }
            self.memory.add(STATUS).write_volatile(0xFF);
        }
        let at = |offset: usize| self.physical + offset as u64;
        let buffers = [
            Buffer { physical: at(HEADER), length: virtio::block::HEADER_SIZE as u32, device_writes: false },
            Buffer { physical: at(DATA), length: count * block::SECTOR as u32, device_writes: !write },
            Buffer { physical: at(STATUS), length: 1, device_writes: true },
        ];
        if self.queue.submit(&buffers).is_none() {
            return block::IO_ERROR; // cannot happen: one request at a time
        }
        // SAFETY: the device's notify register.
        unsafe { out16(self.ports + legacy::QUEUE_NOTIFY, 0) };

        // Wait for it to come back. The interrupt count is what makes
        // this safe: one that fires before the wait begins is not lost.
        while self.queue.take_used().is_none() {
            match self.interrupt {
                Some(line) => {
                    if taaj::interrupt_wait(line).is_err() {
                        return block::IO_ERROR;
                    }
                }
                None => taaj::yield_now(),
            }
        }
        // SAFETY: the status byte, written by the device.
        match unsafe { self.memory.add(STATUS).read_volatile() } {
            virtio::block::OK => block::OK,
            _ => block::IO_ERROR,
        }
    }

    /// The first `length` bytes of the DATA buffer.
    fn data(&mut self, length: usize) -> &mut [u8] {
        // SAFETY: inside the DMA memory (length is at most DATA_MAX),
        // not in the device's hands between requests.
        unsafe { core::slice::from_raw_parts_mut(self.memory.add(DATA), length.min(DATA_MAX)) }
    }

    /// READ_MANY or WRITE_MANY: `count` sectors between the disk and
    /// the client's memory object `lent`. Returns a block status.
    fn transfer_many(&mut self, write: bool, sector: u64, count: u32, lent: Handle) -> u8 {
        if count == 0 || count > block::SECTORS_MAX {
            return block::BAD_REQUEST;
        }
        let length = count as usize * block::SECTOR;
        // Is it a memory object, and big enough? Asked before mapping:
        // copying past the end of a smaller one would be a fault - in
        // this process, the driver.
        if !taaj::memory_size(lent).is_ok_and(|size| size >= length as u64) {
            return block::BAD_REQUEST;
        }
        // Writable only to fill it; refused if the handle lacks the right.
        let Ok(theirs) = taaj::memory_map(lent, !write) else {
            return block::BAD_REQUEST;
        };
        // SAFETY (both copies): `theirs` is a mapping of at least
        // `length` bytes, checked above, and stays mapped until the
        // unmap below. The client may change the bytes meanwhile; then
        // it reads or writes its own garbage, and nothing else.
        let status = if write {
            unsafe { theirs.copy_to_nonoverlapping(self.data(length).as_mut_ptr(), length) };
            self.transfer(true, sector, count)
        } else {
            let status = self.transfer(false, sector, count);
            if status == block::OK {
                unsafe { theirs.copy_from_nonoverlapping(self.data(length).as_ptr(), length) };
            }
            status
        };
        let _ = taaj::memory_unmap(theirs);
        status
    }

    /// Answers one request; returns the reply's length in `reply`.
    /// `lent` is the memory object that came with it, if one did.
    fn serve(&mut self, request: &[u8], reply: &mut [u8], may_write: bool, lent: Option<Handle>) -> usize {
        let op = request.first().copied();
        if matches!(op, Some(block::WRITE | block::WRITE_MANY)) && !may_write {
            reply[0] = block::READ_ONLY;
            return 1;
        }
        if let (Some(op @ (block::READ_MANY | block::WRITE_MANY)), Some(lent)) = (op, lent) {
            let count = request.get(9..13).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
            let sector = request.get(1..9).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
            reply[0] = match (sector, count) {
                (Some(sector), Some(count)) if request.len() == block::MANY_REQUEST_SIZE => {
                    self.transfer_many(op == block::WRITE_MANY, sector, count, lent)
                }
                _ => block::BAD_REQUEST,
            };
            return 1;
        }
        let sector = request.get(1..9).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
        let (status, length) = match (request.first().copied(), sector) {
            (Some(block::INFO), _) => {
                reply[1..9].copy_from_slice(&self.sectors.to_le_bytes());
                reply[9] = u8::from(self.read_only);
                (block::OK, 9)
            }
            (Some(block::READ), Some(sector)) if request.len() == block::REQUEST_SIZE => {
                match self.transfer(false, sector, 1) {
                    block::OK => {
                        reply[1..=block::SECTOR].copy_from_slice(self.data(block::SECTOR));
                        (block::OK, block::SECTOR)
                    }
                    status => (status, 0),
                }
            }
            (Some(block::WRITE), Some(sector)) if request.len() == block::REQUEST_SIZE + block::SECTOR => {
                self.data(block::SECTOR).copy_from_slice(&request[block::REQUEST_SIZE..]);
                (self.transfer(true, sector, 1), 0)
            }
            _ => (block::BAD_REQUEST, 0),
        };
        reply[0] = status;
        1 + length
    }
}

fn main() -> i64 {
    let (Some(ports), Some(dma), Some(device), Some(writers), Some(readers)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
    ) else {
        println!("[blk] started without its device");
        return 1;
    };
    let interrupt = taaj::startup_handle(5);

    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let info = match taaj::receive(device, &mut message, &mut []) {
        Ok((n, _)) => PciDeviceInfo::from_bytes(&message[..n]),
        Err(_) => None,
    };
    let Some(info) = info.filter(|i| i.dma_size >= DMA_NEEDED) else {
        println!("[blk] no usable device description");
        return 2;
    };
    if taaj::ioports_enable(ports).is_err() {
        return 3;
    }
    let memory = match taaj::memory_map(dma, true) {
        Ok(memory) => memory,
        Err(error) => {
            println!("[blk] cannot map the DMA memory: {error:?}");
            return 4;
        }
    };
    let mut disk = match Disk::start(&info, memory, interrupt) {
        Ok(disk) => disk,
        Err(why) => {
            println!("[blk] the device would not start: {why}");
            return 5;
        }
    };
    println!(
        "[blk] virtio disk at {:02x}:{:02x}.{}: {} sectors ({} KiB){}, queue of {}, {}",
        info.bus,
        info.device,
        info.function,
        disk.sectors,
        disk.sectors / 2,
        if disk.read_only { ", read-only" } else { "" },
        disk.queue.size(),
        if interrupt.is_some() { "MSI-X interrupts" } else { "polled" }
    );

    let mut reply = [0u8; abi::MESSAGE_MAX as usize];
    let channels = [writers, readers];
    let mut open = [true, true];
    loop {
        // Whichever channel has a request - both, in turn, if both do.
        let live: [Handle; 2] = match open {
            [true, true] => channels,
            [true, false] => [writers; 2],
            [false, true] => [readers; 2],
            [false, false] => return 0,
        };
        let which = match taaj::object_wait(&live) {
            Ok(n) => n,
            Err(_) => return 6,
        };
        let requests = live[which];
        let may_write = requests == writers;
        // As in fs: room for any message, so none can get stuck.
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match taaj::receive(requests, &mut message, &mut handles) {
            Ok(received) => received,
            Err(Error::PeerClosed) => {
                open[usize::from(!may_write)] = false; // no clients left on it
                continue;
            }
            Err(error) => {
                println!("[blk] cannot receive: {error:?}");
                return 6;
            }
        };
        // The reply channel, and for the "many" requests the client's
        // memory. Anything else: not a request.
        if !(1..=2).contains(&count) {
            for &handle in &handles[..count] {
                let _ = taaj::close(handle);
            }
            continue;
        }
        let lent = (count == 2).then_some(handles[1]);
        let length = disk.serve(&message[..n], &mut reply, may_write, lent);
        let _ = taaj::send(handles[0], &reply[..length], &[]);
        for &handle in &handles[..count] {
            let _ = taaj::close(handle);
        }
    }
}
