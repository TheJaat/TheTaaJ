//! ahci - the SATA disk driver: a disk on an AHCI controller, from ring 3.
//!
//! The controller is any vendor's - the kernel hands on whatever PCI
//! function says it is one (class 01.06.01) - and so is the disk. What
//! the driver does is in the `ahci` crate, tested there against a
//! simulated controller; this program gives it the controller's real
//! registers, its real DMA memory and a way to wait, and serves what it
//! drives with the same protocol as `blk` (`taaj::block_service`). fs
//! cannot tell the two apart.
//!
//! Startup handles, from init - the same as blk's:
//!   1  the controller's registers: the memory of its BAR 5, uncached
//!   2  its DMA memory: physically contiguous, its address known
//!   3  the device channel: the PciDeviceInfo comes first
//!   4  the request channel (abi::block): reads and writes - for fs
//!   5  a second request channel: reads only
//!   6  the controller's interrupt (MSI or MSI-X), if it has one -
//!      else the driver polls
//!
//! On a restart init gives the new driver the same memory and
//! registers. The crate's `Disk::start` stops the port before giving it
//! new lists, which makes it forget whatever the old driver had issued.

#![no_std]
#![no_main]

use abi::{block, PciDeviceInfo};
use ahci::{layout, Dma, Failure, Hba};
use taaj::block_service::{self, Device};
use taaj::{println, Handle};

taaj::program!(main);

/// The DMA memory must hold the lists, then the most one request of the
/// protocol moves.
const DATA_MAX: usize = block::SECTORS_MAX as usize * block::SECTOR;

/// The controller's registers, mapped here, and its interrupt.
struct Registers {
    base: *mut u8,
    size: usize,
    interrupt: Option<Handle>,
}

impl Hba for Registers {
    fn read(&mut self, register: usize) -> u32 {
        assert!(register + 4 <= self.size, "register {register:#x} is past the controller's");
        // SAFETY: inside the mapping (checked above), a 32-bit register
        // at a multiple of four (every offset in the crate is). Volatile:
        // each read is a question to the controller.
        unsafe { self.base.add(register).cast::<u32>().read_volatile() }
    }

    fn write(&mut self, register: usize, value: u32) {
        assert!(register + 4 <= self.size, "register {register:#x} is past the controller's");
        // SAFETY: as for `read`.
        unsafe { self.base.add(register).cast::<u32>().write_volatile(value) }
    }

    /// With an interrupt: until it fires, or the next tick. The count of
    /// interrupts is the kernel's, so one that fired before the wait is
    /// not lost - the wait returns at once. Polled: give the CPU away.
    fn pause(&mut self) {
        match self.interrupt {
            Some(line) => {
                if let Ok(Some(_)) = taaj::object_wait_until(&[line], taaj::ticks() + 1) {
                    let _ = taaj::interrupt_wait(line); // pending: returns at once
                }
            }
            None => taaj::yield_now(),
        }
    }

    fn ticks(&mut self) -> u64 {
        taaj::ticks()
    }
}

struct Sata {
    disk: ahci::Disk<Registers>,
}

impl Device for Sata {
    fn sectors(&self) -> u64 {
        self.disk.identity.sectors
    }

    fn read_only(&self) -> bool {
        false // ATA has no such thing; a disk that refuses says so per write
    }

    fn transfer(&mut self, write: bool, sector: u64, count: u32) -> u8 {
        match self.disk.transfer(write, sector, count) {
            Ok(()) => block::OK,
            Err(Failure::Disk { status, error, interrupt_status }) => {
                println!(
                    "[ahci] {} of sector {sector} (+{count}) failed: status {status:#04x}, error {error:#04x}, port status {interrupt_status:#010x}",
                    if write { "write" } else { "read" }
                );
                block::IO_ERROR
            }
            Err(Failure::TimedOut) => {
                println!("[ahci] {} of sector {sector} (+{count}): no answer", if write { "write" } else { "read" });
                block::IO_ERROR
            }
        }
    }

