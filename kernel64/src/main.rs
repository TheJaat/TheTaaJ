//! TheTaaJ - the 64-bit kernel.
//!
//! The bootloader puts the CPU in long mode and jumps here with a
//! TaajBootInfo. The kernel takes over the CPU tables, then memory: its
//! own page tables, a frame allocator and a heap. Threads, IPC and
//! capabilities get rebuilt on top of this, in Rust.
//!
//! Two crates share this directory:
//!   lib.rs (kernel64::)  pure logic - boot info, frames, page tables,
//!                        heap, locks - tested on the host by `cargo test`
//!   main.rs (this)       the kernel proper:
//!     arch/        the only code that knows which CPU it is on
//!     console      kprint!/kprintln!, over whatever the arch provides
//!     mm           memory management wired to the real machine
//!     selftest     checks that wiring at every boot
//!     framebuffer  drawing on the screen the bootloader set up

// No standard library: there is no OS underneath to provide one. `core`
// (types, traits, formatting) is still available - it needs nothing.
#![no_std]
// No `fn main`: nothing calls it. The entry point is `_start`, in
// arch/x86_64/entry.rs, which the linker script names.
#![no_main]

// Box, Vec, String and friends - backed by the heap in mm.rs.
extern crate alloc;

#[macro_use]
mod console; // first, so its macros are visible in every later module

mod arch;
mod framebuffer;
mod interrupts;
mod irqlock;
mod mm;
mod selftest;
mod threads;

use alloc::string::String;

use kernel64::acpi::{Acpi, IoApic, Madt};
use kernel64::bootinfo::BootInfo;
use kernel64::input::{Decoder, Key};

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
    // which the physical offset (still 0) reflects.
    let info = match unsafe { BootInfo::from_physical(boot_info_address) } {
        Ok(info) => info,
        Err(error) => panic!("boot info at {boot_info_address:#x} is unusable: {error}"),
    };

    report(info);

    // Off the loader's page tables and onto the kernel's own. `info` is
    // re-read through the direct map; the old reference is gone.
    kprintln!();
    let info = mm::init(info, boot_info_address);

    selftest::run();

    // Interrupts: find the controllers, set them up, switch on.
    kprintln!();
    let madt = read_madt(info.acpi_rsdp);
    let ids: alloc::vec::Vec<u8> = madt.processors.iter().filter(|p| p.enabled).map(|p| p.apic_id).collect();
    kprintln!("[acpi] {} CPU(s), APIC ids {ids:?} - this one runs; the rest wait for SMP", ids.len());
    // SAFETY: once, on the boot CPU, interrupts still disabled.
    let platform = unsafe { arch::platform::init(&madt, interrupts::HZ) };
    report_platform(&platform);
    arch::enable_interrupts();
    selftest::timer();

    // Threads: the code running now becomes thread 1, "boot"; thread 0
    // is the idle thread.
    threads::init();
    selftest::threads();
    threads::spawn("echo", echo);

    match info.framebuffer() {
        Some(fb) if framebuffer::paint_welcome(fb) => kprintln!("framebuffer: painted"),
        Some(_) => kprintln!("framebuffer: unsupported pixel format, left alone"),
        None => kprintln!("framebuffer: none"),
    }

    // Only in a build with one of the `fault-*` features: proves each
    // protection holds and is reported. Once there is a kernel command
    // line, this can move there.
    #[cfg(feature = "fault-tests")]
    run_fault_test();

    idle()
}

/// Reads the MADT, or - with no usable ACPI - assumes the standard PC
/// layout: one CPU, the local APIC and one I/O APIC at their
/// architectural default addresses, no overrides.
fn read_madt(rsdp: u64) -> Madt {
    let memory = mm::DirectMap;
    let found = match Acpi::new(&memory, rsdp) {
        Ok(acpi) => {
            let mut names = String::new();
            for signature in acpi.signatures(&memory) {
                names.push_str(core::str::from_utf8(&signature).unwrap_or("????"));
                names.push(' ');
            }
            kprintln!("[acpi] revision {}, OEM {:?}, tables: {names}", acpi.revision,
                core::str::from_utf8(&acpi.oem).unwrap_or("?").trim_end());
            acpi.find(&memory, b"APIC").and_then(|table| Madt::parse(&table))
        }
        Err(error) => Err(error),
    };
    found.unwrap_or_else(|error| {
        kprintln!("[acpi] no usable MADT ({error}) - assuming the standard PC layout");
        Madt {
            local_apic_address: 0xFEE0_0000,
            has_legacy_pics: true,
            io_apics: alloc::vec![IoApic { id: 0, address: 0xFEC0_0000, gsi_base: 0 }],
            ..Madt::default()
        }
    })
}

