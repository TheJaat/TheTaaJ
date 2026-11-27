//! The driver's sequence, run against a simulated AHCI controller.
//!
//! `Sim` is a controller with a disk: registers that behave as the
//! standard says (write-1-to-clear status bits, CR following ST and FR
//! following FRE, COMRESET), and a command engine that reads slot 0's
//! header, table and PRD out of the same memory the driver wrote them
//! to, checks every field the standard constrains, and moves data
//! between that memory and a disk held in a Vec. What it finds wrong it
//! panics on, so a test fails at the first rule the driver breaks.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

const PHYSICAL: u64 = 0x0012_3000;
// Smaller under Miri, which interprets every byte copied: 2 MiB of disk
// made each test take minutes there (K187).
const DISK_SECTORS: u64 = if cfg!(miri) { 256 } else { 4096 };


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

#[derive(Default)]
struct State {
    global: [u32; 0x20],
    ports: [[u32; 0x20]; 32],
    memory: Memory,
    physical: u64,
    disk: Vec<u8>,
    identify: Vec<u8>,
    time: u64,
    /// Commands carried out: (command, lba, sectors).
    done: Vec<(u8, u64, u32)>,
    /// FLUSH CACHE commands that completed.
    flushes: u32,
    /// The next command fails with this error byte.
    fail_next: Option<u8>,
    /// The next command never ends.
    hang_next: bool,
    /// The disk is busy until the link is reset.
    busy_until_reset: bool,
    /// The port never confirms that it stopped.
    stuck_running: bool,
    /// The BIOS owns the controller and lets go after this many ticks
    /// (None: never).
    bios_lets_go_after: Option<u64>,
    bios_asked_at: Option<u64>,
    /// How often an enabled interrupt would have been raised.
    interrupts: u32,
    link_resets: u32,
}

#[derive(Clone)]
struct Sim(Rc<RefCell<State>>);

fn reg(offset: usize) -> usize {
    offset / 4
}

impl State {
    fn port_of(register: usize) -> Option<(usize, usize)> {
        (register >= 0x100).then(|| ((register - 0x100) / 0x80, (register - 0x100) % 0x80))
    }

    /// Bytes of the DMA memory at a physical address.
    fn at(&mut self, physical: u64, length: usize) -> &mut [u8] {
        let start = physical.checked_sub(self.physical).expect("an address below the DMA memory") as usize;
        assert!(start + length <= self.memory.len(), "an address past the DMA memory: {physical:#x}+{length}");
        &mut self.memory[start..start + length]
    }

    fn word(&mut self, physical: u64) -> u32 {
        u32::from_le_bytes(self.at(physical, 4).try_into().unwrap())
    }

    fn raise(&mut self, n: usize, bits: u32) {
        self.ports[n][reg(port::IS)] |= bits;
        self.global[reg(hba::IS)] |= 1 << n;
        if self.ports[n][reg(port::IE)] & bits != 0 && self.global[reg(hba::GHC)] & hba::GHC_IE != 0 {
            self.interrupts += 1;
        }
    }

