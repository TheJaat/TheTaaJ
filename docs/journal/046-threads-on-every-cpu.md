# Threads on Every CPU

*Journal entry #46 · the third part of SMP, and the one the first two were for. The scheduler now runs threads on all the CPUs. Four rules had to be found for that, each closing a hole that cannot exist with one CPU. Three I found by reading the code and asking "what if another CPU is here too?". The fourth I did not find: one boot in three hung, and it took a dump of every thread's state to see why.*

```
[selftest] threads: sleep(5) took 5 ticks; 200 threads spawned and reaped, nothing leaked
[smp] 4 CPUs run threads: each has its own timer and idle thread, and all take from one queue
[selftest] smp: threads ran on all 4 CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked
```

After entry 44 the other CPUs were awake. After entry 45 each could find its own data. They still ran nothing. From this entry on the shell, the file service, the disk driver and whatever is typed run on whichever of the four CPUs is free.

---

## What "interrupts off" used to mean

Everything in `threads.rs` was built on one fact: **with interrupts off, nothing else runs.** The only way another thread could get the CPU was an interrupt, so turning interrupts off made any stretch of code *atomic* — done as one step, as far as the rest of the kernel could tell.

With a second CPU that is simply false. Interrupts off stops *this* CPU from being interrupted. The other CPU is running, right now, possibly the same function.

The data was already safe: every shared structure in the kernel sits behind an `IrqLock`, which is a spin lock *and* interrupts off, and entry 44 showed the spin part working. What was not safe is everything that happens **between** two locked steps and relied on nothing coming in between. I found three such places by reading. A fourth found me.

## The design: one queue

First the shape, because the rules depend on it.

```
                 +-----------------------------+
                 |  ready:  T7  T3  T9  T4 ... |   one queue, one lock
                 +-----------------------------+
                    ^      ^        ^      ^
                 CPU 0   CPU 1    CPU 2   CPU 3
              current   current  current  current
               idle0     idle1    idle2    idle3
```

- **One queue of runnable threads**, shared. A CPU that needs a thread takes the one that has waited longest.
- **Each CPU has a current thread**, a time slice for it, and its own **idle thread** for when the queue is empty.
- **Threads do not belong to CPUs.** A thread that sleeps on CPU 0 may wake on CPU 3.
- **Each CPU has its own timer**, at the same 100 Hz, for its own slices. *Time* — the tick count, and waking sleepers — is kept by the boot CPU alone. Four CPUs counting ticks would make a second four times too short.

The roadmap said "per-CPU run queues". I chose one queue, and say why under the design decisions.

The policy is still pure bookkeeping in `kernel64::sched`, tested on the host; every function now takes the number of the CPU it is called on.

## Rule 1: the lock is held across the switch

`schedule` used to do this:

```
lock
  decide: from A to B.  A goes to the back of the queue.
unlock
switch_context(A, B)        <- saves A's registers, loads B's
```

On one CPU nothing can happen between `unlock` and the switch. On two:

```
CPU 0                               CPU 1
lock; decide A -> B
  A is put in the queue
unlock
                                    lock; takes A from the queue; unlock
                                    switch to A: loads A's saved stack pointer
saving A's registers...             runs A - on the stack CPU 0 is using
```

Two CPUs on one stack. Worse: A's saved stack pointer is the one from the *last* time A was switched out, because this time's has not been written yet. Each CPU overwrites the other's return addresses, and the kernel dies somewhere unrelated a little later.

The fix is to not unlock. `schedule` decides under the lock and switches **still holding it**. The thread it switches *to* releases it:

```rust
core::mem::forget(guard);                     // keep it locked, with no guard to drop
unsafe { context::switch(save, load) };
// Running again - possibly much later, possibly on another CPU -
// holding the lock some other `schedule` took.
unsafe { THREADS.force_unlock() };
```

This is the strangest code in the kernel: a lock taken by one thread on one stack and released by another on another. Rust's guards cannot express it — a guard unlocks when *it* goes out of scope — so the guard is forgotten (`mem::forget`: the value is dropped from the books without running its destructor) and the unlock is done by hand. A thread that has never run before does not come back through `schedule`; it starts in `thread_entry`, so the first line there is the same `force_unlock`.

