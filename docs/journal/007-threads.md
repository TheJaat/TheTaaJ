# Threads: Context Switch, Preemptive Round-Robin, Wait Queues

*Journal entry #7 · one CPU, many things in progress*

With a timer ticking (entry #6), the kernel can do the thing that makes an operating system feel like one: run several activities at once, by switching between them so fast that each seems to run alone. This entry is the scheduler, the context switch, and the mechanism for waiting — which turned out to be the subtle part.

---

## What a thread is

A thread is an **execution in progress**: a stack, and the CPU registers that say where it is. Switching threads means saving those for one and loading them for another. At this step all threads are kernel threads sharing one address space; processes come in entry #8.

## Policy and mechanism, kept apart

Two different questions hide inside "the scheduler":

- **Policy**: *which* thread runs next, and for how long?
- **Mechanism**: *how* does the CPU stop running one and start running another?

The policy is pure bookkeeping — queues and counters. So it lives in the hardware-free library (`sched.rs`), where it can be tested on my development machine. The mechanism is a dozen lines of assembly.

### The policy: round-robin

- A FIFO **run queue** of threads ready to run.
- Each thread runs for a **slice of 2 ticks** (20 ms). When the slice ends, it goes to the back of the queue and the front one runs. If nobody else is waiting, the slice is simply renewed — no pointless switch.
- A thread can be **Ready**, **Running**, **Sleeping** until a given tick, **Blocked** until woken, or **Exited**.
- An **idle thread** runs when nothing else can. It is never put in the queue and can never block — so "is there something to run?" always has an answer.

Ten host tests cover it, including a randomised run that checks fairness: no ready thread is ever passed over indefinitely.

Round-robin is the simplest fair policy. Priorities and smarter policies can replace it later without touching the mechanism — that is the point of the split.

### The mechanism: `switch_context`

How much must be saved to switch threads? Less than I expected. A switch is an ordinary *function call* — and the calling convention already says a called function may destroy RAX, RCX, RDX, RSI, RDI and R8–R11. The caller saved anything it needed from those. What a function must preserve are six registers (RBX, RBP, R12–R15) and the stack pointer. So:

```rust
global_asm!(
    ".global switch_context",
    "switch_context:", // (save: *mut u64 = rdi, load: u64 = rsi)
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov [rdi], rsp",   // remember the old thread's stack
    "    mov rsp, rsi",     // <- now on the new thread's stack
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    ret",              // into the new thread
```

The `ret` is the trick. It pops a return address from the *new* stack — the address the new thread itself pushed when *it* called `switch_context` some time ago. **Every thread that is not running is "inside" `switch_context`**, waiting to return from it.

A brand-new thread was never inside it. So `initial_stack` fakes it: it writes six zeros and a return address onto the fresh stack, so the first switch to the thread "returns" into `thread_trampoline`, which calls the thread's function.

In Rust, a thread is started with a closure — `threads::spawn(move || { ... })` — which is boxed onto the heap and run by that entry code.

## Preemption

A thread that calls `yield` gives up the CPU voluntarily. A thread in an endless loop never does. **Preemption** is the timer taking the CPU away: at the end of each timer interrupt (after the EOI — otherwise no further tick would arrive while the next thread runs), the handler asks the policy whether the slice is over and, if so, switches.

The preempted thread's full register state is already safe: the interrupt stubs pushed it all on that thread's stack. From the scheduler's point of view the interrupted thread just "called" switch from inside the interrupt handler, and will return there when its turn comes round.

## Waiting, and the lost wake-up

Threads wait for things: a key, a message, another thread finishing. The naive way —

```rust
while !condition() {
    block();
}
```

— has a famous bug. Suppose the thread checks the condition (false), and *before it blocks*, an interrupt makes the condition true and wakes the thread. The wake-up finds the thread not blocked, so it does nothing. Then the thread blocks — forever. The wake-up was **lost**.

The cure is to make "check and block" one indivisible step. `WaitQueue` does the check with interrupts **off**, and keeps them off until the thread has actually been switched away from. On one CPU, nothing can come between. Every blocking operation in the kernel since — receiving a message, waiting for a process, waiting for an interrupt — goes through this one mechanism, so the argument was made once.

(Entry #17 extends this same function to be interruptible by a kill.)

## Where threads' stacks live

Each thread gets a 64 KiB kernel stack in its own slot at `0xFFFF_D800_0000_0000` upward, with an **unmapped guard page** below it — the same idea as the boot stack in entry #4. Overflow hits the guard, and the exception handler says "kernel stack overflow" and which thread. A new deliberate-fault build (`fault-thread-stack`) tests exactly that.

## A thread cannot free its own stack

When a thread exits, its stack must be freed — but it is *standing on it*. So an exiting thread only marks itself Exited and switches away; **the next thread to run frees it** ("reaping"). The stack slot is then recycled.

The boot thread itself exits when startup is done; the keyboard echo of entry #6 became a thread that blocks until the keyboard interrupt wakes it.

## How I checked it

At every boot:

- two threads that yield to each other must print strictly alternating: `ababab`;
- a thread that never yields must be preempted, proved by another thread getting to run;
- `sleep(5)` must take exactly 5 ticks;
- **200 threads** are spawned and reaped, after which the number of free frames and stack slots must be exactly what it was — nothing leaked.

## A flaky test, found later

That last check failed once in a while, weeks later (fixed in entry #10's commit). The cause was in the test, not the kernel: the *peak* number of threads alive at once depended on timing. A second round with a higher peak used a stack slot never used before, which needed a new page table — and that one extra frame looked like a leak. Now the threads all wait at a gate before exiting, so both rounds peak identically. A test that compares resource counts must control everything that affects them.

## What I learned

- **A context switch is small** because the calling convention has already done most of the saving.
- **"Check and block" must be atomic.** Get this one primitive right and reuse it everywhere; do not re-derive it per feature.
- **Separating policy from mechanism** made the policy testable in milliseconds, with randomised inputs, on the host.

---

## Testing note

```
make run64
```

In the terminal log:

| Line | Meaning |
|---|---|
| `[selftest] threads: two yielding threads ran as "ababab"` | Voluntary switching, strictly alternating |
| `[selftest] threads: a thread that never yields was preempted` | The timer takes the CPU away |
| `[selftest] threads: sleep(5) took 5 ticks; 200 threads spawned and reaped, nothing leaked` | Sleeping is exact; stacks are recycled |

In the QEMU window:

| Type | You should see |
|---|---|
| `ps` | Several processes listed and a line `N processes, N threads, N channels` — all of them taking turns on one CPU |
| `forever` and then Ctrl+C | A program that never gives up the CPU does not freeze the system: the shell still receives your Ctrl+C |

Automated: `make kernel64-test` (the scheduling policy), `make boot64-test`, `make fault64-test` (includes the thread stack overflow).
