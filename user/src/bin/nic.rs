//! nic - the virtio network card's driver: Ethernet frames, from a
//! program.
//!
//! As `blk` is for the disk: one driver for a card on a PCI bus,
//! reached through I/O ports (the PC), and for one at a fixed place in
//! memory (virtio-mmio: the AArch64 `virt` machine). Which it has been
//! given, it learns from the description that arrives first.
//!
//! Startup handles, from init:
//!   1  the card's registers: I/O ports, or the memory they are in
//!   2  its DMA memory: physically contiguous, its address known
//!   3  the device channel: the PciDeviceInfo or MmioDeviceInfo
//!      comes first
//!   4  the request channel (abi::nic)
//!   5  the card's interrupt, if it has one - else polled
//!
//! A network card differs from a disk in the one way that shapes this
//! file: a disk only answers, a card also *speaks first*. A frame
//! arrives when somebody out there sends one, asked for or not. So the
//! card must always have somewhere to put it:
//!
//!   - the receive queue is kept full of empty buffers, offered ahead
//!     of time; the card fills one per frame and hands it back, and
//!     the driver passes the frame on and offers the buffer again;
//!   - the transmit queue holds frames on their way out; the card
//!     hands each buffer back when the frame is on the wire.
//!
//! The DMA memory holds both queues and all the buffers:
//!
//!   0x00000  the receive queue: descriptors and rings
//!   0x08000  the transmit queue
//!   0x10000  32 receive buffers of 2048 bytes
//!   0x20000  8 transmit buffers of 2048 bytes
//!
//! Each buffer is one frame, with virtio's ten-byte note in front of it
//! (`virtio::net::HEADER_SIZE`).
//!
//! What this driver knows about a frame is its first six bytes - who
//! it is for - and the two that say what kind of thing is in it. A
//! frame for another card is dropped here (an emulated card hands over
//! everything on the wire). The kind decides who gets it: a program
//! listens for the kinds it names (`abi::nic::Listeners`), so the
//! network stack takes IPv4 and ARP, and the driver's test its own
//! kind, without either disturbing the other. Everything else - what
//! is in a frame, what to answer - is the business of whoever listens.
//!
//! No listener is ever waited for. A frame that arrives when no one
//! listens, or when the listener's queue is full, is dropped and
//! counted: the next one may be for someone who is keeping up. A
//! network loses frames anyway, and everything built on one copes.
//!
//! The card is waited for: while every transmit buffer is in its
//! hands, no more requests are read. They stay in the channel, and
//! when that is full the client is told so (QueueFull) - the client
//! that sends faster than the wire takes is the one that waits.
//!
//! On a restart init gives the new driver the same memory and
//! registers; it resets the card, which forgets the old queues, and
//! starts over. Whoever listened must ask again.

#![no_std]
#![no_main]

use abi::nic::{self, Assembler, Fed, Listeners, Stats};
use abi::{MmioDeviceInfo, PciDeviceInfo};
#[cfg(target_arch = "x86_64")]
use taaj::port::{in16, in32, in8, out16, out32, out8};
use taaj::{println, Error, Handle};
use virtio::net::{self, HEADER_SIZE, QUEUE_RECEIVE, QUEUE_TRANSMIT};
use virtio::{legacy, mmio, Buffer, Layout, Queue};

taaj::program!(main);

/// Room for one queue: enough for 1024 entries, the most a card asks.
const QUEUE_AREA: usize = 0x8000;
const RECEIVE_QUEUE: usize = 0;
const TRANSMIT_QUEUE: usize = QUEUE_AREA;
const BUFFER: usize = 2048;
const RECEIVE_BUFFERS: usize = 32;
const TRANSMIT_BUFFERS: usize = 8;
const RECEIVE_AREA: usize = 2 * QUEUE_AREA;
const TRANSMIT_AREA: usize = RECEIVE_AREA + RECEIVE_BUFFERS * BUFFER;
const DMA_NEEDED: u64 = (TRANSMIT_AREA + TRANSMIT_BUFFERS * BUFFER) as u64;
/// A queue has at most this many descriptors; each is one buffer here.
const DESCRIPTORS_MAX: usize = 1024;

