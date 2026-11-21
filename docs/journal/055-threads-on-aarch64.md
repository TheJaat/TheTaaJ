# Threads on AArch64

*Journal entry #55 · Era 3, step 3e. The scheduler, the thread table, wait queues, sleeping, the process and IPC code - everything above the architecture layer but the programs themselves - now compiles for AArch64, and threads run there: taking turns, being interrupted, sleeping, ending. What had to be written was small: a context switch, and a way to notice that a kernel stack has run out.*

```
[selftest] context switch: there and back with a known value in each of 11 registers, none changed
[selftest] threads: two yielding threads ran as "ababab"
[selftest] threads: a thread that never yields was preempted
[selftest] threads: sleep(5) took 5 ticks; 200 threads spawned and reaped, nothing leaked
```

---

## Why

After entry 54 the AArch64 kernel had a tick that counted and nothing else. A tick exists to take the CPU from one thread and give it to another.

Threads are also the last thing in the way of programs. A program is a thread that sometimes runs in user mode; a system call that waits is a thread that sleeps. The PC got threads in entry 7, and everything since - processes, IPC, drivers in user mode - stands on that one module.

## How little was architecture

`threads.rs` is 596 lines. I went through it for what it asks of the CPU:

```
arch::context::switch / initial_stack     the switch itself
arch::cpu()                               which CPU am I?
arch::disable/enable/restore_interrupts   (already there)
arch::wait_for_interrupt                  (already there)
arch::smp::poke(cpus)                     wake other CPUs
arch::set_io_bitmap(ports)                a process's I/O ports
arch::syscall::set_kernel_stack(top)      where to land coming from a program
arch::mmu::activate(root)                 a process's address space
```

Two lines of real work, and five things that on AArch64 are, for now or for ever, nothing:

| Asked for | On AArch64 |
|---|---|
| `cpu()` | `0` - one CPU is running (others: step 3h) |
| `smp::poke` | nothing - there is nobody to poke |
| `set_io_bitmap` | nothing, **for ever**: AArch64 has no I/O ports. Every device is in memory, and page tables decide who may touch it |
| `syscall::set_kernel_stack` | nothing, **for ever**: the kernel's stack pointer is a register of its own (`SP_EL1`) and simply stays where the thread left it |
| `mmu::activate` | refuses any root but the kernel's - until there are programs (step 3f) |
| `syscall::enter_user` | panics: "programs cannot run on AArch64 yet" |

With those, this is what stopped being PC-only in `main.rs`:

```rust
mod interrupts;     // was #[cfg(target_arch = "x86_64")]
mod ipc;            // was
mod memory_object;  // was
mod process;        // was
mod syscall;        // was
mod threads;        // was
```

Not one line inside those six files changed. What is still PC-only is three modules: `devices` (PCI), `lastwords` (the screen) and `programs` (the built-in programs, which are x86 machine code).

