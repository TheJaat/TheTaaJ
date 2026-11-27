//! AHCI: the standard way a PC talks to SATA disks.
//!
//! A SATA disk speaks ATA commands - "read 8 sectors from sector 4096" -
//! as FISes (frame information structures): small packets on the cable.
//! Between the CPU and the cable sits the host bus adapter, the HBA. The
//! Advanced Host Controller Interface is Intel's standard for what that
//! HBA looks like to software, and every SATA controller since about
//! 2005 speaks it: the chipset's, VirtualBox's, VMware's, and QEMU's
//! (`-device ahci`, the ICH9's). One driver for all of them.
//!
//! What it looks like:
//!
//!   - Registers in memory (PCI BAR 5, "ABAR"): a few for the whole
//!     controller, then 0x80 bytes for each of up to 32 ports. A port is
//!     one cable: one disk.
//!   - Each port reads its commands from memory the driver gives it, by
//!     DMA - the same idea as a virtqueue, with different shapes:
//!
//! ```text
//!   command list    32 headers of 32 bytes, one per "slot": each says
//!                   where a command table is, and how much of it to read
//!   command table   the command itself (a FIS the HBA sends to the disk
//!                   as it is), then a list of where the data goes:
//!                   physical address and length, each a "PRD"
//!                   (physical region descriptor)
//!   received FIS    256 bytes where the HBA puts what the disk
//!                   answered: its status, its error
//! ```
//!
//!   - The driver fills slot 0's header and table, then sets bit 0 of the
//!     port's command-issue register (PxCI). The HBA sends the FIS, moves
//!     the data, and clears the bit when the disk says it is done - and
//!     raises an interrupt, if asked to. An error stops the port, sets
//!     the task-file error bit (TFES) in the port's interrupt status,
//!     and leaves the bit in PxCI set.
//!
//! This crate is everything that is not a system call: the register map,
//! the layouts, the commands, reading the disk's IDENTIFY answer, and the
//! sequence that drives one disk (`Disk`), written against `Hba` - "read
//! a register, write one, wait a little" - so that the whole sequence is
//! tested on the host against a simulated controller (`tests.rs`). The
//! program that drives a real one (`user/src/bin/ahci.rs`) supplies the
//! mapping of the registers and the waiting.
//!
//! One command at a time, in slot 0. Native command queueing - up to 32
//! commands in flight, the disk choosing the order - is how a real disk
//! gets faster; later.

#![cfg_attr(not(test), no_std)]

use core::sync::atomic::{fence, Ordering};

#[cfg(test)]
mod tests;

/// The PCI class of an AHCI controller: mass storage (01), SATA (06),
/// speaking AHCI 1.0 (programming interface 01). Any vendor's.
pub const CLASS: u8 = 0x01;
pub const SUBCLASS: u8 = 0x06;
pub const INTERFACE: u8 = 0x01;
/// The BAR its registers are in.
pub const BAR: u8 = 5;

/// The registers of the whole controller: offsets into ABAR.
pub mod hba {
    /// Capabilities: how many ports and slots, 64-bit DMA, ...
    pub const CAP: usize = 0x00;
    /// Global control: reset, interrupts on, AHCI mode on.
    pub const GHC: usize = 0x04;
    /// Which ports want attention: bit n for port n; write 1 to clear.
    pub const IS: usize = 0x08;
    /// Which ports exist on this controller: bit n for port n.
    pub const PI: usize = 0x0C;
    /// AHCI version: major in the high half, minor in the low (BCD).
    pub const VS: usize = 0x10;
    /// More capabilities; bit 0 says the BIOS may own the controller.
    pub const CAP2: usize = 0x24;
    /// BIOS/OS handoff control.
    pub const BOHC: usize = 0x28;

    /// CAP: can address memory above 4 GiB.
    pub const CAP_S64A: u32 = 1 << 31;
    /// CAP: staggered spin-up - each disk is spun up by the driver.
    pub const CAP_SSS: u32 = 1 << 27;
    pub const GHC_HR: u32 = 1 << 0;
    pub const GHC_IE: u32 = 1 << 1;
    pub const GHC_AE: u32 = 1 << 31;
    pub const CAP2_BOH: u32 = 1 << 0;
    /// BOHC: the BIOS owns it; the OS asks for it; the BIOS is busy.
    pub const BOHC_BOS: u32 = 1 << 0;
    pub const BOHC_OOS: u32 = 1 << 1;
    pub const BOHC_BB: u32 = 1 << 4;

