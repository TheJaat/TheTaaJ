//! Devices on the PCI bus: found by the kernel, driven from user space.
//!
//! The kernel's part is mechanism. It scans the bus, logs what is there,
//! and for each device it knows how to hand out, gathers what a driver
//! needs and turns each piece into a capability:
//!
//!   - its I/O ports: an IoPorts object for the BAR's range
//!   - memory it can DMA to: a contiguous memory object, and its
//!     physical address (devices know no other kind)
//!   - its interrupt: MSI-X message 0 pointed at a fresh vector, and an
//!     Interrupt object for that vector
//!
//! and sends them to init, which decides which driver gets them - policy
//! is init's. The kernel never talks to the device again.
//!
//! What this does not protect against: a device does DMA to whatever
//! physical address its driver tells it, so a malicious disk driver could
//! have the disk overwrite the kernel. Confining DMA needs an IOMMU
//! (Intel VT-d, AMD-Vi), programmed so that each device can reach only
//! its own memory objects - a later step. Until then, a user-space driver
//! is protected from by the CPU (it cannot touch anything else directly)
//! but not by the bus.
//!
//! Only virtio block devices are offered so far.

use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::PciDeviceInfo;
use kernel64::cap::Rights;
use kernel64::pci::{self, Bar, Device};

use crate::arch::{pci::Ports, platform};
use crate::ipc::{self, Object};
use crate::memory_object::MemoryObject;
use crate::{interrupts, mm};

/// DMA memory for a block driver: its virtqueue and request buffers.
const BLOCK_DMA_PAGES: u64 = 16;

/// Scans the bus and sends init a message for each device it can drive.
pub fn offer(to_init: &Object) {
    let mut config = Ports;
    let devices = pci::scan(&mut config);
    for d in &devices {
        kprintln!(
            "[pci] {} {:04x}:{:04x} class {:02x}.{:02x} {}",
            d.address, d.vendor, d.device, d.class, d.subclass, d.kind()
        );
    }
    for device in devices.iter().filter(|d| d.vendor == virtio::PCI_VENDOR && d.device == virtio::PCI_DEVICE_BLOCK) {
        match prepare(&mut config, device) {
            Ok((info, handles)) => {
                let mut message = b"pci".to_vec();
                message.extend_from_slice(&info.to_bytes());
                match ipc::kernel_send(to_init, &message, handles) {
                    Ok(()) => kprintln!(
                        "[pci] {} virtio block device: ports {:#x}+{:#x}, {} KiB DMA at {:#x}, {} - handed to init",
                        device.address,
                        info.io_base,
                        info.io_size,
                        info.dma_size / 1024,
                        info.dma_physical,
                        if info.msix { "MSI-X" } else { "no interrupt (polled)" }
                    ),
                    Err(error) => kprintln!("[pci] {}: could not hand it to init: {error:?}", device.address),
                }
            }
            Err(why) => kprintln!("[pci] {} virtio block device not usable: {why}", device.address),
        }
    }
}

type Handles = Vec<(Object, Rights)>;

fn prepare(config: &mut Ports, device: &Device) -> Result<(PciDeviceInfo, Handles), &'static str> {
    let address = device.address;
    let Some(Bar::Io { base, size }) = pci::bar(config, address, 0) else {
        return Err("BAR 0 is not I/O ports (not a legacy-capable device)");
    };
    if base == 0 || base + size > 0x1_0000 {
        return Err("BAR 0 has no usable port range");
    }

    // Decode its ports and memory, let it do DMA, and no pin interrupts.
    let command = pci::command(config, address);
    pci::set_command(
        config,
        address,
        command | pci::COMMAND_IO | pci::COMMAND_MEMORY | pci::COMMAND_BUS_MASTER | pci::COMMAND_INTX_DISABLE,
    );

    let (dma, dma_physical) = MemoryObject::contiguous(BLOCK_DMA_PAGES).ok_or("no contiguous memory for DMA")?;
    let dma_size = dma.size();
    let mut handles: Handles = alloc::vec![
        (Object::IoPorts(base..base + size), Rights::ALL),
        (Object::Memory(Arc::new(dma)), Rights::ALL),
    ];

    let msix = match msix_to_new_line(config, device) {
        Some(line) => {
            handles.push((Object::Interrupt(line), Rights::ALL));
            true
        }
        None => false,
    };
    let info = PciDeviceInfo {
        bus: address.bus,
        device: address.device,
        function: address.function,
        vendor_id: device.vendor,
        device_id: device.device,
        io_base: base as u16,
        io_size: size as u16,
        dma_physical,
        dma_size,
        msix,
    };
    Ok((info, handles))
}

/// Points the device's MSI-X message 0 at a new device line, and turns
/// MSI-X on. None if it has no MSI-X, or no line is left.
fn msix_to_new_line(config: &mut Ports, device: &Device) -> Option<&'static interrupts::InterruptLine> {
    let msix = pci::msix(config, device.address)?;
    let Some(Bar::Memory { base, size, .. }) = pci::bar(config, device.address, msix.table_bar) else {
        return None;
    };
    if u64::from(msix.table_offset) + pci::MSIX_ENTRY_SIZE > size {
        return None;
    }
    let (n, line) = interrupts::allocate_device_line()?;
    let (vector, cpu) = platform::device_vector(n);
    let (message_address, message_data) = pci::msi_message(cpu, vector);

    // The table: in the BAR's memory, device registers, uncached.
    let table = mm::map_mmio(base + u64::from(msix.table_offset), pci::MSIX_ENTRY_SIZE) as *mut u32;
    // SAFETY: entry 0 of the table, just mapped; volatile, as for any
    // device register. The vector control word last: it unmasks.
    unsafe {
        table.write_volatile(message_address as u32);
        table.add(1).write_volatile((message_address >> 32) as u32);
        table.add(2).write_volatile(message_data);
        table.add(3).write_volatile(0);
    }
    pci::enable_msix(config, device.address, &msix);
    Some(line)
}
