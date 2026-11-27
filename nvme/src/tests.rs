//! The driver's sequence, run against a simulated NVMe controller.
//!
//! `Sim` is a controller with one disk, its namespaces a view of it.
//! Its registers behave as the standard says: CC.EN on makes it ready
//! (after checking the admin queues), off forgets every queue; the
//! doorbells make it run the commands between its head and the new
//! tail, and post answers with the right phase. Each command is read out
//! of the memory the driver wrote it to and checked: its opcode, its
//! namespace, its PRPs - the data is moved through them, page by page -
//! and the controller's transfer limit. What it finds wrong it panics
//! on, so a test fails at the first rule the driver breaks.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

const PHYSICAL: u64 = 0x0040_0000;
// Smaller under Miri, which interprets every byte copied (K187).
const BLOCKS: u64 = if cfg!(miri) { 512 } else { 8192 };


/// The DMA memory: bytes, page-aligned as real DMA memory is. The driver
/// reads and writes it in 32- and 64-bit words; in a `Vec<u8>` those were
/// at any address, and a misaligned word is undefined behaviour, which
/// Miri found (K187), as in usb's.
///
/// And one base pointer for everyone: the driver's `Dma` and the
/// simulator's slices both come from it. With the driver's pointer taken
/// through a `&mut [u8]`, each slice the simulator took afterwards was a
/// new exclusive borrow that made the driver's pointer invalid - Rust's
/// aliasing rules, which Miri checks: the simulator plays a device, which
/// on a real machine is outside Rust altogether.
pub struct Memory {
    pages: Vec<Page>,
    length: usize,
    base: *mut u8,
}

impl Default for Memory {
    fn default() -> Memory {
        Memory::new(0)
    }
}

#[derive(Clone, Copy)]
#[repr(C, align(4096))]
struct Page([u8; 4096]);

impl Memory {
    pub fn new(length: usize) -> Memory {
        let mut pages = std::vec![Page([0; 4096]); length.div_ceil(4096)];
        // From the Vec's buffer, with no reference in between; the pages
        // never move (the Vec is never grown).
        let base = pages.as_mut_ptr().cast();
        Memory { pages, length, base }
    }

    /// The memory's start, for the driver's `Dma`.
    pub fn base(&self) -> *mut u8 {
        self.base
    }

    /// Makes it `length` bytes long, if that is shorter (as Vec::truncate).
    pub fn truncate(&mut self, length: usize) {
        self.length = self.length.min(length);
    }
}

impl core::ops::Deref for Memory {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: the pages are `length` bytes and more, one after another,
        // owned by `self`.
        unsafe { core::slice::from_raw_parts(self.base, self.length) }
    }
}

impl core::ops::DerefMut for Memory {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as for `deref`, and borrowed mutably.
        unsafe { core::slice::from_raw_parts_mut(self.base, self.length) }
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct SubmissionQueue {
    address: u64,
    entries: u32,
    head: u32,
    cq: u16,
}

#[derive(Clone, Copy, Default, Debug)]
struct CompletionQueue {
    address: u64,
    entries: u32,
    tail: u32,
    /// What the driver has read up to (its head doorbell).
    head: u32,
    phase: bool,
    interrupts: bool,
}

#[derive(Default)]
struct State {
    cap: u64,
    cc: u32,
    csts: u32,
    aqa: u32,
    asq: u64,
    acq: u64,
    sqs: [Option<SubmissionQueue>; 4],
    cqs: [Option<CompletionQueue>; 4],
    memory: Memory,
    disk: Vec<u8>,
    model: String,
    /// MDTS: the most one command moves, a power of two of 4 KiB pages.
    mdts: u8,
    /// The active namespaces; None: the controller does not know CNS 2.
    namespaces: Option<Vec<u32>>,
    block_shift: u8,
    time: u64,
    /// Commands carried out: (opcode, namespace, lba, blocks).
    done: Vec<(u8, u32, u64, u32)>,
    fail_next: Option<u16>,
    hang_next: bool,
    fatal_next: bool,
    interrupts: u32,
    /// How many times CC.EN went from 0 to 1.
    enables: u32,
}

#[derive(Clone)]
struct Sim(Rc<RefCell<State>>);

impl State {
    fn at(&mut self, physical: u64, length: usize) -> &mut [u8] {
        let start = physical.checked_sub(PHYSICAL).expect("an address below the DMA memory") as usize;
        assert!(start + length <= self.memory.len(), "an address past the DMA memory: {physical:#x}+{length}");
        &mut self.memory[start..start + length]
    }

