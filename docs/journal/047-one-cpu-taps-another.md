# One CPU Taps Another on the Shoulder: the Reschedule Interrupt

*Journal entry #47 · the fourth part of SMP. A thread made ready on a busy CPU used to wait for an idle CPU's next timer tick — up to 10 ms with a CPU doing nothing. Now the busy CPU interrupts the idle one, and the thread runs a few hundred microseconds later.*

```
[selftest] wake-ups: a thread woken by a busy CPU ran on an idle one 243 us later (median of 40; slowest 4397 us; 41 interrupts between CPUs)
```

Entry 46 put threads on every CPU and left this in its list of what was missing:

> **An idle CPU is not told about new work.** It finds it at its next tick, 10 ms at worst.

This entry is that item.

---

## The problem, exactly

An idle CPU executes `hlt`: it stops until an interrupt arrives. That is the right thing to do with nothing to run — it saves power, and on an emulator it gives the host its core back.

But a halted CPU does not look at the ready queue. Something has to interrupt it. Until now the only thing that did was its own timer, a hundred times a second:

```
CPU 0  (busy)   ...wake(T)..........................................
                       |  T is in the queue
CPU 1  (idle)   hlt....|.............................tick -> runs T
                       <---- up to 10 ms, CPU 1 idle --->
```

Ten milliseconds is thirty million instructions on a slow machine. For a pipeline, where every line passes between two programs, or a driver answering the display, that wait is paid again and again — with a free CPU the whole time.

## An interrupt from one CPU to another

Entry 44 used the local APIC's *interrupt command register* to send INIT and STARTUP messages, and entry 46 to send an NMI. The same register sends the most ordinary message of all: "raise interrupt number N on that CPU". The kind is called **fixed**, and it is kind 0:

```rust
pub const fn interrupt_message(vector: u8) -> u32 {
    ASSERT | vector as u32
}
```

The receiving CPU takes it exactly as it takes a device's interrupt: through its IDT, when its interrupts are on. I gave it vector `0xF0`.

People call this an **IPI**, an inter-processor interrupt, and this particular one the *reschedule IPI*.

### The handler that does nothing

```rust
RESCHEDULE_VECTOR => {
    interrupts::poked();              // count it
    apic::end_of_interrupt();
    interrupts::interrupt_done();
}
```

There is no message in it and no work in the handler. It does not need any: since entry 7, *every* interrupt ends in `interrupt_done`, which asks the scheduler whether this CPU should choose again. The interrupt only has to arrive. Arriving ends the `hlt`, and the question gets asked 10 ms early.

## Who decides, and who sends

The scheduler's policy (`sched.rs`) is a library with no hardware in it — that is why it can be tested on the host. It cannot send an interrupt. So it decides and leaves a note:

```rust
fn offer(&mut self, cpu: usize) {
    let free = |c: &Cpu| c.current == c.idle && !c.reschedule;
    let chosen = if free(&self.cpus[cpu]) { Some(cpu) } else { self.cpus.iter().position(free) };
    if let Some(chosen) = chosen {
        self.cpus[chosen].reschedule = true;
        if chosen != cpu {
            self.pokes |= 1 << chosen;
        }
    }
}
```

`offer` runs whenever a thread joins the queue. `cpu` is the CPU doing it. In words:

1. If this CPU idles, it takes the thread itself. It is awake — it is running this code — so nobody needs telling.
2. Otherwise, find another CPU that idles, mark it, and add it to `pokes`: a set of CPUs, one bit each.
3. If every CPU is busy, do nothing. The thread waits for a slice to end, as before.

The thread layer collects the note before it lets go of the lock, and does the sending:

```rust
fn tell_idle_cpus(t: &mut Threads) {
    let cpus = t.policy.take_pokes();
    if cpus != 0 {
        arch::smp::poke(cpus);
    }
}
```

`core::mem::take` (inside `take_pokes`) is a small Rust idiom worth knowing: it returns the value and leaves the type's default — here 0 — in its place, in one step. "Read and clear."

### An offer is a promise

