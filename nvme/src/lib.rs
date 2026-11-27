//! NVM Express: how a PCIe SSD talks to the CPU.
//!
//! SATA and AHCI were made for spinning disks: one queue of 32 commands,
//! and registers the driver reads on every command. NVMe was made for
//! flash, which can do many things at once: up to 65535 queues of up to
//! 65536 commands each, all in memory, and only one register write per
//! command - a "doorbell". Most PCs from the last ten years boot from an
//! NVMe SSD; QEMU has one too (`-device nvme`).
//!
//! How it works:
//!
//!   - Registers in memory (BAR 0): capabilities (CAP), configuration
//!     (CC: on/off, entry sizes), status (CSTS: ready, fatal), where the
//!     admin queues are (AQA, ASQ, ACQ); from 0x1000 on, the doorbells.
//!   - Queues in memory, in pairs. A submission queue (SQ) is a ring of
//!     64-byte commands the driver writes; a completion queue (CQ) is a
//!     ring of 16-byte answers the controller writes. The driver tells
//!     the controller "the SQ's tail is now n" through the SQ's tail
//!     doorbell, and "I have read the CQ up to n" through the CQ's head
//!     doorbell.
//!   - How does the driver know an answer is new? Each one carries a
//!     *phase* bit. The CQ starts zeroed; the controller writes answers
//!     with phase 1 on its first trip round the ring, 0 on the second,
//!     and so on. An entry whose phase is the one expected is new.
//!   - Queue pair 0 is the admin queue: commands about the controller -
//!     "who are you" (IDENTIFY), "make me an I/O queue". Reads and writes
//!     go through I/O queues the driver creates with admin commands.
//!   - Data is named by PRPs (physical region pages): PRP1 is where the
//!     data starts, PRP2 the second page - or, when there are more than
//!     two, the address of a list of the rest.
//!   - Disks are *namespaces*: a controller may divide its flash into
//!     several, numbered from 1. Each has its own size and block size.
//!
//! This crate is everything that is not a system call: the register map,
//! the layouts, the commands, reading IDENTIFY answers, and `Disk`, the
//! sequence that drives one namespace - written against `Bus`, so the
//! whole of it is tested on the host against a simulated controller
//! (`tests.rs`). The program that drives a real one is
//! `user/src/bin/nvme.rs`.
//!
//! One I/O queue pair, one command at a time: fs sends one request at a
//! time anyway. Many queues - one per CPU, many commands in each - are
//! what NVMe is for; later.

#![cfg_attr(not(test), no_std)]

use core::sync::atomic::{fence, Ordering};

#[cfg(test)]
mod tests;

/// The PCI class of an NVMe controller: mass storage (01), non-volatile
/// memory (08), NVM Express (programming interface 02). Any vendor's.
pub const CLASS: u8 = 0x01;
pub const SUBCLASS: u8 = 0x08;
pub const INTERFACE: u8 = 0x02;

/// The controller's registers: offsets into BAR 0.
pub mod reg {
    /// Capabilities, 64 bits: queue size limit, timeout, doorbell
    /// stride, command sets, page sizes.
    pub const CAP: usize = 0x00;
    /// Version: major in bits 31:16, minor 15:8.
    pub const VS: usize = 0x08;
    /// Configuration: enable, command set, page size, entry sizes.
    pub const CC: usize = 0x14;
    /// Status: ready, fatal.
    pub const CSTS: usize = 0x1C;
    /// Admin queue sizes (minus one): SQ in bits 11:0, CQ in 27:16.
    pub const AQA: usize = 0x24;
    /// Admin SQ and CQ addresses, 64 bits each.
    pub const ASQ: usize = 0x28;
    pub const ACQ: usize = 0x30;
    /// Where the doorbells begin.
    pub const DOORBELLS: usize = 0x1000;

    pub const CC_ENABLE: u32 = 1 << 0;
    /// The sizes of an SQ and a CQ entry, as powers of two: 64 and 16
    /// bytes. Command set (bits 6:4) 0 = NVM, page size (bits 10:7)
    /// 0 = 4 KiB, arbitration (13:11) 0 = round robin: all zero.
    pub const CC_ENTRY_SIZES: u32 = 6 << 16 | 4 << 20;
    pub const CSTS_READY: u32 = 1 << 0;
    pub const CSTS_FATAL: u32 = 1 << 1;