    /// Slot 0 of port `n` was issued: do what it says.
    fn execute(&mut self, n: usize) {
        let p = self.ports[n];
        assert!(p[reg(port::CMD)] & port::CMD_ST != 0, "a command issued on a stopped port");
        assert!(p[reg(port::CMD)] & port::CMD_FRE != 0, "a command issued with FIS receiving off");
        let list = u64::from(p[reg(port::CLB)]) | u64::from(p[reg(port::CLBU)]) << 32;
        let received = u64::from(p[reg(port::FB)]) | u64::from(p[reg(port::FBU)]) << 32;
        assert_eq!(list % 1024, 0, "the command list must be 1 KiB aligned");
        assert_eq!(received % 256, 0, "the received-FIS area must be 256-byte aligned");
        assert!(self.at(received, 256).len() == 256);

        let header: Vec<u32> = (0..8).map(|i| self.word(list + 4 * i)).collect();
        assert_eq!(header[0] & 0x1F, 5, "the FIS is five words long");
        let write = header[0] & (1 << 6) != 0;
        let prds = header[0] >> 16;
        assert!(prds <= 1, "at most one PRD entry");
        let table = u64::from(header[2]) | u64::from(header[3]) << 32;
        assert_eq!(table % 128, 0, "a command table must be 128-byte aligned");
        let fis = self.at(table, 20).to_vec();
        assert_eq!(fis[0], ata::FIS_H2D);
        assert_eq!(fis[1], ata::FIS_COMMAND);
        let prd: Vec<u32> = (0..4).map(|i| self.word(table + 0x80 + 4 * i)).collect();
        let buffer = u64::from(prd[0]) | u64::from(prd[1]) << 32;
        // No PRD entry: no data moves (FLUSH CACHE).
        let bytes = if prds == 0 {
            0
        } else {
            assert_eq!(buffer % 2, 0, "a PRD's address must be even");
            assert_eq!(prd[3] & 1, 1, "a PRD's length must be even (stored minus one)");
            (prd[3] & 0x3F_FFFF) as usize + 1
        };

        if self.hang_next {
            self.hang_next = false;
            return; // CI stays set; nothing ever happens
        }
        let command = fis[2];
        let (lba, count) = match command {
            ata::READ_DMA_EXT | ata::WRITE_DMA_EXT => {
                let lba = u64::from_le_bytes([fis[4], fis[5], fis[6], fis[8], fis[9], fis[10], 0, 0]);
                let count = u32::from(fis[12]) | u32::from(fis[13]) << 8;
                (lba, if count == 0 { 65536 } else { count })
            }
            ata::READ_DMA | ata::WRITE_DMA => {
                assert_eq!(fis[8] | fis[9] | fis[10] | fis[13], 0, "28-bit: no high bytes");
                let lba = u64::from_le_bytes([fis[4], fis[5], fis[6], fis[7] & 0x0F, 0, 0, 0, 0]);
                (lba, if fis[12] == 0 { 256 } else { u32::from(fis[12]) })
            }
            _ => (0, 0),
        };
        if matches!(command, ata::READ_DMA_EXT | ata::WRITE_DMA_EXT | ata::READ_DMA | ata::WRITE_DMA) {
            assert_eq!(fis[7] & ata::DEVICE_LBA, ata::DEVICE_LBA, "sector numbers are LBAs");
        }
        let error = match command {
            _ if self.fail_next.is_some() => self.fail_next.take(),
            ata::FLUSH_CACHE_EXT | ata::FLUSH_CACHE => {
                assert!(!write && bytes == 0, "a flush moves no data");
                self.flushes += 1;
                None
            }
            ata::IDENTIFY => {
                assert!(!write && bytes == SECTOR);
                let identify = self.identify.clone();
                self.at(buffer, SECTOR).copy_from_slice(&identify);
                None
            }
            ata::READ_DMA_EXT | ata::READ_DMA | ata::WRITE_DMA_EXT | ata::WRITE_DMA => {
                let is_write = matches!(command, ata::WRITE_DMA_EXT | ata::WRITE_DMA);
                assert_eq!(write, is_write, "the header's direction matches the command");
                assert_eq!(bytes, count as usize * SECTOR, "the PRD covers the sectors");
                if lba + u64::from(count) > (self.disk.len() / SECTOR) as u64 {
                    Some(0x10) // IDNF: no such sector
                } else {
                    let range = lba as usize * SECTOR..(lba as usize + count as usize) * SECTOR;
                    if is_write {
                        let data = self.at(buffer, bytes).to_vec();
                        self.disk[range].copy_from_slice(&data);
                    } else {
                        let data = self.disk[range].to_vec();
                        self.at(buffer, bytes).copy_from_slice(&data);
                    }
                    None
                }
            }
            _ => Some(0x04), // ABRT: no such command
        };
        match error {
            Some(error) => {
                // The disk's status: ready, error; its error byte above.
                self.ports[n][reg(port::TFD)] = 0x51 | u32::from(error) << 8;
                self.raise(n, port::IS_TFES);
            }
            None => {
                self.done.push((command, lba, count));
                let moved = (bytes as u32).to_le_bytes();
                self.at(list + 4, 4).copy_from_slice(&moved);
                self.ports[n][reg(port::TFD)] = 0x50;
                self.ports[n][reg(port::CI)] &= !1;
                self.raise(n, if command == ata::IDENTIFY { port::IS_PSS } else { port::IS_DHRS });
            }
        }
    }
}