The `!c.reschedule` in `free` matters. `reschedule` on an idle CPU now means "a thread has been offered to this CPU". Without it, two threads made ready one after the other would both be offered to the first idle CPU: one interrupt, one thread taken, and the second thread waiting for a tick with two more CPUs idle. With it, the second offer passes over the CPU already promised and goes to the next:

```
wake(A): CPU 1 idle, not promised  -> promise CPU 1, poke it
wake(B): CPU 1 promised            -> promise CPU 2, poke it
wake(C): CPU 3                     -> promise CPU 3, poke it
wake(D): nobody left               -> D waits for a slice
```

And a promise must end. `pick_next` clears the flag: a CPU that has chosen has been asked and has answered. Before I added that line, a CPU whose slice-end request was still set when its thread blocked went idle *looking promised*, and was passed over until its next tick.

### Three places a thread joins the queue

- A thread is created or woken (`add`, `wake`).
- A sleeper's time comes. The boot CPU keeps the time for everyone, but need not run everyone it wakes.
- A running thread gives way (`pick_next` puts it at the back). This one is easy to miss: on CPU 0, thread 1 yields to thread 2 — and thread 1 could run *right now* on an idle CPU 1.

## A promise that is not kept

The interrupt is a hint, never a guarantee, and the design must be right if it is wrong in either direction.

**Told for nothing.** CPU 1 is poked; before its interrupt arrives, CPU 0's thread blocks and CPU 0 takes the queued thread itself. CPU 1 wakes, asks, finds an empty queue, and halts again. Cost: one wasted interrupt.

**Not told.** Suppose the interrupt is lost, or every idle CPU already looked promised. The timer tick of entry 46 is still there and still looks at the queue. The worst case is what the *normal* case was yesterday.

That second point is why this entry was easy and entry 46 was hard: nothing here is needed for correctness. The lost-wake-up rules of entry 46 make the system right; this makes it prompt.

## Sending with a lock held

`tell_idle_cpus` runs with the scheduler lock held and interrupts off. Two questions.

*Can the sender hang?* `apic::send` waits for the message to leave this CPU's APIC — a status bit — not for the other CPU to take it. The other CPU may have interrupts off for a while; the interrupt waits in its APIC.