    fn word(&mut self, physical: u64) -> u32 {
        u32::from_le_bytes(self.at(physical, 4).try_into().unwrap())
    }

    fn stride(&self) -> usize {
        reg::doorbell_stride(self.cap)
    }

    /// The pieces of memory `bytes` of data are in, by its PRPs.
    fn pieces(&mut self, prp1: u64, prp2: u64, bytes: usize) -> Vec<(u64, usize)> {
        let page = layout::PAGE as u64;
        let first = (page - prp1 % page).min(bytes as u64) as usize;
        let mut pieces = vec![(prp1, first)];
        let rest = bytes - first;
        let pages = rest.div_ceil(layout::PAGE);
        if pages == 1 {
            assert_eq!(prp2 % page, 0, "PRP2 must be a page");
            pieces.push((prp2, rest));
        } else if pages > 1 {
            assert_eq!(prp2 % 8, 0, "a PRP list must be 8-byte aligned");
            assert!(prp2 % page + 8 * pages as u64 <= page, "this simulator does not follow chained lists");
            for n in 0..pages {
                let entry = u64::from(self.word(prp2 + 8 * n as u64)) | u64::from(self.word(prp2 + 8 * n as u64 + 4)) << 32;
                assert_eq!(entry % page, 0, "a PRP list entry must be a page");
                pieces.push((entry, (rest - n * layout::PAGE).min(layout::PAGE)));
            }
        } else {
            assert_eq!(prp2, 0, "PRP2 set for data in one page");
        }
        pieces
    }

    fn post(&mut self, cq: u16, sq: u16, sq_head: u32, id: u16, status: u16) {
        let mut q = self.cqs[usize::from(cq)].expect("an answer for a queue that does not exist");
        assert_ne!((q.tail + 1) % q.entries, q.head, "the completion queue is full: the driver did not read it");
        let words = [0u32, 0, sq_head | u32::from(sq) << 16, u32::from(id) | u32::from(q.phase) << 16 | u32::from(status) << 17];
        let address = q.address + 16 * u64::from(q.tail);
        for (n, word) in words.iter().enumerate() {
            self.at(address + 4 * n as u64, 4).copy_from_slice(&word.to_le_bytes());
        }
        q.tail = (q.tail + 1) % q.entries;
        if q.tail == 0 {
            q.phase = !q.phase;
        }
        if q.interrupts {
            self.interrupts += 1;
        }
        self.cqs[usize::from(cq)] = Some(q);
    }

    /// Runs what queue `n`'s SQ holds, up to `tail`.
    fn run(&mut self, n: usize, tail: u32) {
        let mut sq = self.sqs[n].expect("a doorbell for a queue that does not exist");
        assert!(tail < sq.entries, "a tail past the queue's end");
        while sq.head != tail {
            let address = sq.address + 64 * u64::from(sq.head);
            let c: Vec<u32> = (0..16).map(|i| self.word(address + 4 * i)).collect();
            sq.head = (sq.head + 1) % sq.entries;
            self.sqs[n] = Some(sq);
            let (opcode, id) = (c[0] as u8, (c[0] >> 16) as u16);
            assert_eq!((c[0] >> 8) & 0xFF, 0, "no fused commands, PRPs only");
            if self.hang_next {
                self.hang_next = false;
                continue;
            }
            if self.fatal_next {
                self.fatal_next = false;
                self.csts |= reg::CSTS_FATAL;
                return;
            }
            let status = match self.fail_next.take() {
                Some(status) => status,
                None => self.execute(n, opcode, &c),
            };
            self.post(sq.cq, n as u16, sq.head, id, status);
        }
    }