With the lock held, CPU 1 cannot take A from the queue until CPU 0 is entirely on B's stack.

## Rule 2: say you will sleep, then look

Every wait in the kernel — for a message, an interrupt, a thread's exit — is `wait_while(condition)`. It used to be:

```
interrupts off
if the condition no longer holds: return        <- look
join the wait queue
mark myself Blocked; switch away                 <- sleep
```

and the argument (entries 7 and 17) was that with interrupts off nothing comes between the look and the sleep. On two CPUs:

```
CPU 0 (waits for a message)          CPU 1 (sends one)
looks: no message
                                     puts the message in the channel
                                     wakes the waiters: there are none
joins the queue
blocks                               <- for ever. The message is there.
```

This is the *lost wake-up*, the classic bug of concurrent programming, and entry 17 named it. The cure is an order:

```rust
let me = current();
for queue in queues {
    queue.add(me);                                   // 1. where wakers will look
}
announce(deadline.map_or(State::Blocked, State::Sleeping));   // 2. "I am waiting"
let ended = if !condition() { ... };                 // 3. and only now, look
match ended {
    Some(_) => stay(),                               // nothing to wait for: take it back
    None => schedule(),
}
```

Why it works: whatever the waker does, it changes the condition *first* and wakes *after*. So either my look (step 3) sees the change — and I do not sleep — or the change came after my look, and then steps 1 and 2 were already done when the wake-up arrived: it finds me in the queue and marked as waiting.

That leaves one case, and it needs a rule in the policy. The wake-up arrives when I am marked `Blocked` but **have not left my CPU yet** — I am between step 3 and the switch. The old code would put me in the ready queue. Another CPU could take me from there and run me — rule 1's disaster by another road. So:

> **A thread woken before it has left its CPU is not queued. It becomes Running again, where it is.**

When it reaches `schedule` a moment later, the policy sees a running thread and no reason to switch. In the policy this is three lines:

```rust
fn make_runnable(&mut self, id: ThreadId, cpu: usize) {
    if self.cpu_of(id).is_some() {
        self.states.insert(id, State::Running);
    } else {
        self.enqueue(id, cpu);
    }
}
```

and the invariant it keeps is the one to remember: **only a thread that is on no CPU is ever in the queue.**

The kill race of entry 17 is the same race and gets the same order: mark, *then* read the kill flag. `kill` sets the flag first and wakes second. And `sleep`:

```rust
pub fn sleep(ticks: u64) {
    let were_enabled = arch::disable_interrupts();
    announce(State::Sleeping(interrupts::ticks() + ticks));
    if being_killed() { stay(); } else { schedule(); }
    arch::restore_interrupts(were_enabled);
}
```

## Rule 3: a waiter stays in its queue until it leaves

This one is the bug. With rules 1, 2 and 4 in place the kernel booted on four CPUs, passed its new self-test and the whole keyboard-driven boot test. I ran everything again three more times to be sure, and one run hung. So I booted forty times in a loop: about one boot in three stopped in the self-test, with all four CPUs idle.

All idle means no thread is runnable, which means somebody is waiting for a wake-up that will not come. I added a dump of every thread's state at tick 600 and booted until it hung:

```
thread 0 idle Some(Running) cpu Some(0)
thread 1 boot Some(Blocked) cpu None
thread 205 idle Some(Running) cpu Some(1)
...
exited waiters []
```

The boot thread, blocked — and **in no queue**. It was in `join`, waiting in the queue `EXITED` for a thread to end; it was asleep; and the queue was empty. Nobody could ever wake it.

`wake_all` was:

```rust
let waiters = core::mem::take(&mut *self.waiters.lock());   // empty the queue
for id in waiters { wake(id); }
```

It *empties the queue* and wakes whoever was in it. Now the interleaving:

```
boot thread (join, waiting for B)       another CPU (thread A has just ended)
joins EXITED
                                        reaps A; wake_all on EXITED:
                                          takes the list - boot is in it
                                          wake(boot): boot is awake. Nothing to do.
marks itself Blocked
looks: is B gone? No.
sleeps                                  <- in no queue
                                        ...B ends; wake_all on EXITED: nobody there
```

