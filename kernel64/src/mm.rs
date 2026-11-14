//! Memory management, wired to the machine.
//!
//! `kernel64::memory` and `kernel64::paging` are the algorithms; this is
//! where they meet real RAM. At boot it:
//!
//!   1. places the frame bitmap, using the memory map
//!   2. builds the kernel's own page tables, replacing the loader's:
//!
//! ```text
//!        0x0000_0000_0000_0000  lower half: nothing (for user space).
//!                               Address 0 faults, so null pointers do.
//!        0xFFFF_8000_0000_0000  the direct map: all physical memory,
//!                               read/write, never executable
//!        0xFFFF_C000_0000_0000  the kernel heap, growing upward
//!        0xFFFF_D800_0000_0000  kernel thread stacks, each with a guard
//!        0xFFFF_E000_0000_0000  device registers (MMIO), uncached
//!        0xFFFF_FFFF_8000_0000  the kernel image: code read+execute,
//!                               rodata read-only, data read/write -
//!                               and an unmapped guard page below the
//!                               boot stack
//! ```
//!
//!   3. turns on NX and write protection, and switches to the new tables
//!   4. starts the heap, so `alloc` (Box, Vec, String) works
//!
//! Lock order, whenever more than one is held: HEAP, then TABLES, then
//! FRAMES. Taking them in one fixed order is what makes deadlock between
//! them impossible.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ops::Range;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use kernel64::bootinfo::{BootInfo, MEMORY_ACPI_NVS, MEMORY_ACPI_RECLAIMABLE};
use kernel64::memory::addr::{
    align_down, align_up, phys_to_virt, set_physical_offset, PhysAddr, VirtAddr, PAGE_SIZE,
};
use kernel64::memory::frame::FrameAllocator;
use heap::Heap;
use kernel64::memory::layout::find_free_space;
use kernel64::paging::{Flags, MapError, PageSize, PageTables};
use kernel64::acpi::PhysicalMemory;

use crate::arch::mmu;
use crate::irqlock::IrqLock;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Where the bootloader's kernel window and linker.ld put the kernel.
pub const KERNEL_WINDOW: u64 = 0xFFFF_FFFF_8000_0000;
pub const DIRECT_MAP: u64 = 0xFFFF_8000_0000_0000;
pub const HEAP_START: u64 = 0xFFFF_C000_0000_0000;
/// Kernel thread stacks, one slot each (see KernelStack).
pub const STACKS_START: u64 = 0xFFFF_D800_0000_0000;
/// Device registers, mapped uncached, one after another.
pub const MMIO_START: u64 = 0xFFFF_E000_0000_0000;
const MMIO_SIZE: u64 = GIB;
const HEAP_INITIAL: u64 = MIB;
const HEAP_GROWTH: u64 = MIB;
const HEAP_MAX: u64 = 256 * MIB;

pub static FRAMES: IrqLock<Option<FrameAllocator>> = IrqLock::new(None);
pub static TABLES: IrqLock<Option<PageTables>> = IrqLock::new(None);
static NO_EXECUTE: AtomicBool = AtomicBool::new(false);
/// Physical address of the kernel's PML4 - the address space kernel
/// threads run in.
static KERNEL_ROOT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn kernel_root() -> u64 {
    KERNEL_ROOT.load(Ordering::Relaxed)
}

/// Runs `f` with the frame allocator alone - for work on a process's
/// page tables, which are not the kernel's.
pub fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator) -> R) -> R {
    f(FRAMES.lock().as_mut().expect("memory not initialised"))
}

extern "C" {
    // linker.ld. Only their addresses mean anything.
    static __kernel_start: u8;
    static __text_end: u8;
    static __rodata_end: u8;
    static __kernel_end: u8;
    static __boot_stack_guard: u8;
}

/// The kernel's section boundaries, as virtual addresses.
pub struct KernelLayout {
    pub text: Range<u64>,
    pub rodata: Range<u64>,
    pub data: Range<u64>,
    pub stack_guard: u64,
}