    fn execute(&mut self, queue: usize, opcode: u8, c: &[u32]) -> u16 {
        let prp1 = u64::from(c[6]) | u64::from(c[7]) << 32;
        let prp2 = u64::from(c[8]) | u64::from(c[9]) << 32;
        let namespace = c[1];
        if queue == 0 {
            match opcode {
                admin::IDENTIFY => {
                    let mut page = vec![0u8; 4096];
                    match c[10] & 0xFF {
                        admin::CNS_CONTROLLER => {
                            page[24..64].fill(b' ');
                            page[24..24 + self.model.len()].copy_from_slice(self.model.as_bytes());
                            page[77] = self.mdts;
                            page[516..520].copy_from_slice(&4u32.to_le_bytes());
                        }
                        admin::CNS_NAMESPACE => {
                            let active = self.namespaces.clone().unwrap_or(vec![1]);
                            if !active.contains(&namespace) {
                                return 0x0B; // invalid namespace
                            }
                            let blocks = self.disk.len() as u64 >> self.block_shift;
                            page[0..8].copy_from_slice(&blocks.to_le_bytes());
                            page[26] = 1; // format 1 in use
                            page[128 + 4 + 2] = self.block_shift;
                            page[128 + 2] = 12; // format 0: 4 KiB, not in use
                        }
                        admin::CNS_NAMESPACES => {
                            let Some(list) = self.namespaces.clone() else { return 0x02 }; // invalid field
                            for (i, id) in list.iter().enumerate() {
                                page[4 * i..4 * i + 4].copy_from_slice(&id.to_le_bytes());
                            }
                        }
                        cns => panic!("IDENTIFY of CNS {cns}"),
                    }
                    assert_eq!(prp1 % layout::PAGE as u64, 0, "IDENTIFY's answer goes to a page");
                    self.at(prp1, 4096).copy_from_slice(&page);
                    0
                }
                admin::CREATE_CQ => {
                    let (id, entries) = ((c[10] & 0xFFFF) as usize, (c[10] >> 16) + 1);
                    assert_eq!(c[11] & 1, 1, "physically contiguous");
                    assert_eq!(c[11] >> 16, 0, "interrupt vector 0");
                    assert!(entries <= reg::queue_entries_max(self.cap) && entries >= 2);
                    assert_eq!(prp1 % layout::PAGE as u64, 0);
                    assert!(id >= 1 && self.cqs[id].is_none(), "a CQ made twice, or as the admin one");
                    let interrupts = c[11] & 2 != 0;
                    self.cqs[id] = Some(CompletionQueue { address: prp1, entries, tail: 0, head: 0, phase: true, interrupts });
                    0
                }
                admin::CREATE_SQ => {
                    let (id, entries) = ((c[10] & 0xFFFF) as usize, (c[10] >> 16) + 1);
                    assert_eq!(c[11] & 1, 1, "physically contiguous");
                    let cq = (c[11] >> 16) as u16;
                    assert!(self.cqs[usize::from(cq)].is_some(), "an SQ for a CQ that does not exist");
                    assert!(entries <= reg::queue_entries_max(self.cap) && entries >= 2);
                    assert!(id >= 1 && self.sqs[id].is_none());
                    self.sqs[id] = Some(SubmissionQueue { address: prp1, entries, head: 0, cq });
                    0
                }
                other => panic!("admin opcode {other:#x}"),
            }
        } else {
            let active = self.namespaces.clone().unwrap_or(vec![1]);
            assert!(active.contains(&namespace), "I/O to namespace {namespace}, which is not active");
            if opcode == io::FLUSH {
                assert_eq!((prp1, prp2), (0, 0), "a flush has no data");
                self.done.push((opcode, namespace, 0, 0));
                return 0;
            }
            let lba = u64::from(c[10]) | u64::from(c[11]) << 32;
            let blocks = (c[12] & 0xFFFF) + 1;
            let bytes = (blocks as usize) << self.block_shift;
            let limit = layout::PAGE << self.mdts;
            assert!(self.mdts == 0 || bytes <= limit, "{bytes} bytes in one command: more than MDTS allows");
            if lba + u64::from(blocks) > (self.disk.len() >> self.block_shift) as u64 {
                return 0x80; // LBA out of range
            }
            let mut offset = (lba as usize) << self.block_shift;
            for (address, length) in self.pieces(prp1, prp2, bytes) {
                match opcode {
                    io::READ => {
                        let data = self.disk[offset..offset + length].to_vec();
                        self.at(address, length).copy_from_slice(&data);
                    }
                    io::WRITE => {
                        let data = self.at(address, length).to_vec();
                        self.disk[offset..offset + length].copy_from_slice(&data);
                    }
                    other => panic!("I/O opcode {other:#x}"),
                }
                offset += length;
            }
            self.done.push((opcode, namespace, lba, blocks));
            0
        }
    }
}

impl Bus for Sim {
    fn read(&mut self, register: usize) -> u32 {
        let s = self.0.borrow();
        match register {
            reg::CAP => s.cap as u32,
            0x04 => (s.cap >> 32) as u32,
            reg::VS => 0x0001_0400,
            reg::CC => s.cc,
            reg::CSTS => s.csts,
            reg::AQA => s.aqa,
            _ => panic!("a read of register {register:#x}"),
        }
    }