    /// The most entries a queue may have (MQES is that, minus one).
    pub const fn queue_entries_max(cap: u64) -> u32 {
        (cap & 0xFFFF) as u32 + 1
    }
    /// How long the controller may take to become ready or not, in
    /// ticks (CAP.TO counts 500 ms; a tick is 10).
    pub const fn ready_ticks(cap: u64) -> u64 {
        ((cap >> 24) & 0xFF) * 50
    }
    /// Bytes from one doorbell to the next: 4 << DSTRD.
    pub const fn doorbell_stride(cap: u64) -> usize {
        4 << ((cap >> 32) & 0xF)
    }
    /// It speaks the NVM command set (reads and writes of blocks).
    pub const fn has_nvm_commands(cap: u64) -> bool {
        cap & (1 << 37) != 0
    }
    /// Its smallest memory page, as a power of two above 4 KiB.
    pub const fn smallest_page(cap: u64) -> u32 {
        ((cap >> 48) & 0xF) as u32
    }

    /// Queue `queue`'s doorbells: the SQ's tail, the CQ's head.
    pub const fn sq_tail(queue: usize, stride: usize) -> usize {
        DOORBELLS + 2 * queue * stride
    }
    pub const fn cq_head(queue: usize, stride: usize) -> usize {
        DOORBELLS + (2 * queue + 1) * stride
    }
}

/// Admin commands.
pub mod admin {
    pub const CREATE_SQ: u8 = 0x01;
    pub const CREATE_CQ: u8 = 0x05;
    pub const IDENTIFY: u8 = 0x06;
    /// IDENTIFY's CNS (what to identify): a namespace, the controller,
    /// the list of active namespaces.
    pub const CNS_NAMESPACE: u32 = 0;
    pub const CNS_CONTROLLER: u32 = 1;
    pub const CNS_NAMESPACES: u32 = 2;
}

/// NVM commands, on an I/O queue.
pub mod io {
    /// Everything written so far onto the medium (a volatile write
    /// cache emptied); no data. A controller without such a cache
    /// completes it at once.
    pub const FLUSH: u8 = 0x00;
    pub const WRITE: u8 = 0x01;
    pub const READ: u8 = 0x02;
}

/// Where everything lives in the driver's DMA memory: a page each, then
/// the data.
pub mod layout {
    pub const PAGE: usize = 4096;
    pub const ADMIN_SQ: usize = 0x0000;
    pub const ADMIN_CQ: usize = 0x1000;
    pub const IO_SQ: usize = 0x2000;
    pub const IO_CQ: usize = 0x3000;
    /// The PRP list: the data's pages after its first, in order.
    pub const PRP_LIST: usize = 0x4000;
    /// The data: what a read brings, what a write sends. Also where
    /// IDENTIFY answers land (4 KiB each).
    pub const DATA: usize = 0x5000;

    /// How much DMA memory moving `data_max` bytes at once needs.
    pub const fn needed(data_max: usize) -> usize {
        DATA + data_max
    }
}

/// Entries per queue: a page of commands holds 64; the driver uses 16.
pub const QUEUE_ENTRIES: u32 = 16;
/// The I/O queue pair's number.
pub const IO_QUEUE: u16 = 1;
pub const SECTOR: usize = 512;

/// A command: sixteen little-endian words.
///
///   0   opcode (7:0), command identifier (31:16)
///   1   namespace
///   6-7 PRP1, 8-9 PRP2
///   10-15 the command's own
pub fn command(opcode: u8, id: u16, namespace: u32, prp1: u64, prp2: u64, own: [u32; 6]) -> [u32; 16] {
    let mut c = [0u32; 16];
    c[0] = u32::from(opcode) | u32::from(id) << 16;
    c[1] = namespace;
    c[6] = prp1 as u32;
    c[7] = (prp1 >> 32) as u32;
    c[8] = prp2 as u32;
    c[9] = (prp2 >> 32) as u32;
    c[10..16].copy_from_slice(&own);
    c
}

