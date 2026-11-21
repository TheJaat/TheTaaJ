//! blk - the virtio block driver: a disk, from ring 3.
//!
//! The same driver for a disk on a PCI bus, reached through I/O ports
//! (the PC), and for one at a fixed place in memory (virtio-mmio: the
//! AArch64 `virt` machine). Which it has been given, it learns from the
//! description that arrives first: the two are of different lengths.
//! Only setting the device up, telling it of a new request and
//! answering its interrupt differ - `Transport` below; the queue, the
//! requests and the service are the same code.
//!
//! Startup handles, from whoever starts it (init; on AArch64, so far,
//! the kernel's self-test):
//!   1  the device's registers: I/O ports (its BAR 0), or the memory
//!      they are in
//!   2  its DMA memory: physically contiguous, its address known
//!   3  the device channel: the PciDeviceInfo or MmioDeviceInfo
//!      comes first
//!   4  the request channel (abi::block): reads and writes - for the
//!      file service, which owns what is on the disk
//!   5  a second request channel: reads only - for anyone who may look
//!      at the disk but not change it (the shell)
//!   6  the device's interrupt, if it has one - else polled
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

use abi::{block, MmioDeviceInfo, PciDeviceInfo};
#[cfg(target_arch = "x86_64")]
use taaj::port::{in16, in32, out16, out32, out8};
use taaj::{println, Error, Handle};
use virtio::{legacy, mmio, Buffer, Layout, Queue};

taaj::program!(main);

const QUEUE_AREA: usize = 0x8000;
const HEADER: usize = 0x8000;
const STATUS: usize = 0x8000 + virtio::block::HEADER_SIZE;
const DATA: usize = 0x9000;
const DATA_MAX: usize = block::SECTORS_MAX as usize * block::SECTOR;
const DMA_NEEDED: u64 = (DATA + DATA_MAX) as u64;

/// How the device's registers are reached.
#[derive(Clone, Copy)]
enum Transport {
    /// I/O ports, from this one on: a legacy virtio device on a PCI bus.
    #[cfg(target_arch = "x86_64")]
    Ports(u16),
    /// Memory: where the registers are mapped here. Each is 32 bits, at
    /// a multiple of four (`virtio::mmio`).
    Memory(*mut u32),
}

impl Transport {
    /// A memory-mapped register, read.
    ///
    /// # Safety
    /// `registers` must be a mapping of a virtio-mmio device's
    /// registers and `offset` one of them.
    unsafe fn read(registers: *mut u32, offset: u32) -> u32 {
        // SAFETY: the caller's promise. Volatile: a read is a question
        // put to the device, never to be skipped or repeated.
        unsafe { registers.byte_add(offset as usize).read_volatile() }
    }

    /// # Safety
    /// As for `read`.
    unsafe fn write(registers: *mut u32, offset: u32, value: u32) {
        // SAFETY: as for `read`.
        unsafe { registers.byte_add(offset as usize).write_volatile(value) }
    }

    /// "Queue 0 has a new request."
    fn notify(self) {
        match self {
            // SAFETY: the device's notify register.
            #[cfg(target_arch = "x86_64")]
            Transport::Ports(ports) => unsafe { out16(ports + legacy::QUEUE_NOTIFY, 0) },
            // SAFETY: the same register, in memory.
            Transport::Memory(registers) => unsafe { Transport::write(registers, mmio::QUEUE_NOTIFY, 0) },
        }
    }

    /// After an interrupt: tells the device it was seen.
    ///
    /// With MSI-X (the PC) an interrupt is a message, sent once, and
    /// there is nothing to answer. A memory-mapped device raises a line
    /// and holds it up until told which of its reasons were seen. The
    /// order matters to the caller: this first, and only then a look
    /// at the queue - a request that finishes after the look raises
    /// the line again, one that finished before is found by the look.
    /// The other way round, one could finish between the two and never
    /// be noticed.
    fn interrupt_seen(self) {
        match self {
            #[cfg(target_arch = "x86_64")]
            Transport::Ports(_) => {}
            // SAFETY: the two interrupt registers.
            Transport::Memory(registers) => unsafe {
                let why = Transport::read(registers, mmio::INTERRUPT_STATUS);
                Transport::write(registers, mmio::INTERRUPT_ACK, why);
            },
        }
    }
}