// A frame and its note fit a buffer.
const _: () = assert!(HEADER_SIZE + nic::FRAME_MAX <= BUFFER);

/// How the card's registers are reached.
#[derive(Clone, Copy)]
enum Registers {
    /// I/O ports, from this one on: a legacy virtio device on a PCI
    /// bus. `config` is where the card's own configuration begins.
    #[cfg(target_arch = "x86_64")]
    Ports { base: u16, config: u16 },
    /// Memory: where the registers are mapped here (`virtio::mmio`).
    Memory(*mut u32),
}

impl Registers {
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

    /// "Queue `queue` has new buffers."
    fn notify(self, queue: u16) {
        match self {
            // SAFETY: the device's notify register.
            #[cfg(target_arch = "x86_64")]
            Registers::Ports { base, .. } => unsafe { out16(base + legacy::QUEUE_NOTIFY, queue) },
            // SAFETY: the same register, in memory.
            Registers::Memory(registers) => unsafe { Registers::write(registers, mmio::QUEUE_NOTIFY, u32::from(queue)) },
        }
    }

    /// After an interrupt: tells the device it was seen - before the
    /// queues are looked at, for the reason `blk` gives.
    fn interrupt_seen(self) {
        match self {
            #[cfg(target_arch = "x86_64")]
            Registers::Ports { .. } => {}
            // SAFETY: the two interrupt registers.
            Registers::Memory(registers) => unsafe {
                let why = Registers::read(registers, mmio::INTERRUPT_STATUS);
                Registers::write(registers, mmio::INTERRUPT_ACK, why);
            },
        }
    }

    /// One byte of the card's own configuration.
    fn config(self, offset: u32) -> u8 {
        match self {
            // SAFETY: the configuration follows the common registers.
            #[cfg(target_arch = "x86_64")]
            Registers::Ports { config, .. } => unsafe { in8(config + offset as u16) },
            // SAFETY: a byte of the configuration, in memory. It is
            // read a byte at a time, as the specification asks of a
            // field that is not a register's width.
            Registers::Memory(registers) => unsafe {
                registers.cast::<u8>().add((mmio::CONFIG + offset) as usize).read_volatile()
            },
        }
    }
}

/// Which descriptor carries which buffer: descriptors are given out by
/// the queue as it pleases, and the card names a finished buffer by its
/// descriptor.
struct Carried([u8; DESCRIPTORS_MAX]);

impl Carried {
    fn set(&mut self, descriptor: u16, buffer: usize) {
        if let Some(slot) = self.0.get_mut(descriptor as usize) {
            *slot = buffer as u8;
        }
    }

    fn get(&self, descriptor: u16) -> usize {
        self.0.get(descriptor as usize).map_or(0, |&buffer| buffer as usize)
    }
}

struct Card {
    registers: Registers,
    receive: Queue,
    transmit: Queue,
    received_in: Carried,
    sent_in: Carried,
    /// Transmit buffers not in the card's hands: a bit each.
    transmit_free: u32,
    /// Where the DMA memory is mapped here, and where it is for the card.
    memory: *mut u8,
    physical: u64,
    interrupt: Option<Handle>,
    address: [u8; nic::ADDRESS],
    link_up: bool,
    stats: Stats,
    /// Who gets the frames that arrive: by the type of each.
    listeners: Listeners<Handle, { nic::LISTENERS_MAX }>,
    /// The frame a client is sending, as its pieces arrive.
    outgoing: Assembler,
}