    /// How many ports and command slots the controller has.
    pub const fn ports(cap: u32) -> u32 {
        (cap & 0x1F) + 1
    }
    pub const fn slots(cap: u32) -> u32 {
        ((cap >> 8) & 0x1F) + 1
    }

    /// The AHCI version: 1.3.1 is 0x0001_0301 - major in the high half,
    /// then minor, then the third number.
    pub const fn version(vs: u32) -> (u32, u32, u32) {
        (vs >> 16, (vs >> 8) & 0xFF, vs & 0xFF)
    }
}

/// One port's registers: offsets from the port's own start.
pub mod port {
    /// Where port `n`'s register `offset` is, in ABAR.
    pub const fn register(n: usize, offset: usize) -> usize {
        0x100 + 0x80 * n + offset
    }

    /// The command list's physical address (1 KiB aligned), low and high.
    pub const CLB: usize = 0x00;
    pub const CLBU: usize = 0x04;
    /// The received-FIS area's (256-byte aligned).
    pub const FB: usize = 0x08;
    pub const FBU: usize = 0x0C;
    /// Interrupt status: what happened (write 1 to clear), and which of
    /// those raise an interrupt.
    pub const IS: usize = 0x10;
    pub const IE: usize = 0x14;
    pub const CMD: usize = 0x18;
    /// The disk's task file: its status byte, and its error byte above.
    pub const TFD: usize = 0x20;
    /// What is attached, as its first answer said: a disk, a CD drive...
    pub const SIG: usize = 0x24;
    /// The link: is something there, at what speed, awake or asleep.
    pub const SSTS: usize = 0x28;
    pub const SCTL: usize = 0x2C;
    /// Link errors; write 1 to clear.
    pub const SERR: usize = 0x30;
    /// Command issue: bit n set = slot n's command is the HBA's.
    pub const CI: usize = 0x38;

    /// CMD: start (process the command list); spin up; power on; receive
    /// FISes (FRE); and the HBA's answers - FIS receiving running (FR),
    /// command list running (CR).
    pub const CMD_ST: u32 = 1 << 0;
    pub const CMD_SUD: u32 = 1 << 1;
    pub const CMD_POD: u32 = 1 << 2;
    pub const CMD_FRE: u32 = 1 << 4;
    pub const CMD_FR: u32 = 1 << 14;
    pub const CMD_CR: u32 = 1 << 15;

    /// IS: the disk answered (register FIS), a PIO transfer was set up,
    /// a DMA one, "set device bits", a PRD with its interrupt bit done.
    pub const IS_DHRS: u32 = 1 << 0;
    pub const IS_PSS: u32 = 1 << 1;
    pub const IS_DSS: u32 = 1 << 2;
    pub const IS_SDBS: u32 = 1 << 3;
    pub const IS_DPS: u32 = 1 << 5;
    /// IS, the errors: interface (non-fatal and fatal), host bus data,
    /// host bus fatal, and the disk's own (task file error).
    pub const IS_INFS: u32 = 1 << 26;
    pub const IS_IFS: u32 = 1 << 27;
    pub const IS_HBDS: u32 = 1 << 28;
    pub const IS_HBFS: u32 = 1 << 29;
    pub const IS_TFES: u32 = 1 << 30;
    /// The errors that stop the port: a command did not complete.
    pub const IS_ERRORS: u32 = IS_TFES | IS_HBFS | IS_HBDS | IS_IFS;
    /// What this driver is interrupted for: every way a command ends.
    pub const IE_WANTED: u32 = IS_DHRS | IS_PSS | IS_DSS | IS_SDBS | IS_DPS | IS_ERRORS;

    /// TFD's status byte: error, data request, busy.
    pub const TFD_ERR: u32 = 1 << 0;
    pub const TFD_DRQ: u32 = 1 << 3;
    pub const TFD_BSY: u32 = 1 << 7;