pub fn kernel_layout() -> KernelLayout {
    let start = &raw const __kernel_start as u64;
    let text_end = &raw const __text_end as u64;
    let rodata_end = &raw const __rodata_end as u64;
    let end = &raw const __kernel_end as u64;
    KernelLayout {
        text: start..text_end,
        rodata: text_end..rodata_end,
        data: rodata_end..end,
        stack_guard: &raw const __boot_stack_guard as u64,
    }
}

/// NO_EXECUTE if this CPU supports it, nothing otherwise - so the same
/// mapping code works either way.
pub fn no_execute() -> Flags {
    if NO_EXECUTE.load(Ordering::Relaxed) {
        Flags::NO_EXECUTE
    } else {
        Flags::NONE
    }
}

/// Runs `f` with the kernel's page tables and frame allocator, taking
/// the locks in the documented order.
pub fn with_memory<R>(f: impl FnOnce(&mut PageTables, &mut FrameAllocator) -> R) -> R {
    let mut tables = TABLES.lock();
    let mut frames = FRAMES.lock();
    f(
        tables.as_mut().expect("memory not initialised"),
        frames.as_mut().expect("memory not initialised"),
    )
}

/// Maps `range` (physical) into the direct map with 2 MiB pages,
/// skipping 2 MiB pages already there.
fn map_direct(tables: &mut PageTables, frames: &mut FrameAllocator, range: Range<u64>) {
    let size = PageSize::Size2M.bytes();
    let flags = Flags::WRITABLE | Flags::GLOBAL | no_execute();
    let mut physical = align_down(range.start, size);
    while physical < align_up(range.end, size) {
        let virt = VirtAddr::new(DIRECT_MAP + physical);
        match tables.map(virt, PhysAddr::new(physical), PageSize::Size2M, flags, frames) {
            Ok(()) | Err(MapError::AlreadyMapped) => {}
            Err(error) => panic!("direct map of {physical:#x} failed: {error:?}"),
        }
        physical += size;
    }
}

/// Maps the kernel image pages in `range` with `flags`, leaving out the
/// stack guard page.
fn map_kernel(tables: &mut PageTables, frames: &mut FrameAllocator, range: Range<u64>, flags: Flags) {
    let guard = kernel_layout().stack_guard;
    for virt in (range.start..range.end).step_by(PAGE_SIZE as usize) {
        if virt == guard {
            continue;
        }
        let physical = PhysAddr::new(virt - KERNEL_WINDOW);
        tables
            .map(VirtAddr::new(virt), physical, PageSize::Size4K, flags | Flags::GLOBAL, frames)
            .unwrap_or_else(|error| panic!("mapping the kernel at {virt:#x} failed: {error:?}"));
    }
}