    fn write(&mut self, register: usize, value: u32) {
        let mut s = self.0.borrow_mut();
        let on = s.cc & reg::CC_ENABLE != 0 || s.csts & reg::CSTS_READY != 0;
        match register {
            reg::AQA | reg::ASQ | 0x2C | reg::ACQ | 0x34 => {
                assert!(!on, "the admin queues moved while the controller was on");
                match register {
                    reg::AQA => s.aqa = value,
                    reg::ASQ => s.asq = s.asq & !0xFFFF_FFFF | u64::from(value),
                    0x2C => s.asq = s.asq & 0xFFFF_FFFF | u64::from(value) << 32,
                    reg::ACQ => s.acq = s.acq & !0xFFFF_FFFF | u64::from(value),
                    _ => s.acq = s.acq & 0xFFFF_FFFF | u64::from(value) << 32,
                }
            }
            reg::CC => {
                let enable = value & reg::CC_ENABLE != 0;
                if enable && s.cc & reg::CC_ENABLE == 0 {
                    assert_eq!(value & !reg::CC_ENABLE, reg::CC_ENTRY_SIZES, "entries of 64 and 16 bytes, NVM, 4 KiB pages");
                    assert_eq!(s.asq % layout::PAGE as u64, 0, "the admin SQ must be a page");
                    assert_eq!(s.acq % layout::PAGE as u64, 0, "the admin CQ must be a page");
                    let (sq_entries, cq_entries) = ((s.aqa & 0xFFF) + 1, ((s.aqa >> 16) & 0xFFF) + 1);
                    assert!(sq_entries >= 2 && cq_entries >= 2);
                    s.sqs = [None; 4];
                    s.cqs = [None; 4];
                    s.sqs[0] = Some(SubmissionQueue { address: s.asq, entries: sq_entries, head: 0, cq: 0 });
                    let interrupts = true; // the admin CQ always may
                    s.cqs[0] = Some(CompletionQueue { address: s.acq, entries: cq_entries, tail: 0, head: 0, phase: true, interrupts });
                    s.csts = reg::CSTS_READY;
                    s.enables += 1;
                } else if !enable {
                    // Off: a reset. Every queue is gone, every error too.
                    s.sqs = [None; 4];
                    s.cqs = [None; 4];
                    s.csts = 0;
                }
                s.cc = value;
            }
            _ if register >= reg::DOORBELLS => {
                let stride = s.stride();
                let n = (register - reg::DOORBELLS) / stride;
                assert_eq!((register - reg::DOORBELLS) % stride, 0, "a doorbell between doorbells");
                assert!(s.csts & reg::CSTS_READY != 0, "a doorbell rung while the controller is off");
                if n.is_multiple_of(2) {
                    s.run(n / 2, value);
                } else {
                    let mut q = s.cqs[n / 2].expect("a head doorbell for a queue that does not exist");
                    assert!(value < q.entries);
                    q.head = value;
                    s.cqs[n / 2] = Some(q);
                }
            }
            _ => panic!("a write of register {register:#x}"),
        }
    }