    /// SIG: what kind of device answered.
    pub const SIG_ATA: u32 = 0x0000_0101;
    pub const SIG_ATAPI: u32 = 0xEB14_0101;
    pub const SIG_SEMB: u32 = 0xC33C_0101;
    pub const SIG_PM: u32 = 0x9669_0101;

    /// SSTS: device detection (bits 3:0) - 3 is "present, link up" - and
    /// interface power state (bits 11:8) - 1 is "active".
    pub const fn detection(ssts: u32) -> u32 {
        ssts & 0xF
    }
    pub const fn power(ssts: u32) -> u32 {
        (ssts >> 8) & 0xF
    }
    pub const DET_PRESENT: u32 = 3;
    pub const IPM_ACTIVE: u32 = 1;
    /// SCTL's DET field: 1 = hold the link in reset (COMRESET), 0 = let go.
    pub const SCTL_DET_RESET: u32 = 1;
}

/// The ATA commands used, and the shape of the FIS that carries them.
pub mod ata {
    /// "Tell me about yourself": 512 bytes back, by PIO.
    pub const IDENTIFY: u8 = 0xEC;
    /// Read and write by DMA, 48-bit sector numbers (the "EXT" commands)...
    pub const READ_DMA_EXT: u8 = 0x25;
    pub const WRITE_DMA_EXT: u8 = 0x35;
    /// ...and the older 28-bit ones, for a disk without them (at most
    /// 128 GiB, 256 sectors a command).
    pub const READ_DMA: u8 = 0xC8;
    pub const WRITE_DMA: u8 = 0xCA;
    /// "Put what is in your cache on the medium", no data either way -
    /// in its 48-bit form and the older one.
    pub const FLUSH_CACHE_EXT: u8 = 0xEA;
    pub const FLUSH_CACHE: u8 = 0xE7;

    /// FIS type: register, host to device - a command.
    pub const FIS_H2D: u8 = 0x27;
    /// Its length, in the 32-bit words the command header counts.
    pub const FIS_H2D_WORDS: u32 = 5;
    /// The FIS carries a command (not a control-register write).
    pub const FIS_COMMAND: u8 = 0x80;
    /// The device register: sector numbers are LBAs, not cylinders.
    pub const DEVICE_LBA: u8 = 0x40;
}

/// Where everything lives in the DMA memory the driver is given.
/// Each start is aligned as the HBA requires.
pub mod layout {
    /// 32 command headers of 32 bytes (1 KiB aligned) - only slot 0's
    /// is used, but the HBA owns the whole kilobyte.
    pub const COMMAND_LIST: usize = 0x000;
    /// What the disk sent back (256-byte aligned).
    pub const RECEIVED_FIS: usize = 0x400;
    pub const RECEIVED_FIS_SIZE: usize = 0x100;
    /// Slot 0's command table (128-byte aligned): the FIS at 0, the PRD
    /// list at 0x80, one entry.
    pub const COMMAND_TABLE: usize = 0x800;
    pub const PRDT: usize = COMMAND_TABLE + 0x80;
    /// The data: what a read brings, what a write sends.
    pub const DATA: usize = 0x1000;

    /// How much DMA memory moving `data_max` bytes at once needs.
    pub const fn needed(data_max: usize) -> usize {
        DATA + data_max
    }
}

pub const SECTOR: usize = 512;

/// The command FIS for `command`, on `count` sectors (1 to 65536; or 256
/// for 28-bit) from `lba` on. Zero sectors means "none" for IDENTIFY;
/// for the ATA protocol itself, 0 in the count register means the most.
pub fn command_fis(command: u8, lba: u64, count: u32, lba48: bool) -> [u8; 20] {
    let mut fis = [0u8; 20];
    fis[0] = ata::FIS_H2D;
    fis[1] = ata::FIS_COMMAND; // port multiplier 0
    fis[2] = command;
    let l = lba.to_le_bytes();
    fis[4] = l[0];
    fis[5] = l[1];
    fis[6] = l[2];
    if lba48 {
        fis[7] = ata::DEVICE_LBA;
        fis[8] = l[3];
        fis[9] = l[4];
        fis[10] = l[5];
        fis[12] = count as u8;
        fis[13] = (count >> 8) as u8;
    } else {
        // 28 bits: the top four live in the device register.
        fis[7] = ata::DEVICE_LBA | (l[3] & 0x0F);
        fis[12] = count as u8;
    }
    fis
}