The self-tests moved the same way: `timer()` and `threads()` went from the PC-only file to the shared one, and the only edit was `arch::tsc()` becoming `arch::clock()` - a name both CPUs can answer to (the TSC on a PC, the CPU's counter on AArch64).

## The context switch

The idea is the PC's, from entry 7: a switch is an ordinary function call. The calling convention has already made the caller save every register it is allowed to lose, so the switch only has to keep the ones a function must hand back unchanged.

| | x86_64 | AArch64 |
|---|---|---|
| Registers a function must keep | rbx, rbp, r12-r15 (6) | x19-x28, x29 (11) |
| The return address | on the stack, pushed by `call` | **in a register**, x30 |
| So the switch saves | 6, and `ret` pops the address | 12: the 11, and x30 |

```asm
switch_context:                     // (save: *mut u64 = x0, load: u64 = x1)
    sub     sp, sp, #96
    stp     x19, x20, [sp]
    ...
    stp     x29, x30, [sp, #80]
    mov     x9, sp
    str     x9, [x0]                // the old thread's stack pointer, kept
    mov     sp, x1                  // <- now on the new thread's stack
    ldp     x19, x20, [sp]
    ...
    ldp     x29, x30, [sp, #80]
    add     sp, sp, #96
    ret                             // to the NEW thread's x30
```

That x30 is the whole trick on this CPU. `ret` does not look at the stack; it jumps to whatever x30 holds. Loading the new thread's x30 is what makes the function return *somewhere else* than it was called from.

A new thread has never been switched out, so its stack is given what it would have stored: eleven zeroes and an x30 of `thread_trampoline`.

No floating-point registers are saved. The kernel is built for a target that never uses them (`softfloat`). Programs will; that is a debt for step 3f, and the PC has the same one.

## A stack that has run out

Every kernel thread's stack is a 64 KiB slot whose lowest page is not mapped (as on the PC; `mm.rs` is shared now). Run off the end and the CPU faults instead of writing over the next thread.

Then the fault has to be *handled* - and on AArch64, as entry 53 noted, that was impossible. The CPU does not switch stacks when the kernel faults. The first thing the handler does is push 272 bytes of registers... onto the stack that has just run out. Which faults. Whose handler pushes 272 bytes. For ever, in silence.

The PC avoids this with a hardware feature: its double-fault handler is given a stack of its own by a table the CPU consults. AArch64 has no such table. The check has to be done by hand, in the first instructions of the handler, and there is a difficulty: **there is not one free register.** All 31 hold the interrupted code's values, and the only place to save one is the stack in question.

```asm
    sub     sp, sp, #272
    add     sp, sp, x0
    sub     x0, sp, x0              // x0 = the stack pointer
    tst     x0, #0xe000
    b.eq    stack_overflow
    sub     x0, sp, x0              // x0 = what it was
    sub     sp, sp, x0              // and so is the stack pointer
```

Lines two and three swap without a spare: afterwards `x0` holds the stack pointer and `sp` holds the *sum* of both. Nothing is lost - the last two lines undo it. (I did not invent this; Linux does the same in the same place.)

In between, one test. Because every slot starts at a multiple of 64 KiB, the low 16 bits of a stack pointer say how far up its slot it is. If bits 13 to 15 are all zero, it is in the lowest two pages: the missing one, or the last one above it. Either way there is no room to handle anything, and the frame goes on a 12 KiB stack kept for this:

```
*** KERNEL STACK OVERFLOW: the boot stack ran out
    kernel write at 0xffffffff800c0f90: the page kept unmapped below the stack
    pc  0xffffffff8008d560  state 0x60000345  sp  0xffffffff800c1190
```

For this to work the **boot stack** had to take the same shape. It was 64 KiB with a guard page wherever the linker put it; it is now a 64 KiB-aligned slot of one missing page and 60 KiB. And the overflow stack itself sits one page up from a 64 KiB boundary, so that its own addresses never look like a stack that has run out.

The check is only in the four vector slots for "the kernel was interrupted". Coming from a program, the stack pointer is the kernel's, at the top of a fresh stack.

## Checking the switch itself

When I broke the switch on purpose - one pair of registers loaded from the wrong place - **every thread test passed**. Two hundred threads were created, switched hundreds of times, and ended, with two registers wrong after each switch.

The reason is worth understanding. Between a test and `switch_context` are Rust functions: `yield_now`, `schedule`. Each, obeying the calling convention, saves the callee-saved registers *it* uses when it starts and restores them when it returns. So when the switch hands back a wrong x21, `schedule` promptly overwrites it with the right one from its own frame. The convention that makes the switch cheap also hides its mistakes - until the day the compiler arranges things differently.

So there is now a check with nothing in between, in assembly: put a known number in each of the eleven registers, switch to a second context on a spare stack - which fills all eleven with other numbers and switches straight back - and compare.

```
[selftest] context switch: there and back with a known value in each of 11 registers, none changed
```

With the broken switch: `2 of 11 registers were changed by a switch to another stack and back`.

It is the third check of this kind on this CPU: registers across an exception (entry 50), across an interrupt (entry 54), across a switch. All three exist because the thing being checked usually works by luck when it is wrong.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How much to un-mark | Everything that compiles: threads, interrupts, process, IPC, memory objects, system calls | *Threads only, with the process parts cut out by `cfg`.* `Thread` holds an `Option<Arc<Process>>`; cutting it out means `cfg` inside the scheduler's central function. Two stubs that panic are honest and stay out of shared code. |
| The kernel's stack model | The kernel always runs on `SP_EL1`; overflow is detected in the vector | *Threads on `SP_EL0`, exceptions on `SP_EL1`.* The CPU would then switch stacks by itself on every exception, and overflow would be reportable for free. But a thread preempted from an interrupt would be switched away from while its registers sit on the *shared* exception stack - and programs need `SP_EL0` for themselves. |
| How overflow is detected | From the stack pointer's low bits | *Look at the fault address.* That needs a register to read it into - the thing there is none of - and says nothing when the exception is an interrupt. |
| The margin | The missing page and the one above it | *Only the missing page.* An exception arriving with 200 bytes left would be given a frame that does not fit. The price: an interrupt in a thread's last 4 KiB of stack ends the kernel, though nothing had overflowed yet. A thread that deep is about to. |
| The tick counter | Back in `interrupts.rs`, shared | Entry 54 kept it in the AArch64 code only because `interrupts.rs` needed threads. |
| Built-in programs on AArch64 | An empty list | The real list is x86 machine code. `find` returns "not found", which the callers already handle. |

## Bugs

**The kernel did not have one this time; the tests did** - the context switch could be wrong unnoticed, as told above. Found only because I broke it on purpose. The PC's switch has the same blind spot and has had it since entry 7; it has also had every boot since then to show a problem, which is weaker evidence than a test but not nothing. An equivalent check for the PC is on the list.

**A test that matched too much.** The boot with no interrupt controller must run no timer or thread test. I wrote the check as "no line beginning `[selftest] t`" - which also matches `[selftest] top of memory`. The test failed on a correct kernel. A reminder that a pattern is code, and gets bugs.

## How I checked it

**Automated** (`make test64`):

- **Host tests**: 166, unchanged. The scheduler's policy is host-tested (`kernel64::sched`) and is the same code on both CPUs.
- **AArch64 boots**: thirteen ordinary ones. The twelve with an interrupt controller must print the four lines at the top: the switch check, the turns, the preemption, and "sleep(5) took 5, 6 or 7 ticks; 200 threads spawned and reaped, nothing leaked". The one without a controller must print none of them.
- **AArch64 fault boots**: six. Two new: a recursion that runs the **boot stack** out, and one that runs **a thread's stack** out. Each must print exactly one report, name the right stack, and give a fault address in the lowest page of a 64 KiB slot.
- **x86_64**: the whole suite. `timer()` and `threads()` are now the shared ones, using `arch::clock()`.

**By hand - breaking it on purpose** (each put back):

| # | Change | Result |
|---|---|---|
| 1 | two registers loaded from the wrong place in a switch | **passed**; with the new check: panic, *2 of 11 registers were changed* |
| 2 | the scheduler not asked at the end of an interrupt | panic: *the spinning thread was never preempted* |
| 3 | no check for a stack that has run out | a confused data-abort report instead of the overflow report; the boot test fails |
| 4 | only the missing page counts, not the page above it | still an overflow report - see below |
| 5 | x30 not saved in a switch | instruction abort at the first switch back |

**Not tested, and why:**

- **The margin page** (experiment 4). With it gone, the first fault's frame still fitted in the 400 bytes that happened to be left; the handler then overflowed itself, and *that* was reported correctly. The report was of the second fault, not the first, and my test cannot tell them apart. So the margin is reasoned, not shown.
- **An interrupt arriving on a nearly full stack** - the other way into the overflow path. Its message exists and has never been printed.
- **Process, IPC and system-call code on AArch64.** It compiles. None of it has run: there is no program to run it.
- **Two CPUs.** The scheduler's four rules for them (the top of `threads.rs`) are exercised only on the PC.
- **A stack overflow so large it jumps over the missing page** into the next slot. Not detected on either CPU.

## What is still missing

- **Programs** - step 3f: exception level 0, the `svc` instruction, address spaces in the low half (`mmu::activate`), the ELF loader accepting AArch64 files, and the user library and libc built for it.
- **Floating-point registers** are not saved on a switch, on either CPU.
- **A context-switch check for the PC.**
- **The idle thread's log queue** (`write_queued`) runs on AArch64 but nothing writes to the queue yet.

## What I learned

- **The architecture layer was thinner than I thought.** Six modules - threads, interrupts, processes, IPC, memory objects, system calls - needed two functions and five one-liners to move. That is the dividend of a rule the kernel has had from its start: nothing outside `arch/` names a CPU.
- **A return address in a register changes what "switch" means.** On x86 the switch swaps stacks and the return address comes along for free. On AArch64 it must be carried. Same idea, one more register, and a completely wrong mental picture if I had copied the x86 code by shape.
- **Things that are "nothing" on one CPU are worth writing down.** `set_io_bitmap` is an empty function with a comment saying *why* it will always be empty. Otherwise someone - me - will one day "finish" it.
- **Without a spare register you can still swap two, by adding.** `a += b; b = a - b; a -= b` is a puzzle answer I knew and never expected to need. Here it is the only way in.
- **A calling convention can hide a broken context switch.** Callee-saved registers get repaired by the callers. Correct-looking behaviour, for the wrong reason, until the compiler changes its mind.
- **Three times now: put a known value everywhere, do the thing, compare.** Exceptions, interrupts, switches. It is cheap, it runs at every boot, and each time it has been the only test that could see the fault.

## Testing note

**1. Threads on AArch64**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[selftest] timer: 10 ticks in 99 ms (expected 100)` | the shared timer test, now on this CPU too (98 to about 130 is normal) |
| `[selftest] context switch: there and back with a known value in each of 11 registers, none changed` | the switch keeps every register it must |
| `[selftest] threads: two yielding threads ran as "ababab"` | threads take turns |
| `[selftest] threads: a thread that never yields was preempted` | the tick takes the CPU back |
| `[selftest] threads: sleep(5) took 5 ticks; 200 threads spawned and reaped, nothing leaked` | sleeping works; stacks are recycled (5, 6 or 7 ticks) |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

**2. Stacks that run out**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-stack
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-thread-stack
```

The first command should end `aarch64-test: all passed` and leaves the kernels in `build/aarch64/`. Leave each QEMU with **Ctrl+A, then X**.

| Kernel | Look for |
|---|---|
| `Image-fault-stack` | `[test] overflowing the boot stack...` then `*** KERNEL STACK OVERFLOW: the boot stack ran out` and `kernel write at 0xffffffff800c0f90: the page kept unmapped below the stack` |
| `Image-fault-thread-stack` | `[test] overflowing a thread's stack...` then `*** KERNEL STACK OVERFLOW: a thread's stack ran out` and `kernel write at 0xffffd80000020e40: the page kept unmapped below the stack` |

Each is followed by a register dump and nothing after it. The addresses may differ in their middle digits; the last four are always below `1000` - the lowest page of a slot.

**3. The PC, with the shared tests**

```
make run64
```

In the terminal it was started from, before the prompt:

| Look for | Meaning |
|---|---|
| `[selftest] timer: 10 ticks in … ms (expected 100)` | as before, now timed through `arch::clock()` |
| `[selftest] threads: sleep(5) took … ticks; 200 threads spawned and reaped, nothing leaked` | as before |

| Type | You should see |
|---|---|
| `ps` | the list of processes |
| `uptime` | the time since boot |

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched
```

should end `test result: ok.` with no failures - the scheduler's policy, the same code both CPUs now run.

Automated: `make test64`