impl Card {
    /// What both kinds of card need once their queues exist: the
    /// address read, every receive buffer offered.
    fn finish(
        registers: Registers,
        receive: Queue,
        transmit: Queue,
        memory: *mut u8,
        physical: u64,
        interrupt: Option<Handle>,
        accepted: u32,
    ) -> Card {
        let mut address = [0u8; nic::ADDRESS];
        if accepted & net::FEATURE_MAC != 0 {
            for (n, byte) in address.iter_mut().enumerate() {
                *byte = registers.config(net::CONFIG_MAC + n as u32);
            }
        }
        let status = u16::from(registers.config(net::CONFIG_STATUS)) | u16::from(registers.config(net::CONFIG_STATUS + 1)) << 8;
        let mut card = Card {
            registers,
            receive,
            transmit,
            received_in: Carried([0; DESCRIPTORS_MAX]),
            sent_in: Carried([0; DESCRIPTORS_MAX]),
            transmit_free: (1 << TRANSMIT_BUFFERS) - 1,
            memory,
            physical,
            interrupt,
            address,
            link_up: net::link_up(accepted, status),
            stats: Stats::default(),
            listeners: Listeners::new(),
            outgoing: Assembler::new(),
        };
        // Sent frames are looked for when a buffer is wanted, not when
        // the card says so: no interrupt needed for them.
        card.transmit.quiet(true);
        for buffer in 0..RECEIVE_BUFFERS {
            card.offer(buffer);
        }
        card.registers.notify(QUEUE_RECEIVE);
        card
    }

    /// Sets the card up, following the legacy sequence: reset, say
    /// hello, agree on features, give it the queues, say ready.
    #[cfg(target_arch = "x86_64")]
    fn start(info: &PciDeviceInfo, memory: *mut u8, interrupt: Option<Handle>) -> Result<Card, &'static str> {
        let ports = info.io_base;
        let register = |offset: u16| ports + offset;
        // SAFETY (each port access below): the device's own registers,
        // enabled for this process, used as the virtio spec says.
        unsafe {
            out8(register(legacy::STATUS), 0); // reset: forget any old queues
            out8(register(legacy::STATUS), legacy::ACKNOWLEDGE);
            out8(register(legacy::STATUS), legacy::ACKNOWLEDGE | legacy::DRIVER);

            let accepted = net::accepted(in32(register(legacy::HOST_FEATURES)));
            out32(register(legacy::GUEST_FEATURES), accepted);

            if info.msix {
                out16(register(legacy::MSIX_CONFIG_VECTOR), legacy::NO_VECTOR);
            }
            // A queue: its size is the card's to say; its memory ours.
            let queue = |index: u16, at: usize| -> Result<Queue, &'static str> {
                out16(register(legacy::QUEUE_SELECT), index);
                if info.msix {
                    // Both queues interrupt with MSI-X message 0.
                    out16(register(legacy::MSIX_QUEUE_VECTOR), 0);
                    if in16(register(legacy::MSIX_QUEUE_VECTOR)) != 0 {
                        return Err("the device refused the MSI-X vector");
                    }
                }
                let size = in16(register(legacy::QUEUE_SIZE));
                let layout = Layout::legacy(size);
                if size == 0 || size as usize > DESCRIPTORS_MAX || layout.total > QUEUE_AREA {
                    return Err("no queue, or one too big for the DMA memory");
                }
                // SAFETY: QUEUE_AREA bytes of the DMA memory, page-
                // aligned, used for nothing else.
                let queue = Queue::new(memory.add(at), layout);
                out32(register(legacy::QUEUE_PFN), ((info.dma_physical + at as u64) / 4096) as u32);
                Ok(queue)
            };
            let queues = queue(QUEUE_RECEIVE, RECEIVE_QUEUE).and_then(|receive| Ok((receive, queue(QUEUE_TRANSMIT, TRANSMIT_QUEUE)?)));
            let (receive, transmit) = match queues {
                Ok(queues) => queues,
                Err(why) => {
                    out8(register(legacy::STATUS), legacy::FAILED);
                    return Err(why);
                }
            };
            out8(register(legacy::STATUS), legacy::ACKNOWLEDGE | legacy::DRIVER | legacy::DRIVER_OK);