impl Hba for Sim {
    fn read(&mut self, register: usize) -> u32 {
        let s = self.0.borrow();
        assert_eq!(register % 4, 0);
        match State::port_of(register) {
            Some((n, offset)) => s.ports[n][reg(offset)],
            None => s.global[reg(register)],
        }
    }

    fn write(&mut self, register: usize, value: u32) {
        let mut s = self.0.borrow_mut();
        let Some((n, offset)) = State::port_of(register) else {
            match register {
                hba::IS => s.global[reg(hba::IS)] &= !value,
                hba::BOHC => {
                    if value & hba::BOHC_OOS != 0 && s.bios_asked_at.is_none() {
                        s.bios_asked_at = Some(s.time);
                    }
                    let kept = s.global[reg(hba::BOHC)] & (hba::BOHC_BOS | hba::BOHC_BB);
                    s.global[reg(hba::BOHC)] = value & hba::BOHC_OOS | kept;
                }
                hba::CAP | hba::PI | hba::VS | hba::CAP2 => panic!("a read-only register written: {register:#x}"),
                _ => s.global[reg(register)] = value,
            }
            return;
        };
        let running = s.ports[n][reg(port::CMD)] & (port::CMD_CR | port::CMD_FR) != 0;
        match offset {
            port::CLB | port::CLBU | port::FB | port::FBU => {
                assert!(!running, "the lists moved while the port was running");
                s.ports[n][reg(offset)] = value;
            }
            port::IS | port::SERR => s.ports[n][reg(offset)] &= !value,
            port::CMD => {
                let mut command = value & !(port::CMD_CR | port::CMD_FR);
                if value & port::CMD_ST != 0 {
                    assert!(value & port::CMD_FRE != 0, "ST set before FRE");
                    command |= port::CMD_CR;
                } else if s.stuck_running {
                    command |= port::CMD_CR;
                } else {
                    s.ports[n][reg(port::CI)] = 0; // stopping forgets what was issued
                }
                if value & port::CMD_FRE != 0 {
                    command |= port::CMD_FR;
                }
                s.ports[n][reg(port::CMD)] = command;
                if value & port::CMD_ST != 0 && s.ports[n][reg(port::TFD)] & 0x01 != 0 {
                    s.ports[n][reg(port::TFD)] = 0x50; // a restart clears the error
                }
            }
            port::CI => {
                assert_eq!(s.ports[n][reg(port::CI)] & value, 0, "a slot issued twice");
                s.ports[n][reg(port::CI)] |= value;
                if value & 1 != 0 {
                    s.execute(n);
                }
            }
            port::SCTL => {
                if value & 0xF == port::SCTL_DET_RESET {
                    s.link_resets += 1;
                    s.ports[n][reg(port::SSTS)] = 0;
                } else if s.ports[n][reg(port::SCTL)] & 0xF == port::SCTL_DET_RESET {
                    s.ports[n][reg(port::SSTS)] = 0x113;
                    s.ports[n][reg(port::TFD)] = 0x50;
                    s.busy_until_reset = false;
                }
                s.ports[n][reg(port::SCTL)] = value;
            }
            port::IE => s.ports[n][reg(offset)] = value,
            _ => panic!("a read-only port register written: {offset:#x}"),
        }
    }

