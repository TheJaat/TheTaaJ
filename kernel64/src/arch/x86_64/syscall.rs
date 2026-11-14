//! Entering and leaving user mode.
//!
//! Into ring 3, the first time: `enter_user` builds the frame an
//! interrupt return expects - the user's SS, RSP, RFLAGS, CS, RIP - and
//! executes `iretq`, which "returns" to a place the thread has never been.
//!
//! Back into the kernel, deliberately: the `syscall` instruction. It is
//! fast because it does very little: RIP goes to RCX, RFLAGS to R11, CS
//! and SS come from the STAR register, RIP from LSTAR, and the flags in
//! FMASK are cleared (interrupts among them). It does NOT switch stacks -
//! RSP is still the user's, which the kernel must never trust or use. So
//! the entry stub's first act is to swap in the thread's kernel stack.
//!
//! Back to the user: `sysretq`, the mirror image - RCX to RIP, R11 to
//! RFLAGS, user CS and SS from STAR.

use core::arch::{asm, global_asm};

use super::gdt::{KERNEL_CODE, USER_CODE, USER_DATA};
use super::mmu::{read_msr, write_msr, EFER};

const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const FMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1 << 0; // system call extensions: syscall/sysret on

/// RFLAGS bits cleared on entry: TF (trap), IF (interrupts), DF
/// (direction - the ABI wants it clear), NT, AC (alignment checks).
const ENTRY_CLEARED_FLAGS: u64 = 0x100 | 0x200 | 0x400 | 0x4000 | 0x40000;

/// The kernel stack `syscall_entry` switches to: the running thread's,
/// set on every thread switch. One CPU, so one global; with SMP this
/// becomes per-CPU data reached through GS (and `swapgs`).
static mut KERNEL_RSP: u64 = 0;
/// Where the entry stub parks the user's RSP for the two instructions
/// before it is on the kernel stack. Safe on one CPU because interrupts
/// are off until it has been pushed.
static mut USER_RSP: u64 = 0;

/// The user's registers, as `syscall_entry` saves them (lowest address
/// first - the reverse of the pushes).
#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,    // from RCX
    pub rflags: u64, // from R11
    pub rsp: u64,
}

global_asm!(
    ".global syscall_entry",
    "syscall_entry:",
    "    mov [rip + {user_rsp}], rsp",
    "    mov rsp, [rip + {kernel_rsp}]",
    "    push qword ptr [rip + {user_rsp}]",
    "    push r11",
    "    push rcx",
    "    push rax",
    "    push rbx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push rbp",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    // 16 pushes: RSP is still 16-byte aligned for the call.
    "    mov rdi, rsp",
    "    call {handler}",
    // Interrupts off before RSP becomes the user's again: an interrupt
    // taken in ring 0 would push onto whatever RSP holds.
    "    cli",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rbp",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rbx",
    "    pop rax",
    "    pop rcx",
    "    pop r11",
    "    pop rsp",
    "    sysretq",
    user_rsp = sym USER_RSP,
    kernel_rsp = sym KERNEL_RSP,
    handler = sym syscall_handler,
);

extern "C" {
    fn syscall_entry();
}

/// RFLAGS a user may have: the arithmetic flags, direction, and
/// alignment check. Nothing that changes privilege (IOPL) or traps.
const USER_FLAGS: u64 = 0x8D5 | 0x400 | 0x40000; // CF PF AF ZF SF OF, DF, AC
const INTERRUPTS_ON: u64 = 0x200;

extern "C" fn syscall_handler(frame: &mut SyscallFrame) {
    let args = [frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9];
    frame.rax = crate::syscall::dispatch(frame.rax, args);

    // `sysretq` to a non-canonical address faults - in ring 0, but on the
    // user's stack (a known x86 hazard). Never attempt it.
    if frame.rip >= abi::USER_END {
        crate::process::kill_current("returned to a kernel or non-canonical address");
    }
    frame.rflags = (frame.rflags & USER_FLAGS) | INTERRUPTS_ON;
}

/// Turns on `syscall`/`sysret`.
///
/// # Safety
/// Once, on the boot CPU, after the GDT is loaded.
pub unsafe fn init() {
    write_msr(EFER, read_msr(EFER) | EFER_SCE);
    // STAR[47:32]: the kernel CS (SS is CS + 8). STAR[63:48]: a base from
    // which sysret takes user SS = base + 8 and user CS = base + 16. The
    // GDT order (user data, then user code) exists to fit this.
    let user_base = u64::from(USER_DATA) - 8;
    write_msr(STAR, (user_base << 48) | (u64::from(KERNEL_CODE) << 32));
    write_msr(LSTAR, syscall_entry as *const () as u64);
    write_msr(FMASK, ENTRY_CLEARED_FLAGS);
    const _: () = assert!(USER_DATA + 8 == USER_CODE, "sysret needs user code right after user data");
}

/// The kernel stack for the next `syscall` and for interrupts from ring 3.
pub fn set_kernel_stack(top: u64) {
    super::gdt::set_kernel_stack(top);
    // SAFETY: written with interrupts off on a thread switch; read only by
    // syscall_entry, which runs with interrupts off until it is done.
    unsafe { KERNEL_RSP = top };
}

/// Drops to ring 3 at `entry`, with stack pointer `stack` and two
/// arguments for `_start` in RDI and RSI. Every other general register
/// is zeroed first, so nothing of the kernel's leaks to the program.
///
/// # Safety
/// The current address space must map `entry` and the stack for user
/// mode, and `set_kernel_stack` must name this thread's kernel stack.
pub unsafe fn enter_user(entry: u64, stack: u64, first: u64, second: u64) -> ! {
    asm!(
        "push {ss}",
        "push {rsp}",
        "push {rflags}",
        "push {cs}",
        "push {rip}",
        "xor eax, eax",
        "xor ebx, ebx",
        "xor ecx, ecx",
        "xor edx, edx",
        "xor ebp, ebp",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",
        "xor r12d, r12d",
        "xor r13d, r13d",
        "xor r14d, r14d",
        "xor r15d, r15d",
        "iretq",
        ss = const USER_DATA as u64,
        cs = const USER_CODE as u64,
        rflags = const INTERRUPTS_ON | 0x2, // bit 1 always reads as 1
        rsp = in(reg) stack,
        rip = in(reg) entry,
        in("rdi") first,
        in("rsi") second,
        options(noreturn),
    )
}