/// Sets up physical memory, the kernel's page tables and the heap.
///
/// `info` must be the boot info as read through the loader's identity
/// map, at physical `info_address`. Returns it re-read through the
/// direct map: the old reference is invalid once this returns.
pub fn init(info: &'static BootInfo, info_address: u64) -> &'static BootInfo {
    let regions = info.memory_map();
    let usable = || regions.iter().filter(|r| r.is_usable()).map(|r| r.base..r.base + r.length);

    // ---- 1. physical frames ---------------------------------------------

    let top = usable().map(|r| r.end).max().expect("the memory map has no usable memory");
    let frame_count = (top / PAGE_SIZE) as usize;
    let words = FrameAllocator::words_for(frame_count);
    let bitmap_bytes = (words * 8) as u64;

    let layout = kernel_layout();
    let kernel = layout.text.start - KERNEL_WINDOW..layout.data.end - KERNEL_WINDOW;
    let map_start = regions.as_ptr() as u64; // identity map: virtual == physical
    let reserved = [
        0..MIB, // real-mode IVT, BIOS data, the loader, its page tables
        kernel.clone(),
        info.ramdisk_address..info.ramdisk_address + info.ramdisk_size,
        info_address..info_address + size_of::<BootInfo>() as u64,
        map_start..map_start + size_of_val(regions) as u64,
    ];

    // Below 4 GiB: until the switch, only the loader's identity map can
    // reach it.
    let bitmap_address = find_free_space(usable(), &reserved, bitmap_bytes, PAGE_SIZE, 4 * GIB)
        .expect("no room for the frame bitmap");

    // SAFETY: the range is usable RAM that nothing else occupies (that is
    // what find_free_space checked), identity-mapped right now.
    let bitmap = unsafe { core::slice::from_raw_parts_mut(bitmap_address as *mut u64, words) };
    let mut frames = FrameAllocator::new(bitmap, frame_count);
    for range in usable() {
        frames.free_range(PhysAddr::new(range.start), PhysAddr::new(range.end));
    }
    // Firmware maps can overlap; anything not usable wins.
    for region in regions.iter().filter(|r| !r.is_usable()) {
        frames.reserve_range(PhysAddr::new(region.base), PhysAddr::new(region.base + region.length));
    }
    for range in reserved.iter().cloned().chain(core::iter::once(bitmap_address..bitmap_address + bitmap_bytes)) {
        frames.reserve_range(PhysAddr::new(range.start), PhysAddr::new(range.end));
    }
    kprintln!(
        "[memory] {} frames free of {} ({} MiB), bitmap {} KiB at {:#x}",
        frames.free_frames(),
        frames.total_frames(),
        frames.free_frames() as u64 * PAGE_SIZE / MIB,
        bitmap_bytes / 1024,
        bitmap_address
    );

    // ---- 2. the kernel's page tables -------------------------------------

    let nx = mmu::supports_no_execute();
    NO_EXECUTE.store(nx, Ordering::Relaxed);

    // SAFETY: frames come lowest-first, from RAM below 4 GiB, which the
    // identity map (offset 0) reaches.
    let mut tables = unsafe { PageTables::new(&mut frames, 0) }.expect("no frame for the PML4");

    map_direct(&mut tables, &mut frames, 0..4 * GIB);
    for region in regions {
        let interesting = region.is_usable()
            || region.kind == MEMORY_ACPI_RECLAIMABLE
            || region.kind == MEMORY_ACPI_NVS;
        if interesting {
            map_direct(&mut tables, &mut frames, region.base..region.base + region.length);
        }
    }
    if let Some(fb) = info.framebuffer() {
        let size = u64::from(fb.pitch) * u64::from(fb.height);
        map_direct(&mut tables, &mut frames, fb.address..fb.address + size);
    }

    map_kernel(&mut tables, &mut frames, layout.text.clone(), Flags::NONE);
    map_kernel(&mut tables, &mut frames, layout.rodata.clone(), no_execute());
    map_kernel(&mut tables, &mut frames, layout.data.clone(), Flags::WRITABLE | no_execute());

    // Every process shares the kernel half by copying these PML4 slots
    // once, so they must never change: give each one its PDPT now.
    tables.fill_kernel_slots(&mut frames).expect("no frames for the kernel's PDPTs");

    // ---- 3. switch -------------------------------------------------------

    let root = tables.root();
    KERNEL_ROOT.store(root.as_u64(), Ordering::Relaxed);
    // SAFETY: the new tables map the kernel's code, data and stack (the
    // boot stack is in .bss) and the direct map; nothing after the switch
    // uses an identity-mapped address - `regions` and `info` are dead.
    unsafe {
        mmu::enable_protection(nx);
        mmu::activate(root.as_u64());
        set_physical_offset(DIRECT_MAP);
        tables.set_offset(DIRECT_MAP);
        let bitmap = phys_to_virt(PhysAddr::new(bitmap_address)).as_mut_ptr::<u64>();
        frames.relocate(core::slice::from_raw_parts_mut(bitmap, words));
    }
    kprintln!(
        "[memory] own page tables at {:#x}: direct map {:#x}, NX {}, write protection on",
        root,
        DIRECT_MAP,
        if nx { "on" } else { "unsupported" }
    );

    *TABLES.lock() = Some(tables);
    *FRAMES.lock() = Some(frames);

    // ---- 4. the heap -----------------------------------------------------

    assert!(HEAP.state.lock().grow(HEAP_INITIAL), "no memory for the initial heap");
    kprintln!("[memory] heap at {HEAP_START:#x}, {} KiB to start", HEAP_INITIAL / 1024);

    // SAFETY: the boot info is below 1 MiB, now reached via the direct map.
    unsafe { BootInfo::from_physical(info_address) }.expect("boot info vanished")
}

// ---- the heap ---------------------------------------------------------

struct HeapState {
    heap: Heap,
    /// The first unmapped heap address.
    end: u64,
}

impl HeapState {
    /// Maps `bytes` more at the end of the heap and hands them to it.
    /// Returns whether any memory was added.
    fn grow(&mut self, bytes: u64) -> bool {
        let bytes = bytes.min(HEAP_START + HEAP_MAX - self.end);
        let mut mapped = 0;
        {
            let mut tables = TABLES.lock();
            let mut frames = FRAMES.lock();
            let (Some(tables), Some(frames)) = (tables.as_mut(), frames.as_mut()) else {
                return false; // allocating before init: no heap yet
            };
            let flags = Flags::WRITABLE | Flags::GLOBAL | no_execute();
            while mapped < bytes {
                let Some(frame) = frames.allocate() else { break };
                let virt = VirtAddr::new(self.end + mapped);
                tables
                    .map(virt, frame, PageSize::Size4K, flags, frames)
                    .unwrap_or_else(|error| panic!("heap mapping at {virt:#x} failed: {error:?}"));
                mapped += PAGE_SIZE;
            }
        }
        if mapped == 0 {
            return false;
        }
        // SAFETY: just mapped, writable, and used by nothing else.
        unsafe { self.heap.add_region(self.end as usize, mapped as usize) };
        self.end += mapped;
        true
    }
}

struct KernelHeap {
    state: IrqLock<HeapState>,
}

/// What `alloc` - Box, Vec, String, ... - uses for memory.
#[global_allocator]
static HEAP: KernelHeap =
    KernelHeap { state: IrqLock::new(HeapState { heap: Heap::empty(), end: HEAP_START }) };

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut state = self.state.lock();
        loop {
            if let Some(pointer) = state.heap.allocate(layout) {
                return pointer;
            }
            // Enough for this request even at its worst alignment.
            let needed = align_up(layout.size() as u64 + layout.align() as u64, HEAP_GROWTH);
            if !state.grow(needed) {
                return ptr::null_mut(); // `alloc` turns this into a panic
            }
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        self.state.lock().heap.deallocate(pointer, layout);
    }
}