    fn pause(&mut self) {
        let mut s = self.0.borrow_mut();
        s.time += 1;
        if let (Some(after), Some(asked)) = (s.bios_lets_go_after, s.bios_asked_at) {
            if s.time >= asked + after {
                s.global[reg(hba::BOHC)] &= !(hba::BOHC_BOS | hba::BOHC_BB);
            }
        }
        if s.busy_until_reset {
            for p in s.ports.iter_mut() {
                if p[reg(port::SSTS)] != 0 {
                    p[reg(port::TFD)] = 0x80;
                }
            }
        }
    }

    fn ticks(&mut self) -> u64 {
        self.0.borrow().time
    }
}

/// What IDENTIFY answers, built word by word as the standard lays it out.
fn identify_data(sectors: u64, lba48: bool, model: &str) -> Vec<u8> {
    let mut words = [0u16; 256];
    words[49] = 1 << 9; // LBA
    words[60] = sectors.min(0x0FFF_FFFF) as u16;
    words[61] = (sectors.min(0x0FFF_FFFF) >> 16) as u16;
    words[83] = 0x4000 | if lba48 { 1 << 10 } else { 0 };
    for i in 0..4 {
        words[100 + i] = if lba48 { (sectors >> (16 * i)) as u16 } else { 0 };
    }
    let mut text = [b' '; 40];
    text[..model.len()].copy_from_slice(model.as_bytes());
    for n in 0..20 {
        words[27 + n] = u16::from_be_bytes([text[2 * n], text[2 * n + 1]]);
    }
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// A controller with six ports, a disk on port `disk_on` (if any), a CD
/// drive on port 1, and memory for 16 sectors at a time.
fn machine(disk_on: Option<usize>) -> Sim {
    let mut s = State {
        memory: Memory::new(layout::needed(16 * SECTOR)),
        physical: PHYSICAL,
        disk: (0..DISK_SECTORS as usize * SECTOR).map(|i| (i * 7 + i / SECTOR) as u8).collect(),
        identify: identify_data(DISK_SECTORS, true, "QEMU HARDDISK"),
        ..State::default()
    };
    // Six ports, 32 slots, 64-bit DMA; AHCI 1.0.
    s.global[reg(hba::CAP)] = hba::CAP_S64A | 31 << 8 | 5;
    s.global[reg(hba::PI)] = 0x3F;
    s.global[reg(hba::VS)] = 0x0001_0000;
    s.ports[1][reg(port::SSTS)] = 0x113;
    s.ports[1][reg(port::SIG)] = port::SIG_ATAPI;
    if let Some(n) = disk_on {
        s.ports[n][reg(port::SSTS)] = 0x113; // present, gen 1, active
        s.ports[n][reg(port::SIG)] = port::SIG_ATA;
        s.ports[n][reg(port::TFD)] = 0x50;
    }
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
    // SAFETY: the data buffer, not the HBA's outside `transfer`.
    unsafe { core::slice::from_raw_parts_mut(base, length) }
}

#[test]
fn the_first_disk_is_found_and_set_up() {
    let sim = machine(Some(3));
    let disk = start(&sim, true).unwrap();
    assert_eq!(disk.port(), 3, "port 1's CD drive is passed over");
    assert_eq!(disk.identity.sectors, DISK_SECTORS);
    assert_eq!(disk.identity.model(), "QEMU HARDDISK");
    assert_eq!((disk.ports, disk.version), (6, (1, 0, 0)));
    assert_eq!(disk.sectors_max(), 16, "as much as the memory holds");
    let s = sim.0.borrow();
    let p = s.ports[3];
    assert_eq!(p[reg(port::CLB)], (PHYSICAL + layout::COMMAND_LIST as u64) as u32);
    assert_eq!(p[reg(port::FB)], (PHYSICAL + layout::RECEIVED_FIS as u64) as u32);
    assert_eq!(p[reg(port::CMD)] & (port::CMD_ST | port::CMD_FRE), port::CMD_ST | port::CMD_FRE);
    assert_eq!(p[reg(port::IE)], port::IE_WANTED);
    assert_eq!(s.global[reg(hba::GHC)], hba::GHC_AE | hba::GHC_IE);
    assert_eq!(s.done, vec![(ata::IDENTIFY, 0, 0)]);
    assert_eq!(s.interrupts, 1, "IDENTIFY's end interrupted");
    assert_eq!(s.ports[1][reg(port::CMD)], 0, "the CD drive's port is left alone");
}

#[test]
fn without_an_interrupt_nothing_is_enabled() {
    let sim = machine(Some(0));
    start(&sim, false).unwrap();
    let s = sim.0.borrow();
    assert_eq!(s.ports[0][reg(port::IE)], 0);
    assert_eq!(s.global[reg(hba::GHC)], hba::GHC_AE);
    assert_eq!(s.interrupts, 0);
}

#[test]
#[cfg_attr(miri, ignore = "sectors past the smaller disk Miri gets (K187)")]
fn sectors_go_to_the_disk_and_come_back() {
    let sim = machine(Some(0));
    let mut disk = start(&sim, true).unwrap();
    let pattern: Vec<u8> = (0..16 * SECTOR).map(|i| (i % 251) as u8).collect();
    data(&mut disk, 16 * SECTOR).copy_from_slice(&pattern);
    disk.transfer(true, 1000, 16).unwrap();
    assert_eq!(&sim.0.borrow().disk[1000 * SECTOR..1016 * SECTOR], &pattern[..]);

    data(&mut disk, 16 * SECTOR).fill(0);
    disk.transfer(false, 1003, 2).unwrap();
    assert_eq!(data(&mut disk, 2 * SECTOR), &pattern[3 * SECTOR..5 * SECTOR]);
    // The last sector, and an LBA with bits above 24 in the FIS.
    disk.transfer(false, DISK_SECTORS - 1, 1).unwrap();
    let expected = sim.0.borrow().disk[(DISK_SECTORS as usize - 1) * SECTOR..].to_vec();
    assert_eq!(data(&mut disk, SECTOR), &expected[..]);
    let done = sim.0.borrow().done.clone();
    assert_eq!(
        &done[1..],
        &[(ata::WRITE_DMA_EXT, 1000, 16), (ata::READ_DMA_EXT, 1003, 2), (ata::READ_DMA_EXT, DISK_SECTORS - 1, 1)]
    );
    // The HBA's count of bytes moved, in the header: the last command's.
    let moved = sim.0.borrow_mut().word(PHYSICAL + 4);
    assert_eq!(moved, SECTOR as u32);
}

#[test]
fn a_running_port_is_stopped_before_its_lists_move() {
    let sim = machine(Some(2));
    {
        // The firmware left port 2 running, with lists of its own.
        let mut s = sim.0.borrow_mut();
        s.ports[2][reg(port::CMD)] = port::CMD_ST | port::CMD_CR | port::CMD_FRE | port::CMD_FR;
        s.ports[2][reg(port::CLB)] = 0x9_0000;
    }
    // The simulator panics if CLB or FB change while it runs.
    let disk = start(&sim, true).unwrap();
    assert_eq!(disk.port(), 2);
    assert_eq!(sim.0.borrow().ports[2][reg(port::CLB)], PHYSICAL as u32);
}

#[test]
fn a_port_that_will_not_stop_is_given_up() {
    let sim = machine(Some(0));
    {
        let mut s = sim.0.borrow_mut();
        s.ports[0][reg(port::CMD)] = port::CMD_ST | port::CMD_CR;
        s.stuck_running = true;
    }
    assert_eq!(start(&sim, true).err(), Some("the port would not stop"));
    // And it gave up after the standard's 500 ms, not sooner or never.
    let time = sim.0.borrow().time;
    assert!((STOP_WAIT..STOP_WAIT + 3).contains(&time), "gave up after {time} ticks");
}

#[test]
fn an_error_is_reported_and_the_port_recovers() {
    let sim = machine(Some(0));
    let mut disk = start(&sim, true).unwrap();
    sim.0.borrow_mut().fail_next = Some(0x40); // UNC: an unreadable sector
    let failure = disk.transfer(false, 7, 1).unwrap_err();
    assert_eq!(failure, Failure::Disk { status: 0x51, error: 0x40, interrupt_status: port::IS_TFES });
    // The next command works: the port was restarted, the error cleared.
    disk.transfer(false, 7, 1).unwrap();
    let expected = sim.0.borrow().disk[7 * SECTOR..8 * SECTOR].to_vec();
    assert_eq!(data(&mut disk, SECTOR), &expected[..]);
    let s = sim.0.borrow();
    assert_eq!(s.ports[0][reg(port::IS)], 0, "every status bit was cleared");
    assert_eq!(s.global[reg(hba::IS)], 0);
}

#[test]
fn a_sector_past_the_end_is_the_disks_error() {
    // The service checks ranges first; if one slipped through, the
    // disk's own refusal (IDNF) comes back as an error, not a hang.
    let sim = machine(Some(0));
    let mut disk = start(&sim, false).unwrap();
    let failure = disk.transfer(false, DISK_SECTORS, 1).unwrap_err();
    assert!(matches!(failure, Failure::Disk { error: 0x10, .. }));
}

#[test]
fn a_command_that_never_ends_times_out_and_the_port_recovers() {
    let sim = machine(Some(0));
    let mut disk = start(&sim, true).unwrap();
    let before = sim.0.borrow().time;
    sim.0.borrow_mut().hang_next = true;
    assert_eq!(disk.transfer(false, 0, 1), Err(Failure::TimedOut));
    let waited = sim.0.borrow().time - before;
    assert!((COMMAND_WAIT..COMMAND_WAIT + 3).contains(&waited), "waited {waited} ticks");
    disk.transfer(false, 0, 1).unwrap();
}

#[test]
fn a_disk_stuck_busy_gets_a_link_reset() {
    let sim = machine(Some(0));
    {
        let mut s = sim.0.borrow_mut();
        s.ports[0][reg(port::TFD)] = 0x80;
        s.busy_until_reset = true;
    }
    let disk = start(&sim, true).unwrap();
    assert_eq!(disk.identity.sectors, DISK_SECTORS);
    assert_eq!(sim.0.borrow().link_resets, 1);
}

#[test]
fn no_disk_no_driver() {
    // Port 1's CD drive is all there is.
    assert_eq!(start(&machine(None), true).err(), Some("no disk is attached"));
    // A disk on a port the controller does not implement is not looked at.
    let sim = machine(Some(7));
    assert_eq!(start(&sim, true).err(), Some("no disk is attached"));
    // A disk whose link is asleep neither.
    let sim = machine(Some(0));
    sim.0.borrow_mut().ports[0][reg(port::SSTS)] = 0x613;
    assert_eq!(start(&sim, true).err(), Some("no disk is attached"));
}

#[test]
fn memory_above_4_gib_needs_64_bit_dma() {
    let sim = machine(Some(0));
    {
        let mut s = sim.0.borrow_mut();
        s.global[reg(hba::CAP)] &= !hba::CAP_S64A;
    }
    let memory = Memory::new(layout::needed(SECTOR));
    // SAFETY: as in `start`; never handed to the HBA (refused first).
    let dma = unsafe { Dma::new(memory.base(), 0xFFFF_F000, memory.len()) };
    assert_eq!(
        Disk::start(sim.clone(), dma, true).err(),
        Some("the DMA memory is above 4 GiB, which this controller cannot reach")
    );
}

#[test]
#[cfg_attr(miri, ignore = "sectors past the smaller disk Miri gets (K187)")]
fn a_28_bit_disk_gets_the_old_commands() {
    let sim = machine(Some(0));
    sim.0.borrow_mut().identify = identify_data(DISK_SECTORS, false, "OLD DISK");
    let mut disk = start(&sim, false).unwrap();
    assert!(!disk.identity.lba48);
    disk.transfer(false, 0x0ABC, 3).unwrap();
    disk.transfer(true, 5, 1).unwrap();
    disk.flush().unwrap();
    let done = sim.0.borrow().done.clone();
    assert_eq!(&done[1..], &[(ata::READ_DMA, 0x0ABC, 3), (ata::WRITE_DMA, 5, 1), (ata::FLUSH_CACHE, 0, 0)]);
}

#[test]
fn a_flush_is_a_command_with_no_data() {
    let sim = machine(Some(0));
    let mut disk = start(&sim, true).unwrap();
    disk.transfer(true, 7, 1).unwrap();
    disk.flush().unwrap();
    assert_eq!(sim.0.borrow().flushes, 1);
    assert_eq!(sim.0.borrow().done.last(), Some(&(ata::FLUSH_CACHE_EXT, 0, 0)));
    // A disk that fails it: the error comes back, and the port recovers.
    sim.0.borrow_mut().fail_next = Some(0x04);
    assert!(matches!(disk.flush(), Err(Failure::Disk { .. })));
    disk.flush().unwrap();
    assert_eq!(sim.0.borrow().flushes, 2);
}

#[test]
fn the_bios_is_asked_to_let_go_first() {
    let sim = machine(Some(0));
    {
        let mut s = sim.0.borrow_mut();
        s.global[reg(hba::CAP2)] = hba::CAP2_BOH;
        s.global[reg(hba::BOHC)] = hba::BOHC_BOS | hba::BOHC_BB;
        s.bios_lets_go_after = Some(30);
    }
    start(&sim, true).unwrap();
    let s = sim.0.borrow();
    assert_eq!(s.bios_asked_at, Some(0), "asked before anything else");
    assert!(s.time >= 30, "and waited for the BIOS");

    let sim = machine(Some(0));
    {
        let mut s = sim.0.borrow_mut();
        s.global[reg(hba::CAP2)] = hba::CAP2_BOH;
        s.global[reg(hba::BOHC)] = hba::BOHC_BOS;
    }
    assert_eq!(start(&sim, true).err(), Some("the BIOS would not let go of the controller"));
}

#[test]
fn a_restarted_driver_takes_over_from_the_old_one() {
    // init restarts a driver that died: the new one finds the port
    // running, with the old one's lists and a command still issued.
    let sim = machine(Some(0));
    let first = start(&sim, true).unwrap();
    sim.0.borrow_mut().ports[0][reg(port::CI)] |= 1; // in flight when it died
    drop(first);
    let mut second = start(&sim, true).unwrap();
    assert_eq!(sim.0.borrow().ports[0][reg(port::CI)], 0, "stopping the port forgot it");
    second.transfer(false, 3, 1).unwrap();
}

#[test]
fn commands_are_laid_out_as_the_standard_says() {
    // READ DMA EXT of 0x101 sectors from 0x0000_1234_5678_9ABC.
    let fis = command_fis(ata::READ_DMA_EXT, 0x1234_5678_9ABC, 0x101, true);
    assert_eq!(fis[..4], [0x27, 0x80, 0x25, 0x00]);
    assert_eq!(fis[4..8], [0xBC, 0x9A, 0x78, 0x40], "LBA 0-23, then the device register");
    assert_eq!(fis[8..12], [0x56, 0x34, 0x12, 0x00], "LBA 24-47");
    assert_eq!(fis[12..14], [0x01, 0x01], "the count, low then high");
    // 28-bit: LBA bits 24-27 in the device register's low nibble.
    let fis = command_fis(ata::READ_DMA, 0x0ABC_DEF0, 256, false);
    assert_eq!(fis[4..14], [0xF0, 0xDE, 0xBC, 0x4A, 0, 0, 0, 0, 0x00, 0]);
    // A write of one PRD, table at 0x1_0000_0880.
    assert_eq!(command_header(true, 1, 0x1_0000_0880)[..4], [0x0001_0045, 0, 0x0000_0880, 1]);
    assert_eq!(command_header(false, 1, 0x800)[0], 0x0001_0005);
    assert_eq!(prd(0x2_0000_1000, 0x2_0000), [0x1000, 2, 0, 0x1_FFFF]);
}

#[test]
fn what_is_on_a_port() {
    assert_eq!(attached(0x113, port::SIG_ATA), Attached::Disk);
    assert_eq!(attached(0x133, port::SIG_ATA), Attached::Disk, "any speed");
    assert_eq!(attached(0x113, port::SIG_ATAPI), Attached::Optical);
    assert_eq!(attached(0x113, port::SIG_PM), Attached::Other(port::SIG_PM));
    assert_eq!(attached(0x000, port::SIG_ATA), Attached::Nothing);
    assert_eq!(attached(0x001, port::SIG_ATA), Attached::Nothing, "present, no link");
    assert_eq!(attached(0x613, port::SIG_ATA), Attached::Asleep);
    assert_eq!(hba::version(0x0001_0301), (1, 3, 1));
    assert_eq!((hba::ports(0xC734_FF05), hba::slots(0xC734_FF05)), (6, 32));
}

#[test]
fn identify_answers_are_read_and_checked() {
    let answer = |data: Vec<u8>| identify(&data.try_into().unwrap());
    let id = answer(identify_data(1 << 33, true, "Samsung SSD 870 EVO 1TB")).unwrap();
    assert_eq!((id.sectors, id.lba48, id.model()), (1 << 33, true, "Samsung SSD 870 EVO 1TB"));
    assert_eq!(id.commands(), (ata::READ_DMA_EXT, ata::WRITE_DMA_EXT, 65536));
    let id = answer(identify_data(1000, false, "X")).unwrap();
    assert_eq!((id.sectors, id.lba48, id.model()), (1000, false, "X"));
    assert_eq!(id.commands(), (ata::READ_DMA, ata::WRITE_DMA, 256));
    // Word 83 counts only when it says it is filled in.
    let mut data = identify_data(1000, true, "X");
    data[167] = 0x00; // word 83's high byte: not "01" in its top bits
    assert!(!answer(data).unwrap().lba48);

    let mut data = identify_data(1000, true, "X");
    data[1] = 0x80; // word 0 bit 15: an ATAPI device
    assert_eq!(answer(data), Err("not an ATA disk"));
    let mut data = identify_data(1000, true, "X");
    data[99] = 0; // word 49: no LBA
    assert_eq!(answer(data), Err("the disk does not number its sectors (no LBA)"));
    assert_eq!(answer(identify_data(0, true, "X")), Err("the disk says it has no sectors"));
    // A 4Kn disk: logical sectors of 2048 words.
    let mut data = identify_data(1000, true, "X");
    data[212..214].copy_from_slice(&(0x4000u16 | 1 << 12).to_le_bytes());
    data[234..236].copy_from_slice(&2048u16.to_le_bytes());
    assert_eq!(answer(data), Err("its sectors are not 512 bytes"));
    // 512-byte ones, said explicitly, are fine.
    let mut data = identify_data(1000, true, "X");
    data[212..214].copy_from_slice(&(0x4000u16 | 1 << 12).to_le_bytes());
    data[234..236].copy_from_slice(&256u16.to_le_bytes());
    assert!(answer(data).is_ok());
    // A name that is not text.
    let mut data = identify_data(1000, true, "X");
    data[54] = 0x01;
    assert_eq!(answer(data), Err("the disk's name is not text"));
}