/// A command header, as eight little-endian words: the FIS length, the
/// direction ("write" = the HBA reads memory and sends it), how many PRD
/// entries the table has, and where the table is.
pub fn command_header(write: bool, prds: u16, table: u64) -> [u32; 8] {
    let mut header = [0u32; 8];
    header[0] = ata::FIS_H2D_WORDS | if write { 1 << 6 } else { 0 } | u32::from(prds) << 16;
    header[1] = 0; // bytes moved: the HBA counts them here
    header[2] = table as u32;
    header[3] = (table >> 32) as u32;
    header
}

/// A PRD entry: `bytes` (even, at most 4 MiB) at `physical`. Its own
/// interrupt bit stays off: the command's end interrupts anyway.
pub fn prd(physical: u64, bytes: u32) -> [u32; 4] {
    [physical as u32, (physical >> 32) as u32, 0, (bytes - 1) & 0x3F_FFFF]
}

/// What is on a port, from its link status and signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attached {
    Nothing,
    /// A disk: what this driver drives.
    Disk,
    /// A CD or DVD drive (ATAPI): SCSI commands inside ATA ones.
    Optical,
    /// Something else: a port multiplier, an enclosure.
    Other(u32),
    /// A device whose link is asleep (a power-saving state).
    Asleep,
}

pub fn attached(ssts: u32, sig: u32) -> Attached {
    if port::detection(ssts) != port::DET_PRESENT {
        return Attached::Nothing;
    }
    if port::power(ssts) != port::IPM_ACTIVE {
        return Attached::Asleep;
    }
    match sig {
        port::SIG_ATA => Attached::Disk,
        port::SIG_ATAPI => Attached::Optical,
        other => Attached::Other(other),
    }
}

/// What a disk said about itself, in answer to IDENTIFY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub sectors: u64,
    /// It knows the 48-bit commands (READ DMA EXT and friends).
    pub lba48: bool,
    model: [u8; 40],
    model_length: usize,
}

impl Identity {
    /// The model name, as the disk spells it: "QEMU HARDDISK".
    pub fn model(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_length]).unwrap_or("?")
    }

    /// The commands to read and write with, and the most sectors one
    /// of them may move.
    pub fn commands(&self) -> (u8, u8, u32) {
        if self.lba48 {
            (ata::READ_DMA_EXT, ata::WRITE_DMA_EXT, 65536)
        } else {
            (ata::READ_DMA, ata::WRITE_DMA, 256)
        }
    }
}

/// Reads the 512 bytes IDENTIFY returns: 256 little-endian words, each
/// with a meaning fixed by the ATA standard.
pub fn identify(data: &[u8; SECTOR]) -> Result<Identity, &'static str> {
    let word = |n: usize| u16::from_le_bytes([data[2 * n], data[2 * n + 1]]);
    if word(0) & 0x8000 != 0 {
        return Err("not an ATA disk");
    }
    if word(49) & (1 << 9) == 0 {
        return Err("the disk does not number its sectors (no LBA)");
    }
    // Words 83 and 106 mean something only when their top two bits are
    // 01 - the standard's way of saying "this word is filled in".
    let valid = |w: u16| w & 0xC000 == 0x4000;
    let lba48 = valid(word(83)) && word(83) & (1 << 10) != 0;
    // Bigger logical sectors (4Kn disks): the protocol above is built on
    // 512-byte ones.
    if valid(word(106)) && word(106) & (1 << 12) != 0 {
        let words = u32::from(word(117)) | u32::from(word(118)) << 16;
        if words as usize * 2 != SECTOR {
            return Err("its sectors are not 512 bytes");
        }
    }
    let sectors = if lba48 {
        (0..4).fold(0u64, |n, i| n | u64::from(word(100 + i)) << (16 * i))
    } else {
        u64::from(word(60)) | u64::from(word(61)) << 16
    };
    if sectors == 0 {
        return Err("the disk says it has no sectors");
    }
    // Text is two characters a word, the first in the high byte, padded
    // with spaces.
    let mut model = [0u8; 40];
    for n in 0..20 {
        let [high, low] = word(27 + n).to_be_bytes();
        model[2 * n] = high;
        model[2 * n + 1] = low;
    }
    let printable = |b: &u8| (0x20..0x7F).contains(b);
    // Padding is spaces (some disks use NULs); anything else is name.
    let model_length = model.iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |n| n + 1);
    if !model[..model_length].iter().all(printable) {
        return Err("the disk's name is not text");
    }
    Ok(Identity { sectors, lba48, model, model_length })
}