/// (bytes in use, bytes the heap holds).
pub fn heap_usage() -> (usize, usize) {
    let state = HEAP.state.lock();
    (state.heap.used(), state.heap.total())
}

// ---- device memory ------------------------------------------------------

/// The next free address in the MMIO window. Taken before TABLES.
static MMIO_NEXT: IrqLock<u64> = IrqLock::new(MMIO_START);

/// Maps `size` bytes of device registers at physical `physical`,
/// uncached, and returns the virtual address of `physical`.
///
/// The direct map also covers most device memory, but with normal
/// (write-back) caching: a read could be answered from the cache instead
/// of the device, and writes could be delayed or merged. Registers need
/// every access to reach the device, in order.
pub fn map_mmio(physical: u64, size: u64) -> u64 {
    let start = align_down(physical, PAGE_SIZE);
    let end = align_up(physical + size, PAGE_SIZE);
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::GLOBAL | no_execute();

    let mut next = MMIO_NEXT.lock();
    let virt = *next;
    assert!(virt + (end - start) <= MMIO_START + MMIO_SIZE, "MMIO window full");
    with_memory(|tables, frames| {
        for offset in (0..end - start).step_by(PAGE_SIZE as usize) {
            tables
                .map(VirtAddr::new(virt + offset), PhysAddr::new(start + offset), PageSize::Size4K, flags, frames)
                .unwrap_or_else(|error| panic!("MMIO mapping of {physical:#x} failed: {error:?}"));
        }
    });
    *next += end - start;
    virt + (physical - start)
}

