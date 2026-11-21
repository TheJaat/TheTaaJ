# Programs on AArch64

*Journal entry #56 · Era 3, step 3f. The AArch64 kernel runs programs: each in its own address space, at exception level 0, asking for things with `svc`. The `user/` crate compiles for both CPUs with one function different, and the four self-tests of programs - user mode, IPC, files, kill - moved out of the PC-only file and run on both.*

```
[hello] hello from user mode! I am process 1, running at exception level 0
[peek] reading the kernel's code at 0xffffffff80100000...
[process] data abort in a program at 0x400030 in user mode
[process]   user read at 0xffffffff80100000: protection violation (at level 3)
[process] 2 "peek" killed: data abort in a program
[regs] at the start: 29 registers zero, the stack pointer a multiple of 16
[regs] 26 registers and the stack pointer, through system calls and timer interrupts for 5 ticks: none changed
[selftest] user: hello exited 42, spin, heapcheck and regs exited 0, peek, cli and memro were killed; 14 processes, nothing leaked
[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left
[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left
[selftest] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked
```

---

## Why

Entry 55 ended with threads, and with `process.rs`, `ipc.rs` and `syscall.rs` already compiling for AArch64 - against two stubs. `enter_user` panicked, and `mmu::activate` refused any root but the kernel's. Everything a program needs from the kernel above the CPU was there; what was missing was the CPU.

On the PC that part took all of entry 8 and came back in entry 45: a GDT with user segments, a TSS to say where the kernel's stack is, three MSRs to set up `syscall`, `swapgs` on every way in and out. I wanted to see what the same job looks like on a CPU designed thirty years later.

## What a program needs from a CPU

Four things, and the table is most of this entry:

| | x86_64 | AArch64 |
|---|---|---|
| A less privileged mode | ring 3: two bits in the CS register, so segments must exist (GDT) | exception level 0: two bits in the state register |
| Its own memory | one root (CR3); the kernel's half is *copied into* every process's top table | two roots: `TTBR0` for the low half, `TTBR1` for the high. A process's tree has no kernel in it |
| A way to ask | `syscall`: jumps to an address in an MSR, switches nothing | `svc`: an exception like any other, through the vector table |
| A kernel stack when it arrives | from the TSS (interrupts) or by hand through GS (`syscall`) | the CPU has a stack pointer *per level*: at level 1, `sp` is simply a different register |

The right column has no entry where the left has "by hand". That is the whole difference. The AArch64 side of this step is three short pieces of assembly - a dozen lines to enter a program, four to save its stack pointer, a dozen to change address space - where the PC needed a GDT, a TSS, three MSRs and `swapgs`.

## Going in: `eret` to a place never left

There is no instruction for "start a program". There is only "return from an exception" - `eret` - which loads the program counter from `ELR_EL1` and the state from `SPSR_EL1`. So the first entry into a program is a return from an exception that never happened. The PC does the same with `iretq`.

I already had code that returns from exceptions: the tail of `exception_common` in `vectors.rs`, which pops a 272-byte `Frame` (31 registers, the return address, the state, the stack pointer) and ends in `eret`. So `enter_user` does not get its own copy. It *makes up a frame* and jumps to that tail:

```asm
enter_user_frame:               // (top, entry, stack, first, second)
    msr     daifset, #0xf       // nothing may arrive half-way through
    sub     x0, x0, #272
    mov     sp, x0              // the very top of this thread's kernel stack
    mov     x9, #0
1:  str     xzr, [x0, x9]       // the whole frame: zero
    add     x9, x9, #8
    cmp     x9, #272
    b.lo    1b
    stp     x3, x4, [sp]        // x0 and x1: the two arguments
    str     x1, [sp, #248]      // elr: the entry point
    str     x2, [sp, #264]      // sp: the program's stack
    b       exception_return
```

State zero means "level 0, 64-bit, interrupts on" - the one state a program should have, and conveniently all zeroes. Every register but the two arguments comes out of the frame as zero, so nothing of the kernel's reaches the program.

Two things about that code I had to think through:

**It is assembly because of the stack.** I first wanted to write the frame from Rust: `(top - 272) as *mut Frame`. But the code doing so *is running on that stack*, not far below the top. The thread is only a few calls deep, and writing 272 bytes there could land on the frame of the function doing the writing. Assembly moves `sp` first and touches only registers until it has.

