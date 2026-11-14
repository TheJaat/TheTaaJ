//! TheTaaJ - the 64-bit kernel.
//!
//! This is the first step of the new base: the bootloader puts the CPU
//! in long mode and jumps here with a TaajBootInfo, and the kernel proves
//! it can read it. Everything the 32-bit kernel does - memory management,
//! threads, IPC, capabilities - gets rebuilt on top of this, in Rust.
//!
//! Layout:
//!   arch/        the only code that knows which CPU it is on
//!   console      kprint!/kprintln!, over whatever the arch provides
//!   bootinfo     the TaajBoot protocol, mirrored from taajboot.h
//!   framebuffer  drawing on the screen the bootloader set up

// No standard library: there is no OS underneath to provide one. `core`
// (types, traits, formatting) is still available - it needs nothing.
#![no_std]
// No `fn main`: nothing calls it. The entry point is `_start`, in
// arch/x86_64/entry.rs, which the linker script names.
#![no_main]

#[macro_use]
mod console; // first, so its macros are visible in every later module

mod arch;
mod bootinfo;
mod framebuffer;

use bootinfo::BootInfo;

/// The first Rust code to run, called by `_start` with the physical
/// address of the TaajBootInfo (in RDI, per the C calling convention).
///
/// `-> !` means "never returns": there is nothing to return to.
extern "C" fn kernel_main(boot_info_address: u64) -> ! {
    arch::init_early_console();

    kprintln!();
    kprintln!("Hello from TheTaaJ!");
    // `{:p}` prints a pointer; the address shows the kernel really runs
    // in the higher half.
    kprintln!("64-bit kernel, kernel_main at {:p}", kernel_main as *const ());

    // From here on, a CPU fault is a message rather than a reboot.
    arch::init_cpu();
    kprintln!("[cpu] GDT, TSS and IDT loaded");
    arch::breakpoint();
    kprintln!("[cpu] resumed after the breakpoint - exceptions round-trip");

    // SAFETY: the TaajBoot protocol guarantees RDI held the structure's
    // physical address, and that the first 4 GiB are identity-mapped -
    // so that address is also a valid pointer, for as long as the
    // kernel keeps the loader's page tables.
    let info = match unsafe { BootInfo::from_physical(boot_info_address) } {
        Ok(info) => info,
        Err(error) => panic!("boot info at {boot_info_address:#x} is unusable: {error}"),
    };

    report(info);

    match info.framebuffer() {
        Some(fb) if framebuffer::paint_welcome(fb) => kprintln!("framebuffer: painted"),
        Some(_) => kprintln!("framebuffer: unsupported pixel format, left alone"),
        None => kprintln!("framebuffer: none"),
    }

    // Only in a build with `--features fault-tests` (a page fault) or
    // `--features double-fault-test`: proves the handlers report real
    // faults. Once there is a kernel command line, this can move there.
    #[cfg(feature = "fault-tests")]
    run_fault_test();

    kprintln!();
    kprintln!("Nothing more to do yet - halting.");
    arch::halt_forever()
}

#[cfg(feature = "fault-tests")]
fn run_fault_test() {
    kprintln!();
    if cfg!(feature = "double-fault-test") {
        kprintln!("[test] forcing a double fault...");
        arch::fault_tests::double_fault();
    }
    kprintln!("[test] forcing a page fault...");
    let value = arch::fault_tests::page_fault();
    kprintln!("[test] FAILED: read {value:#x} from unmapped memory");
}

/// Prints everything the bootloader told us.
fn report(info: &BootInfo) {
    kprintln!();
    kprintln!("boot info:");
    kprintln!("  loader         {} ({})", info.loader_name(), info.boot_method_name());
    kprintln!(
        "  kernel         {:#x} bytes at physical {:#x}, virtual {:#x}",
        info.kernel_size,
        info.kernel_physical,
        info.kernel_virtual
    );

    match (info.ramdisk_address, info.ramdisk_size) {
        (0, _) => kprintln!("  ramdisk        none"),
        (address, size) => kprintln!("  ramdisk        {size:#x} bytes at {address:#x}"),
    }
    match info.acpi_rsdp {
        0 => kprintln!("  ACPI RSDP      not found"),
        rsdp => kprintln!("  ACPI RSDP      {rsdp:#x}"),
    }
    if info.device_tree != 0 {
        kprintln!("  device tree    {:#x}", info.device_tree);
    }
    if let Some(line) = info.command_line() {
        kprintln!("  command line   {line}");
    }
    if let Some(fb) = info.framebuffer() {
        kprintln!(
            "  framebuffer    {}x{}, {} bpp, pitch {}, at {:#x}",
            fb.width,
            fb.height,
            fb.bpp,
            fb.pitch,
            fb.address
        );
    }

    let regions = info.memory_map();
    let mut usable: u64 = 0;
    kprintln!("  memory map     {} regions:", regions.len());
    for region in regions {
        kprintln!(
            "    {:#014x} - {:#014x}  {:>10} KiB  {}",
            region.base,
            region.base + region.length,
            region.length / 1024,
            region.kind_name()
        );
        if region.is_usable() {
            usable += region.length;
        }
    }
    kprintln!("  usable memory  {} MiB", usable / (1024 * 1024));
}

/// What happens on a bug the kernel cannot recover from: say so, loudly,
/// and stop. `PanicInfo` formats as the location plus the message.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    kprintln!();
    kprintln!("*** KERNEL PANIC: {info}");
    arch::halt_forever()
}