Rule 2 promised that a wake-up after my look finds me "in the queue and marked as waiting". But a wake-up meant for **someone else's event** came *before* my mark, could not wake me because I was awake, and took me out of the queue anyway. Every `join` waits in the same queue, each for a different thread, so wake-ups for other people's events are normal there.

On one CPU, joining the queue and going to sleep were one step — interrupts were off — so nobody could empty the queue with an awake thread in it.

The fix is that waking takes nobody out:

```rust
pub fn wake_all(&self) {
    let waiters = self.waiters.lock();
    for &id in waiters.iter() {
        wake(id);
    }
}
```

A waiter leaves the queue by itself when its wait is over (that code was already there, to leave the queues that had *not* fired). Being in the queue now means "interested", and has nothing to do with being asleep.

After the fix: forty boots of forty.

My argument for rule 2 was right and my code was wrong, because the argument had a silent assumption — *the queue still contains me* — that I never wrote down. The host tests of the policy could not find it: the wait queues are not in the policy.

## Rule 4: a thread is gone when no CPU is on it

`exit` used to mark the thread `Exited`, wake everyone in `join`, and switch away. On one CPU the joiner cannot run until the switch has happened. On two it runs at once, on another CPU, while the exiting thread is still executing its last instructions on its stack.

For `join` itself that might be tolerable. But the self-tests do `join` and then count free memory, and the thread's stack and its process cannot be freed while a CPU is on them. So `join` now means *gone*: `exit` wakes nobody. The next thread to run on that CPU reaps the dead one — the policy refuses to forget a thread while any CPU has it as current — and *then* wakes the joiners.

## The TLB, and a decision not to build something

Each CPU caches the translations it has used, in its TLB. `invlpg` makes the CPU that executes it forget one. It does nothing to the other CPUs.

So what happens when one CPU unmaps a page that another CPU has cached? The other goes on using the old translation — reading and writing a frame that may by now belong to someone else. The usual cure is a *TLB shootdown*: interrupt every other CPU and make it forget too.

Before building that I listed everything the kernel ever unmaps:

| What is unmapped | Can another CPU have it cached? |
|---|---|
| A process's own pages (`memory_unmap`, exit) | **No.** A process has one thread, so it is on one CPU at a time. And a CPU that stops running a process always loads other page tables before that process can run anywhere else — at the very least its idle thread's. Loading page tables empties the TLB. So when a process comes back to a CPU, that CPU remembers nothing about it. |
| Kernel thread stacks, when a thread ends | **Yes.** They are kernel pages, the same on every CPU, and marked *global* so that they survive loading new page tables. A stack freed, and its address later mapped to other frames for a new thread, could be written through a stale entry. |
| The kernel heap, device registers | Never unmapped. |

One real case. And it has a cure that needs no interrupts at all: **never unmap a stack.** A thread that ends gives its stack back whole, still mapped; the next thread to start takes it as it is. Nothing is ever forgotten because nothing ever changes.

The price: the memory for the most threads there have ever been at once stays stacks. Sixty kilobytes each.