    fn pause(&mut self) {
        self.0.borrow_mut().time += 1;
    }

    fn ticks(&mut self) -> u64 {
        self.0.borrow().time
    }
}

/// CAP: up to `entries` entries a queue, a timeout of 1 s, doorbells
/// 4 << `stride` apart, the NVM command set, 4 KiB pages.
fn cap(entries: u32, stride: u64) -> u64 {
    u64::from(entries - 1) | 2 << 24 | stride << 32 | 1 << 37
}

fn machine() -> Sim {
    let s = State {
        cap: cap(64, 0),
        memory: Memory::new(layout::needed(32 * layout::PAGE)),
        disk: (0..BLOCKS as usize * SECTOR).map(|i| (i * 13 + i / SECTOR) as u8).collect(),
        model: "QEMU NVMe Ctrl".into(),
        mdts: 7,
        namespaces: Some(vec![1]),
        block_shift: 9,
        ..State::default()
    };
    Sim(Rc::new(RefCell::new(s)))
}

fn start(sim: &Sim, interrupts: bool) -> Result<Disk<Sim>, &'static str> {
    let (base, size) = {
        let mut s = sim.0.borrow_mut();
        (s.memory.base(), s.memory.len())
    };
    // SAFETY: the Vec stays alive (and is never resized) as long as the
    // Sim, which the Disk holds.
    let dma = unsafe { Dma::new(base, PHYSICAL, size) };
    Disk::start(sim.clone(), dma, interrupts)
}

fn data(disk: &mut Disk<Sim>, length: usize) -> &mut [u8] {
    let (base, size) = disk.data();
    assert!(length <= size);
    // SAFETY: the data buffer, not the controller's outside `transfer`.
    unsafe { core::slice::from_raw_parts_mut(base, length) }
}

fn disk_bytes(sim: &Sim, block: u64, blocks: u64) -> Vec<u8> {
    sim.0.borrow().disk[block as usize * SECTOR..(block + blocks) as usize * SECTOR].to_vec()
}

#[test]
fn the_controller_is_set_up_and_its_namespace_found() {
    let sim = machine();
    let disk = start(&sim, true).unwrap();
    assert_eq!(disk.controller.model(), "QEMU NVMe Ctrl");
    assert_eq!(disk.namespace, Namespace { id: 1, blocks: BLOCKS, block_size: 512 });
    assert_eq!(disk.version, (1, 4));
    assert_eq!(disk.sectors_max(), 256);
    assert_eq!(disk.sectors_per_command(), 256, "MDTS 7 is 512 KiB: more than the memory holds");
    let s = sim.0.borrow();
    assert_eq!((s.asq, s.acq), (PHYSICAL, PHYSICAL + 0x1000));
    assert_eq!(s.aqa, 15 << 16 | 15);
    let (sq, cq) = (s.sqs[1].unwrap(), s.cqs[1].unwrap());
    assert_eq!((sq.address, sq.entries, sq.cq), (PHYSICAL + 0x2000, 16, 1));
    assert_eq!((cq.address, cq.entries, cq.interrupts), (PHYSICAL + 0x3000, 16, true));
    assert_eq!(s.interrupts, 5, "three IDENTIFY answers and two queues made, on the admin CQ");
}

