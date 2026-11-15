//! The architecture layer.
//!
//! The rest of the kernel calls only what is re-exported here, never an
//! arch module directly. Adding AArch64 means adding `arch/aarch64/`
//! with the same functions - and nothing outside `arch/` changes.

#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use self::x86_64::{
    breakpoint, console_write, console_write_bytes, disable_interrupts, enable_interrupts, halt_forever, init_cpu,
    init_early_console, mmu, pci, platform, context, syscall, restore_interrupts, set_io_bitmap, tsc,
    wait_for_interrupt,
};

#[cfg(all(target_arch = "x86_64", feature = "fault-tests"))]
pub use self::x86_64::fault_tests;