struct Disk {
    transport: Transport,
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
    #[cfg(target_arch = "x86_64")]
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
                transport: Transport::Ports(ports),
                queue,
                memory,
                physical: info.dma_physical,
                interrupt,
                sectors,
                read_only: offered & virtio::block::FEATURE_READ_ONLY != 0,
            })
        }
    }

    /// The same sequence for a device whose registers are in memory, at
    /// `registers`. Three things differ from the PCI one:
    ///
    ///   - the driver says how big a page is (the queue's address is
    ///     given in pages) and how the used ring is aligned;
    ///   - the driver chooses the queue's size, up to the device's
    ///     limit - so it takes the largest its memory holds;
    ///   - there is no choosing of interrupts: the device has one line.
    ///
    /// # Safety
    /// `registers` must be a mapping of the device's registers, and
    /// `memory` one of the DMA memory `info` describes.
    unsafe fn start_mapped(
        info: &MmioDeviceInfo,
        registers: *mut u32,
        memory: *mut u8,
        interrupt: Option<Handle>,
    ) -> Result<Disk, &'static str> {
        // SAFETY (each register access below): the caller's promise.
        unsafe {
            let read = |offset| Transport::read(registers, offset);
            let write = |offset, value| Transport::write(registers, offset, value);
            let status = |bits: u8| write(mmio::STATUS, u32::from(bits));

            // The kernel looked already, but this process trusts what
            // it reads, not what it was told.
            if read(mmio::MAGIC) != mmio::MAGIC_VALUE || read(mmio::DEVICE_ID) != mmio::DEVICE_BLOCK {
                return Err("these are not a virtio block device's registers");
            }
            if read(mmio::VERSION) != 1 {
                return Err("only the legacy register interface is spoken here");
            }
            status(0); // reset: forget any old queue, let go of the interrupt
            status(legacy::ACKNOWLEDGE);
            status(legacy::ACKNOWLEDGE | legacy::DRIVER);

            write(mmio::HOST_FEATURES_SELECT, 0);
            let offered = read(mmio::HOST_FEATURES);
            write(mmio::GUEST_FEATURES_SELECT, 0);
            write(mmio::GUEST_FEATURES, 0);

            write(mmio::GUEST_PAGE_SIZE, 4096);
            write(mmio::QUEUE_SELECT, 0);
            let Some(size) = mmio::queue_size(read(mmio::QUEUE_SIZE_MAX), QUEUE_AREA) else {
                status(legacy::FAILED);
                return Err("no queue, or none that fits the DMA memory");
            };
            let layout = Layout::legacy(size);
            // SAFETY: as in `start` - the start of the DMA memory.
            let queue = Queue::new(memory, layout);
            write(mmio::QUEUE_SIZE, u32::from(size));
            write(mmio::QUEUE_ALIGN, 4096);
            write(mmio::QUEUE_PFN, (info.dma_physical / 4096) as u32);
            status(legacy::ACKNOWLEDGE | legacy::DRIVER | legacy::DRIVER_OK);

            let sectors = u64::from(read(mmio::CONFIG)) | u64::from(read(mmio::CONFIG + 4)) << 32;
            Ok(Disk {
                transport: Transport::Memory(registers),
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
        self.transport.notify();

        // Wait for it to come back. The interrupt count is what makes
        // this safe: one that fires before the wait begins is not lost.
        while self.queue.take_used().is_none() {
            match self.interrupt {
                Some(line) => {
                    if taaj::interrupt_wait(line).is_err() {
                        return block::IO_ERROR;
                    }
                    // Before the loop looks at the queue again.
                    self.transport.interrupt_seen();
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

/// The description is a PCI device's: its registers are I/O ports.
#[cfg(target_arch = "x86_64")]
fn start_on_pci(info: &PciDeviceInfo, ports: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Disk, i64> {
    if info.dma_size < DMA_NEEDED {
        println!("[blk] no usable device description");
        return Err(2);
    }
    if taaj::ioports_enable(ports).is_err() {
        return Err(3);
    }
    let memory = map(dma, "the DMA memory")?;
    let disk = Disk::start(info, memory, interrupt).map_err(|why| {
        println!("[blk] the device would not start: {why}");
        5
    })?;
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
    Ok(disk)
}

/// No PCI bus on this CPU's machines so far.
#[cfg(not(target_arch = "x86_64"))]
fn start_on_pci(_: &PciDeviceInfo, _: Handle, _: Handle, _: Option<Handle>) -> Result<Disk, i64> {
    println!("[blk] a PCI device, which this build cannot drive");
    Err(2)
}

/// The description is of a device in memory.
fn start_in_memory(info: &MmioDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Disk, i64> {
    // What must hold for the register accesses to be inside what gets
    // mapped: asked of the kernel, not taken from the description.
    let end = info.offset_in_page() as u64 + u64::from(info.registers_size);
    let fits = taaj::memory_size(registers).is_ok_and(|size| end <= size);
    if info.dma_size < DMA_NEEDED || u64::from(info.registers_size) < u64::from(mmio::CONFIG) + 8 || !fits {
        println!("[blk] no usable device description");
        return Err(2);
    }
    let page = map(registers, "the device's registers")?;
    let memory = map(dma, "the DMA memory")?;
    // SAFETY: inside the mapping (checked above); the registers begin
    // on a multiple of four, as the cast needs, or the device is not
    // one - which `start_mapped` finds out from the magic number.
    let disk = unsafe {
        let registers = page.add(info.offset_in_page()).cast::<u32>();
        if !registers.is_aligned() {
            println!("[blk] no usable device description");
            return Err(2);
        }
        Disk::start_mapped(info, registers, memory, interrupt)
    }
    .map_err(|why| {
        println!("[blk] the device would not start: {why}");
        5
    })?;
    println!(
        "[blk] virtio disk at {:#x}: {} sectors ({} KiB){}, queue of {}, {}",
        info.registers,
        disk.sectors,
        disk.sectors / 2,
        if disk.read_only { ", read-only" } else { "" },
        disk.queue.size(),
        if interrupt.is_some() { "interrupts" } else { "polled" }
    );
    Ok(disk)
}

fn map(memory: Handle, what: &str) -> Result<*mut u8, i64> {
    taaj::memory_map(memory, true).map_err(|error| {
        println!("[blk] cannot map {what}: {error:?}");
        4
    })
}

fn main() -> i64 {
    let (Some(registers), Some(dma), Some(device), Some(writers), Some(readers)) = (
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
    let described = match taaj::receive(device, &mut message, &mut []) {
        Ok((n, _)) => &message[..n],
        Err(_) => &[],
    };
    // Which kind of device: the two descriptions differ in length.
    let started = if let Some(info) = PciDeviceInfo::from_bytes(described) {
        start_on_pci(&info, registers, dma, interrupt)
    } else if let Some(info) = MmioDeviceInfo::from_bytes(described) {
        start_in_memory(&info, registers, dma, interrupt)
    } else {
        println!("[blk] no usable device description");
        Err(2)
    };
    let mut disk = match started {
        Ok(disk) => disk,
        Err(code) => return code,
    };

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
        // The test of a driver that has stopped: answered first.
        let mut stall = 0;
        if n == block::REQUEST_SIZE && message[0] == block::STALL {
            let mut ticks = [0u8; 8];
            ticks.copy_from_slice(&message[1..9]);
            stall = u64::from_le_bytes(ticks);
        }
        let length = if (1..=abi::STALL_MAX).contains(&stall) {
            reply[0] = block::OK;
            1
        } else {
            stall = 0;
            disk.serve(&message[..n], &mut reply, may_write, lent)
        };
        let _ = taaj::send(handles[0], &reply[..length], &[]);
        for &handle in &handles[..count] {
            let _ = taaj::close(handle);
        }
        if stall > 0 {
            println!("[blk] asked to stall: answering nothing for {stall} ticks");
            taaj::sleep(stall);
            println!("[blk] answering again");
        }
    }
}