    fn flush(&mut self) -> u8 {
        match self.disk.flush() {
            Ok(()) => block::OK,
            Err(Failure::Disk { status, error, interrupt_status }) => {
                println!("[ahci] flush failed: status {status:#04x}, error {error:#04x}, port status {interrupt_status:#010x}");
                block::IO_ERROR
            }
            Err(Failure::TimedOut) => {
                println!("[ahci] flush: no answer");
                block::IO_ERROR
            }
        }
    }

    fn data(&mut self, length: usize) -> &mut [u8] {
        let (base, size) = self.disk.data();
        // SAFETY: the data buffer, inside the DMA memory; the controller
        // uses it only during `transfer`, and this borrow ends before
        // the next one.
        unsafe { core::slice::from_raw_parts_mut(base, length.min(size)) }
    }
}

fn map(memory: Handle, what: &str) -> Result<*mut u8, i64> {
    taaj::memory_map(memory, true).map_err(|error| {
        println!("[ahci] cannot map {what}: {error:?}");
        4
    })
}

fn start(info: &PciDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Sata, i64> {
    let is_ahci = (info.class, info.subclass, info.interface) == (ahci::CLASS, ahci::SUBCLASS, ahci::INTERFACE);
    // Where the registers begin in the mapping, and whether they all
    // fit in it: asked of the kernel, not taken from the description.
    let offset = (info.registers % 4096) as usize;
    let size = info.registers_size as usize;
    let fits = taaj::memory_size(registers).is_ok_and(|mapped| (offset + size) as u64 <= mapped);
    if !is_ahci || !info.registers_in_memory() || !fits || (info.dma_size as usize) < layout::needed(DATA_MAX) {
        println!("[ahci] no usable device description");
        return Err(2);
    }
    let page = map(registers, "the controller's registers")?;
    let memory = map(dma, "the DMA memory")?;
    // SAFETY: inside the mapping (checked above).
    let base = unsafe { page.add(offset) };
    // SAFETY: the DMA memory, all of it, mapped here and used for
    // nothing else; `dma_physical` is where the kernel says it is.
    let dma = unsafe { Dma::new(memory, info.dma_physical, layout::needed(DATA_MAX)) };
    let hba = Registers { base, size, interrupt };
    let disk = ahci::Disk::start(hba, dma, interrupt.is_some()).map_err(|why| {
        println!("[ahci] the controller would not start: {why}");
        5
    })?;
    if disk.sectors_max() < block::SECTORS_MAX {
        println!("[ahci] the disk cannot move {} sectors at once", block::SECTORS_MAX);
        return Err(5);
    }
    let (major, minor, _) = disk.version;
    let sectors = disk.identity.sectors;
    println!(
        "[ahci] AHCI {major}.{minor} controller at {:02x}:{:02x}.{}, {} ports; disk on port {}: {}, {} sectors ({} MiB), {}",
        info.bus,
        info.device,
        info.function,
        disk.ports,
        disk.port(),
        disk.identity.model(),
        sectors,
        sectors / 2048,
        match (info.msix, info.msi) {
            (true, _) => "MSI-X interrupts",
            (_, true) => "MSI interrupts",
            _ => "polled",
        }
    );
    Ok(Sata { disk })
}

fn main() -> i64 {
    let (Some(registers), Some(dma), Some(device), Some(writers), Some(readers)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
    ) else {
        println!("[ahci] started without its device");
        return 1;
    };
    let interrupt = taaj::startup_handle(5);

    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let described = match taaj::receive(device, &mut message, &mut []) {
        Ok((n, _)) => &message[..n],
        Err(_) => &[],
    };
    let Some(info) = PciDeviceInfo::from_bytes(described) else {
        println!("[ahci] no usable device description");
        return 2;
    };
    let mut sata = match start(&info, registers, dma, interrupt) {
        Ok(sata) => sata,
        Err(code) => return code,
    };
    block_service::serve(&mut sata, "ahci", writers, readers)
}