**Where the kernel stack pointer comes from.** On x86 the kernel tells the CPU where the kernel stack is on every switch (`set_kernel_stack` writes the TSS and the per-CPU block). On AArch64 there is nothing to tell. `SP_EL1` is a real register that stays where the kernel left it; when the program takes an exception, the CPU is at level 1 and `sp` *means* `SP_EL1` again. Setting it to the top of the stack in `enter_user_frame`, once, is the whole mechanism. `set_kernel_stack` on AArch64 just remembers the number for that one use.

## Coming back: one more register in the frame

A program's exceptions arrive at vector slots 8 to 11 ("from a 64-bit program"). They use the same `exception_common` as everything else, with one addition. The frame has a slot for "the stack pointer of whoever was interrupted". For the kernel that is `sp` before the frame was pushed. For a program it is the *other* register:

```asm
    add     x3, sp, #272        // the kernel's stack pointer, before
    tst     x2, #0xf            // x2 = saved state; low bits 0: a program
    b.ne    1f
    mrs     x3, sp_el0          // whose stack pointer is this one
1:  stp     x2, x3, [sp, #256]
```

and the mirror image on the way out (`msr sp_el0, x3`).

That is also all it takes for each thread to have its own program stack. I expected to have to save `SP_EL0` in the context switch. It is not needed: a thread only ever leaves the CPU from *inside* the kernel, and by then its program's stack pointer is already in its frame, on its own kernel stack.

## `svc`

In `exception()`, vector 8 with syndrome class `0x15`:

```rust
if syndrome.class() == Syndrome::SYSTEM_CALL {
    let args = [frame.x[0], frame.x[1], frame.x[2], frame.x[3], frame.x[4], frame.x[5]];
    frame.x[0] = crate::syscall::dispatch(frame.x[8], args);
    return;
}
```

`dispatch` is the same function the PC calls. The convention is mine to choose, and I chose what everyone uses on this CPU:

```
             instruction   number   arguments                  result
  x86_64     syscall       RAX      RDI RSI RDX R10 R8 R9      RAX
  AArch64    svc #0        x8       x0  x1  x2  x3  x4 x5      x0
```

Three things x86 needs here and this does not:

- No clobbered registers. `syscall` itself destroys RCX and R11; `svc` changes nothing, and the kernel returns every register but x0 as it found it.
- No fix-up of the return address. For a breakpoint the saved address is the `brk` itself (entry 50 had to add 4). For `svc` it is already the next instruction.
- No check that the return address is sane. `sysretq` to a bad address faults *in ring 0* on the user's stack, so the PC must check (entry 8). `eret` to a bad address just faults in the program.

On the user side, `taaj::syscall6` is the only function with two bodies:

```rust
#[cfg(target_arch = "aarch64")]
pub fn syscall6(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    let result: u64;
    unsafe {
        asm!("svc #0",
             in("x8") number,
             inlateout("x0") a => result,
             in("x1") b, in("x2") c, in("x3") d, in("x4") e, in("x5") f,
             options(nostack));
    }
    result
}
```

`#[cfg(target_arch = "…")]` is how Rust says "compile this item only for that CPU". Two functions with the same name can sit side by side as long as their conditions never both hold. `inlateout("x0") a => result` says: x0 carries `a` in and `result` out, and the output is written only after all inputs are read.

## A fault in a program ends the program

Anything else at vector 8 means the program broke a rule:

```rust
let name = syndrome.class_name();
kprintln!("[process] {name} at {:#x} in user mode", frame.elr);
if let Some(abort) = syndrome.abort() {
    kprintln!("[process]   {abort} at {:#x}: {}", fault_address(), abort.cause);
}
crate::process::kill_current(name);
```

The decoding of the syndrome register was written in entry 50 and already knew the "from a program" classes; this is their first use. The three rule-breakers each get a different answer from the CPU:

| Program | What it does | What the CPU says |
|---|---|---|
| `peek` | reads kernel memory | data abort, *protection violation*: the page is there, but not for level 0 |
| `memro` | writes to a read-only mapping | data abort, protection violation, `user write` |
| `cli` | `msr daifset, #2` - masks interrupts | *trapped system register access* |
| `echod` (asked to crash) | reads address 0 | data abort, *not mapped* |