fn report_platform(p: &arch::platform::Report) {
    kprintln!("[irq] local APIC id {}; legacy PICs remapped and masked", p.cpu_apic_id);
    for &(id, address, gsi_base, inputs) in &p.io_apics {
        kprintln!("[irq] I/O APIC {id} at {address:#x}: GSIs {gsi_base}-{}", gsi_base + inputs - 1);
    }
    kprintln!(
        "[irq] APIC timer at {} Hz (bus clock {} MHz), TSC {} MHz",
        p.timer_hz,
        p.bus_hz / 1_000_000,
        p.tsc_hz / 1_000_000
    );
    match p.keyboard_gsi {
        Some(gsi) => kprintln!("[irq] PS/2 keyboard on GSI {gsi}"),
        None => kprintln!("[irq] no PS/2 keyboard"),
    }
}

/// The end of booting: the boot thread's work is done, so it exits and
/// leaves the CPU to the threads it started - and to the idle thread.
fn idle() -> ! {
    kprintln!();
    kprintln!("[kernel] idle - {} threads; type, and each line is echoed back", threads::count() - 1);
    threads::exit()
}

/// A thread that echoes keyboard lines - the first thread that blocks
/// for real: it sleeps until the keyboard interrupt wakes it, using no
/// CPU at all while nobody types.
fn echo() {
    let mut decoder = Decoder::new();
    let mut line = String::new();
    loop {
        interrupts::KEYBOARD_WAITERS.wait_while(|| interrupts::KEYBOARD.is_empty());
        while let Some(byte) = interrupts::KEYBOARD.pop() {
            let Some(event) = decoder.feed(byte) else { continue };
            if !event.pressed {
                continue;
            }
            match event.key {
                Key::Char(c) => {
                    line.push(c);
                    kprint!("{c}");
                }
                Key::Backspace => {
                    if line.pop().is_some() {
                        kprint!("\x08 \x08"); // back, blank, back
                    }
                }
                Key::Enter => {
                    kprintln!();
                    let me = threads::current();
                    kprintln!("[keyboard] line: {line:?} (thread {me}, {:?})", threads::name(me).unwrap_or("?"));
                    line.clear();
                }
                _ => {}
            }
        }
    }
}

#[cfg(feature = "fault-tests")]
fn run_fault_test() {
    use arch::fault_tests as test;
    kprintln!();
    if cfg!(feature = "fault-double") {
        kprintln!("[test] forcing a double fault...");
        test::double_fault();
    }
    if cfg!(feature = "fault-rodata") {
        kprintln!("[test] writing to read-only data...");
        test::write_rodata();
    }
    if cfg!(feature = "fault-nx") {
        kprintln!("[test] executing data...");
        test::execute_data();
    }
    if cfg!(feature = "fault-stack") {
        kprintln!("[test] overflowing the boot stack...");
        let depth = test::overflow_stack(0);
        kprintln!("[test] FAILED: recursion returned {depth}");
    }
    if cfg!(feature = "fault-thread-stack") {
        kprintln!("[test] overflowing a thread's stack...");
        let id = threads::spawn("overflow", || {
            let depth = test::overflow_stack(0);
            kprintln!("[test] FAILED: recursion returned {depth}");
        });
        threads::join(id);
    }
    if cfg!(feature = "fault-page") {
        kprintln!("[test] forcing a page fault...");
        let value = test::page_fault();
        kprintln!("[test] FAILED: read {value:#x} from unmapped memory");
    }
    kprintln!("[test] FAILED: the fault did not happen");
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
    arch::disable_interrupts();
    // SAFETY: halting right after; whoever held the console never resumes.
    unsafe { console::break_lock() };
    kprintln!();
    kprintln!("*** KERNEL PANIC: {info}");
    arch::halt_forever()
}
