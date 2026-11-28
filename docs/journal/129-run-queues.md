# A run queue per CPU (part 1): where a thread waits, and who takes it

*Journal entry #129 · a step, MaK-WSL/sched's K68, part 1, with the placing half of K101. Written on MaK-WSL/sched. Until now every runnable thread waited in one queue. Now each CPU has its own: a woken thread waits at the CPU that is to take it, or at the one it last ran on; an idle CPU takes work from a busy one's queue; and busy threads still share the CPUs exactly fairly. Still under the one lock: splitting it is part 2 (K193).*

## Why

K68's row: "a run queue per CPU, with an idle CPU taking work from a busy one, instead of one queue under one lock". Two things are in that sentence:

- **Per-CPU queues** are where a thread's CPU is decided. They are what K101 (a thread preferring the CPU whose cache has its data) and the `devices` lane's K79 (an interrupt queue per CPU) build on.
- **One lock for all** is what costs on many CPUs: every switch on every CPU takes it. Removing it means each CPU's switch takes only its own queue's lock, and a thread moving between CPUs takes two, in an order. Every switch holds the lock across it today (rule 1 of `threads.rs`), and entries 104 and 105 were spent finding races in exactly this code. So that half is its own step, K193, on top of this one: the queues first, tested on the host; the locks after.

## How it works

```
 CPU 0's queue: [ a(0) ]        CPU 1's queue: [ c(0) d(-5) ]
 CPU 0 chooses: d - the most important allowed, from any queue
 then, among equals, the one that joined first, from any queue
```

- **Placing** (K101's half). A thread made ready goes to the queue of the CPU that is asked to take it: **this CPU, if it idles** (it takes the thread at once, no interrupt); else **the CPU it last ran on, if that idles** (its data may still be in that cache); else any idle CPU; else the CPU running the least important thread, if that is less important. If none takes it, it waits in its **home**: the allowed CPU with the fewest waiting, its last CPU first among equals.
- **Choosing** stays strict across the machine: a CPU takes the most important thread it may run, from any queue, and among equals the one that joined a queue first, whichever queue that is. So an idle CPU takes work from a busy one, and nobody waits behind a less important thread on another CPU.
- **What each thread carries**: its last CPU (set when it goes to wait, and for a new thread the CPU that made it), next to its priority and affinity; each queue entry, the moment it joined.

## The numbers

`schedbench`, MaK-WSL under KVM, on the machine as it now is (about 1.5 times slower than in entry 124 since a restart). `main` is entry 128.

| | `main` | this branch |
|---|---|---|
| yield, 1 / 2 / 4 CPUs | 495 / 431 / 668 ns | 464 / 717 / 557 ns |
| four busy programs, 2 s, two CPUs: CPU each | 990 997 1002 1002 ms | **998 998 998 999 ms** |
| woken with every CPU busy at nice 0, median, 2 CPUs (three runs) | 12.0, 1.4, 0.8 ms | 16.8, 1.5, 7.1 ms |
| the same, 4 CPUs | 7.6, 0.4, 1.2 ms | 17.8, 5.2, 0.8 ms |
| woken with every CPU busy at nice 10, median | 1 / 34 / 39 us | 1 / 24 / 25 us |

- **Switches cost the same**, within this machine's spread.
- **Sharing is exact** (bug 1 is how it was not).
- **The woken thread with equals busy** varies from run to run on both, between under a millisecond and most of a slice: it waits for some CPU's slice to end, and where the slices fall is chance. The branch is higher in two runs of three. The choosing for this case is now the same as `main`'s, so I have no cause; part 2 (K193) rebuilds this path and measures it again.

## Design decisions

| Decision | Instead of | Why (and what it costs) |
|---|---|---|
| The oldest of the most important, from any queue | A CPU's own queue first among equals | Fairness: own-queue-first let one of four busy programs keep a CPU nearly to itself (bug 1). It gives up part of K101 - a thread is not kept on its CPU while others wait as long |
| An idle deciding CPU takes a woken thread itself | Sending it to the CPU it last ran on | The quickest a woken thread can run is now, on a CPU that is already awake; sending it elsewhere costs an interrupt (and under KVM, waking a halted CPU, K192) |
| A home with the fewest waiting | Always the last CPU | Busy threads spread over the CPUs |
| The queues first, the locks in K193 | Both at once | The locking is where the races were (entries 104, 105); the policy is tested on the host first |

## Bugs

**1. One busy program kept a CPU nearly to itself.** My first version had each CPU take from its own queue first among equals, for the cache. The host tests passed, including one of three busy threads on two CPUs. The benchmark did not: four busy programs on two CPUs got 819, 837, 839 and **1497** ms, every time (three runs). A new host test of 3, 4 and 5 threads on 2 CPUs and 5 and 6 on 4, with slices ending in random order, passed too - so the kernel calls the policy in some pattern my model does not make, and I have not found which. Rather than tune a rule I could not explain, choosing is now FIFO across all queues, which is as fair as one queue for all by construction: 998, 998, 998, 999 ms.

**2. My first rule sent a woken thread back to its last CPU even when the CPU deciding was idle.** An existing test (`a_thread_runs_on_whichever_cpu_is_free`) failed: woken by idle CPU 1, the thread went to CPU 0 by an interrupt instead of running on CPU 1 at once. The test was right: this CPU first if it idles, then the last one.

**3. Counting a queue's threads walked forty lists.** Finding a home asks each CPU how many wait; that was a sum over the forty priority levels, on every enqueue. Each queue keeps its count now.

## How I checked it

- `cargo test` of the policy: 42 tests, 5 new - oldest first across queues and an idle CPU taking work from a busy one's queue; a more important thread in another CPU's queue before this one's own; a woken thread back to its idle last CPU when the deciding CPU is busy; three busy threads on two CPUs in lockstep; and 3 to 6 threads on 2 and 4 CPUs with slices ending in random order, each within 30 % of its share.
- The randomized runs (40 000 steps on four CPUs, with priorities and affinities) check, after every step, that no thread is queued twice or while on a CPU, and that no thread waiting for a CPU is more important than the one it chose.
- `kernel64-test`, `smp64-test`, `aarch64-test`; `schedbench64-test` six times, against `main` three times.
- `make test64`.

## What is still missing

- **K193**: a lock per CPU's queue instead of one for all (K68's other half).
- **K101's other half**: keeping a thread on its CPU while equals wait elsewhere - the rule bug 1 took out. With K193, and with a test that reproduces bug 1 on the host first.
- Why the host model did not reproduce bug 1.

## What I learned

- **A model that passes is not the system.** The policy's host tests are the best tool this project has for the scheduler, and here they passed a rule the benchmark failed at once. The benchmark is part of the test.

## Testing note

```
make schedbench64-test
```

| Look for | You should see |
|---|---|
| `4 busy programs, 2 s: CPU each` | On 2 CPUs about 1000 ms each, all four - not three at about 830 and one at 1500 |
| `schedbench64-test passed` | |

Automated: `make test64`
