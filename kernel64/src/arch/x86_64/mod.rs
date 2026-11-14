//! x86_64.

mod apic;
pub mod context;
mod entry;
mod gdt;
mod idt;
pub mod mmu;
mod pic;
mod pit;
pub mod platform;
mod port;
mod ps2;
mod serial;
pub mod syscall;

use core::arch::asm;

/// Sets up the earliest output there is: the COM1 serial port.
pub fn init_early_console() {
    serial::init();
}

pub fn console_write(text: &str) {
    serial::write_str(text);
}

/// Replaces the bootloader's descriptor tables with the kernel's own:
/// a GDT with kernel and user segments, a TSS with a separate stack for
/// double faults, and an IDT that catches every CPU exception.
pub fn init_cpu() {
    // SAFETY: called once, from kernel_main, on the only CPU running,
    // with interrupts disabled - the conditions both inits require.
    unsafe {
        gdt::init();
        idt::init();
        syscall::init();
    }
}

/// Whether this CPU accepts interrupts now (RFLAGS.IF).
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: reading RFLAGS has no side effects.
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags)) };
    flags & (1 << 9) != 0
}

/// Disables interrupts, returning whether they were enabled - to hand
/// back to `restore_interrupts`.
pub fn disable_interrupts() -> bool {
    let were_enabled = interrupts_enabled();
    // SAFETY: cli only changes RFLAGS.IF.
    unsafe { asm!("cli", options(nomem, nostack)) };
    were_enabled
}

pub fn restore_interrupts(were_enabled: bool) {
    if were_enabled {
        enable_interrupts();
    }
}

pub fn enable_interrupts() {
    // SAFETY: the IDT handles every vector by the time anyone calls this.
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// Enables interrupts and sleeps until one arrives. `sti` takes effect
/// only after the NEXT instruction, so no interrupt can slip in between
/// the two and leave `hlt` waiting for a second one.
pub fn wait_for_interrupt() {
    // SAFETY: as for enable_interrupts.
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

/// The CPU's time-stamp counter: cycles since reset.
pub fn tsc() -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: rdtsc only reads the counter.
    unsafe { asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags)) };
    (u64::from(high) << 32) | u64::from(low)
}

/// Raises a breakpoint exception, which the IDT reports and resumes
/// from. Proves the whole exception path - stub, save, Rust, restore,
/// `iretq` - round-trips correctly.
pub fn breakpoint() {
    // SAFETY: int3 only raises vector 3, whose handler returns.
    unsafe { core::arch::asm!("int3", options(nomem, nostack)) };
}

/// Stops this CPU for good. `hlt` waits for an interrupt; with
/// interrupts disabled only an NMI can wake it, hence the loop.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: cli and hlt touch no memory; this CPU simply stops.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Deliberately faults, for checking the handlers and the page-table
/// protections. Only built with one of the `fault-*` features.
#[cfg(feature = "fault-tests")]
pub mod fault_tests {
    /// Reads an address no page table maps: a page fault.
    pub fn page_fault() -> u64 {
        // 256 GiB, in the lower half - which the kernel maps nothing in.
        let address = 0x40_0000_0000 as *const u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.read_volatile() }
    }

    /// Points the stack at unmapped memory, then raises an exception.
    /// The CPU cannot push the exception frame, so delivering *that*
    /// page fault fails too - a double fault, which only works because
    /// its handler runs on its own IST stack.
    pub fn double_fault() -> ! {
        // SAFETY: none - this is meant to fault, and never returns.
        unsafe { core::arch::asm!("mov rsp, 0x4000000000", "int3", options(noreturn)) }
    }

    /// An immutable static: the linker puts it in .rodata, which the
    /// kernel maps read-only.
    static READ_ONLY: u64 = 0x1234;

    /// Writes to read-only data. Without CR0.WP the kernel would be
    /// allowed to; with it, this is a protection fault.
    pub fn write_rodata() {
        let address = &raw const READ_ONLY as *mut u64;
        // SAFETY: none - this is meant to fault.
        unsafe { address.write_volatile(0xBAD) };
    }

    /// One `ret` instruction, stored as data in .data, which is mapped
    /// no-execute. `#[used]` keeps it even though nothing reads it as data.
    #[used]
    static mut DATA: [u8; 1] = [0xC3];

    /// Calls into data. Code injected into a buffer is exactly this - and
    /// NX is what stops it running.
    pub fn execute_data() {
        // black_box hides where the pointer came from. Without it, LLVM
        // sees a call to a data object, reasons about it as if it were a
        // function, and the build fails to link.
        let address = core::hint::black_box(&raw const DATA as *const ());
        // SAFETY: none - this is meant to fault.
        let function: extern "C" fn() = unsafe { core::mem::transmute(address) };
        function();
    }

    /// Recurses until the boot stack runs into its guard page.
    #[allow(unconditional_recursion)]
    pub fn overflow_stack(depth: u64) -> u64 {
        // A big frame, kept alive by black_box, so this can neither be
        // turned into a loop nor optimised away.
        let frame = core::hint::black_box([depth; 64]);
        overflow_stack(depth + 1) + frame[7]
    }
}