            let registers = Registers::Ports { base: ports, config: register(legacy::device_config(info.msix)) };
            Ok(Card::finish(registers, receive, transmit, memory, info.dma_physical, interrupt, accepted))
        }
    }

    /// The same sequence for a card whose registers are in memory.
    ///
    /// # Safety
    /// `registers` must be a mapping of the device's registers, and
    /// `memory` one of the DMA memory `info` describes.
    unsafe fn start_mapped(
        info: &MmioDeviceInfo,
        registers: *mut u32,
        memory: *mut u8,
        interrupt: Option<Handle>,
    ) -> Result<Card, &'static str> {
        // SAFETY (each register access below): the caller's promise.
        unsafe {
            let read = |offset| Registers::read(registers, offset);
            let write = |offset, value| Registers::write(registers, offset, value);
            let status = |bits: u8| write(mmio::STATUS, u32::from(bits));

            // The kernel looked already, but this process trusts what
            // it reads, not what it was told.
            if read(mmio::MAGIC) != mmio::MAGIC_VALUE || read(mmio::DEVICE_ID) != mmio::DEVICE_NETWORK {
                return Err("these are not a virtio network card's registers");
            }
            if read(mmio::VERSION) != 1 {
                return Err("only the legacy register interface is spoken here");
            }
            status(0); // reset: forget any old queues, let go of the interrupt
            status(legacy::ACKNOWLEDGE);
            status(legacy::ACKNOWLEDGE | legacy::DRIVER);

            write(mmio::HOST_FEATURES_SELECT, 0);
            let accepted = net::accepted(read(mmio::HOST_FEATURES));
            write(mmio::GUEST_FEATURES_SELECT, 0);
            write(mmio::GUEST_FEATURES, accepted);

            write(mmio::GUEST_PAGE_SIZE, 4096);
            // A queue: its size is ours to choose, up to the card's
            // limit - the largest its share of the memory holds.
            let queue = |index: u16, at: usize| -> Result<Queue, &'static str> {
                write(mmio::QUEUE_SELECT, u32::from(index));
                let size = mmio::queue_size(read(mmio::QUEUE_SIZE_MAX), QUEUE_AREA)
                    .filter(|&size| size as usize <= DESCRIPTORS_MAX)
                    .ok_or("no queue, or none that fits the DMA memory")?;
                // SAFETY: as in `start`.
                let queue = Queue::new(memory.add(at), Layout::legacy(size));
                write(mmio::QUEUE_SIZE, u32::from(size));
                write(mmio::QUEUE_ALIGN, 4096);
                write(mmio::QUEUE_PFN, ((info.dma_physical + at as u64) / 4096) as u32);
                Ok(queue)
            };
            let queues = queue(QUEUE_RECEIVE, RECEIVE_QUEUE).and_then(|receive| Ok((receive, queue(QUEUE_TRANSMIT, TRANSMIT_QUEUE)?)));
            let (receive, transmit) = match queues {
                Ok(queues) => queues,
                Err(why) => {
                    status(legacy::FAILED);
                    return Err(why);
                }
            };
            status(legacy::ACKNOWLEDGE | legacy::DRIVER | legacy::DRIVER_OK);

            Ok(Card::finish(Registers::Memory(registers), receive, transmit, memory, info.dma_physical, interrupt, accepted))
        }
    }

    /// Offers the card receive buffer `buffer`, empty. (Not notified
    /// here: several are offered at a time.)
    fn offer(&mut self, buffer: usize) {
        let offered = Buffer { physical: self.physical + (RECEIVE_AREA + buffer * BUFFER) as u64, length: BUFFER as u32, device_writes: true };
        if let Some(descriptor) = self.receive.submit(&[offered]) {
            self.received_in.set(descriptor, buffer);
        }
    }

    /// Every frame the card has handed over: passed on, or dropped;
    /// its buffer offered again.
    fn take_received(&mut self) {
        let mut offered = false;
        while let Some((descriptor, length)) = self.receive.take_used() {
            let buffer = self.received_in.get(descriptor);
            let length = (length as usize).min(BUFFER);
            self.stats.received += 1;
            // SAFETY: a receive buffer, inside the DMA memory; the card
            // handed it back and does not touch it until it is offered
            // again, below.
            let filled = unsafe { core::slice::from_raw_parts(self.memory.add(RECEIVE_AREA + buffer * BUFFER), length) };
            match filled.get(HEADER_SIZE..) {
                Some(frame) if (nic::FRAME_MIN..=nic::FRAME_MAX).contains(&frame.len()) => {
                    if nic::for_us(frame, &self.address) {
                        self.pass_on(buffer, frame.len());
                    } else {
                        self.stats.not_ours += 1;
                    }
                }
                // Not a frame at all: nothing to pass on.
                _ => self.stats.dropped += 1,
            }
            self.offer(buffer);
            offered = true;
        }
        if offered {
            self.registers.notify(QUEUE_RECEIVE);
        }
    }

    /// Sends the frame in receive buffer `buffer` to whoever listens
    /// for frames of its type.
    fn pass_on(&mut self, buffer: usize, length: usize) {
        // SAFETY: as in `take_received`, whose buffer this is.
        let frame = unsafe { core::slice::from_raw_parts(self.memory.add(RECEIVE_AREA + buffer * BUFFER + HEADER_SIZE), length) };
        let Some(listener) = nic::frame_type(frame).and_then(|kind| self.listeners.find(kind)) else {
            self.stats.dropped += 1;
            return;
        };
        let to = listener.who;
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let mut send = |op: u8, piece: &[u8]| {
            message[0] = op;
            message[1..=piece.len()].copy_from_slice(piece);
            taaj::send(to, &message[..=piece.len()], &[])
        };
        let mut outcome = Ok(());
        if listener.torn {
            outcome = send(nic::DROP, &[]);
            listener.torn = outcome.is_err();
        }
        if outcome.is_ok() {
            for (n, (op, piece)) in nic::split(frame).enumerate() {
                outcome = send(op, piece);
                if outcome.is_err() {
                    listener.torn = n > 0;
                    break;
                }
            }
        }
        match outcome {
            Ok(()) => self.stats.delivered += 1,
            Err(Error::QueueFull) => self.stats.dropped += 1,
            // Gone (or a channel that cannot be sent on): nobody
            // listens any more.
            Err(_) => {
                self.stats.dropped += 1;
                self.listeners.remove(to);
                let _ = taaj::close(to);
            }
        }
    }

    /// True if every transmit buffer is still in the card's hands -
    /// and then the card has been asked to say when one comes back.
    /// (Asked first and looked at after: a buffer handed back between
    /// the two would otherwise be one nobody is told about.)
    fn transmit_full(&mut self) -> bool {
        if self.transmit_free != 0 {
            return false;
        }
        self.transmit.quiet(false);
        self.take_sent();
        if self.transmit_free != 0 {
            self.transmit.quiet(true);
            return false;
        }
        true
    }

    /// Takes back the transmit buffers the card has finished with.
    fn take_sent(&mut self) {
        let was_full = self.transmit_free == 0;
        while let Some((descriptor, _)) = self.transmit.take_used() {
            self.transmit_free |= 1 << self.sent_in.get(descriptor);
        }
        if was_full && self.transmit_free != 0 {
            self.transmit.quiet(true);
        }
    }

    /// Puts `frame` on the wire. False if it is not a frame a card can
    /// send, or the card has no room.
    fn send(&mut self, frame: &[u8]) -> bool {
        if !(nic::FRAME_MIN..=nic::FRAME_MAX).contains(&frame.len()) {
            return false;
        }
        self.take_sent();
        if self.transmit_free == 0 {
            return false;
        }
        let buffer = self.transmit_free.trailing_zeros() as usize;
        // On the wire a frame is at least WIRE_MIN bytes: a shorter one
        // is filled up with zeroes.
        let length = frame.len().max(nic::WIRE_MIN);
        // SAFETY: a transmit buffer, inside the DMA memory, not in the
        // card's hands (its bit is set). Volatile is not needed: the
        // card reads it only after the notify below.
        unsafe {
            let at = self.memory.add(TRANSMIT_AREA + buffer * BUFFER);
            at.write_bytes(0, HEADER_SIZE + length);
            at.add(HEADER_SIZE).copy_from_nonoverlapping(frame.as_ptr(), frame.len());
        }
        let out = Buffer {
            physical: self.physical + (TRANSMIT_AREA + buffer * BUFFER) as u64,
            length: (HEADER_SIZE + length) as u32,
            device_writes: false,
        };
        let Some(descriptor) = self.transmit.submit(&[out]) else {
            return false;
        };
        self.sent_in.set(descriptor, buffer);
        self.transmit_free &= !(1 << buffer);
        self.registers.notify(QUEUE_TRANSMIT);
        true
    }

    /// One message with no handle: a piece of a frame to send.
    fn piece(&mut self, message: &[u8]) {
        // The frame is copied out of the assembler: `send` needs the
        // whole card, and the assembler is a part of it.
        let mut frame = [0u8; nic::FRAME_MAX];
        let length = match self.outgoing.feed(message) {
            Fed::Frame(whole) => {
                frame[..whole.len()].copy_from_slice(whole);
                whole.len()
            }
            Fed::More => return,
            Fed::TooLong | Fed::NotAFrame => {
                self.stats.refused += 1;
                return;
            }
        };
        if self.send(&frame[..length]) {
            self.stats.sent += 1;
        } else {
            self.stats.refused += 1;
        }
    }

    /// Answers one request that came with a reply channel; returns the
    /// reply's length in `reply`.
    fn serve(&mut self, request: &[u8], reply: &mut [u8]) -> usize {
        match request {
            [nic::INFO] => {
                reply[0] = nic::OK;
                reply[1..=nic::ADDRESS].copy_from_slice(&self.address);
                reply[1 + nic::ADDRESS] = u8::from(self.link_up);
                1 + nic::INFO_SIZE
            }
            [nic::STATS] => {
                reply[0] = nic::OK;
                reply[1..=Stats::SIZE].copy_from_slice(&self.stats.to_bytes());
                1 + Stats::SIZE
            }
            _ => {
                reply[0] = nic::BAD_REQUEST;
                1
            }
        }
    }
}

