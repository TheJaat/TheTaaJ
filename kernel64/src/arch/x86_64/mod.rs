//! x86_64.

mod entry;
mod port;
mod serial;

/// Sets up the earliest output there is: the COM1 serial port.
pub fn init_early_console() {
    serial::init();
}

pub fn console_write(text: &str) {
    serial::write_str(text);
}

/// Stops this CPU for good. `hlt` waits for an interrupt; with
/// interrupts disabled only an NMI can wake it, hence the loop.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: cli and hlt touch no memory; this CPU simply stops.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
