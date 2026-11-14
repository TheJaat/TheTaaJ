//! The APICs: the local APIC in each CPU, and the I/O APICs that collect
//! device interrupts and send them to CPUs.
//!
//! Both are memory-mapped devices: their registers are addresses, read
//! and written with volatile accesses through an uncached mapping.
//!
//!   device --IRQ line--> I/O APIC --message--> local APIC --> CPU
//!
//! The I/O APIC's redirection table says, for each input, which vector to
//! raise on which CPU, and how to read the signal (edge or level, active
//! high or low). The local APIC also has its own timer - the kernel's
//! heartbeat - and must be told "end of interrupt" (EOI) after each one
//! before it delivers another of the same or lower priority.

use core::sync::atomic::{AtomicU64, Ordering};

use kernel64::acpi::{Polarity, Trigger};

use super::pit;

// Local APIC register offsets.
const ID: u64 = 0x020;
const TASK_PRIORITY: u64 = 0x080;
const EOI: u64 = 0x0B0;
const SPURIOUS: u64 = 0x0F0;
const LVT_TIMER: u64 = 0x320;
const LVT_ERROR: u64 = 0x370;
const TIMER_INITIAL: u64 = 0x380;
const TIMER_CURRENT: u64 = 0x390;
const TIMER_DIVIDE: u64 = 0x3E0;

const SOFTWARE_ENABLE: u32 = 1 << 8;
const MASKED: u32 = 1 << 16;
const TIMER_PERIODIC: u32 = 1 << 17;
const DIVIDE_BY_16: u32 = 0b0011;

/// Virtual address of this CPU's local APIC registers.
static LOCAL_BASE: AtomicU64 = AtomicU64::new(0);

fn read(register: u64) -> u32 {
    let address = LOCAL_BASE.load(Ordering::Relaxed) + register;
    // SAFETY: init() mapped the register page uncached; register offsets
    // are fixed by the hardware.
    unsafe { (address as *const u32).read_volatile() }
}

fn write(register: u64, value: u32) {
    let address = LOCAL_BASE.load(Ordering::Relaxed) + register;
    // SAFETY: as for read.
    unsafe { (address as *mut u32).write_volatile(value) };
}

/// Turns the local APIC on, with `spurious_vector` for the interrupts it
/// raises when it has nothing real to deliver.
///
/// # Safety
/// `base` must be an uncached mapping of the local APIC's registers.
pub unsafe fn init_local(base: u64, spurious_vector: u8) {
    LOCAL_BASE.store(base, Ordering::Relaxed);
    write(TASK_PRIORITY, 0); // accept every priority
    write(LVT_ERROR, MASKED);
    write(LVT_TIMER, MASKED);
    write(SPURIOUS, SOFTWARE_ENABLE | u32::from(spurious_vector));
}

pub fn local_id() -> u8 {
    (read(ID) >> 24) as u8
}

pub fn end_of_interrupt() {
    write(EOI, 0);
}

/// How fast the APIC timer and the TSC run, measured against the PIT:
/// (timer ticks per second, TSC cycles per second).
///
/// # Safety
/// Interrupts disabled; uses PIT channel 2.
pub unsafe fn calibrate() -> (u64, u64) {
    const WINDOW_US: u64 = 10_000;
    write(TIMER_DIVIDE, DIVIDE_BY_16);
    write(LVT_TIMER, MASKED); // count, but raise nothing
    pit::start(WINDOW_US);
    let tsc_start = super::tsc();
    write(TIMER_INITIAL, u32::MAX);
    pit::wait();
    let remaining = read(TIMER_CURRENT);
    let tsc_end = super::tsc();
    write(TIMER_INITIAL, 0); // stop
    let per_window = u64::from(u32::MAX - remaining);
    (per_window * 1_000_000 / WINDOW_US, (tsc_end - tsc_start) * 1_000_000 / WINDOW_US)
}

/// Starts the timer raising `vector` `hz` times a second.
pub fn start_timer(vector: u8, ticks_per_second: u64, hz: u64) {
    write(TIMER_DIVIDE, DIVIDE_BY_16);
    write(LVT_TIMER, TIMER_PERIODIC | u32::from(vector));
    write(TIMER_INITIAL, (ticks_per_second / hz).max(1) as u32);
}

/// One I/O APIC. Its registers sit behind a two-register window: write
/// the register number to IOREGSEL, then read or write IOWIN.
pub struct IoApic {
    base: u64,
    pub gsi_base: u32,
    pub inputs: u32,
}

const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;
const IOAPICVER: u32 = 0x01;
const REDIRECTION_TABLE: u32 = 0x10;

impl IoApic {
    /// # Safety
    /// `base` must be an uncached mapping of an I/O APIC's registers.
    pub unsafe fn new(base: u64, gsi_base: u32) -> IoApic {
        let mut io = IoApic { base, gsi_base, inputs: 0 };
        io.inputs = ((io.read(IOAPICVER) >> 16) & 0xFF) + 1;
        // Mask every input until a driver asks for one.
        for input in 0..io.inputs {
            io.write(REDIRECTION_TABLE + 2 * input, MASKED);
        }
        io
    }

    fn read(&self, register: u32) -> u32 {
        // SAFETY: `new`'s contract; the window protocol is two accesses.
        unsafe {
            ((self.base + IOREGSEL) as *mut u32).write_volatile(register);
            ((self.base + IOWIN) as *const u32).read_volatile()
        }
    }

    fn write(&self, register: u32, value: u32) {
        // SAFETY: as for read.
        unsafe {
            ((self.base + IOREGSEL) as *mut u32).write_volatile(register);
            ((self.base + IOWIN) as *mut u32).write_volatile(value);
        }
    }

    pub fn handles(&self, gsi: u32) -> bool {
        (self.gsi_base..self.gsi_base + self.inputs).contains(&gsi)
    }

    /// Sends global interrupt `gsi` to `vector` on the CPU whose local
    /// APIC id is `cpu`, unmasked.
    pub fn route(&self, gsi: u32, vector: u8, polarity: Polarity, trigger: Trigger, cpu: u8) {
        let input = gsi - self.gsi_base;
        let mut low = u32::from(vector); // fixed delivery, physical destination
        if polarity == Polarity::ActiveLow {
            low |= 1 << 13;
        }
        if trigger == Trigger::Level {
            low |= 1 << 15;
        }
        // High half first: the entry goes live when the low half (with
        // the mask bit clear) is written.
        self.write(REDIRECTION_TABLE + 2 * input + 1, u32::from(cpu) << 24);
        self.write(REDIRECTION_TABLE + 2 * input, low);
    }
}