/// An answer, from a completion queue's four words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    /// Word 0: a command's own result.
    pub result: u32,
    /// How far the controller has read the SQ.
    pub sq_head: u16,
    pub sq: u16,
    /// Which command this answers.
    pub id: u16,
    pub phase: bool,
    /// Status: code type (bits 10:8) and code (7:0); 0 is success.
    pub status: u16,
}

impl Completion {
    pub fn from_words(w: [u32; 4]) -> Completion {
        Completion {
            result: w[0],
            sq_head: w[2] as u16,
            sq: (w[2] >> 16) as u16,
            id: w[3] as u16,
            phase: w[3] & (1 << 16) != 0,
            status: ((w[3] >> 17) & 0x7FF) as u16,
        }
    }
}

/// PRP1 and PRP2 for `bytes` of data at `data` (page-aligned, physically
/// contiguous), whose pages after the first are listed at `list`.
pub fn prps(data: u64, list: u64, bytes: usize) -> (u64, u64) {
    let pages = bytes.div_ceil(layout::PAGE);
    match pages {
        0 | 1 => (data, 0),
        2 => (data, data + layout::PAGE as u64),
        _ => (data, list),
    }
}

/// Text from an IDENTIFY answer: ASCII, padded with spaces.
fn text(bytes: &[u8]) -> Result<([u8; 40], usize), &'static str> {
    let mut out = [0u8; 40];
    let n = bytes.len().min(40);
    out[..n].copy_from_slice(&bytes[..n]);
    let length = out[..n].iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |i| i + 1);
    if !out[..length].iter().all(|b| (0x20..0x7F).contains(b)) {
        return Err("a name in the IDENTIFY answer is not text");
    }
    Ok((out, length))
}

/// What the controller said about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controller {
    model: [u8; 40],
    model_length: usize,
    /// The most data one command moves, as a power of two of the
    /// smallest page (MDTS); None: no limit.
    pub transfer_max: Option<u32>,
    /// How many namespaces it may have.
    pub namespaces: u32,
}

impl Controller {
    pub fn model(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_length]).unwrap_or("?")
    }
}

/// The controller's IDENTIFY answer (CNS 1): model at bytes 24-63, the
/// transfer limit at 77, the number of namespaces at 516.
pub fn identify_controller(data: &[u8; 4096]) -> Result<Controller, &'static str> {
    let (model, model_length) = text(&data[24..64])?;
    let namespaces = u32::from_le_bytes(data[516..520].try_into().unwrap_or([0; 4]));
    Ok(Controller { model, model_length, transfer_max: (data[77] != 0).then_some(u32::from(data[77])), namespaces })
}

/// A namespace: how many blocks, and how big each is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Namespace {
    pub id: u32,
    pub blocks: u64,
    pub block_size: u32,
}

/// A namespace's IDENTIFY answer (CNS 0): its size in blocks (NSZE, bytes
/// 0-7), which of its formats is in use (FLBAS, byte 26), and the
/// formats themselves (from byte 128, four bytes each: the block size as
/// a power of two in the third byte).
pub fn identify_namespace(id: u32, data: &[u8; 4096]) -> Result<Namespace, &'static str> {
    let blocks = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
    if blocks == 0 {
        return Err("the namespace has no blocks");
    }
    let format = (data[26] & 0x0F) as usize;
    let shift = data[128 + 4 * format + 2];
    if !(9..=16).contains(&shift) {
        return Err("the namespace's block size is not one of the sizes NVMe allows");
    }
    Ok(Namespace { id, blocks, block_size: 1 << shift })
}

/// The first active namespace in a list (CNS 2): 1024 numbers, ending
/// at the first zero.
pub fn first_namespace(data: &[u8; 4096]) -> Option<u32> {
    let first = u32::from_le_bytes(data[0..4].try_into().ok()?);
    (first != 0).then_some(first)
}