`cli` is the same program as on the PC with one line under `cfg`: the privileged instruction is `cli` there and `msr daifset` here.

And the interrupt path gained the line the PC has had since entry 17: after an interrupt that arrived *in a program*, `stop_if_killed()`. A program that never makes a system call still passes there on every tick.

## The second root

`mmu::activate(root)` was a stub that asserted. Now:

```asm
    mrs  t, tcr_el1
    orr  t, t, #(1 << 7)      // EPD0: look nothing up through TTBR0
    msr  tcr_el1, t
    isb
    tlbi vmalle1              // forget every remembered translation
    dsb  nsh
    isb
    cbnz off, 2f              // the kernel's own root: leave the low half off
    msr  ttbr0_el1, root
    bic  t, t, #(1 << 7)
    msr  tcr_el1, t
    isb
2:
```

The order is the one from entry 53: off, forget, new tree, on. There is never a moment when the CPU could hold one process's translation while walking another's tables.

What did *not* need writing is the interesting part. `PageTables::new_user` copies the kernel's half into each new tree - on x86. For AArch64 it was already written in entry 52 to copy nothing, because the kernel's half lives in the other register and never moves. A kernel thread (no process) simply runs with the low half switched off, so a stray null pointer in the kernel faults whichever thread it happens in.

Forgetting *everything* on each switch is the plain way. The better way is an ASID - a small number per process that tags what the CPU remembers, so nothing needs forgetting. Not yet.

## A thing x86 does for you: two caches

Loading a program means writing its instructions into fresh pages as data, then running them. An x86 CPU notices. An AArch64 CPU has separate data and instruction caches and does **not** keep them in step: the bytes may still be in the data cache, and the instruction cache may hold whatever those addresses had before.

So the loader now calls `mmu::code_written(page, length)` after filling any executable page. On x86 it is empty. On AArch64:

```rust
// each line of the data cache that was written: push it out
asm!("dc cvau, {}", in(reg) address);
// then: empty the instruction cache, on every CPU
asm!("dsb ish", "ic ialluis", "dsb ish", "isb");
```

The line size comes from `CTR_EL0`. I empty the whole instruction cache instead of line by line because the code was written through the direct map and will run at a different address, and on some caches invalidating by one address does not cover the other.

I cannot test this. QEMU has no caches; a kernel without the call runs every program. I am writing it down as untested rather than leaving it out.

## Building programs for two CPUs

```
user/.cargo/config.toml     default target x86_64-unknown-none; flags for both
user/target/x86_64-unknown-none/release/…            the PC's programs
user/target/aarch64-unknown-none-softfloat/release/… the same sources, again
```

`make user-aarch64` builds twelve programs by name: `hello peek cli spin echod echoc fs fscheck memro heapcheck forever regs`. By name, because five programs are for the PC only and would not compile: `kbd`, `port`, `blk` (I/O ports), `gs` (a segment register), and through them `init`, `shell`, `display` and `registry` have nothing to run with yet. In the library, the `port` module is under `cfg(target_arch = "x86_64")`.

"softfloat" matters: the kernel leaves the floating-point registers switched off (and does not save them on a switch), so a program that touched one would be ended for it. The softfloat target never does.

`kernel64/src/programs.rs` picks the directory by `cfg`, and holds a new table, `USER_CHECK`: which programs the user-mode self-test runs on this CPU and how each must end. `elf::parse` now checks the file's machine number against the kernel's own (`EM_X86_64` 62, `EM_AARCH64` 183), so each kernel refuses the other's programs with `WrongMachine` instead of running nonsense.

With that, `user()`, `ipc()`, `files()` and `kill()` moved from `selftest/system.rs` (PC only) to `selftest/user.rs` (shared), unchanged but for reading the list from `USER_CHECK`. All four passed the first time the kernel booted with them. `system.rs` is left with the two tests of several CPUs.

## `regs`: checking what cannot be seen

Two properties of this step are invisible when they work.

**That a program starts with nothing of the kernel's.** A normal `_start` is Rust and has used half the registers before its first line. So `regs` has its own, in assembly, that counts the registers other than x0 and x1 which are not zero, and checks the stack pointer is a multiple of 16.

