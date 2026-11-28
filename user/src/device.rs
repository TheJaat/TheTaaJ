//! A device handle, from a driver's side (call::DEVICE, abi::device;
//! K75, TheTaaJ entry 171).
//!
//! The kernel hands every device it finds to init the same way - a
//! description and a device handle - and knows no kind of device. So
//! what the kernel used to decide for each kind is the driver's now:
//! which BAR its registers are in, and which kind of interrupt suits it
//! (an Intel e1000 wants MSI, an xHCI controller MSI before MSI-X).
//! `claim_pci` and `claim_mmio` ask for those and fill in the parts of
//! the description the kernel used to (`registers`, `io_base`, `msix`,
//! `msi`; init fills `dma_physical` and `dma_size`), so that a driver's
//! start goes on as before, with the handles it used to be given.

use abi::device::{self as op, Bar};
use abi::{call, Error, MmioDeviceInfo, PciDeviceInfo};

use crate::{println, syscall5, Handle};

fn device(handle: Handle, which: u64, argument: u64, out: u64) -> Result<u64, Error> {
    abi::decode(syscall5(call::DEVICE, handle, which, argument, out, 0))
}

/// `width` bytes (1, 2 or 4) of the device's configuration space at
/// `offset` (PCI).
pub fn config_read(handle: Handle, offset: u64, width: u64) -> Result<u64, Error> {
    device(handle, op::CONFIG_READ, op::config(offset, width), 0)
}

/// BAR `n`: a handle to it (I/O ports, or memory to map), and what it is.
pub fn bar(handle: Handle, n: u64) -> Result<(Handle, Bar), Error> {
    let mut out = [0u8; Bar::SIZE];
    let bar = device(handle, op::BAR, n, out.as_mut_ptr() as u64)?;
    Ok((bar, Bar::from_bytes(&out).unwrap_or_default()))
}

/// The device's interrupt, of `kind` (op::MSIX, op::MSI, op::WIRED).
pub fn interrupt(handle: Handle, kind: u64) -> Result<Handle, Error> {
    device(handle, op::INTERRUPT, kind, 0)
}

/// `pages` pages of contiguous memory for the device to DMA to, and
/// their physical address.
pub fn dma(handle: Handle, pages: u64) -> Result<(Handle, u64), Error> {
    let mut physical = 0u64;
    let memory = device(handle, op::DMA, pages, &raw mut physical as u64)?;
    Ok((memory, physical))
}

/// A PCI driver's start: the first of `kinds` of interrupt the device
/// has (none, if it has none of them: the driver polls), then BAR `n` -
/// in that order, since a device whose interrupt is MSI gets its whole
/// BAR, MSI-X table and all (abi::device::BAR). Fills `info` as the
/// kernel used to, and says on the log what the driver was given, under
/// its `name`. The registers' handle, and the interrupt's.
pub fn claim_pci(handle: Handle, info: &mut PciDeviceInfo, n: u64, kinds: &[u64], name: &str) -> Result<(Handle, Option<Handle>), Error> {
    let mut interrupt = None;
    for &kind in kinds {
        match self::interrupt(handle, kind) {
            Ok(line) => {
                info.msix = kind == op::MSIX;
                info.msi = kind == op::MSI;
                interrupt = Some(line);
                break;
            }
            Err(Error::NotFound) => continue,
            Err(error) => return Err(error),
        }
    }
    let (registers, bar) = self::bar(handle, n)?;
    if bar.io {
        info.io_base = bar.base as u16;
        info.io_size = bar.size as u16;
    } else {
        info.registers = bar.base;
        info.registers_size = bar.size;
    }
    let what = if bar.io { "ports" } else { "registers" };
    println!(
        "[{name}] {:02x}:{:02x}.{} {:04x}:{:04x}: {what} {:#x}+{:#x} (BAR {n}), {} KiB DMA at {:#x}, {}",
        info.bus,
        info.device,
        info.function,
        info.vendor_id,
        info.device_id,
        bar.base,
        bar.size,
        info.dma_size / 1024,
        info.dma_physical,
        match (info.msix, info.msi) {
            (true, _) => "MSI-X",
            (_, true) => "MSI",
            _ => "no interrupt (polled)",
        }
    );
    Ok((registers, interrupt))
}

/// The same for a place in memory: its registers, and the interrupt
/// the device tree names, if it names one.
pub fn claim_mmio(handle: Handle, info: &MmioDeviceInfo, name: &str) -> Result<(Handle, Option<Handle>), Error> {
    let interrupt = match info.interrupt {
        true => match self::interrupt(handle, op::WIRED) {
            Ok(line) => Some(line),
            Err(Error::NotFound) => None,
            Err(error) => return Err(error),
        },
        false => None,
    };
    let (registers, _) = self::bar(handle, 0)?;
    println!(
        "[{name}] {:#x} virtio kind {}: registers in the page at {:#x}, {} KiB DMA at {:#x}, {}",
        info.registers,
        info.device,
        info.registers & !0xFFF,
        info.dma_size / 1024,
        info.dma_physical,
        if interrupt.is_some() { "an interrupt" } else { "no interrupt (polled)" }
    );
    Ok((registers, interrupt))
}