/// The controller as the driver reaches it: its registers (offsets into
/// BAR 0), and a way to wait.
pub trait Bus {
    fn read(&mut self, register: usize) -> u32;
    fn write(&mut self, register: usize, value: u32);
    /// Waits a little: until the controller interrupts, or a tick
    /// passes, whichever is first. Polled, it may return at once.
    fn pause(&mut self);
    /// The time, in ticks (100 a second), for the waits' limits.
    fn ticks(&mut self) -> u64;

    /// A 64-bit register, as two 32-bit reads (the standard allows it):
    /// the low half first.
    fn read64(&mut self, register: usize) -> u64 {
        u64::from(self.read(register)) | u64::from(self.read(register + 4)) << 32
    }
    fn write64(&mut self, register: usize, value: u64) {
        self.write(register, value as u32);
        self.write(register + 4, (value >> 32) as u32);
    }
}

/// How long a command may take, in ticks.
pub const COMMAND_WAIT: u64 = 500;

/// Memory both the driver and the controller use: where it is mapped
/// here, and where it is for the controller.
pub struct Dma {
    base: *mut u8,
    physical: u64,
    size: usize,
}

impl Dma {
    /// # Safety
    /// `base` must be a mapping of `size` bytes, writable, that nothing
    /// else uses for as long as the `Dma` lives, aligned to 8 bytes (it is
    /// read and written in words: a page, as DMA memory always is); `physical` is where
    /// those bytes are for the device (one unbroken range), page-aligned.
    pub unsafe fn new(base: *mut u8, physical: u64, size: usize) -> Dma {
        assert!((base as usize).is_multiple_of(8), "DMA memory must be aligned to 8 bytes");
        Dma { base, physical, size }
    }

    pub fn physical(&self, offset: usize) -> u64 {
        self.physical + offset as u64
    }

    /// Volatile, every access: the controller reads and writes this
    /// memory behind the compiler's back.
    fn write_words(&mut self, offset: usize, words: &[u32]) {
        assert!(offset + 4 * words.len() <= self.size);
        for (n, &word) in words.iter().enumerate() {
            // SAFETY: inside the mapping (checked above), which is ours;
            // every offset used is a multiple of four.
            unsafe { self.base.add(offset + 4 * n).cast::<u32>().write_volatile(word) };
        }
    }

    fn read_words<const N: usize>(&self, offset: usize) -> [u32; N] {
        assert!(offset + 4 * N <= self.size);
        // SAFETY: as in `write_words`.
        core::array::from_fn(|n| unsafe { self.base.add(offset + 4 * n).cast::<u32>().read_volatile() })
    }

    fn zero(&mut self, offset: usize, length: usize) {
        assert!(offset + length <= self.size);
        for n in 0..length {
            // SAFETY: as in `write_words`.
            unsafe { self.base.add(offset + n).write_volatile(0) };
        }
    }

    fn read_page(&self, offset: usize) -> [u8; 4096] {
        let mut page = [0u8; 4096];
        assert!(offset + page.len() <= self.size);
        for (n, byte) in page.iter_mut().enumerate() {
            // SAFETY: as in `write_words`.
            *byte = unsafe { self.base.add(offset + n).read_volatile() };
        }
        page
    }
}

/// Why a command failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The controller answered with this status (type << 8 | code).
    Status(u16),
    /// No answer within COMMAND_WAIT.
    TimedOut,
    /// The controller says it has failed (CSTS.CFS).
    Fatal,
}

/// One queue pair, from the driver's side: where its rings are, where
/// the next command goes, where the next answer comes, and the phase
/// that answer will have.
#[derive(Clone, Copy)]
struct Pair {
    id: usize,
    sq: usize,
    cq: usize,
    entries: u32,
    tail: u32,
    head: u32,
    phase: bool,
}

impl Pair {
    const fn new(id: usize, sq: usize, cq: usize, entries: u32) -> Pair {
        Pair { id, sq, cq, entries, tail: 0, head: 0, phase: true }
    }
}