/// The controller as the driver reaches it: its registers (offsets into
/// ABAR), and a way to wait.
pub trait Hba {
    fn read(&mut self, register: usize) -> u32;
    fn write(&mut self, register: usize, value: u32);
    /// Waits a little: until the controller interrupts, or a tick
    /// passes - whichever is first. Polled, it may return at once.
    fn pause(&mut self);
    /// The time, in ticks (100 a second), for the waits' limits.
    fn ticks(&mut self) -> u64;
}

/// The limits on waiting, in ticks. The standard's own numbers where it
/// has them (500 ms for a port to stop); generous elsewhere.
pub const STOP_WAIT: u64 = 50;
pub const READY_WAIT: u64 = 100;
pub const LINK_WAIT: u64 = 100;
pub const HANDOFF_WAIT: u64 = 200;
pub const COMMAND_WAIT: u64 = 500;

/// Memory both the driver and the HBA use: where it is mapped here,
/// and where it is for the HBA.
pub struct Dma {
    base: *mut u8,
    physical: u64,
    size: usize,
}

impl Dma {
    /// # Safety
    /// `base` must be a mapping of `size` bytes, writable, that nothing
    /// else uses for as long as the `Dma` lives, aligned to 8 bytes (it is
    /// read and written in words: a page, as DMA memory always is); and
    /// `physical` where
    /// those bytes are for the device (one unbroken range).
    pub unsafe fn new(base: *mut u8, physical: u64, size: usize) -> Dma {
        assert!((base as usize).is_multiple_of(8), "DMA memory must be aligned to 8 bytes");
        Dma { base, physical, size }
    }

    pub fn physical(&self, offset: usize) -> u64 {
        self.physical + offset as u64
    }

    /// Volatile, every access: the HBA reads and writes this memory
    /// behind the compiler's back, so no access may be merged, moved
    /// past another or left out.
    fn write_bytes(&mut self, offset: usize, bytes: &[u8]) {
        assert!(offset + bytes.len() <= self.size);
        for (n, &byte) in bytes.iter().enumerate() {
            // SAFETY: inside the mapping (checked above), which is ours.
            unsafe { self.base.add(offset + n).write_volatile(byte) };
        }
    }

    fn write_words(&mut self, offset: usize, words: &[u32]) {
        for (n, word) in words.iter().enumerate() {
            self.write_bytes(offset + 4 * n, &word.to_le_bytes());
        }
    }

    fn zero(&mut self, offset: usize, length: usize) {
        assert!(offset + length <= self.size);
        for n in 0..length {
            // SAFETY: as in `write_bytes`.
            unsafe { self.base.add(offset + n).write_volatile(0) };
        }
    }

    fn read_bytes(&self, offset: usize, out: &mut [u8]) {
        assert!(offset + out.len() <= self.size);
        for (n, byte) in out.iter_mut().enumerate() {
            // SAFETY: as in `write_bytes`.
            *byte = unsafe { self.base.add(offset + n).read_volatile() };
        }
    }
}

/// Why a command failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The disk refused it: its status and error bytes, and the port's
    /// interrupt status (which error bits).
    Disk { status: u8, error: u8, interrupt_status: u32 },
    /// No answer within COMMAND_WAIT.
    TimedOut,
}

