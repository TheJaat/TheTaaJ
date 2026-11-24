//! nvme - the NVMe disk driver: a disk on an NVMe controller, from ring 3.
//!
//! The controller is any vendor's - the kernel hands on whatever PCI
//! function says it is one (class 01.08.02). What the driver does is in
//! the `nvme` crate, tested there against a simulated controller; this
//! program gives it the controller's real registers, its real DMA memory
//! and a way to wait, and serves the first namespace with the same
//! protocol as `blk` and `ahci` (`taaj::block_service`). fs cannot tell
//! the three apart.
//!
//! Startup handles, from init - the same as blk's and ahci's:
//!   1  the controller's registers: the memory of its BAR 0, uncached,
//!      cut short of the MSI-X table if that is in BAR 0 too
//!   2  its DMA memory: physically contiguous, its address known
//!   3  the device channel: the PciDeviceInfo comes first
//!   4  the request channel (abi::block): reads and writes - for fs
//!   5  a second request channel: reads only
//!   6  the controller's interrupt (MSI-X or MSI), if it has one -
//!      else the driver polls
//!
//! On a restart init gives the new driver the same memory and
//! registers. The crate's `Disk::start` turns the controller off before
//! giving it new queues, which makes it forget the old driver's.

#![no_std]
#![no_main]

use abi::{block, PciDeviceInfo};
use nvme::{layout, reg, Bus, Dma, Failure};
use taaj::block_service::{self, Device};
use taaj::{println, Handle};

taaj::program!(main);

/// The DMA memory must hold the queues and the PRP list, then the most
/// one request of the protocol moves.
const DATA_MAX: usize = block::SECTORS_MAX as usize * block::SECTOR;

/// The controller's registers, mapped here, and its interrupt.
struct Registers {
    base: *mut u8,
    size: usize,
    interrupt: Option<Handle>,
}

impl Bus for Registers {
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

struct Nvme {
    disk: nvme::Disk<Registers>,
}

impl Device for Nvme {
    fn sectors(&self) -> u64 {
        self.disk.namespace.blocks
    }

    fn read_only(&self) -> bool {
        false // a write-protected namespace refuses each write itself
    }

    fn transfer(&mut self, write: bool, sector: u64, count: u32) -> u8 {
        let what = if write { "write" } else { "read" };
        match self.disk.transfer(write, sector, count) {
            Ok(()) => block::OK,
            Err(Failure::Status(status)) => {
                println!(
                    "[nvme] {what} of sector {sector} (+{count}) failed: status type {}, code {:#04x}",
                    status >> 8,
                    status & 0xFF
                );
                block::IO_ERROR
            }
            Err(Failure::TimedOut) => {
                println!("[nvme] {what} of sector {sector} (+{count}): no answer - the controller was reset");
                block::IO_ERROR
            }
            Err(Failure::Fatal) => {
                println!("[nvme] {what} of sector {sector} (+{count}): the controller failed - it was reset");
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
        println!("[nvme] cannot map {what}: {error:?}");
        4
    })
}

fn start(info: &PciDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Nvme, i64> {
    let is_nvme = (info.class, info.subclass, info.interface) == (nvme::CLASS, nvme::SUBCLASS, nvme::INTERFACE);
    // Where the registers begin in the mapping, and whether they all
    // fit in it: asked of the kernel, not taken from the description.
    let offset = (info.registers % 4096) as usize;
    let size = info.registers_size as usize;
    let fits = taaj::memory_size(registers).is_ok_and(|mapped| (offset + size) as u64 <= mapped);
    if !is_nvme || !info.registers_in_memory() || !fits || (info.dma_size as usize) < layout::needed(DATA_MAX) {
        println!("[nvme] no usable device description");
        return Err(2);
    }
    let page = map(registers, "the controller's registers")?;
    let memory = map(dma, "the DMA memory")?;
    // SAFETY: inside the mapping (checked above).
    let base = unsafe { page.add(offset) };
    let mut bus = Registers { base, size, interrupt };

    // The doorbells of both queue pairs must be in what the kernel gave:
    // it keeps the MSI-X table, which may follow them in the same BAR.
    let stride = reg::doorbell_stride(bus.read64(reg::CAP));
    let last = reg::cq_head(usize::from(nvme::IO_QUEUE), stride);
    if last + 4 > size {
        println!("[nvme] the doorbells ({stride} bytes apart) are past the {size:#x} bytes of registers given");
        return Err(5);
    }

    // SAFETY: the DMA memory, all of it, mapped here and used for
    // nothing else; `dma_physical` is where the kernel says it is.
    let dma = unsafe { Dma::new(memory, info.dma_physical, layout::needed(DATA_MAX)) };
    let disk = nvme::Disk::start(bus, dma, interrupt.is_some()).map_err(|why| {
        println!("[nvme] the controller would not start: {why}");
        5
    })?;
    if disk.sectors_max() < block::SECTORS_MAX {
        println!("[nvme] the disk cannot move {} sectors at once", block::SECTORS_MAX);
        return Err(5);
    }
    let (major, minor) = disk.version;
    let sectors = disk.namespace.blocks;
    println!(
        "[nvme] NVMe {major}.{minor} controller at {:02x}:{:02x}.{}: {}; namespace {}, {} sectors ({} MiB), {} KiB a command, {}",
        info.bus,
        info.device,
        info.function,
        disk.controller.model(),
        disk.namespace.id,
        sectors,
        sectors / 2048,
        disk.sectors_per_command() / 2,
        match (info.msix, info.msi) {
            (true, _) => "MSI-X interrupts",
            (_, true) => "MSI interrupts",
            _ => "polled",
        }
    );
    Ok(Nvme { disk })
}

fn main() -> i64 {
    let (Some(registers), Some(dma), Some(device), Some(writers), Some(readers)) = (
        taaj::startup_handle(0),
        taaj::startup_handle(1),
        taaj::startup_handle(2),
        taaj::startup_handle(3),
        taaj::startup_handle(4),
    ) else {
        println!("[nvme] started without its device");
        return 1;
    };
    let interrupt = taaj::startup_handle(5);

    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let described = match taaj::receive(device, &mut message, &mut []) {
        Ok((n, _)) => &message[..n],
        Err(_) => &[],
    };
    let Some(info) = PciDeviceInfo::from_bytes(described) else {
        println!("[nvme] no usable device description");
        return 2;
    };
    let mut nvme = match start(&info, registers, dma, interrupt) {
        Ok(nvme) => nvme,
        Err(code) => return code,
    };
    block_service::serve(&mut nvme, "nvme", writers, readers)
}