#[test]
fn without_an_interrupt_the_io_queue_is_quiet() {
    let sim = machine();
    let mut disk = start(&sim, false).unwrap();
    let before = sim.0.borrow().interrupts;
    disk.transfer(false, 0, 1).unwrap();
    assert!(!sim.0.borrow().cqs[1].unwrap().interrupts);
    assert_eq!(sim.0.borrow().interrupts, before);
}

#[test]
fn every_shape_of_transfer_goes_and_comes_back() {
    let sim = machine();
    let mut disk = start(&sim, true).unwrap();
    // One sector; a page; a page and a sector (PRP2 a page); two pages;
    // two and a bit (PRP2 a list); all 256 sectors (a list of 31).
    for (n, &count) in [1u32, 8, 9, 16, 17, 256].iter().enumerate() {
        // On the disk whatever its size (smaller under Miri); with 8192
        // blocks, 1000 to 2500 as always.
        let at = (1000 + 300 * n as u64) % (BLOCKS - 256);
        let pattern: Vec<u8> = (0..count as usize * SECTOR).map(|i| (i % 253) as u8 ^ n as u8).collect();
        data(&mut disk, pattern.len()).copy_from_slice(&pattern);
        disk.transfer(true, at, count).unwrap();
        assert_eq!(disk_bytes(&sim, at, u64::from(count)), pattern, "{count} sectors written");
        data(&mut disk, pattern.len()).fill(0);
        disk.transfer(false, at, count).unwrap();
        assert_eq!(data(&mut disk, pattern.len()), &pattern[..], "{count} sectors read");
    }
    // The last block, and an LBA above 32 bits would show in word 11.
    disk.transfer(false, BLOCKS - 1, 1).unwrap();
    assert_eq!(data(&mut disk, SECTOR), &disk_bytes(&sim, BLOCKS - 1, 1)[..]);
    let done = sim.0.borrow().done.clone();
    assert_eq!(done.last(), Some(&(io::READ, 1, BLOCKS - 1, 1)));
    // And a flush: a command of its own, for the namespace, with no data.
    disk.flush().unwrap();
    assert_eq!(sim.0.borrow().done.last(), Some(&(io::FLUSH, 1, 0, 0)));
}

#[test]
fn the_phase_turns_as_the_queues_wrap() {
    let sim = machine();
    let mut disk = start(&sim, true).unwrap();
    for n in 0..50u64 {
        disk.transfer(false, n, 1).unwrap();
        assert_eq!(data(&mut disk, SECTOR), &disk_bytes(&sim, n, 1)[..], "read {n}");
    }
    assert_eq!(sim.0.borrow().done.len(), 50, "three times round a queue of 16");
}

#[test]
fn a_small_transfer_limit_splits_a_transfer() {
    let sim = machine();
    // 16 KiB, 32 sectors a command: four pages, so each command has a
    // PRP list of its own - the part of the one list for its pages.
    sim.0.borrow_mut().mdts = 2;
    let mut disk = start(&sim, true).unwrap();
    assert_eq!(disk.sectors_per_command(), 32);
    let pattern: Vec<u8> = (0..256 * SECTOR).map(|i| (i / 7) as u8).collect();
    data(&mut disk, pattern.len()).copy_from_slice(&pattern);
    disk.transfer(true, 100, 256).unwrap();
    assert_eq!(disk_bytes(&sim, 100, 256), pattern);
    data(&mut disk, 256 * SECTOR).fill(0);
    disk.transfer(false, 100, 72).unwrap();
    let done = sim.0.borrow().done.clone();
    assert_eq!(done.len(), 8 + 3);
    assert_eq!(&done[8..], &[(io::READ, 1, 100, 32), (io::READ, 1, 132, 32), (io::READ, 1, 164, 8)]);
    assert_eq!(data(&mut disk, 72 * SECTOR), &pattern[..72 * SECTOR]);
    // A limit of 8 KiB: two pages, PRP2 the second - no list at all.
    let sim = machine();
    sim.0.borrow_mut().mdts = 1;
    let mut disk = start(&sim, true).unwrap();
    data(&mut disk, 256 * SECTOR).copy_from_slice(&pattern);
    disk.transfer(true, 0, 256).unwrap();
    assert_eq!(disk_bytes(&sim, 0, 256), pattern);
    assert_eq!(sim.0.borrow().done.len(), 16);
}