/// One disk on one port of an AHCI controller, set up and ready.
pub struct Disk<H: Hba> {
    hba: H,
    dma: Dma,
    port: usize,
    pub identity: Identity,
    /// The most bytes one `transfer` moves: what fits in the DMA memory.
    data_max: usize,
    /// What `start` found, for a log line.
    pub version: (u32, u32, u32),
    pub ports: u32,
}

impl<H: Hba> Disk<H> {
    /// Takes the controller over and sets up the first disk on it:
    ///
    ///   1. ask the BIOS to let go, if it may own the controller;
    ///   2. switch AHCI mode on (it may be in its old IDE-like mode);
    ///   3. find the first port with a disk on it;
    ///   4. stop that port - the firmware may have left it running, with
    ///      a command list of its own - and give it ours;
    ///   5. let it receive FISes, wait for the disk to be ready, start
    ///      it, and switch on its interrupts if the driver has one;
    ///   6. ask the disk what it is (IDENTIFY).
    ///
    /// No controller reset: it would reset every port's link, and a real
    /// disk can take seconds to come back. Stopping the one port is
    /// enough to make it forget whatever it was doing - which is also
    /// what a restarted driver needs.
    pub fn start(mut hba: H, mut dma: Dma, interrupts: bool) -> Result<Disk<H>, &'static str> {
        if dma.size < layout::needed(SECTOR) {
            return Err("too little DMA memory");
        }
        let cap = hba.read(hba::CAP);
        if hba.read(hba::CAP2) & hba::CAP2_BOH != 0 {
            take_from_bios(&mut hba)?;
        }
        let ghc = hba.read(hba::GHC);
        hba.write(hba::GHC, ghc | hba::GHC_AE);

        let implemented = hba.read(hba::PI);
        let port = (0..32)
            .filter(|&n| implemented & (1 << n) != 0)
            .find(|&n| {
                let ssts = hba.read(port::register(n, port::SSTS));
                let sig = hba.read(port::register(n, port::SIG));
                attached(ssts, sig) == Attached::Disk
            })
            .ok_or("no disk is attached")?;

        if dma.physical(dma.size) > 1 << 32 && cap & hba::CAP_S64A == 0 {
            return Err("the DMA memory is above 4 GiB, which this controller cannot reach");
        }
        let at = |offset| port::register(port, offset);
        stop(&mut hba, port)?;
        dma.zero(layout::COMMAND_LIST, layout::DATA);
        let list = dma.physical(layout::COMMAND_LIST);
        let received = dma.physical(layout::RECEIVED_FIS);
        hba.write(at(port::CLB), list as u32);
        hba.write(at(port::CLBU), (list >> 32) as u32);
        hba.write(at(port::FB), received as u32);
        hba.write(at(port::FBU), (received >> 32) as u32);
        hba.write(at(port::SERR), u32::MAX);
        hba.write(at(port::IS), u32::MAX);
        let mut command = hba.read(at(port::CMD)) | port::CMD_FRE;
        if cap & hba::CAP_SSS != 0 {
            command |= port::CMD_SUD;
        }
        hba.write(at(port::CMD), command);
        if !ready(&mut hba, port) {
            reset_link(&mut hba, port)?;
            if !ready(&mut hba, port) {
                return Err("the disk stays busy");
            }
        }
        let command = hba.read(at(port::CMD));
        hba.write(at(port::CMD), command | port::CMD_ST);
        if interrupts {
            hba.write(at(port::IE), port::IE_WANTED);
            let ghc = hba.read(hba::GHC);
            hba.write(hba::GHC, ghc | hba::GHC_IE);
        } else {
            hba.write(at(port::IE), 0);
        }

