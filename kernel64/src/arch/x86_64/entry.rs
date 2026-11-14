//! `_start` - the very first instruction of the kernel.
//!
//! The bootloader jumps here in 64-bit mode with RDI = the TaajBootInfo
//! and RSP pointing at a scrap of its own low memory. Before any Rust
//! runs, the kernel needs a stack it owns. That cannot be written in
//! Rust - Rust code assumes a valid stack from its first instruction -
//! so these few lines are assembly.

use core::arch::global_asm;

const STACK_SIZE: usize = 64 * 1024;

/// The boot stack: 64 KiB in .bss, which costs nothing in the kernel
/// file. 16-byte aligned, as the x86_64 calling convention requires.
#[repr(C, align(16))]
struct Stack([u8; STACK_SIZE]);

/// `static mut` because the CPU writes to it behind Rust's back. Rust
/// code never touches it, so none of the usual hazards of a mutable
/// static apply; only the assembly below names it.
///
/// Its own section, so linker.ld can put the guard page right below it.
#[link_section = ".bss.boot_stack"]
static mut BOOT_STACK: Stack = Stack([0; STACK_SIZE]);

global_asm!(
    // In its own section so linker.ld can place it first.
    ".section .text.entry, \"ax\"",
    ".global _start",
    "_start:",
    // Stacks grow down, so RSP starts at the END of the array.
    // RIP-relative: correct wherever the kernel is mapped.
    "    lea rsp, [rip + {stack} + {size}]",
    // A zero frame pointer marks the bottom of the call chain, for
    // anything that later walks the stack.
    "    xor ebp, ebp",
    // RDI still holds the boot info address - the first argument.
    "    call {main}",
    // kernel_main never returns; if it somehow did, stop here.
    "2:  cli",
    "    hlt",
    "    jmp 2b",
    stack = sym BOOT_STACK,
    size = const STACK_SIZE,
    main = sym crate::kernel_main,
);