struct Address([u8; nic::ADDRESS]);

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let a = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", a[0], a[1], a[2], a[3], a[4], a[5])
    }
}

fn map(memory: Handle, what: &str) -> Result<*mut u8, i64> {
    taaj::memory_map(memory, true).map_err(|error| {
        println!("[nic] cannot map {what}: {error:?}");
        4
    })
}

/// The description is a PCI device's: its registers are I/O ports.
#[cfg(target_arch = "x86_64")]
fn start_on_pci(info: &PciDeviceInfo, ports: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Card, i64> {
    if info.dma_size < DMA_NEEDED {
        println!("[nic] no usable device description");
        return Err(2);
    }
    if taaj::ioports_enable(ports).is_err() {
        return Err(3);
    }
    let memory = map(dma, "the DMA memory")?;
    let card = Card::start(info, memory, interrupt).map_err(|why| {
        println!("[nic] the card would not start: {why}");
        5
    })?;
    println!(
        "[nic] virtio network card at {:02x}:{:02x}.{}: address {}, link {}, queues of {} and {}, {}",
        info.bus,
        info.device,
        info.function,
        Address(card.address),
        if card.link_up { "up" } else { "down" },
        card.receive.size(),
        card.transmit.size(),
        if interrupt.is_some() { "MSI-X interrupts" } else { "polled" }
    );
    Ok(card)
}

/// No PCI bus on this CPU's machines so far.
#[cfg(not(target_arch = "x86_64"))]
fn start_on_pci(_: &PciDeviceInfo, _: Handle, _: Handle, _: Option<Handle>) -> Result<Card, i64> {
    println!("[nic] a PCI device, which this build cannot drive");
    Err(2)
}

/// The description is of a device in memory.
fn start_in_memory(info: &MmioDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Card, i64> {
    // What must hold for the register accesses to be inside what gets
    // mapped: asked of the kernel, not taken from the description.
    let end = info.offset_in_page() as u64 + u64::from(info.registers_size);
    let fits = taaj::memory_size(registers).is_ok_and(|size| end <= size);
    if info.dma_size < DMA_NEEDED || u64::from(info.registers_size) < u64::from(mmio::CONFIG) + 8 || !fits {
        println!("[nic] no usable device description");
        return Err(2);
    }
    let page = map(registers, "the card's registers")?;
    let memory = map(dma, "the DMA memory")?;
    // SAFETY: inside the mapping (checked above); the registers begin
    // on a multiple of four, as the cast needs, or the device is not
    // one - which `start_mapped` finds out from the magic number.
    let card = unsafe {
        let registers = page.add(info.offset_in_page()).cast::<u32>();
        if !registers.is_aligned() {
            println!("[nic] no usable device description");
            return Err(2);
        }
        Card::start_mapped(info, registers, memory, interrupt)
    }
    .map_err(|why| {
        println!("[nic] the card would not start: {why}");
        5
    })?;
    println!(
        "[nic] virtio network card at {:#x}: address {}, link {}, queues of {} and {}, {}",
        info.registers,
        Address(card.address),
        if card.link_up { "up" } else { "down" },
        card.receive.size(),
        card.transmit.size(),
        if interrupt.is_some() { "interrupts" } else { "polled" }
    );
    Ok(card)
}

fn main() -> i64 {
    let (Some(registers), Some(dma), Some(device), Some(requests)) =
        (taaj::startup_handle(0), taaj::startup_handle(1), taaj::startup_handle(2), taaj::startup_handle(3))
    else {
        println!("[nic] started without its device");
        return 1;
    };
    let interrupt = taaj::startup_handle(4);

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
        println!("[nic] no usable device description");
        Err(2)
    };
    let mut card = match started {
        Ok(card) => card,
        Err(code) => return code,
    };

    let mut reply = [0u8; abi::MESSAGE_MAX as usize];
    loop {
        // What the card has done since the last look, first: frames in,
        // send buffers back. Before waiting again the queues must have
        // been looked at *after* the interrupt was answered.
        card.take_received();
        card.take_sent();

        // The card, or a client - or the card alone, while it has no
        // room for another frame to send. Without an interrupt the
        // card is asked again each tick (it polls): slower, and correct.
        let full = card.transmit_full();
        let ready = match card.interrupt {
            Some(line) if full => taaj::object_wait(&[line, line]).map(|_| Some(0)),
            Some(line) => taaj::object_wait(&[line, requests]).map(Some),
            None if full => {
                taaj::sleep(1);
                Ok(None)
            }
            None => taaj::object_wait_until(&[requests, requests], taaj::ticks() + 1),
        };
        match ready {
            Ok(Some(0)) if card.interrupt.is_some() => {
                // Taken, so that the next one can come; answered, so
                // that the card lets go of its line. The queues are
                // looked at above.
                if card.interrupt.is_some_and(|line| taaj::interrupt_wait(line).is_err()) {
                    return 6;
                }
                card.registers.interrupt_seen();
                continue;
            }
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(_) => return 6,
        }

        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let (n, count) = match taaj::receive(requests, &mut message, &mut handles) {
            Ok(received) => received,
            Err(Error::PeerClosed) => return 0, // no clients left, and none can come
            Err(error) => {
                println!("[nic] cannot receive: {error:?}");
                return 6;
            }
        };
        let request = &message[..n];
        match (request.first().copied(), count) {
            // A piece of a frame to send: no handle, no answer.
            (_, 0) => card.piece(request),
            // A new listener: for the frame types it names, in place
            // of whoever listened for them. (Closing a channel is how a
            // listener is told that it is one no longer.)
            (Some(nic::LISTEN), 1) => match nic::listen_types(&request[1..]) {
                Some((types, count)) => {
                    card.listeners.add(handles[0], &types[..count], |ended| {
                        let _ = taaj::close(ended);
                    });
                }
                None => {
                    let _ = taaj::close(handles[0]);
                }
            },
            // A question, with the channel to answer on.
            (_, 1) => {
                // The test of a driver that has stopped: answered first.
                let mut stall = 0;
                if n == 9 && request[0] == nic::STALL {
                    stall = u64::from_le_bytes(request[1..9].try_into().unwrap_or([0; 8]));
                }
                let length = if (1..=abi::STALL_MAX).contains(&stall) {
                    reply[0] = nic::OK;
                    1
                } else {
                    stall = 0;
                    card.serve(request, &mut reply)
                };
                let _ = taaj::send(handles[0], &reply[..length], &[]);
                let _ = taaj::close(handles[0]);
                if stall > 0 {
                    println!("[nic] asked to stall: doing nothing for {stall} ticks");
                    taaj::sleep(stall);
                    println!("[nic] working again");
                }
            }
            // Anything else: not a request.
            _ => {
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle);
                }
            }
        }
    }
}