#[test]
fn an_error_status_comes_back_and_the_next_command_works() {
    let sim = machine();
    let mut disk = start(&sim, true).unwrap();
    sim.0.borrow_mut().fail_next = Some(0x281); // media error: unrecovered read
    assert_eq!(disk.transfer(false, 5, 1), Err(Failure::Status(0x281)));
    disk.transfer(false, 5, 1).unwrap();
    assert_eq!(data(&mut disk, SECTOR), &disk_bytes(&sim, 5, 1)[..]);
    // The disk's own refusal of a block past its end.
    assert_eq!(disk.transfer(false, BLOCKS, 1), Err(Failure::Status(0x80)));
}

#[test]
fn a_command_that_never_ends_resets_the_controller() {
    let sim = machine();
    let mut disk = start(&sim, true).unwrap();
    sim.0.borrow_mut().hang_next = true;
    let before = sim.0.borrow().time;
    assert_eq!(disk.transfer(false, 0, 1), Err(Failure::TimedOut));
    assert!(sim.0.borrow().time - before >= COMMAND_WAIT);
    // Off and on again: whatever the controller was doing with the lost
    // command, it has forgotten it, and has new queues.
    assert_eq!(sim.0.borrow().enables, 2, "the controller was not reset");
    disk.transfer(false, 7, 2).unwrap();
    assert_eq!(data(&mut disk, 2 * SECTOR), &disk_bytes(&sim, 7, 2)[..]);
}

#[test]
fn a_fatal_controller_is_reset() {
    let sim = machine();
    let mut disk = start(&sim, true).unwrap();
    sim.0.borrow_mut().fatal_next = true;
    assert_eq!(disk.transfer(true, 0, 1), Err(Failure::Fatal));
    assert_eq!(sim.0.borrow().csts, reg::CSTS_READY, "off and on again: no longer fatal");
    disk.transfer(false, 3, 1).unwrap();
}

#[test]
fn a_controller_left_on_is_turned_off_first() {
    // The firmware, or the driver before this one, left it on, with
    // queues of its own somewhere. The simulator panics if the admin
    // queues move while it is on.
    let sim = machine();
    let first = start(&sim, true).unwrap();
    drop(first);
    // A command issued and never answered, as a dying driver's: its I/O
    // queue's doorbell rung, the controller still busy with it.
    sim.0.borrow_mut().hang_next = true;
    sim.clone().write(reg::sq_tail(usize::from(IO_QUEUE), 4), 1);
    assert!(!sim.0.borrow().hang_next);
    assert_eq!(sim.0.borrow().cc & reg::CC_ENABLE, reg::CC_ENABLE);
    let mut second = start(&sim, true).unwrap();
    second.transfer(false, 9, 1).unwrap();
    assert_eq!(data(&mut second, SECTOR), &disk_bytes(&sim, 9, 1)[..]);
}

#[test]
fn the_first_active_namespace_is_used() {
    let sim = machine();
    sim.0.borrow_mut().namespaces = Some(vec![3, 4]);
    let mut disk = start(&sim, true).unwrap();
    assert_eq!(disk.namespace.id, 3);
    disk.transfer(false, 0, 1).unwrap();
    assert_eq!(sim.0.borrow().done[0].1, 3);
    // A controller without the list (before NVMe 1.1): namespace 1.
    let sim = machine();
    sim.0.borrow_mut().namespaces = None;
    assert_eq!(start(&sim, true).unwrap().namespace.id, 1);
    // An empty list.
    let sim = machine();
    sim.0.borrow_mut().namespaces = Some(vec![]);
    assert_eq!(start(&sim, true).err(), Some("the controller has no active namespace"));
}

