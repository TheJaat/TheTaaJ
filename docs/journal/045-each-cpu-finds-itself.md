# Each CPU Finds Itself: GS and `swapgs`

*Journal entry #45 · the second part of SMP. Nothing new appears on the screen. Two global variables that were only correct with one CPU are gone, and in their place every CPU has a block of memory of its own, found through a register — with an exchange of that register at every door between a program and the kernel.*

```
[smp] CPU 1 awake: APIC id 1, stack below 0xffffd80000010000      <- "1" now read through GS
...
[gs] system calls, timer interrupts, then a read through GS...
[process] page fault at 0x400060 in user mode
    user read at 0x5a5: page not present
[process] 8 "gs" killed: page fault
```

Entry 44 woke the other CPUs and listed what stood between them and running threads. First on the list was a comment I wrote in entry 8, above the variable that holds the kernel stack for system calls:

> One CPU, so one global; with SMP this becomes per-CPU data reached through GS (and `swapgs`).

This entry is that sentence, done.

---

## The problem, exactly

When a program executes `syscall`, the CPU jumps into the kernel and changes almost nothing. In particular the stack pointer is still the program's. The kernel must not use it: it could point anywhere. So the first thing the entry code does is load the *running thread's kernel stack*.

From where? It was this:

```asm
syscall_entry:
    mov [rip + USER_RSP], rsp        ; park the program's stack pointer
    mov rsp, [rip + KERNEL_RSP]      ; the thread's kernel stack
```

Two globals. With four CPUs each running a different thread, `KERNEL_RSP` would have to hold four values at once, and two system calls at the same moment would park two stack pointers in the one `USER_RSP`.

The same was true in the TSS code: `set_kernel_stack` and `set_io_bitmap` wrote to *the* TSS, and since entry 44 there is one per CPU.

So each CPU needs a block of its own. That part is easy. The hard part is *finding* it at the first instruction of a system call. Every general register holds something of the program's that must be preserved. There is no stack. There is nothing to compute an address with.

## GS: a register nobody was using

In 64-bit mode the segment registers no longer describe segments. But two of them, FS and GS, kept one thing: a **base address**. `mov rax, gs:[8]` means "read the memory at GS's base plus 8". The base is not in any general register, so using it disturbs nothing.

Point each CPU's GS base at that CPU's block, and one instruction reads different memory on each CPU:

```
               CPU 0                        CPU 1
GS base  -->  +--------------+   GS base  -->  +--------------+
        0     | this         |           0     | this         |
        8     | kernel_rsp   |           8     | kernel_rsp   |
        16    | user_rsp     |           16    | user_rsp     |
        24    | index = 0    |           24    | index = 1    |
        32    | tables       |           32    | tables       |
              +--------------+                 +--------------+
```

```rust
#[repr(C)]
pub struct PerCpu {
    this: *mut PerCpu,
    kernel_rsp: u64,
    user_rsp: u64,
    pub index: usize,
    pub(super) tables: *mut CpuTables,
}
```

`#[repr(C)]` because the assembly reads fields by offset: without it Rust may order the fields as it likes. Two `const` assertions check that `kernel_rsp` really is at 8 and `user_rsp` at 16, so that changing the struct without the assembly is a build error and not a crash.

The first field is the block's own address. That looks odd, but `gs:[0]` is then a pointer to the block that ordinary Rust can use; reading the GS base itself would need a slow special instruction.

## The catch: GS is the program's too

A program may use GS. It can load a segment into it, which resets the base. If the kernel simply trusted GS, a program could point the kernel's "this CPU's block" wherever it liked — at memory it had prepared with a fake kernel stack address.

So the CPU has **two** GS bases, and one instruction to exchange them, which only ring 0 may execute:

| | |
|---|---|
| `GS_BASE` | the one `gs:` uses now |
| `KERNEL_GS_BASE` | the other, waiting |
| `swapgs` | exchange them |

The rule I keep:

> **In ring 0, GS is this CPU's block. In ring 3, it is the program's.**