        let data_max = dma.size - layout::DATA;
        let unknown = Identity { sectors: 0, lba48: false, model: [0; 40], model_length: 0 };
        let mut disk = Disk {
            hba,
            dma,
            port,
            identity: unknown,
            data_max,
            version: hba::version(0),
            ports: hba::ports(cap),
        };
        disk.version = hba::version(disk.hba.read(hba::VS));
        disk.issue(ata::IDENTIFY, 0, 0, SECTOR as u32, false).map_err(|_| "the disk did not answer IDENTIFY")?;
        let mut answer = [0u8; SECTOR];
        disk.dma.read_bytes(layout::DATA, &mut answer);
        disk.identity = identify(&answer)?;
        Ok(disk)
    }

    pub fn port(&self) -> usize {
        self.port
    }

    /// The most sectors one `transfer` moves.
    pub fn sectors_max(&self) -> u32 {
        let (_, _, most) = self.identity.commands();
        ((self.data_max / SECTOR) as u32).min(most)
    }

    /// The data buffer, and how long it is: what a read left there, or
    /// what a write is to send. It is the HBA's only during `transfer`.
    pub fn data(&mut self) -> (*mut u8, usize) {
        // SAFETY: DATA is inside the memory, `new`'s promise.
        (unsafe { self.dma.base.add(layout::DATA) }, self.data_max)
    }

    /// `count` sectors from `sector` on, between the disk and the data
    /// buffer. The caller checks the range against `identity.sectors`.
    pub fn transfer(&mut self, write: bool, sector: u64, count: u32) -> Result<(), Failure> {
        assert!(count >= 1 && count <= self.sectors_max(), "a transfer of {count} sectors");
        let (read, written, _) = self.identity.commands();
        let command = if write { written } else { read };
        self.issue(command, sector, count, count * SECTOR as u32, write)
    }

    /// Everything the disk was given onto the medium: FLUSH CACHE (EXT).
    /// A disk with no write cache has nothing to do, and says so at once.
    pub fn flush(&mut self) -> Result<(), Failure> {
        let command = if self.identity.lba48 { ata::FLUSH_CACHE_EXT } else { ata::FLUSH_CACHE };
        self.issue(command, 0, 0, 0, false)
    }

    /// Puts one command in slot 0, issues it and waits for its end.
    fn issue(&mut self, command: u8, lba: u64, count: u32, bytes: u32, write: bool) -> Result<(), Failure> {
        let at = |offset| port::register(self.port, offset);
        let table = self.dma.physical(layout::COMMAND_TABLE);
        let fis = command_fis(command, lba, count, self.identity.lba48 || command == ata::IDENTIFY);
        self.dma.zero(layout::COMMAND_TABLE, 0x80);
        self.dma.write_bytes(layout::COMMAND_TABLE, &fis);
        // A command that moves no data (FLUSH CACHE) has no PRD entry.
        let prds = u16::from(bytes > 0);
        if bytes > 0 {
            self.dma.write_words(layout::PRDT, &prd(self.dma.physical(layout::DATA), bytes));
        }
        self.dma.write_words(layout::COMMAND_LIST, &command_header(write, prds, table));
        // Everything above is in memory before the HBA is told: on a CPU
        // that may reorder stores to memory and to devices, a barrier.
        fence(Ordering::SeqCst);
        self.hba.write(at(port::IS), u32::MAX); // nothing old is mistaken for this
        self.hba.write(at(port::CI), 1);

        let deadline = self.hba.ticks() + COMMAND_WAIT;
        loop {
            // Clear what is reported before looking at CI: a command that
            // ends after the look raises the status (and the interrupt)
            // again; one that ended before is seen by the look.
            let status = self.hba.read(at(port::IS));
            if status != 0 {
                self.hba.write(at(port::IS), status);
                self.hba.write(hba::IS, 1 << self.port);
            }
            if status & port::IS_ERRORS != 0 {
                let tfd = self.hba.read(at(port::TFD));
                self.recover();
                return Err(Failure::Disk { status: tfd as u8, error: (tfd >> 8) as u8, interrupt_status: status });
            }
            if self.hba.read(at(port::CI)) & 1 == 0 {
                fence(Ordering::SeqCst); // the data is read after this, not before
                return Ok(());
            }
            if self.hba.ticks() >= deadline {
                self.recover();
                return Err(Failure::TimedOut);
            }
            self.hba.pause();
        }
    }

    /// After a failed command: the port stopped itself (or must be
    /// stopped); clear its errors, reset the link if the disk is stuck
    /// busy, and start it again. Best effort - the next command will
    /// tell whether it worked.
    fn recover(&mut self) {
        let port = self.port;
        let at = |offset| port::register(port, offset);
        let command = self.hba.read(at(port::CMD));
        self.hba.write(at(port::CMD), command & !port::CMD_ST);
        let _ = wait_for(&mut self.hba, STOP_WAIT, |hba| hba.read(at(port::CMD)) & port::CMD_CR == 0);
        self.hba.write(at(port::SERR), u32::MAX);
        self.hba.write(at(port::IS), u32::MAX);
        if !ready(&mut self.hba, port) {
            let _ = reset_link(&mut self.hba, port);
            let _ = ready(&mut self.hba, port);
        }
        let command = self.hba.read(at(port::CMD));
        self.hba.write(at(port::CMD), command | port::CMD_ST);
    }
}