So there is no TLB shootdown in this kernel, and I can say exactly why not, and exactly what will make it necessary: a process with two threads, or a kernel that gives stack memory back. The first argument also forbids an optimisation every real kernel has (keeping a process's page tables loaded while idling, "lazy TLB"); the comment on `ACTIVE_ROOT` says so, for the day I am tempted.

## When the kernel stops, everyone stops

Something I had planned for the next entry turned out to be needed for this one to pass its own tests.

When the kernel cannot go on — a fault in its own code, a panic, init ending — it takes the screen, writes why in dark red (entry 25), and halts. "Halts" meant: this CPU. With four CPUs, the other three carried on, and the fault tests failed in a way I could have predicted and did not: *reported on the serial port, but not on the screen*. The display service, alive and well on another CPU, had simply painted the console back over the kernel's last words.

The others have to be told, and telling them must work whatever they are doing — including spinning with interrupts off on a lock the dying CPU holds. There is one message a CPU cannot refuse: a **non-maskable interrupt** (NMI). The local APIC can send one to "everyone but me" in a single write:

```rust
pub fn stop_other_cpus() {
    disable_interrupts();
    if STOPPING.swap(true, Ordering::AcqRel) {
        halt_forever();            // someone else is already stopping the system
    }
    apic::nmi_others();
    for _ in 0..200_000 { core::hint::spin_loop(); }
}
```

and the handler for vector 2, when `STOPPING` is set, halts without a word. The fatal exception path, the panic handler and "init has ended" all call it before they print.

Two details. If two CPUs fail at the same moment, the `swap` lets exactly one report; the other halts. And the NMI gets a stack of its own (it shares the double fault's): it can arrive in the one instant after `syscall` when the stack pointer is still the program's, and the kernel must never push onto that.

## Bringing the CPUs in

The order at boot:

1. Start the other CPUs; count together (entry 44). They wait, spinning.
2. `threads::init()`, and the thread self-test — **on one CPU**. Those checks are about taking turns ("two yielding threads ran as `ababab`"), and turns are only taken where there is one CPU to share. With four, the two threads simply run at the same time and the log is `aabbab` or anything else.
3. `smp::release()`: the scheduler is given an idle thread for each CPU; each CPU starts its timer and becomes that thread.
4. A new self-test, and then everything else, on all of them.

A started CPU's last lines:

```rust
platform::start_timer_here();
IDLING.fetch_add(1, Ordering::Release);
crate::threads::run_idle()        // never returns
```

It *is* its idle thread from there on, on the stack it was started on. Everything else it ever runs, it reaches by a timer interrupt switching away from that loop.

## The self-test

**Threads run on every CPU.** One thread per CPU, none of which ever yields; each notes the number of the CPU it finds itself on. One CPU could run all four in turn by preempting them — but then only one number would ever be noted. The test wants all four.

**No wake-up is lost.** Two threads hand a token back and forth 20 000 times through one wait queue. Before each wait a thread spins for a varying short time, so that it is often still deciding to sleep at the moment the other wakes it — the window rule 2 closes. One lost wake-up and both sleep for ever and the boot never finishes.

Both run twice, and the second run must leave memory exactly as the first did.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Run queues | One, shared | *One per CPU* (what the roadmap said). Faster when there are many CPUs fighting for one lock, and keeps a thread on a CPU whose cache knows it. But it needs a balancer to move threads from busy CPUs to idle ones, and every wake-up must choose a CPU. With four CPUs and a lock held for microseconds, the one queue balances perfectly by itself. It is the right first design; per-CPU queues are an optimisation with a measurement to justify them, and I have no such measurement. |
| The switch race | Hold the lock across the switch | *A per-thread "still on a CPU" flag that the taker waits on* (what Linux does). Finer, more code, same guarantee. |
| The lost wake-up | Mark, then check; undo if not needed | *Hold the condition's own lock until asleep* (the textbook "condition variable"). Would mean passing a lock into every wait; conditions here are closures over anything. |
| Who keeps time | The boot CPU only | *Every CPU.* Then each tick must be divided, or sleepers' deadlines kept per CPU. |
| Stale TLB entries | Avoided: stacks stay mapped; no lazy TLB | *Shootdown interrupts.* Needed one day; today there is exactly one case and it has a cheaper answer. |
| `join` | Returns when the thread is reaped | *When it is marked exited.* Then "nothing leaked" checks would race the exiting thread. |
| The turn-taking self-test | Run before the other CPUs join | *Rewritten for four CPUs.* It tests round-robin, which is a statement about one CPU. |
| Stopping the system | An NMI to every other CPU | *An ordinary interrupt.* A CPU with interrupts off - which is where a CPU waiting for a lock is - would never take it. |
| Waking an idle CPU | It finds new work at its next timer tick, at most 10 ms later | *An interrupt sent to it at once.* That is the next entry. In the common case nothing waits: a thread that wakes another and then blocks hands its own CPU over on the spot. |

## Bugs

The lost wake-up of rule 3 is the bug of this entry, and it is told there. Two things about how it was found:

- **Passing once proved nothing.** The first complete run passed. So did the second boot test. The failure came on the first of three repeats I ran "to be sure". If I had stopped at the first green run I would have committed a kernel that hangs one boot in three.
- **The symptom said almost exactly what was wrong.** Every CPU idle and a thread blocked in no queue: once I could *see* that, the cause took minutes. Getting to see it took a tool I did not have — a dump of all threads — and a loop that boots until it fails and then asks QEMU where every CPU is.

**The fault tests** failed next, for the reason told above under "everyone stops": a problem I had listed as *future work* in entry 44 was a failing test the moment other CPUs did anything.

**And QEMU itself.** Twice in some sixty boots the emulator exited with an empty serial log. The record of CPU resets showed only the firmware's own start-up of the other CPUs: QEMU had ended before the bootloader ran, never mind the kernel. I do not know why (it only happens with more than one CPU). It is not a result about my kernel, so `qemu-boot.sh` now starts again in exactly that case — nothing on the serial line *and* the boot CPU reset no more than the two times every start has. A kernel that resets the machine before its first word resets the boot CPU a third time, and is not retried.

A smaller one: while removing my debugging lines I reverted `interrupts.rs` with `git checkout` and took the real change in that file with them (the timer telling CPUs apart). I noticed because my next command searched the file for a function the change had added, and found nothing.

## How I checked it

**Automated** (`make test64`):

- **20 host tests of the policy** (10 new): two CPUs taking different threads from the one queue; a thread moving to whichever CPU is free; each CPU falling back to its own idle thread; a thread woken before it left its CPU not being queued; the same for a sleeper whose tick comes; a wait taken back; an exited thread not forgotten while a CPU is on it; an idle CPU finding work at its tick; a CPU that joins late. And a randomized test: four CPUs, thirty threads, 40 000 random steps in a random order of CPUs — sometimes with the switch delayed so that another CPU gets in first — checking after every step that no thread is on two CPUs, none on a CPU is also in the queue, none is queued twice, no idle thread is queued or on a foreign CPU, nobody starves, and threads really move between CPUs.
- **The boot self-test** above, on every boot, with 4 CPUs (BIOS and UEFI), and in `smp64-test` with 1, 2 and 20 (of which the kernel uses 16).
- **Everything else**: the user-mode, IPC, file-service and kill self-tests, and the couple of hundred shell commands of the boot test, now run on four CPUs. None of them was changed, except that leak checks count stacks waiting for reuse as unused memory.

**By hand:**

Breaking each rule on purpose and booting with 4 CPUs. This went differently from what I expected, and the table says what happened, not what I hoped:

| Change | Boots | What happened |
|---|---|---|
| Rule 1 broken: the lock released *before* the switch | 12 | **All 12 passed.** |
| The same, with a short delay (2 000 spins) between the release and the switch | 1 | `CPU EXCEPTION 8: double fault` on another CPU, with `rsp 0x246` — a CPU took a thread whose stack pointer had not been saved yet |
| Rule 2 broken: look first, then announce | 3 | The third boot hung after the `[smp] 4 CPUs run threads` line: a lost wake-up |
| Rule 3 broken: `wake_all` empties the queue again | 42 | **All 42 passed** — the bug that hung one boot in three when I found it |
| No `stop_other_cpus` | — | The fault tests fail: *reported on the serial port, but not on the screen* |

Two of the four rules can be broken without one test failing. For rule 1 the widened window proves the bug is real; the tests just do not land in a gap a few instructions wide. For rule 3 I cannot fully explain why the same mistake that hung a third of my boots no longer shows: the kernel around it changed while I fixed it (rule 4 came after), and the window left is between two neighbouring steps of the wait. I tried to write a test that arranges the situation instead of waiting for it, and the test I wrote passed with the bug in — the waiter puts itself back in the queue every time round its loop, so only a wake-up landing *between* "join the queue" and "say I am waiting" does harm, and nothing can be made to run there on purpose. I deleted that test rather than keep one that checks nothing.

So: rules 1 and 3 are held up by the argument in this entry, not by a test. That is weaker than I would like, and it is the truth.

Repeats of the real kernel, after the last change: the whole of `make test64`, then the boot test three more times, the UEFI boot once, `smp64-test` (1, 2 and 20 CPUs) three times and the seven fault boots twice. All passed; QEMU did not die in its firmware once in those.

**Not tested:**

- **Real hardware, and real parallelism.** QEMU without KVM runs each CPU as a host thread; this machine has four cores, so they do run at once, but an emulated CPU executes whole blocks of instructions between looking for interrupts, which makes some interleavings rarer than on silicon and others impossible. A race that needs two CPUs within a few instructions of each other may never happen here.
- **Long runs.** The tests take minutes. Races are found by hours.
- **Rules 1 and 3**, as said above: no test fails when they are broken.
- A CPU that fails to join the scheduler: the message exists and has never been printed.

## What is still missing

- **An idle CPU is not told about new work.** It finds it at its next tick, 10 ms at worst. Telling it at once needs an interrupt from one CPU to another that is *not* fatal — the same mechanism as stopping, with a handler that returns. Next.
- **No affinity.** A thread runs wherever it lands; its cache contents are left behind every time it moves.
- **One lock for all scheduling.** Fine for four CPUs.
- **Device interrupts all go to CPU 0.**
- `ps` does not say which CPU a process is on, and nothing shows how busy each CPU is.

## What I learned

- **Run it again.** The most important thing I did in this entry was to not believe a passing test. One boot in three is a rate you can miss twice in a row with one chance in two.
- **"Interrupts off" was two guarantees, and I had never told them apart.** *Nothing interrupts me*, and *nobody else runs*. On one CPU they are the same thing. On two only the first is true, and every bug here is the second one, missing.
- **The data was never the problem.** Every structure had its lock since entry 4. The problems were all in the *gaps between* locked steps.
- **A decision that has not been carried out is a lie other CPUs can act on.** "A is in the queue" was written while A was still running. Holding the lock until the switch is done makes the statement true by the time anyone can read it.
- **Order beats atomicity.** The lost wake-up is not fixed by making check-and-sleep one step — there is no instruction for that. It is fixed by ordering four events (mark, look; change, wake) so that every interleaving is safe.
- **An invariant in one sentence.** "Only a thread on no CPU is in the queue." The randomized host test checks exactly that sentence 40 000 times, and breaking it on purpose made four tests fail at once.
- **`mem::forget` is safe, and that is surprising.** Leaking a lock guard keeps the lock locked for ever — Rust allows it without `unsafe`, because leaking breaks no memory. The `unsafe` is on the other side: the `force_unlock` that must be exactly the right one.
- **Sometimes the best mechanism is none.** I expected to write a TLB shootdown. Listing what is actually unmapped showed one case, and "never unmap it" needed no interrupts and ten lines.
- **A thread is not on a CPU; it is on a CPU *right now*.** `arch::cpu()` is an answer about a moment. Every use is inside a lock or with interrupts off, and the comment on it says why.

## Testing note

```
make run64
```

**In the log** (the terminal `make run64` was started from)

| Look for | You should see |
|---|---|
| `[selftest] threads: two yielding` | `ran as "ababab"` — still, because this runs before the other CPUs join |
| `[smp] 4 CPUs run threads` | `[smp] 4 CPUs run threads: each has its own timer and idle thread, and all take from one queue` |
| `[selftest] smp:` | `threads ran on all 4 CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked` |
| `FAILED` | Nothing |

Start it five or six times. Every start should get to the prompt: the bug of this entry showed as one start in three stopping after the `[smp] 4 CPUs run threads` line.

**In the window** — the system is now on four CPUs; it should behave exactly as before

| Type | You should see |
|---|---|
| `primes` | The primes, as before |
| `ps` | The process list. Run it a few times |
| `forever &` four times, then `jobs` | Four jobs running. With one CPU the shell would now feel slow; with four it should not: type `echo still quick` and see it at once |
| `kill 1`, `kill 2`, `kill 3`, `kill 4` | Each `forever` stopped *(spinning programs being killed from another CPU; the automated test kills one at a time)* |
| `cat welcome \| wc` if you have a file, or `ls \| wc` | A count: a pipeline's two programs may now run on two CPUs |
| `cask`, a name, numbers, Ctrl+D | As in entry 43 |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | `[selftest] smp: 1 CPU; 40000 hand-offs between two threads, none lost; nothing leaked`, and a working system |
| `make run64 RUN_CPUS=2` | `[smp] 2 CPUs run threads…` and `threads ran on all 2 CPUs at once` |

**On the host**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched
```

should end `20 passed`.

Automated: `make test64`