*Does the receiver deadlock?* It wakes, and its handler wants the scheduler lock the sender still holds. It spins for the few instructions until the sender releases it. A short wait, not a deadlock: the sender never waits for the receiver.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What the interrupt carries | Nothing; the handler only acknowledges it | *A message saying which thread to run.* The queue already says it, under a lock; a second copy could disagree with it. |
| Who chooses the CPU | The policy library, as a set of CPUs to tell | *The thread layer looks for an idle CPU itself.* Then the choice could not be tested on the host. |
| Which idle CPU | The first one not already promised a thread | *The one that ran the thread last* (its cache may still hold the thread's data). Better, and needs per-thread history the scheduler does not keep yet. *All idle CPUs.* Wakes three to run one. |
| A busy CPU and a new thread | Not interrupted | *Interrupt a CPU running something less important.* There are no priorities yet. |
| When to send | Before the scheduler lock is released | *After.* The set would have to be carried out of the lock by every caller; and the receiver waits a few instructions either way. |
| The tick's look at the queue | Kept | *Removed as redundant.* It is what makes a lost interrupt harmless. |
| Vector | `0xF0`: a higher priority class than any device | *A low one, next to the timer.* It makes no difference today - handlers run with interrupts off, so nothing interrupts a handler - but if two are pending at once the APIC delivers the higher first, and "choose again" should not queue behind a disk. |

## Bugs

Two, both found by host tests before the kernel was ever booted with the change — I wrote this step in a copy of the sources while entry 46's long test runs still had the real tree.

**A request that outlived its reason.** `reschedule` used to mean only "choose again soon". A busy CPU whose slice ran out had it set; if its thread then blocked by itself, the CPU went idle with the flag still up. Harmless before. Now the flag on an idle CPU reads as "already promised a thread", and `offer` passed that CPU over until its next tick cleared it. The randomized test's sentence about offers caught it. The fix is one line in `pick_next`.

**A test of mine that was wrong, not the code.** The randomized test first collected the set of CPUs to interrupt once per step, and failed with *busy CPU 3 was interrupted*. CPU 3 had been idle when it was chosen and had picked up a thread later in the same step. The set must be looked at after each operation, as the kernel does.

**The kernel ran out of room.** The first full run of the tests failed in a place that has nothing to do with CPUs:

```
rust-lld: error: the kernel image ends above 2 MiB, where the bootloader refuses to load it - see linker.ld
```

The bootloader loads the kernel at 1 MiB and its ramdisk at 2 MiB, so the kernel — code, the programs it carries, and its zeroed data — has one megabyte. The ordinary kernel still fitted, with about 10 KiB to spare; one of the seven fault-test kernels, which carries a little extra code, did not. I am glad of the check I put in the linker script in an earlier entry: without it this would have been a boot that silently never starts.

What I did is the small thing: the queue for the debug log was 64 KiB of that megabyte, and is now 32. What it needs is the big thing — moving the ramdisk and the loader's staging area up, in the bootloader, which the old 32-bit kernel shares. That is its own piece of work and I have not done it here.

**Tests that failed for being slow.** With room made, four of the seven fault boots failed — each stopped at a different place. It looked like a hang in the kernel's stop path and was not: `fault-test.sh` gave each boot 15 seconds, the firmware and the loader take nine and a half of them before the kernel prints a line, and the new self-test added most of a second. On a quiet machine at night that fitted. In the morning, with a desktop in use on the same four cores, it did not. The limit is now 40 seconds, as for every other boot.

I made this harder for myself: I started a second run of the fault tests to see whether the failure repeated while the first suite's last boots were still going. Two test runs sharing one build directory and four cores told me nothing, and I threw those results away.

And a wrong guess: I wrote the self-test's limit as "median under 2 ms" before I had a single measurement. The second boot measured 1.4 ms; a boot on a busy host, 2.8 ms. A time limit on an emulator is a test of the host. See below.

## How I checked it

**Automated** (`make test64`):

- **27 host tests of the policy** (7 new): a thread made ready on a busy CPU is offered to an idle one, once; an idle CPU that makes a thread ready takes it itself and tells nobody; nobody is told when every CPU is busy; two threads made ready together go to two CPUs, a third to the third, a fourth to nobody; a thread that gives way is offered; a CPU told for nothing goes on idling and can be offered the next. The randomized test of entry 46 now also checks, after every operation, that every CPU to be interrupted is idle, was offered a thread, and is not the CPU that did it — and one sentence about the whole system: *if threads wait and CPUs idle, at least as many CPUs have been offered a thread as there are threads waiting (or all of them have)*. It runs twice: with thirty threads, and with three — fewer threads than CPUs, so that there is nearly always someone to tell.
- **One host test of the message**: `0x40F0` for vector `0xF0`.
- **A self-test at every boot**: `selftest::wakeups`. A thread sleeps in a wait queue. The boot thread wakes it and then *spins* — it keeps its CPU busy, so the sleeper can only run on another — until the sleeper answers, and measures the time. Forty times, each at a different point in the 10 ms between ticks. It then requires one thing: that about one interrupt per wake-up was sent *and taken* (at least 20 for the 40). The time is printed and not judged, and the numbers say why:

| | Median of 40 wake-ups, one number per boot | Interrupts taken |
|---|---|---|
| With the interrupt, 4 CPUs | 207, 223, 243, 447, 459, 1373 µs | 40 or 41 |
| With the interrupt, 2 CPUs | 388 µs | 41 |
| Sending switched off, 4 CPUs | 2841, 4684, 4801 µs | 0 |
| Sending switched off, 2 CPUs | 5618 µs | 0 |
| With the interrupt, 4 CPUs, while the host was busy with other work | 2778 µs (slowest 44 165) | 43 |

Typically ten to twenty times quicker — but QEMU without KVM gave one boot at 1.4 ms, and without the interrupt three idle CPUs with three timers find the thread in under 3 ms on a lucky boot. No limit on the time separates the two safely. The count of interrupts does: it is 0 or it is 40.

The slowest single wake-up is still several milliseconds in every boot. I have not found out why; it is one or two rounds in forty.

**By hand:**

- Breaking the policy three ways, in the host tests:

| Change | Tests that failed |
|---|---|
| Offers do not pass over a CPU already promised | 3 |
| Another CPU is chosen but never added to the set | 6 |
| `pick_next` leaves the request set | 2 |

- Switching the sending off (`tell_idle_cpus` collects the set and throws it away), four boots: every one stopped with `KERNEL PANIC: only 0 CPUs were interrupted in 40 wake-ups`. The medians in the table above are from those boots.
- Eight boots with 1, 2 and 4 CPUs while settling the limit: the numbers above.
- After the last change: the whole of `make test64`, then the boot test twice more, `smp64-test` twice and the UEFI boot once. All passed, with medians between 176 and 379 µs in the seventeen boots that have more than one CPU.

**Not tested:**

- Real hardware. An emulated interrupt between CPUs is a function call inside QEMU; on silicon it is a message on a bus, and takes time I have not measured.
- What it buys a real workload. I measured one wake-up at a time, not a pipeline or a compile.
- A lost interrupt, for real. The host test supposes one; I have never seen one.

## What is still missing

- **The kernel has about 40 KiB left to grow** before the bootloader's 2 MiB limit stops it again. The limit has to be raised in the bootloader; halving a buffer only bought time.
- **No affinity.** A thread goes to the first idle CPU, not the one whose cache still holds its data.
- **Busy CPUs are never interrupted for a new thread.** With priorities this would matter: a key press should not wait for a slice.
- **`ps` does not show CPUs.** Nothing on the screen says which CPU a program is on.
- Of SMP itself, what remains is the milestone: a C program compiled on the host, put on the disk, and run on four CPUs.

## What I learned

- **An interrupt can be only a knock.** I expected to design a message. The handler is three lines because the question it leads to — "should this CPU choose again?" — was already asked at the end of every interrupt.
- **Separate being right from being fast.** Entry 46 made the scheduler right with a tick as the only wake-up. Because of that, this entry could not break anything: a missing interrupt is yesterday's behaviour.
- **Measure before choosing a limit — and then do not use a time limit.** My first one would have failed one boot in six. The check that works counts interrupts.
- **A timeout is a statement about the machine, not the code.** Fifteen seconds was true at night.
- **A flag that means "asked" needs a moment when it means "answered".** The stale request that followed a CPU into idling was found by one sentence in a randomized test, not by a boot.
- **Policy decides, mechanism acts.** The library returns a set of bits; the kernel turns bits into interrupts. The seven tests of who gets told run in milliseconds on my laptop and never touch an APIC.
- **`mem::take`** for read-and-clear.

## Testing note

```
make run64
```

**In the log** (the terminal `make run64` was started from)

| Look for | You should see |
|---|---|
| `[selftest] wake-ups:` | `a thread woken by a busy CPU ran on an idle one NNN us later (median of 40; slowest NNNN us; 40 interrupts between CPUs)`. The first number should be in the hundreds, sometimes a little over a thousand; 10 000 would be a whole tick. The last should be 40 or 41 |
| `FAILED` | Nothing |

**In the window** — nothing new to type; the system should feel the same or a little quicker

| Type | You should see |
|---|---|
| `ls \| wc` | A count, at once: the two programs of a pipeline hand every line to each other, and each hand-over used to be able to wait for a tick |
| `forever &` then `echo still quick` | The echo at once, with one CPU spinning |
| `kill 1` | The `forever` stopped |
| `primes`, `ps`, `cask` | As before |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | `[selftest] wake-ups: 1 CPU, so nobody to tell`, and a working system |
| `make run64 RUN_CPUS=2` | The `wake-ups` line as with four |

**On the host**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched
```

should end `27 passed`.

Automated: `make test64`
