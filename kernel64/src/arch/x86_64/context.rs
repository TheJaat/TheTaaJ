//! Switching the CPU from one thread to another.
//!
//! A thread, to the CPU, is just its registers. Most of them need no
//! saving here: `switch_context` is an ordinary function call, and the
//! calling convention already lets it destroy the "caller-saved"
//! registers (RAX, RCX, RDX, RSI, RDI, R8-R11) - the caller saved
//! anything it needed in them. What a callee must preserve are the six
//! "callee-saved" registers and the stack pointer. So a switch is:
//!
//!   push the six onto the old thread's stack
//!   store RSP into the old thread's record
//!   load RSP from the new thread's record      <- now on the new stack
//!   pop the six the new thread pushed when it was switched out
//!   ret                                         <- into the new thread
//!
//! That `ret` returns to wherever the new thread called switch_context
//! from - the new thread's own earlier call. Every thread is "inside"
//! switch_context while it is not running.
//!
//! A brand-new thread was never inside it, so `initial_stack` fakes the
//! frame: six zeros and a return address of `thread_trampoline`.

use core::arch::global_asm;

global_asm!(
    ".global switch_context",
    "switch_context:", // (save: *mut u64 = rdi, load: u64 = rsi)
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov [rdi], rsp",
    "    mov rsp, rsi",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    ret",
    "",
    // Where a new thread's first `ret` lands. RBP = 0 ends the chain of
    // frames for anything walking the stack; the ABI wants RSP 16-byte
    // aligned at a call.
    ".global thread_trampoline",
    "thread_trampoline:",
    "    xor ebp, ebp",
    "    and rsp, -16",
    "    call {entry}",
    "    ud2", // thread_entry never returns
    entry = sym crate::threads::thread_entry,
);

extern "C" {
    fn switch_context(save: *mut u64, load: u64);
    static thread_trampoline: u8;
}

/// Saves the current thread's stack pointer to `*save` and resumes the
/// thread whose stack pointer is `load`. Returns when something switches
/// back to this thread.
///
/// # Safety
/// Interrupts disabled; `save` valid for writing; `load` a stack pointer
/// saved by this function or made by `initial_stack`, for a thread that
/// is not running.
pub unsafe fn switch(save: *mut u64, load: u64) {
    switch_context(save, load);
}

/// Prepares a fresh stack ending at `top` so that switching to it starts
/// `thread_trampoline`. Returns the stack pointer to switch to.
///
/// # Safety
/// `top` must be the 16-byte aligned end of a mapped, writable stack.
pub unsafe fn initial_stack(top: u64) -> u64 {
    let stack = top as *mut u64;
    stack.sub(1).write(0); // padding: keeps the frame 16-byte shaped
    stack.sub(2).write(&raw const thread_trampoline as u64); // ret target
    for slot in 3..=8 {
        stack.sub(slot).write(0); // rbp, rbx, r12-r15
    }
    top - 8 * 8
}