/// One namespace of an NVMe controller, set up and ready.
pub struct Disk<B: Bus> {
    bus: B,
    dma: Dma,
    cap: u64,
    stride: usize,
    admin: Pair,
    io: Pair,
    next_id: u16,
    interrupts: bool,
    pub controller: Controller,
    pub namespace: Namespace,
    /// The most sectors one command moves: what the DMA memory holds,
    /// and the controller's own limit.
    sectors_per_command: u32,
    data_max: usize,
    pub version: (u32, u32),
}

impl<B: Bus> Disk<B> {
    /// Takes the controller over and sets up its first namespace:
    ///
    ///   1. turn it off (CC.EN = 0) and wait until it says so - the
    ///      firmware, or a driver before this one, may have left it on,
    ///      with queues of its own; turning it off forgets them all;
    ///   2. give it the admin queues, set the entry sizes, turn it on,
    ///      wait until it is ready;
    ///   3. IDENTIFY the controller, find the first active namespace,
    ///      IDENTIFY that;
    ///   4. create one I/O completion queue, then one submission queue.
    pub fn start(bus: B, mut dma: Dma, interrupts: bool) -> Result<Disk<B>, &'static str> {
        if dma.size < layout::needed(layout::PAGE) || !dma.physical.is_multiple_of(layout::PAGE as u64) {
            return Err("too little DMA memory, or not page-aligned");
        }
        let mut bus = bus;
        let cap = bus.read64(reg::CAP);
        if !reg::has_nvm_commands(cap) {
            return Err("the controller does not speak the NVM command set");
        }
        if reg::smallest_page(cap) > 0 {
            return Err("the controller's pages are bigger than 4 KiB");
        }
        let entries = QUEUE_ENTRIES.min(reg::queue_entries_max(cap));
        if entries < 2 {
            return Err("the controller's queues are too small");
        }
        let vs = bus.read(reg::VS);
        // The data's pages after the first, listed once: they never move.
        let data_pages = (dma.size - layout::DATA) / layout::PAGE;
        let list: [u64; 512] = core::array::from_fn(|n| dma.physical(layout::DATA + (n + 1) * layout::PAGE));
        let list_words: [u32; 1024] = core::array::from_fn(|n| (list[n / 2] >> (32 * (n % 2))) as u32);
        dma.write_words(layout::PRP_LIST, &list_words[..2 * data_pages.min(512)]);

        let unknown_controller = Controller { model: [0; 40], model_length: 0, transfer_max: None, namespaces: 0 };
        let mut disk = Disk {
            bus,
            dma,
            cap,
            stride: reg::doorbell_stride(cap),
            admin: Pair::new(0, layout::ADMIN_SQ, layout::ADMIN_CQ, entries),
            io: Pair::new(usize::from(IO_QUEUE), layout::IO_SQ, layout::IO_CQ, entries),
            next_id: 0,
            interrupts,
            controller: unknown_controller,
            namespace: Namespace { id: 0, blocks: 0, block_size: 0 },
            sectors_per_command: 1,
            data_max: data_pages * layout::PAGE,
            version: (vs >> 16, (vs >> 8) & 0xFF),
        };
        disk.enable()?;

