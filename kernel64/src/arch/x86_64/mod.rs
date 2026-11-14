//! x86_64.

mod entry;
mod gdt;
mod idt;
mod port;
mod serial;

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
    }
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

/// Deliberately faults, for checking the handlers. Only built with the
/// `fault-tests` feature.
#[cfg(feature = "fault-tests")]
pub mod fault_tests {
    /// Reads an address no page table maps: a page fault.
    pub fn page_fault() -> u64 {
        // 256 GiB - far past the 4 GiB the bootloader identity-maps.
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
}
