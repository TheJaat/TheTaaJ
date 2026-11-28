//! e1000 - the Intel Ethernet card's driver: the 82540EM that QEMU and
//! VirtualBox give a guest, the 82574L, and their family.
//!
//! What the driver does with the card is in the `e1000` crate, tested
//! there against a simulated card; this program gives it the card's
//! real registers and its real DMA memory, and serves it with the same
//! protocol as `nic` (`taaj::nic_service`). The network stack cannot
//! tell the two apart.
//!
//! Startup handles, from init - the same as nic's:
//!   1  the card's registers: the memory of its BAR 0, uncached
//!   2  its DMA memory: physically contiguous, its address known
//!   3  the device channel: the PciDeviceInfo comes first
//!   4  the request channel (abi::nic)
//!   5  the card's interrupt (MSI), if it has one - else polled. The
//!      82540EM has only the old interrupt pin, which needs ACPI's
//!      routing tables to find: it is polled, a tick at a time.
//!
//! On a restart init gives the new driver the same memory and
//! registers. The crate's `Card::start` resets the card, which forgets
//! the old driver's rings.

#![no_std]
#![no_main]

use abi::{nic, PciDeviceInfo};
use e1000::{layout, reg, Bus, Dma};
use taaj::nic_service::{self, Address};
use taaj::{println, Handle};

taaj::program!(main);

/// The last register the driver uses is RAH0: the registers given must
/// reach past it.
const REGISTERS_NEEDED: usize = reg::RAH0 + 4;

/// The card's registers, mapped here.
struct Registers {
    base: *mut u8,
    size: usize,
}

impl Bus for Registers {
    fn read(&mut self, register: usize) -> u32 {
        assert!(register + 4 <= self.size, "register {register:#x} is past the card's");
        // SAFETY: inside the mapping (checked above), a 32-bit register
        // at a multiple of four (every offset in the crate is). Volatile:
        // each read is a question to the card - and reading ICR clears it.
        unsafe { self.base.add(register).cast::<u32>().read_volatile() }
    }

    fn write(&mut self, register: usize, value: u32) {
        assert!(register + 4 <= self.size, "register {register:#x} is past the card's");
        // SAFETY: as for `read`.
        unsafe { self.base.add(register).cast::<u32>().write_volatile(value) }
    }

    fn pause(&mut self) {
        taaj::sleep(1);
    }

    fn ticks(&mut self) -> u64 {
        taaj::ticks()
    }
}

struct Intel(e1000::Card<Registers>);

impl nic_service::Card for Intel {
    fn address(&self) -> [u8; nic::ADDRESS] {
        self.0.address
    }

    /// Read when asked: the card's status register says it now.
    fn link_up(&mut self) -> bool {
        self.0.link_up()
    }

    fn receive(&mut self, pass: &mut dyn FnMut(Option<&[u8]>)) {
        self.0.receive(pass);
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        self.0.send(frame)
    }

    fn take_sent(&mut self) {
        self.0.take_sent();
    }

    /// The card interrupts for every frame it sends (TXDW), so when it
    /// is full the next interrupt is the one that says it has room.
    fn transmit_full(&mut self) -> bool {
        self.0.transmit_full()
    }

    fn interrupt_seen(&mut self) {
        // Reading the cause is the answer; what the cause was does not
        // matter - the service looks at both rings after every interrupt.
        let _ = self.0.interrupt_seen();
    }
}

fn map(memory: Handle, what: &str) -> Result<*mut u8, i64> {
    taaj::memory_map(memory, true).map_err(|error| {
        println!("[e1000] cannot map {what}: {error:?}");
        4
    })
}

fn start(info: &PciDeviceInfo, registers: Handle, dma: Handle, interrupt: Option<Handle>) -> Result<Intel, i64> {
    let model = e1000::model(info.vendor_id, info.device_id);
    // Where the registers begin in the mapping, and whether they all
    // fit in it: asked of the kernel, not taken from the description.
    let offset = (info.registers % 4096) as usize;
    let size = info.registers_size as usize;
    let fits = taaj::memory_size(registers).is_ok_and(|mapped| (offset + size) as u64 <= mapped);
    if model.is_none() || !info.registers_in_memory() || !fits || size < REGISTERS_NEEDED || (info.dma_size as usize) < layout::NEEDED {
        println!("[e1000] no usable device description");
        return Err(2);
    }
    let page = map(registers, "the card's registers")?;
    let memory = map(dma, "the DMA memory")?;
    // SAFETY: inside the mapping (checked above).
    let base = unsafe { page.add(offset) };
    // SAFETY: the DMA memory, all of it, mapped here and used for
    // nothing else; `dma_physical` is where the kernel says it is.
    let dma = unsafe { Dma::new(memory, info.dma_physical, layout::NEEDED) };
    let mut card = e1000::Card::start(Registers { base, size }, dma).map_err(|why| {
        println!("[e1000] the card would not start: {why}");
        5
    })?;
    if interrupt.is_some() {
        card.enable_interrupts();
    }
    println!(
        "[e1000] Intel {} at {:02x}:{:02x}.{}: address {}, link {}, rings of {} and {}, {}",
        model.unwrap_or("?"),
        info.bus,
        info.device,
        info.function,
        Address(card.address),
        if card.link_up() { "up" } else { "down" },
        e1000::RX_DESCRIPTORS,
        e1000::TX_DESCRIPTORS,
        match (interrupt.is_some(), info.msix) {
            (false, _) => "polled",
            (true, false) => "MSI interrupts",
            (true, true) => "MSI-X interrupts", // not given to this card (devices.rs), but said if it were
        }
    );
    Ok(Intel(card))
}

fn main() -> i64 {
    // The device's handle (K75: its registers and interrupt are asked
    // of it), its DMA memory (init's), its description, and requests.
    let (Some(handle), Some(dma), Some(device), Some(requests)) =
        (taaj::startup_handle(0), taaj::startup_handle(1), taaj::startup_handle(2), taaj::startup_handle(3))
    else {
        println!("[e1000] started without its device");
        return 1;
    };

    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let described = match taaj::receive(device, &mut message, &mut []) {
        Ok((n, _)) => &message[..n],
        Err(_) => &[],
    };
    let Some(mut info) = PciDeviceInfo::from_bytes(described) else {
        println!("[e1000] no usable device description");
        return 2;
    };
    // Its registers in BAR 0; MSI, or no interrupt. Not MSI-X, which an
    // 82574L has: in MSI-X mode it routes each cause to a vector of its
    // own (IVAR), which this driver does not set up; with MSI it
    // behaves as the 82540EM does. (Until K75 the kernel knew this.)
    let (registers, interrupt) = match taaj::device::claim_pci(handle, &mut info, 0, &[abi::device::MSI], "e1000") {
        Ok(claimed) => claimed,
        Err(error) => {
            println!("[e1000] the card's registers or interrupt: {error:?}");
            return 2;
        }
    };
    let mut card = match start(&info, registers, dma, interrupt) {
        Ok(card) => card,
        Err(code) => return code,
    };
    nic_service::serve(&mut card, "e1000", requests, interrupt)
}