Every way from a program into the kernel does `swapgs` first; every way back does it last. There are exactly three doors.

### Door 1: the system call

```asm
syscall_entry:
    swapgs
    mov gs:[16], rsp           ; user_rsp
    mov rsp, gs:[8]            ; kernel_rsp
    push qword ptr gs:[16]
    ...
    pop rsp
    swapgs
    sysretq
```

Always from a program, so always a swap.

### Door 2: interrupts and exceptions

These arrive from a program *or* from the kernel itself — a timer tick does not care what was running. Swapping when the kernel was interrupted would be exactly wrong: the kernel's GS is already in place, and the swap would hand the kernel the program's.

The CPU pushes the interrupted code's CS, and the low two bits of CS are the ring. So:

```asm
isr_common:
    test byte ptr [rsp + 24], 3     ; the saved CS: from ring 3?
    jz 2f
    swapgs
2:  ...save registers, call the handler, restore...
    cli
    test byte ptr [rsp + 8], 3      ; going back to ring 3?
    jz 3f
    swapgs
3:  iretq
```

The way out asks the saved CS again; it does not remember the answer from the way in. Between the two the handler may have switched threads, and then the frame on the stack belongs to a different thread, interrupted somewhere else.

### Door 3: the first entry

A new program has never been in the kernel "from ring 3": it gets there by `iretq` from `enter_user`. One `swapgs` before that `iretq`, with interrupts off.

### What is not saved

Nothing per thread. A thread in the kernel uses the GS of whichever CPU it is on — that is what "per CPU" means. And a program's own GS base is not saved either, because a program has no way to set it to anything but zero: writing the base directly needs a CPU feature (FSGSBASE) the kernel leaves off.

## The boot CPU and the others

The boot CPU's block is a static, installed in `init_cpu` right after the GDT (loading the GDT reloads GS, which resets the base). The other CPUs' blocks are boxed by the boot CPU and installed in `ap_main`.

A started CPU now prints its number from its own block rather than from the argument it was handed:

```rust
kprintln!("[smp] CPU {} awake: ...", percpu::index(), ...);
```

Three CPUs printing 1, 2, 3 is each of them finding itself through GS.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How a CPU finds its block | GS base, with `swapgs` | *Ask the local APIC for its id and index a table.* Needs a free register and a memory read through a device mapping — and the system call entry has no free register. *`rdtscp`/`rdpid`* (a per-CPU number in an MSR): not on every CPU, and still needs a register. |
| Where the kernel stack for system calls lives | In the per-CPU block, set on every switch | *Read it from the TSS (`RSP0`), which holds the same value.* The TSS is found through GS anyway, and its field is unaligned. |
| Whether to swap on an interrupt | Decided by the saved CS | *Compare the GS base with a known value.* A slow MSR read on every interrupt. |
| The program's GS base | Not saved per thread | It can only be zero while FSGSBASE is off. When programs get thread-local storage (FS), that will need saving; GS can stay as it is. |
| Layout shared with assembly | `repr(C)`, offsets as constants, checked by `const` assertions | *Hand-written numbers in the assembly.* The first reordering of the struct would break system calls silently. |

## What can still go wrong

One hole is known and left open. A **non-maskable interrupt** or a **machine check** can arrive at any instruction, including the one between `syscall` and `swapgs` — in ring 0 (so the CS test says "do not swap") but with the program's GS still in place. The kernel has no handlers for those that do anything but report and stop, and the report does not use GS, so today nothing depends on it. An operating system that handles NMIs properly has to compare the GS base itself on those two vectors. Noted for the day NMIs are used.

## Bugs

None that ran. The change is small — about seventy lines changed and one new file of 120 — and each line is in a place where a mistake stops the machine at once, which makes it easy to get right or obviously wrong. What I did instead of debugging was break it on purpose; see below.

## How I checked it

**Automated** (`make test64`):