#[test]
fn what_cannot_be_driven_is_refused() {
    let sim = machine();
    sim.0.borrow_mut().block_shift = 12;
    assert_eq!(start(&sim, true).err(), Some("the namespace's blocks are not 512 bytes"));
    let sim = machine();
    sim.0.borrow_mut().cap &= !(1 << 37);
    assert_eq!(start(&sim, true).err(), Some("the controller does not speak the NVM command set"));
    let sim = machine();
    sim.0.borrow_mut().cap |= 1 << 48;
    assert_eq!(start(&sim, true).err(), Some("the controller's pages are bigger than 4 KiB"));
}

#[test]
fn wider_doorbells_and_smaller_queues() {
    let sim = machine();
    sim.0.borrow_mut().cap = cap(4, 1); // 4 entries, doorbells 8 bytes apart
    let mut disk = start(&sim, true).unwrap();
    for n in 0..10 {
        disk.transfer(false, n, 1).unwrap();
    }
    let s = sim.0.borrow();
    assert_eq!(s.sqs[1].unwrap().entries, 4);
    assert_eq!(s.done.len(), 10);
}

#[test]
fn commands_and_answers_are_laid_out_as_the_standard_says() {
    let c = command(io::READ, 0x1234, 1, 0x1_0000_5000, 0x4000, [0x8765_4321, 0x1, 255, 0, 0, 0]);
    assert_eq!(c[0], 0x1234_0002);
    assert_eq!(c[1], 1);
    assert_eq!(&c[6..10], &[0x5000, 1, 0x4000, 0]);
    assert_eq!(&c[10..13], &[0x8765_4321, 1, 255]);
    let a = Completion::from_words([7, 0, 0x0001_0003, 0x1234 | 1 << 16 | 0x281 << 17]);
    assert_eq!(a, Completion { result: 7, sq_head: 3, sq: 1, id: 0x1234, phase: true, status: 0x281 });
    assert_eq!(prps(0x5000, 0x4000, 512), (0x5000, 0));
    assert_eq!(prps(0x5000, 0x4000, 4096), (0x5000, 0));
    assert_eq!(prps(0x5000, 0x4000, 4097), (0x5000, 0x6000));
    assert_eq!(prps(0x5000, 0x4000, 8193), (0x5000, 0x4000));
    assert_eq!((reg::sq_tail(1, 4), reg::cq_head(1, 4), reg::cq_head(1, 8)), (0x1008, 0x100C, 0x1018));
    assert_eq!(reg::ready_ticks(cap(64, 0)), 100);
}

#[test]
fn identify_answers_are_read() {
    let mut page = [0u8; 4096];
    page[24..64].fill(b' ');
    page[24..37].copy_from_slice(b"Samsung 990  ");
    page[77] = 9;
    page[516..520].copy_from_slice(&1u32.to_le_bytes());
    let c = identify_controller(&page).unwrap();
    assert_eq!((c.model(), c.transfer_max, c.namespaces), ("Samsung 990", Some(9), 1));
    page[77] = 0;
    assert_eq!(identify_controller(&page).unwrap().transfer_max, None, "0: no limit");
    page[30] = 0x07;
    assert_eq!(identify_controller(&page), Err("a name in the IDENTIFY answer is not text"));

    let mut ns = [0u8; 4096];
    ns[0..8].copy_from_slice(&1_000_000u64.to_le_bytes());
    ns[26] = 0x12; // format 2 (the high bits are metadata settings)
    ns[128 + 8 + 2] = 9;
    assert_eq!(identify_namespace(5, &ns), Ok(Namespace { id: 5, blocks: 1_000_000, block_size: 512 }));
    ns[128 + 8 + 2] = 3;
    assert!(identify_namespace(5, &ns).is_err());
    assert_eq!(identify_namespace(5, &[0; 4096]), Err("the namespace has no blocks"));
    assert_eq!(first_namespace(&[0; 4096]), None);
}