**That a program gets its registers back.** The kernel-side checks of the last few entries put a known value in every register and take an exception - *from the kernel*. A program's exceptions use different vector slots and one more saved register. So the same idea, from the other side: register *n* gets `0xA00 + n`, then system calls in a loop until five ticks have passed (so interrupts arrive in between too), then count what changed. Around each `svc`, x1 to x8 are loaded and compared as well: the kernel promises to change x0 only.

It is an AArch64-only program for now; on the PC it prints that it is not written for that CPU and exits 0. The PC's equivalent risk is what `gs` covers.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| First entry to a program | make up a frame, reuse the exception return path | a separate `eret` sequence: a second copy of "restore everything", which would drift from the first |
| Where `SP_EL0` is saved | in the exception frame | in the context switch: works, but saves it on every switch for threads that have no program |
| System call registers | x8 number, x0-x5 arguments, x0 result | the x86 layout mapped onto x-registers: no reason to differ from what every tool on this CPU expects |
| Address-space switch | flush everything | ASIDs: right, but a second mechanism to get wrong before the first is proven |
| Kernel threads' low half | switched off (EPD0) | left pointing at the last process: a stray kernel pointer would then quietly read some program's memory |
| After writing code | `dc cvau` per line, then `ic ialluis` | `ic ivau` per line: not enough when the code runs at another address than it was written through |
| Programs for both CPUs | one crate, built twice, `cfg` on one function | a second crate for AArch64: two copies of every wrapper |
| Which self-tests run | a per-CPU table in `programs.rs` | `cfg` inside the test: the test would grow a branch per CPU |
| Program stack alignment check | on (`SCTLR_EL1.SA0`) | off: a misaligned stack would only show as a strange fault much later |
| Initial stack pointer | top - 16 on AArch64, top - 8 on x86 (`ENTRY_STACK_GAP`) | one number: x86 wants "as if just called" (a return address pushed), AArch64 wants a multiple of 16 |

## Bugs

None in the kernel path this time that a boot showed me - the first boot with programs ran all four self-tests. What the mutation runs showed is in the next section: two properties nothing was checking.

