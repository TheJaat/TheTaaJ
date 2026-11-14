//! Checks the kernel runs on itself at every boot.
//!
//! The host tests prove the algorithms; these prove the wiring - that
//! the real frame allocator, the real page tables and the real heap work
//! on the real machine. Each one asserts, so a failure is a kernel panic
//! with the exact line, and `make boot64-test` fails on it.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use kernel64::memory::addr::{phys_to_virt, VirtAddr};
use kernel64::paging::{Flags, PageSize};

use crate::arch::{self, mmu};
use crate::{interrupts, mm};

pub fn run() {
    frames();
    paging();
    protections();
    heap();
    kprintln!("[selftest] memory: all passed");
}

fn frames() {
    mm::with_memory(|_, frames| {
        let before = frames.free_frames();
        let a = frames.allocate().expect("no free frame");
        let b = frames.allocate().expect("no free frame");
        assert_ne!(a, b);
        frames.free(a);
        assert_eq!(frames.allocate(), Some(a), "a freed frame is reused first");
        frames.free(a);
        frames.free(b);
        assert_eq!(frames.free_frames(), before);
    });
    kprintln!("[selftest] frames: allocate, free, reuse");
}

/// An address nothing else uses.
const TEST_PAGE: u64 = 0xFFFF_D000_0000_0000;

fn paging() {
    mm::with_memory(|tables, frames| {
        let virt = VirtAddr::new(TEST_PAGE);
        let frame = frames.allocate().expect("no free frame");
        tables
            .map(virt, frame, PageSize::Size4K, Flags::WRITABLE | mm::no_execute(), frames)
            .expect("mapping the test page failed");

        // Write through the new mapping, read through the direct map:
        // the same physical memory, seen at two virtual addresses.
        let page = virt.as_mut_ptr::<u64>();
        let alias = phys_to_virt(frame).as_ptr::<u64>();
        // SAFETY: both point at the frame just mapped, which nothing
        // else uses.
        unsafe {
            page.write_volatile(0x5441_414A_0000_0001);
            assert_eq!(alias.read_volatile(), 0x5441_414A_0000_0001);
        }
        assert_eq!(tables.translate(virt).map(|t| t.physical), Some(frame));

        assert_eq!(tables.unmap(virt), Some(frame));
        mmu::flush(TEST_PAGE);
        assert_eq!(tables.translate(virt), None);
        frames.free(frame);
    });
    kprintln!("[selftest] paging: map, alias through the direct map, unmap");
}

/// An immutable static, so it lands in .rodata.
static READ_ONLY: u64 = 7;
/// A mutable one, in .data/.bss.
static WRITABLE: AtomicU64 = AtomicU64::new(0);

/// The kernel's own mappings have the permissions linker.ld intends.
fn protections() {
    mm::with_memory(|tables, _| {
        let flags = |address: u64| tables.translate(VirtAddr::new(address)).map(|t| t.flags);
        let nx = mm::no_execute();

        let code = flags(run as *const () as u64).expect("kernel code is unmapped");
        assert!(!code.contains(Flags::WRITABLE) && !code.contains(Flags::NO_EXECUTE), "code is {code:?}");

        let rodata = flags(&raw const READ_ONLY as u64).expect("rodata is unmapped");
        assert!(!rodata.contains(Flags::WRITABLE) && rodata.contains(nx), "rodata is {rodata:?}");

        WRITABLE.fetch_add(1, Ordering::Relaxed);
        let data = flags(&raw const WRITABLE as u64).expect("data is unmapped");
        assert!(data.contains(Flags::WRITABLE | nx), "data is {data:?}");

        assert_eq!(flags(0), None, "the null page must be unmapped");
        assert_eq!(flags(mm::kernel_layout().stack_guard), None, "the stack guard must be unmapped");
    });
    kprintln!("[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped");
}

fn heap() {
    let (used_before, total_before) = mm::heap_usage();
    {
        let boxed = Box::new(42u64);
        assert_eq!(*boxed, 42);

        // 4 MiB - well past the initial 1 MiB, so the heap has to grow.
        let numbers: Vec<u64> = (0..512 * 1024).collect();
        let sum: u64 = numbers.iter().sum();
        assert_eq!(sum, 512 * 1024 * (512 * 1024 - 1) / 2);

        let mut text = String::from("Hello");
        text.push_str(", heap");
        assert_eq!(format!("{text}!"), "Hello, heap!");

        let (_, total) = mm::heap_usage();
        assert!(total > total_before, "the heap did not grow");
        kprintln!("[selftest] heap: Box, Vec, String; grew from {} KiB to {} KiB", total_before / 1024, total / 1024);
    }
    let (used_after, _) = mm::heap_usage();
    assert_eq!(used_after, used_before, "dropping everything did not free everything");
}

/// The timer interrupt arrives, at the rate it was set to: ten ticks,
/// timed with the TSC (calibrated separately, against the PIT).
pub fn timer() {
    let tsc_hz = arch::platform::tsc_hz();
    let first = interrupts::ticks();
    // Line up with a tick boundary, then count ten.
    while interrupts::ticks() == first {
        arch::wait_for_interrupt();
    }
    let start_ticks = interrupts::ticks();
    let start = arch::tsc();
    while interrupts::ticks() < start_ticks + 10 {
        arch::wait_for_interrupt();
    }
    let ms = (arch::tsc() - start) * 1000 / tsc_hz.max(1);
    let expected = 10 * 1000 / interrupts::HZ;
    // Generous bounds: a VM's clocks can stretch under load.
    assert!(ms >= expected / 2 && ms <= expected * 3, "10 timer ticks took {ms} ms, expected {expected}");
    kprintln!("[selftest] timer: 10 ticks in {ms} ms (expected {expected})");
}
