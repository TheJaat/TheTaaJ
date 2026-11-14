//! The GDT and TSS: the kernel's own, replacing the bootloader's.
//!
//! In 64-bit mode segmentation is almost gone - bases and limits are
//! ignored - but two things still depend on these tables:
//!
//!   - privilege: CS says whether the CPU runs kernel (ring 0) or user
//!     (ring 3) code, and SS must agree with it
//!   - the TSS: not a "task" any more, just a table of stack pointers the
//!     CPU switches to when an interrupt arrives. Its IST entries are what
//!     make a double fault survivable.

use core::arch::asm;
use core::mem::size_of;

/// Selectors - the byte offset of each descriptor in the GDT, plus the
/// requested privilege level in the low two bits.
///
/// The order is not arbitrary. `syscall`/`sysret`, which user mode will
/// use later, derive the user selectors from one base: user data must be
/// immediately followed by user code. Laying it out now saves reshuffling
/// the table later.
pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
#[allow(dead_code)] // used once user mode exists
pub const USER_DATA: u16 = 0x18 | 3;
#[allow(dead_code)]
pub const USER_CODE: u16 = 0x20 | 3;
pub const TSS: u16 = 0x28;

/// IST slot used by the double-fault handler (1-based, as the IDT wants).
pub const DOUBLE_FAULT_IST: u8 = 1;

// Descriptor bits. In 64-bit mode the only ones that matter for code and
// data are: present, privilege level, code/data, and L (64-bit code).
const PRESENT: u64 = 1 << 47;
const USER: u64 = 3 << 45; // DPL = 3
const CODE_OR_DATA: u64 = 1 << 44; // "S": not a system descriptor
const EXECUTABLE: u64 = 1 << 43;
const WRITABLE: u64 = 1 << 41; // for data; readable, for code
const LONG_MODE: u64 = 1 << 53; // L: 64-bit code

const fn code(user: bool) -> u64 {
    PRESENT | CODE_OR_DATA | EXECUTABLE | WRITABLE | LONG_MODE | if user { USER } else { 0 }
}

const fn data(user: bool) -> u64 {
    PRESENT | CODE_OR_DATA | WRITABLE | if user { USER } else { 0 }
}

/// The 64-bit TSS. `packed(4)` because the 64-bit fields start at
/// offset 4 - natural alignment would insert padding the CPU does not
/// expect.
#[repr(C, packed(4))]
struct TaskStateSegment {
    reserved0: u32,
    /// Stacks for entering rings 0-2 from a less privileged ring. Slot 0
    /// will hold each thread's kernel stack once user mode exists.
    privilege_stacks: [u64; 3],
    reserved1: u64,
    /// Interrupt Stack Table: an IDT entry may name one of these, and the
    /// CPU then switches to it unconditionally.
    interrupt_stacks: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    /// Offset of the I/O permission bitmap; past the end means "none".
    io_map_base: u16,
}

const _: () = assert!(size_of::<TaskStateSegment>() == 104);

const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stack([u8; DOUBLE_FAULT_STACK_SIZE]);

/// The stack a double fault runs on. A double fault often *means* the
/// normal stack is unusable - overflowed, or pointing at nothing - so
/// its handler needs one that is known to be good.
static mut DOUBLE_FAULT_STACK: Stack = Stack([0; DOUBLE_FAULT_STACK_SIZE]);

static mut TSS_SEGMENT: TaskStateSegment = TaskStateSegment {
    reserved0: 0,
    privilege_stacks: [0; 3],
    reserved1: 0,
    interrupt_stacks: [0; 7],
    reserved2: 0,
    reserved3: 0,
    io_map_base: size_of::<TaskStateSegment>() as u16,
};

/// Seven slots: null, four segments, and the TSS, which takes two
/// because it holds a full 64-bit base address.
static mut GDT: [u64; 7] = [
    0,
    code(false), // 0x08 kernel code
    data(false), // 0x10 kernel data
    data(true),  // 0x18 user data
    code(true),  // 0x20 user code
    0,           // 0x28 TSS, low half  - filled in by init()
    0,           // 0x30 TSS, high half
];

/// The operand of `lgdt` and `lidt`: a size and an address, packed.
#[repr(C, packed)]
pub struct TablePointer {
    pub limit: u16,
    pub base: u64,
}

/// Builds the TSS descriptor - the one entry whose contents depend on a
/// runtime address, which is why GDT starts with zeros there.
fn tss_descriptor(base: u64) -> (u64, u64) {
    let limit = (size_of::<TaskStateSegment>() - 1) as u64;
    let low = (limit & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | (0x9 << 40) // type 9: available 64-bit TSS
        | PRESENT
        | (((limit >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = base >> 32;
    (low, high)
}

/// Installs the GDT and TSS. Must run once, before interrupts are used.
///
/// # Safety
/// Called once, on the boot CPU, with interrupts disabled. It rewrites
/// mutable statics and reloads every segment register.
pub unsafe fn init() {
    // `&raw mut` takes an address without creating a Rust reference -
    // the right tool for statics the CPU itself reads and writes.
    let stack_top = (&raw mut DOUBLE_FAULT_STACK) as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
    let tss = &raw mut TSS_SEGMENT;
    (*tss).interrupt_stacks[(DOUBLE_FAULT_IST - 1) as usize] = stack_top;

    let gdt = &raw mut GDT;
    let (low, high) = tss_descriptor(tss as u64);
    (*gdt)[5] = low;
    (*gdt)[6] = high;

    let pointer = TablePointer { limit: (size_of::<[u64; 7]>() - 1) as u16, base: gdt as u64 };

    asm!(
        "lgdt [{pointer}]",
        // CS cannot be loaded with `mov`. A far return pops a new RIP and
        // CS, so push the two and "return" to the very next instruction.
        "push {code}",
        "lea {scratch}, [rip + 2f]",
        "push {scratch}",
        "retfq",
        "2:",
        // The data segment registers take ordinary moves.
        "mov ds, {data:x}",
        "mov es, {data:x}",
        "mov ss, {data:x}",
        "mov fs, {data:x}",
        "mov gs, {data:x}",
        // Load the task register, which is how the CPU finds the TSS.
        "ltr {tss:x}",
        pointer = in(reg) &pointer,
        code = const KERNEL_CODE as u64,
        scratch = out(reg) _,
        data = in(reg) KERNEL_DATA as u64,
        tss = in(reg) TSS as u64,
    );
}