The bug was in my tests. The first full `make test64` failed on every AArch64 boot - with a kernel that worked. The boot script expected `kernel_main at 0xffffffff8008…`, a write fault `at 0xffffffff800…`, and "all but 1 to 3 MiB free". All three were true only while the kernel was small. With twelve programs inside it the image is 1.2 MiB, and the addresses and the free count moved. The patterns now say what I meant (an address in the kernel's image; all but a few MiB) instead of what happened to be printed that day.

One near miss worth recording is the one above: writing the made-up frame from Rust onto the stack the code was running on. I caught it before running it, by asking where `top - 272` was relative to `sp`.

## How I checked it

**Automated** (`make test64`, which ends `test64: all passed`):

- Host: 167 tests in the kernel library. New: `a_program_is_only_for_the_machine_asked_for` (each kernel takes its own ELF machine number and refuses the other's and a 32-bit PC's); `control_values` now expects the program stack alignment bit.
- Every AArch64 boot that has interrupts (twelve of the thirteen; `nogic` has no threads, so no programs) must print the lines at the top of this entry, plus the `cli` and `memro` kills and both `regs` lines.
- The PC boots run the same four self-tests from their new file; their output is unchanged.

**By hand** - eight deliberate breakages, each built and booted:

| Change | Result |
|---|---|
| do not restore `SP_EL0` on the way out | caught: programs do not end as expected |
| do not save `SP_EL0` on the way in | caught: `[regs] FAILED: 100` (the stack pointer changed) |
| system call result put in x1 | caught: `hello` exits 1 |
| no `stop_if_killed` after an interrupt | caught: the kill test never finishes |
| initial stack pointer at top - 8 | **passed at first** - QEMU does not check alignment. `regs` now checks it at `_start`: caught |
| registers not zeroed at entry | **passed at first.** `regs` now counts them: caught, *18 registers* held kernel values |
| no `tlbi` in `activate` | passes: QEMU forgets translations by itself when `TCR` is written |
| `code_written` does nothing | passes: QEMU has no caches |

The two in bold are why `regs` has a `_start` of its own. The last two I cannot catch on an emulator.

**Not tested:** real hardware (so: the cache step, the flush, the alignment trap as the CPU itself would raise it); a program using floating point (it should be killed with "floating point used while it is switched off" - I have not written the program); 32-bit programs (slots 12-15 stay fatal; nothing can enter that state).

## What is still missing

- **No init, shell or disk on AArch64.** The programs that need devices need virtio over MMIO and the PL011 as input: step 3g.
- **No ASIDs.** Every switch between processes forgets every translation.
- **No floating point** for programs on either CPU.
- **No thread pointer.** `TPIDR_EL0`, the register a program keeps its thread-local data in, is neither saved nor cleared. Nothing uses it yet. (The kernel borrows it in the stack-overflow path, which never returns.)
- **`regs` for x86_64**, and the context-switch register check for x86_64 from entry 55: both still owed.
- **The C library** for AArch64: its start-up code and system call stub are x86 assembly. Next.
- **One CPU.** `KERNEL_STACK` is a single number, not one per CPU.

## What I learned

- **Starting a program is returning to it.** Neither CPU has "enter user mode". Both have "return from an exception", and you forge the thing to return from.
- **A banked register is a stack switch you do not write.** Half of entry 45 was about finding the kernel stack when every register still belongs to the program. AArch64 answers that with a second `sp`.
- **Two roots make processes cheap to describe.** No copied kernel half, no rule that the kernel's top-level table must never change.
- **A test that passes when the feature is absent is not a test.** Zeroed registers and an aligned stack looked fine in every boot. They looked fine with the code removed too.
- **`cfg` belongs at the bottom.** One function in the library and one line in `cli` differ; everything above them compiled for the new CPU untouched.

## Testing note

**1. Programs on AArch64**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[hello] hello from user mode! I am process 1, running at exception level 0` | a program runs, and its system calls work |
| `[hello] the kernel refused all 5 bad pointers` | bad pointers are refused, not followed |
| `[process]   user read at 0xffffffff80100000: protection violation (at level 3)` then `"peek" killed: data abort in a program` | a program cannot read the kernel |
| `[process] … "cli" killed: trapped system register access` | a program cannot mask interrupts |
| `[process]   user write at 0x100000000000: protection violation (at level 3)` then `"memro" killed` | read-only means read-only |
| `[regs] at the start: 29 registers zero, the stack pointer a multiple of 16` | nothing of the kernel's leaks into a new program |
| `[regs] 26 registers and the stack pointer, through system calls and timer interrupts for 5 ticks: none changed` | a program gets its registers back |
| `[selftest] user: hello exited 42, spin, heapcheck and regs exited 0, peek, cli and memro were killed; 14 processes, nothing leaked` | all seven, twice, every frame returned |
| `[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left` | channels and handles between programs |
| `[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left` | the file service and its heap |
| `[selftest] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked` | ending a program from outside |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

The lines of different programs are interleaved - they run at once - and process numbers above 1 can differ.

**2. The same on other machines**

```
make run-aarch64 RUN_A64="-M virt,virtualization=on,gic-version=3"
make run-aarch64 RUN_A64="-cpu cortex-a53 -m 6G"
```

The same lines as in 1.

**3. The PC, with the tests from their new file**

```
make run64
```

In the terminal it was started from, before the prompt:

| Look for | Meaning |
|---|---|
| `[hello] hello from user mode! I am process 1, running in ring 3` | the same program, the other CPU's word for it |
| `[selftest] user: hello exited 42, spin and heapcheck exited 0, peek, cli, port, memro and gs were killed; 16 processes, nothing leaked` | unchanged |
| `[selftest] ipc: …`, `[selftest] files: …`, `[selftest] kill: …` | unchanged |

| Type | You should see |
|---|---|
| `run hello` | `[hello] hello from user mode! …` and `the kernel refused all 5 bad pointers` |
| `ps` | the list of processes |

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu elf
readelf -h user/target/aarch64-unknown-none-softfloat/release/hello | grep Machine
readelf -h user/target/x86_64-unknown-none/release/hello | grep Machine
```

The first should end `test result: ok.`; the other two (from the top directory, after `make user-aarch64 user64`) should say `AArch64` and `Advanced Micro Devices X86-64`.

Automated: `make test64`