/// Polls `done` until it holds or `limit` ticks pass. Whether it held.
fn wait_for<H: Hba>(hba: &mut H, limit: u64, mut done: impl FnMut(&mut H) -> bool) -> bool {
    let deadline = hba.ticks() + limit;
    loop {
        if done(hba) {
            return true;
        }
        if hba.ticks() >= deadline {
            return false;
        }
        hba.pause();
    }
}

/// The BIOS/OS handoff: on a machine whose firmware uses the controller
/// itself (and says so in CAP2), the OS asks for it and waits until the
/// BIOS has finished what it was doing and let go.
fn take_from_bios<H: Hba>(hba: &mut H) -> Result<(), &'static str> {
    let bohc = hba.read(hba::BOHC);
    hba.write(hba::BOHC, bohc | hba::BOHC_OOS);
    let released = wait_for(hba, HANDOFF_WAIT, |hba| {
        let bohc = hba.read(hba::BOHC);
        bohc & hba::BOHC_BOS == 0 && bohc & hba::BOHC_BB == 0
    });
    if released {
        Ok(())
    } else {
        Err("the BIOS would not let go of the controller")
    }
}

/// Stops a port: no command list processing, no FIS receiving - and
/// waits until the HBA confirms both, as it must before the lists move.
fn stop<H: Hba>(hba: &mut H, n: usize) -> Result<(), &'static str> {
    let at = |offset| port::register(n, offset);
    let command = hba.read(at(port::CMD));
    hba.write(at(port::CMD), command & !port::CMD_ST);
    if !wait_for(hba, STOP_WAIT, |hba| hba.read(at(port::CMD)) & port::CMD_CR == 0) {
        return Err("the port would not stop");
    }
    let command = hba.read(at(port::CMD));
    hba.write(at(port::CMD), command & !port::CMD_FRE);
    if !wait_for(hba, STOP_WAIT, |hba| hba.read(at(port::CMD)) & port::CMD_FR == 0) {
        return Err("the port would not stop receiving");
    }
    Ok(())
}

/// Whether the disk becomes ready - neither busy nor waiting for data -
/// within READY_WAIT.
fn ready<H: Hba>(hba: &mut H, n: usize) -> bool {
    let tfd = port::register(n, port::TFD);
    wait_for(hba, READY_WAIT, |hba| hba.read(tfd) & (port::TFD_BSY | port::TFD_DRQ) == 0)
}

/// COMRESET: holds the link in reset for a moment, lets go, and waits
/// for the disk to come back - the last resort for a disk stuck busy.
fn reset_link<H: Hba>(hba: &mut H, n: usize) -> Result<(), &'static str> {
    let sctl = port::register(n, port::SCTL);
    let control = hba.read(sctl) & !0xF;
    hba.write(sctl, control | port::SCTL_DET_RESET);
    // At least 1 ms in reset: one tick is ten.
    let start = hba.ticks();
    let _ = wait_for(hba, 2, |hba| hba.ticks() > start);
    hba.write(sctl, control);
    let ssts = port::register(n, port::SSTS);
    if !wait_for(hba, LINK_WAIT, |hba| port::detection(hba.read(ssts)) == port::DET_PRESENT) {
        return Err("the disk did not come back after a link reset");
    }
    hba.write(port::register(n, port::SERR), u32::MAX);
    Ok(())
}