- A new test program, `gs`, runs twice in the kernel's self-test at every boot. It makes system calls, spins for five ticks so that timer interrupts arrive in ring 3, loads a segment into GS, sleeps, and then reads `gs:[0x5a5]`. A program's GS base is 0 and nothing is mapped there, so it must be killed by a page fault at exactly `0x5a5`. `check-boot.sh` requires that line twice. A fault at any other address would mean the program was running with the kernel's GS.
- Everything else is the test: every system call and every interrupt in the whole suite now goes through `swapgs`, on a machine with 4 CPUs. The seven fault boots (kernel faults, a double fault on its own stack) pass through the changed interrupt path too.

**By hand, once each** — removing or adding one `swapgs` and booting:

| Change | What happened |
|---|---|
| No `swapgs` on the way out of an interrupt | The boot hung in the user self-test: no `0x5a5` line, never reached idle |
| `swapgs` on every interrupt, from the kernel too | `CPU EXCEPTION 14: kernel read at 0x0` |
| No `swapgs` in `enter_user` | `CPU EXCEPTION 14: kernel write at 0x10` — the system call entry storing the stack pointer through the program's GS |
| No `swapgs` before `sysretq` | The same |

All four fail the automated test. `kernel write at 0x10` is `user_rsp` at offset 16 from a base of 0: the very thing the design predicts.

**Not tested:**

- A system call or interrupt **on a CPU other than 0**. The other CPUs have their blocks and their GS, and prove it by printing their own number, but no thread runs on them yet. The system call path on CPU 1 runs for the first time in the next step.
- NMIs and machine checks, as said above.
- Real hardware.

## What is still missing

Of SMP: the scheduler on every CPU (next), then CPUs telling each other things (TLB shootdown; stopping everyone on a panic), then the milestone.

`percpu::current()` returns a pointer that is only right while the thread stays on this CPU. Today threads never move. When they can, every use must have interrupts off — they do already, but nothing enforces it.

## What I learned

- **GS is a pointer the calling convention does not know about.** That is its whole value: it is there at the first instruction after `syscall`, when every other register is the program's.
- **`swapgs` is a door rule, not a save and restore.** I first thought of it as "save the user's GS, load the kernel's". It is simpler to think of ring 0 and ring 3 each *owning* one of two registers, and every door exchanging them.
- **An interrupt must ask where it came from.** The same handler code runs for a tick that interrupted a program and one that interrupted the kernel, and must do opposite things with GS.
- **Ask again on the way out.** The frame on the stack when a handler returns may not be the frame it was entered with. This is the first place in the kernel where forgetting that a handler can switch threads would have been a bug.
- **`repr(C)` plus a `const` assertion** is how Rust and assembly agree on a layout: the struct is written once, in Rust, and the build fails if the assembly's numbers go stale.
- **Breaking it on purpose is a test of the test.** Four one-line changes, four different failures, each one explained by the design. That gave me more confidence than the passing run did.

## Testing note

```
make run64
```

Nothing new to type. In the kernel's log (the terminal `make run64` was started from):

| Look for | You should see |
|---|---|
| `[smp] CPU 1 awake` | The three `awake` lines as in entry 44, numbered 1, 2, 3 — each number now read by that CPU through its own GS |
| `[gs] system calls` | Twice: `[gs] system calls, timer interrupts, then a read through GS...`, each followed a few lines later by `[process] page fault at 0x… in user mode`, `user read at 0x5a5: page not present`, and `[process] N "gs" killed: page fault` |
| `[gs] FAILED` | Nothing. This line must not be there |
| `[selftest] user:` | `hello exited 42, spin and heapcheck exited 0, peek, cli, port, memro and gs were killed; 16 processes, nothing leaked` |

**In the window**

| Type | You should see |
|---|---|
| `primes`, `ps`, `cask` and a line, anything | Everything as before. Every key pressed is an interrupt and every command dozens of system calls, all through `swapgs` |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | The same `[gs]` lines and a working system: GS is used with one CPU too |

Automated: `make test64`