        let page = disk.identify(admin::CNS_CONTROLLER, 0)?;
        disk.controller = identify_controller(&page)?;
        // The first active namespace; a controller older than NVMe 1.1
        // has no list, and its namespaces are numbered from 1.
        let id = match disk.identify(admin::CNS_NAMESPACES, 0) {
            Ok(page) => first_namespace(&page).ok_or("the controller has no active namespace")?,
            Err(_) => 1,
        };
        let page = disk.identify(admin::CNS_NAMESPACE, id)?;
        disk.namespace = identify_namespace(id, &page)?;
        if disk.namespace.block_size as usize != SECTOR {
            return Err("the namespace's blocks are not 512 bytes");
        }
        let limit = match disk.controller.transfer_max {
            Some(power) if power < 20 => (layout::PAGE << power) / SECTOR,
            _ => usize::MAX,
        };
        disk.sectors_per_command = (disk.data_max / SECTOR).min(limit) as u32;
        disk.create_io_queues()?;
        Ok(disk)
    }

    /// Off, then on with our admin queues. The CQ memory is zeroed, so
    /// no old answer looks new.
    fn enable(&mut self) -> Result<(), &'static str> {
        let limit = reg::ready_ticks(self.cap).max(50);
        let config = self.bus.read(reg::CC);
        if config & reg::CC_ENABLE != 0 {
            self.bus.write(reg::CC, config & !reg::CC_ENABLE);
        }
        if !self.wait(limit, |bus| bus.read(reg::CSTS) & reg::CSTS_READY == 0) {
            return Err("the controller would not turn off");
        }
        self.dma.zero(0, layout::PRP_LIST);
        self.admin = Pair::new(0, layout::ADMIN_SQ, layout::ADMIN_CQ, self.admin.entries);
        self.io = Pair::new(usize::from(IO_QUEUE), layout::IO_SQ, layout::IO_CQ, self.io.entries);
        let size = self.admin.entries - 1;
        self.bus.write(reg::AQA, size << 16 | size);
        let (sq, cq) = (self.dma.physical(layout::ADMIN_SQ), self.dma.physical(layout::ADMIN_CQ));
        self.bus.write64(reg::ASQ, sq);
        self.bus.write64(reg::ACQ, cq);
        self.bus.write(reg::CC, reg::CC_ENTRY_SIZES | reg::CC_ENABLE);
        let ready = self.wait(limit, |bus| bus.read(reg::CSTS) & (reg::CSTS_READY | reg::CSTS_FATAL) != 0);
        if !ready || self.bus.read(reg::CSTS) & reg::CSTS_FATAL != 0 {
            return Err("the controller did not become ready");
        }
        Ok(())
    }

    /// The I/O queue pair: its CQ first (the SQ names it), interrupting
    /// on MSI-X vector 0 if the driver has an interrupt.
    fn create_io_queues(&mut self) -> Result<(), &'static str> {
        let size = self.io.entries - 1;
        let id = u32::from(IO_QUEUE);
        let contiguous = 1;
        let interrupts = if self.interrupts { 1 << 1 } else { 0 };
        let cq = self.dma.physical(layout::IO_CQ);
        let own = [size << 16 | id, interrupts | contiguous, 0, 0, 0, 0];
        self.admin_command(admin::CREATE_CQ, 0, cq, own).map_err(|_| "the controller would not create an I/O completion queue")?;
        let sq = self.dma.physical(layout::IO_SQ);
        let own = [size << 16 | id, id << 16 | contiguous, 0, 0, 0, 0];
        self.admin_command(admin::CREATE_SQ, 0, sq, own).map_err(|_| "the controller would not create an I/O submission queue")?;
        Ok(())
    }

    fn identify(&mut self, cns: u32, namespace: u32) -> Result<[u8; 4096], &'static str> {
        let data = self.dma.physical(layout::DATA);
        self.admin_command(admin::IDENTIFY, namespace, data, [cns, 0, 0, 0, 0, 0])
            .map_err(|_| "the controller did not answer IDENTIFY")?;
        Ok(self.dma.read_page(layout::DATA))
    }

    fn admin_command(&mut self, opcode: u8, namespace: u32, prp1: u64, own: [u32; 6]) -> Result<Completion, Failure> {
        let mut pair = self.admin;
        let result = self.run(&mut pair, opcode, namespace, (prp1, 0), own);
        self.admin = pair;
        result
    }

    /// Puts one command in `pair`'s SQ, rings its doorbell and waits for
    /// its answer.
    fn run(&mut self, pair: &mut Pair, opcode: u8, namespace: u32, prps: (u64, u64), own: [u32; 6]) -> Result<Completion, Failure> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let entry = command(opcode, id, namespace, prps.0, prps.1, own);
        self.dma.write_words(pair.sq + 64 * pair.tail as usize, &entry);
        pair.tail = (pair.tail + 1) % pair.entries;
        // The command is in memory before the controller is told.
        fence(Ordering::SeqCst);
        self.bus.write(reg::sq_tail(pair.id, self.stride), pair.tail);

        let deadline = self.bus.ticks() + COMMAND_WAIT;
        loop {
            let answer = Completion::from_words(self.dma.read_words::<4>(pair.cq + 16 * pair.head as usize));
            if answer.phase == pair.phase {
                // Read the rest of it (and the data) only after the
                // phase said it is new.
                fence(Ordering::SeqCst);
                pair.head = (pair.head + 1) % pair.entries;
                if pair.head == 0 {
                    pair.phase = !pair.phase;
                }
                self.bus.write(reg::cq_head(pair.id, self.stride), pair.head);
                if answer.id != id {
                    continue; // an answer to something older: not ours
                }
                return if answer.status == 0 { Ok(answer) } else { Err(Failure::Status(answer.status)) };
            }
            if self.bus.read(reg::CSTS) & reg::CSTS_FATAL != 0 {
                return Err(Failure::Fatal);
            }
            if self.bus.ticks() >= deadline {
                return Err(Failure::TimedOut);
            }
            self.bus.pause();
        }
    }

    fn wait(&mut self, limit: u64, mut done: impl FnMut(&mut B) -> bool) -> bool {
        let deadline = self.bus.ticks() + limit;
        loop {
            if done(&mut self.bus) {
                return true;
            }
            if self.bus.ticks() >= deadline {
                return false;
            }
            self.bus.pause();
        }
    }

    /// The data buffer, and how long it is: what a read left there, or
    /// what a write is to send. The controller's only during `transfer`.
    pub fn data(&mut self) -> (*mut u8, usize) {
        // SAFETY: DATA is inside the memory, `new`'s promise.
        (unsafe { self.dma.base.add(layout::DATA) }, self.data_max)
    }

    /// The most sectors one `transfer` moves.
    pub fn sectors_max(&self) -> u32 {
        (self.data_max / SECTOR) as u32
    }

    /// The most one command moves; `transfer` uses as many as it needs.
    pub fn sectors_per_command(&self) -> u32 {
        self.sectors_per_command
    }

    /// `count` sectors from `sector` on, between the disk and the data
    /// buffer - in as many commands as the controller's limit needs. The
    /// caller checks the range against `namespace.blocks`.
    ///
    /// After a timeout or a fatal error the controller is reset and its
    /// queues made again, so that the next request has a chance.
    pub fn transfer(&mut self, write: bool, sector: u64, count: u32) -> Result<(), Failure> {
        assert!(count >= 1 && count <= self.sectors_max(), "a transfer of {count} sectors");
        let mut done = 0;
        while done < count {
            let n = (count - done).min(self.sectors_per_command);
            let offset = layout::DATA + done as usize * SECTOR;
            let first = self.dma.physical(offset);
            // The list entry for the page after `first`.
            let list = self.dma.physical(layout::PRP_LIST + 8 * (done as usize * SECTOR / layout::PAGE));
            let prps = prps(first, list, n as usize * SECTOR);
            let lba = sector + u64::from(done);
            let own = [lba as u32, (lba >> 32) as u32, n - 1, 0, 0, 0];
            let opcode = if write { io::WRITE } else { io::READ };
            let mut pair = self.io;
            let result = self.run(&mut pair, opcode, self.namespace.id, prps, own);
            self.io = pair;
            match result {
                Ok(_) => done += n,
                Err(failure) => {
                    if matches!(failure, Failure::TimedOut | Failure::Fatal) {
                        let _ = self.reset();
                    }
                    return Err(failure);
                }
            }
        }
        Ok(())
    }

    /// Everything written onto the medium: one FLUSH command for the
    /// namespace. Reset after a timeout or fatal error, as `transfer`.
    pub fn flush(&mut self) -> Result<(), Failure> {
        let mut pair = self.io;
        let result = self.run(&mut pair, io::FLUSH, self.namespace.id, (0, 0), [0; 6]);
        self.io = pair;
        if let Err(failure) = result {
            if matches!(failure, Failure::TimedOut | Failure::Fatal) {
                let _ = self.reset();
            }
            return Err(failure);
        }
        Ok(())
    }

    /// Off and on again, with fresh queues: what follows a controller
    /// that stopped answering.
    pub fn reset(&mut self) -> Result<(), &'static str> {
        self.enable()?;
        self.create_io_queues()
    }
}