/// Physical memory as the ACPI parser reads it: through the direct map,
/// refusing any range the direct map does not cover - firmware tables
/// can point anywhere, and a bad pointer must be an error, not a fault.
pub struct DirectMap;

impl PhysicalMemory for DirectMap {
    fn read(&self, address: u64, buffer: &mut [u8]) -> bool {
        if buffer.is_empty() {
            return true;
        }
        let Some(end) = address.checked_add(buffer.len() as u64) else { return false };
        let mapped = with_memory(|tables, _| {
            (align_down(address, PAGE_SIZE)..end)
                .step_by(PAGE_SIZE as usize)
                .all(|page| tables.translate(phys_to_virt(PhysAddr::new(page))).is_some())
        });
        if mapped {
            // SAFETY: every page of the range is mapped, checked above.
            let source = phys_to_virt(PhysAddr::new(address)).as_ptr::<u8>();
            unsafe { core::ptr::copy_nonoverlapping(source, buffer.as_mut_ptr(), buffer.len()) };
        }
        mapped
    }
}

// ---- kernel stacks ------------------------------------------------------

/// Each thread's stack lives in its own 64 KiB slot: the lowest page is
/// left unmapped as a guard, the other 15 pages (60 KiB) are the stack.
/// Overflowing the stack touches the guard page and faults at once,
/// instead of silently overwriting the slot below - another thread's
/// stack.
const STACK_SLOT: u64 = 64 * 1024;
const STACK_SLOTS: u64 = 16 * 1024; // 1 GiB of address space

struct Slots {
    next: u64,
    free: alloc::vec::Vec<u64>,
}

/// Taken before TABLES.
static SLOTS: IrqLock<Slots> = IrqLock::new(Slots { next: 0, free: alloc::vec::Vec::new() });

/// A mapped kernel stack. Dropping it unmaps the pages and frees the
/// frames.
pub struct KernelStack {
    slot: u64,
}

impl KernelStack {
    fn base(&self) -> u64 {
        STACKS_START + self.slot * STACK_SLOT
    }

    /// The highest address: stacks grow down from here.
    pub fn top(&self) -> u64 {
        self.base() + STACK_SLOT
    }
}

pub fn allocate_stack() -> Option<KernelStack> {
    let mut slots = SLOTS.lock();
    let slot = match slots.free.pop() {
        Some(slot) => slot,
        None if slots.next < STACK_SLOTS => {
            slots.next += 1;
            slots.next - 1
        }
        None => return None,
    };
    let stack = KernelStack { slot };
    let flags = Flags::WRITABLE | Flags::GLOBAL | no_execute();
    let mapped = with_memory(|tables, frames| {
        for page in (stack.base() + PAGE_SIZE..stack.top()).step_by(PAGE_SIZE as usize) {
            let Some(frame) = frames.allocate() else { return false };
            tables
                .map(VirtAddr::new(page), frame, PageSize::Size4K, flags, frames)
                .unwrap_or_else(|error| panic!("stack mapping at {page:#x} failed: {error:?}"));
        }
        true
    });
    drop(slots);
    // On failure, dropping `stack` unmaps whatever was mapped.
    mapped.then_some(stack)
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let mut slots = SLOTS.lock();
        with_memory(|tables, frames| {
            for page in (self.base() + PAGE_SIZE..self.top()).step_by(PAGE_SIZE as usize) {
                if let Some(frame) = tables.unmap(VirtAddr::new(page)) {
                    mmu::flush(page);
                    frames.free(frame);
                }
            }
        });
        slots.free.push(self.slot);
    }
}

/// Whether `address` is in the guard page of some thread's stack.
pub fn is_stack_guard(address: u64) -> bool {
    (STACKS_START..STACKS_START + STACK_SLOTS * STACK_SLOT).contains(&address)
        && (address - STACKS_START) % STACK_SLOT < PAGE_SIZE
}

/// How many stack slots have ever been used - stays low if stacks are
/// recycled.
pub fn stack_slots_used() -> u64 {
    SLOTS.lock().next
}
