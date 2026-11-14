//! Interrupt setup for a PC, and the first stop for every interrupt.
//!
//! Vectors (the IDT slot each source raises):
//!
//!   0x00-0x1F  CPU exceptions (idt.rs)
//!   0x20       local APIC timer
//!   0x21       keyboard (ISA IRQ 1, through the I/O APIC)
//!   0xE0-0xEF  the disabled 8259 PICs - only ever spurious
//!   0xFF       local APIC spurious interrupt
//!
//! An interrupt's vector is also its priority (vector / 16): the timer
//! and keyboard are lowest, which is fine while they are the only ones.

use alloc::vec::Vec;

use kernel64::acpi::Madt;

use super::{apic, pic, ps2};
use crate::interrupts;
use crate::irqlock::IrqLock;
use crate::mm;

pub const TIMER_VECTOR: u8 = 0x20;
pub const KEYBOARD_VECTOR: u8 = 0x21;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// What `init` found and set up, for the boot log.
pub struct Report {
    pub cpu_apic_id: u8,
    pub io_apics: Vec<(u8, u64, u32, u32)>, // id, physical address, first GSI, inputs
    pub timer_hz: u64,
    pub bus_hz: u64,
    pub tsc_hz: u64,
    pub keyboard_gsi: Option<u32>,
}

static IO_APICS: IrqLock<Vec<apic::IoApic>> = IrqLock::new(Vec::new());
static TSC_HZ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn tsc_hz() -> u64 {
    TSC_HZ.load(core::sync::atomic::Ordering::Relaxed)
}

/// Sets up every interrupt source the kernel uses. Interrupts stay
/// disabled; the caller enables them when it is ready.
///
/// # Safety
/// Once, on the boot CPU, interrupts disabled, after memory is set up.
pub unsafe fn init(madt: &Madt, hz: u64) -> Report {
    // Every PC has the 8259s, whatever the MADT's flag says: switch them
    // off unconditionally.
    pic::disable();

    let local = mm::map_mmio(madt.local_apic_address, 4096);
    apic::init_local(local, SPURIOUS_VECTOR);
    let cpu = apic::local_id();

    let mut report = Report {
        cpu_apic_id: cpu,
        io_apics: Vec::new(),
        timer_hz: hz,
        bus_hz: 0,
        tsc_hz: 0,
        keyboard_gsi: None,
    };

    {
        let mut io_apics = IO_APICS.lock();
        for io in &madt.io_apics {
            let base = mm::map_mmio(io.address, 4096);
            let apic = apic::IoApic::new(base, io.gsi_base);
            report.io_apics.push((io.id, io.address, io.gsi_base, apic.inputs));
            io_apics.push(apic);
        }

        // The keyboard: ISA IRQ 1, wherever the firmware wired it.
        if ps2::init() {
            let route = madt.isa_route(1);
            if let Some(io) = io_apics.iter().find(|io| io.handles(route.gsi)) {
                io.route(route.gsi, KEYBOARD_VECTOR, route.polarity, route.trigger, cpu);
                report.keyboard_gsi = Some(route.gsi);
            }
        }
    }

    let (bus_hz, tsc_hz) = apic::calibrate();
    report.bus_hz = bus_hz;
    report.tsc_hz = tsc_hz;
    TSC_HZ.store(tsc_hz, core::sync::atomic::Ordering::Relaxed);
    apic::start_timer(TIMER_VECTOR, bus_hz, hz);
    report
}

/// Called by the IDT stub for every vector from 32 up, with interrupts
/// disabled. Must acknowledge the interrupt (EOI) or the local APIC will
/// deliver nothing more at this priority.
pub fn handle_interrupt(vector: u8) {
    match vector {
        TIMER_VECTOR => {
            interrupts::timer_tick();
            apic::end_of_interrupt();
        }
        KEYBOARD_VECTOR => {
            if let Some(byte) = ps2::read() {
                interrupts::keyboard_byte(byte);
            }
            apic::end_of_interrupt();
        }
        // Spurious: nothing happened. Neither the APIC nor the masked
        // PICs expect an EOI for these.
        SPURIOUS_VECTOR => {}
        v if (pic::VECTOR_BASE..pic::VECTOR_BASE + 16).contains(&v) => {}
        _ => {
            kprintln!("[irq] unexpected interrupt {vector:#x}");
            apic::end_of_interrupt();
        }
    }
}
