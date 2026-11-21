//! Checks what the kernel does with a program's registers.
//!
//! **At the start** they must hold nothing: the two arguments, and
//! zero everywhere else. Whatever else were there would be the
//! kernel's - a number left over from the thread's life before it
//! became this program - and none of a program's business.
//!
//! **After that** they must be the program's own. It enters the
//! kernel two ways: by asking (a system call) and without asking (an
//! interrupt, wherever it happens to be). Both times the kernel saves
//! every register the program has, uses them for its own work, and
//! must put each one back - or the program goes on with a wrong number
//! somewhere, and fails much later, somewhere else. So: a known value
//! in every register, then into the kernel both ways, then count the
//! registers that hold something else. The stack pointer too - a
//! program's is a register of its own on AArch64 (`SP_EL0`), saved
//! and restored apart from the rest.
//!
//! AArch64 only, for now. On x86_64 this would need its own assembly;
//! there the `gs` program checks the one thing most likely to go
//! wrong on that CPU's way in and out.

#![no_std]
#![no_main]

#[cfg(not(target_arch = "aarch64"))]
taaj::program!(main);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    r#"
    // Where the kernel starts this program - not the usual `_start`
    // (taaj::program!), which is Rust and has used half the registers
    // before its first line. Counts the registers other than the two
    // arguments that are not zero, adds 100 if the stack pointer is
    // not a multiple of 16 (the calling convention's rule), and goes
    // on to Rust with the count as a third argument.
    .section .text._start, "ax"
    .global _start
_start:
    cmp     x2, #0
    cset    x2, ne
    .irp    n, 3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
    cmp     x\n, #0
    cinc    x2, x2, ne
    .endr
    mov     x3, sp
    tst     x3, #15
    b.eq    1f
    add     x2, x2, #100
1:
    b       regs_start

    // u64 regs_round_trip(u64 ticks_call, u64 wait)
    //
    // Register n gets 0xA00 + n, for n = 9 to 30 except the three the
    // loop needs (19, 20 and 21: the call number, the tick to wait
    // for, and the stack pointer as it was). Then system calls - "what
    // time is it?" - one after another until `wait` more ticks have
    // passed, so that timer interrupts arrive in between as well.
    // Returns how many registers changed, plus 100 if the stack
    // pointer did.
    //
    // The system call's own registers (x0 to x8) are loaded with known
    // values before every call and checked after it: the kernel
    // promises to change x0 and nothing else.
    .section .text.regs_round_trip, "ax"
    .global regs_round_trip
regs_round_trip:
    stp     x29, x30, [sp, #-96]!
    stp     x19, x20, [sp, #16]
    stp     x21, x22, [sp, #32]
    stp     x23, x24, [sp, #48]
    stp     x25, x26, [sp, #64]
    stp     x27, x28, [sp, #80]
    mov     x19, x0                 // the call number
    mov     x22, x1
    mov     x21, sp
    mov     x8, x19
    svc     #0
    add     x20, x0, x22            // the tick to wait for
    .irp    n, 9,10,11,12,13,14,15,16,17,18,22,23,24,25,26,27,28,29,30
    mov     x\n, #(0xA00 + \n)
    .endr
    mov     x9, #0                  // x9 counts what the calls changed
2:
    .irp    n, 1,2,3,4,5,6,7
    mov     x\n, #(0xB00 + \n)
    .endr
    mov     x8, x19
    svc     #0
    .irp    n, 1,2,3,4,5,6,7
    cmp     x\n, #(0xB00 + \n)
    cinc    x9, x9, ne
    .endr
    cmp     x8, x19
    cinc    x9, x9, ne
    cmp     x0, x20
    b.lo    2b
    mov     x0, x9
    .irp    n, 10,11,12,13,14,15,16,17,18,22,23,24,25,26,27,28,29,30
    cmp     x\n, #(0xA00 + \n)
    cinc    x0, x0, ne
    .endr
    mov     x1, sp
    cmp     x1, x21
    b.eq    3f
    add     x0, x0, #100
    mov     sp, x21
3:
    ldp     x27, x28, [sp, #80]
    ldp     x25, x26, [sp, #64]
    ldp     x23, x24, [sp, #48]
    ldp     x21, x22, [sp, #32]
    ldp     x19, x20, [sp, #16]
    ldp     x29, x30, [sp], #96
    ret
"#
);

#[cfg(target_arch = "aarch64")]
extern "C" {
    fn regs_round_trip(ticks_call: u64, wait: u64) -> u64;
}

/// The rest of what `taaj::program!` would have done.
#[cfg(target_arch = "aarch64")]
#[no_mangle]
extern "C" fn regs_start(startup_handles: u64, argument: u64, not_fresh: u64) -> ! {
    taaj::set_startup(startup_handles, argument);
    taaj::exit(main(not_fresh))
}

#[cfg(target_arch = "aarch64")]
fn main(not_fresh: u64) -> i64 {
    if not_fresh != 0 {
        taaj::println!("[regs] FAILED at the start: {not_fresh} (registers that were not zero, plus 100 for a misaligned stack pointer)");
        return 2;
    }
    taaj::println!("[regs] at the start: 29 registers zero, the stack pointer a multiple of 16");
    // SAFETY: the function keeps to the calling convention.
    let changed = unsafe { regs_round_trip(taaj::call::TICKS, 5) };
    if changed == 0 {
        taaj::println!("[regs] 26 registers and the stack pointer, through system calls and timer interrupts for 5 ticks: none changed");
        0
    } else {
        taaj::println!("[regs] FAILED: {changed} (the number of registers that changed, plus 100 for the stack pointer)");
        1
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn main() -> i64 {
    taaj::println!("[regs] not written for this CPU yet");
    0
}
