# A lock per CPU's run queue: the switch stops waiting for the other CPUs

*Journal entry #NNN · a step, MaK-WSL/sched's K193 (K68's other half). Written on MaK-WSL/sched. Until now one lock covered every queue, every thread's state and the thread table, and every switch on every CPU held it - across the switch itself. Now each CPU's queue has its own lock, a CPU choosing reads the other queues' published first keys without their locks ("peek, then lock one"), and a switch holds no lock at all while it changes stacks: a flag per thread says when its registers are saved.*

## Why

Entry 129 gave each CPU a queue of its own, but left them under the one lock that every scheduling decision took: `THREADS`, around the table of threads and the whole policy. Every switch, every tick, every wake-up and every `getpid`-sized question about the current thread took it - on every CPU. And a switch held it across `context::switch` (rule 1 of `threads.rs`), so while one CPU changed stacks, no other CPU could even look at its own queue.

On two CPUs that is cheap. On sixteen it is the whole machine waiting for one cache line. K193 splits it, so that two CPUs share a lock only when they touch the same thing.

## The decision: exact fairness, or fewer locks

One lock made one thing easy: choosing **the oldest of the most important** threads across all queues, which is what made four busy programs on two CPUs get 998 ms each in entry 129 (bug 1 there: taking from a CPU's own queue first gave one of them 1497 ms). With a lock per queue, a CPU can no longer look at every queue at once without taking every lock. There were three ways:

| | What a choosing CPU does | Fairness | Cost |
|---|---|---|---|
| **Peek, then lock one** (chosen) | Reads each queue's published first key - level and join number, one atomic - without its lock; locks only the queue it takes from, and checks again there | Exact unless two CPUs change queues in the same instant: then one thread off, never one that must not run | One atomic read per CPU per choice |
| Own queue first, rebalance at ticks | Takes from its own queue; queues are evened out at each tick | About a slice off (entry 129's 1497 ms) | Cheapest |
| Keep one lock | - | Exact | What K193 is meant to remove |

I chose the first (2026-11-27).

## How it works

### What each lock covers

```
 a thread's settings lock ──▶ one CPU's lock ──▶ (sleepers' lock)
   (wake-ups, new priorities)   (its queue, the thread on it,
                                 its slice, `rq` of its threads)
 a second CPU's lock: waited for only if its number is higher;
 a lower one is only tried (64 times), and passed over if busy
```

- **Each CPU's lock** covers its queue, the thread it runs, its slice, and where its threads are (`rq`). A switch takes only its own - and, if it takes a thread from another CPU's queue, that one's too, for a moment.
- **Each thread's settings lock** is taken by whoever changes the thread from outside: a wake-up, `set_priority`, `set_affinity`. Two CPUs waking the same thread take turns, so it is queued once.
- **The directory** (thread number to record) is locked only for a look-up, an addition or a removal.
- **`THREADS`** (in `threads.rs`) now covers only the table of threads: taken to spawn, to reap and to list - never by a switch, a tick or a wake-up.

**Why no two CPUs can wait for each other.** Locks are taken in one order: a thread's settings, then a CPU's. A second CPU's lock is waited for only if its number is higher than the one held, and a lower one is only tried. Somebody holding a higher-numbered lock never waits for a lower one - so whoever waits for a higher one is waiting for somebody who will finish.

### Ready and running are one state

The policy used to change a thread's state in a table: `Ready` when queued, `Running` when on a CPU. With a lock per CPU, the CPU choosing would have had to take the thread's lock as well as its own - in the wrong order for a waker, who holds the thread's lock and wants the CPU's. Linux's answer, used here: one kind, **runnable**, and *where* the thread is (`rq`, under that CPU's lock) says whether it waits or runs. A waker changes only the kind (blocked → runnable), and the CPU reads it under its own lock.

```rust
// A waker, holding the thread's settings lock:
let on = rec.rq.load(SeqCst);
if on == NOWHERE {            // left its CPU: queue it
    rec.kind.store(RUNNABLE, SeqCst);
    self.enqueue(rec, settings, cpu);
} else {                      // said "block" but has not left yet
    let _guard = self.cpus[on].local.lock();
    if rec.rq.load(SeqCst) == on {
        rec.kind.store(RUNNABLE, SeqCst);   // it stays
    }                         // else it left meanwhile: look again
}
```

That `lock()` is the one that matters most. Without it, the CPU the thread is on can decide "blocked: off this CPU" in the same instant as the waker decides "still on its CPU: it stays" - and the thread is runnable, in no queue and on no CPU, for ever. The host test of CPUs at once finds exactly that, every run, when the lock is taken out (How I checked it).

### Peeking

```
 CPU 0 queue: [ 23 (-1, CPU 0 only)  19 (3) ]   publishes (-1, joined 194)
 CPU 1 queue: [ 6 (1) ... ]                     publishes ( 1, joined 198)
 CPU 2 queue: [ 20 (-1)  17 (0) ]               publishes (-1, joined 197)

 CPU 1 chooses: CPU 0's key is best - lock it, look inside: 23 may not
 run here; the best it has for CPU 1 is 19, behind CPU 2's key. Note
 that, let it go, look into CPU 2: take 20.
```

After every change to a queue, under its lock, it publishes the key of its first thread: priority level in the top byte, join number below - one `u64`, one atomic store. A choosing CPU compares its own queue's best (exactly, under its own lock) with the others' published keys, locks the queue with the best, and checks there. A queue's first thread may be one this CPU may not run; then what that queue has for this CPU is further back, and may lose to another queue's first - so a queue looked into is from then on judged by what was found in it, and taken from only if that still beats every other (bug 2).

### Rule 1, without a lock across the switch

The one lock held across the switch existed for one moment: a thread that gives way is put in a queue *before* its CPU has saved its registers. Another CPU taking it then would load registers not yet saved - two CPUs on one stack.

Now each thread has a `leaving` flag:

1. `schedule` sets it on the running thread **before** `pick_next` may queue it;
2. the CPU switches away, and the first thing the thread switched *to* does (`switched`, also in `thread_entry`) is clear the previous thread's flag;
3. a CPU about to load a thread waits while its flag is set.

```
 CPU 0: mark T leaving ─ pick_next (T queued) ─ save T ─────▶ [U] clear T's flag
 CPU 1:                        take T ─ wait ······ flag clear ─ load T
```

The wait cannot become a cycle: a CPU waits only for a thread that was queued by a `pick_next` that had finished before this CPU chose it, and that CPU's own wait (if any) is for a thread queued earlier still.

`rule_1_holds` changes with it: after every switch, the thread just left must have been marked. A `schedule` that forgot to mark it fails at the first switch of every boot.

### Nobody sleeps with work waiting

A CPU about to idle says so (`idling`), *then* looks at the queues once more. A thread being queued is published *before* the queuer looks for an idling CPU to tell. Both are sequentially consistent, so at least one sees the other.

## The numbers

`schedbench`, MaK-WSL under KVM, 1 / 2 / 4 CPUs. Three runs each; `main` is entry 130.

| | `main` | this branch |
|---|---|---|
| yield (a switch), 1 / 2 / 4 CPUs | 693-824 / 755-821 / 801-888 ns | 462-768 / **352-470** / **314-329** ns |
| hand-off, one way, 1 / 2 / 4 CPUs | 1741-2165 / 7141-10435 / 11593-17335 ns | 1426-2266 / 7483-11545 / 7311-9782 ns |
| four busy programs, 2 s, two CPUs: CPU each | 994-1000 ms | 985-1002 ms |
| wake-to-run, two CPUs: median | 46, 53, 48 us | 71, 56, 53 us |
| woken with every CPU busy at nice 0, median, 2 CPUs | 13.7, 13.4, 13.9 ms | 0.6, 16.6, 1.2 ms |
| the same, 4 CPUs | 7.3, 1.4, 4.1 ms | 5.0, 5.4, 5.1 ms |

- **A switch on two or four CPUs costs half what it did.** Every switch used to take the one lock, which another CPU's tick or switch was likely to hold - and to have in its cache. Now it takes its own CPU's lock, which nobody else usually wants. On one CPU there was never anyone to wait for, and the numbers are within this machine's spread.
- **Sharing is as exact as before** (peeking, not own-queue-first): 985-1002 ms each.
- **The woken thread among equals** waits as long as before, or less: it still waits for some CPU's slice to end, and any CPU's may. This row was 15 ms on four CPUs before bug 6 was found - it is how bug 6 was found.
- **Wake-to-run between two CPUs** is K192's (KVM waking a halted CPU, 46-109 us since entry 128) and is within its spread.

## Design decisions

| Decision | Instead of | Why (and what it costs) |
|---|---|---|
| Peek, then lock one | Own queue first; one lock | The user's choice: exact sharing (entry 129, bug 1) without every lock |
| Ready and running one state, where-it-is under the CPU's lock | A state per thread under its own lock | The choosing CPU would need the thread's lock after its own - the wrong order for a waker. Linux does the same |
| A `leaving` flag per thread | Holding a lock across the switch | The lock made every CPU wait for every switch. The flag makes only the CPU that takes that very thread wait, and only for the few instructions that save it |
| A thread that gives way waits in its own CPU's queue | Its "home" (the CPU with fewest waiting), as in entry 129 | Its own CPU's lock is the one held; another would need a second lock in the middle of a switch. An idling CPU is still told, and takes it from there |
| A stack freed before the policy forgets its thread | Forgotten, then freed (as before) | `join` waits for the policy to forget it, and must find the stack back (bug 9) |
| A lower-numbered queue only tried, then passed over | Waiting for it | Waiting both ways is how two CPUs deadlock. Passed over, the CPU asks itself to choose again, and `schedule` does at once, its own lock let go (bug 6) |
| The directory under its own lock | No directory (handles everywhere) | Wake-ups name a thread by number (`WaitQueue` holds numbers). Taken for a look-up only |

## Bugs

**1. A sleeper left behind in its set.** The randomized tests: "the sleeper sets and the states disagree: (110, 6)". A thread that had said "sleep" and then, before it left its CPU, something else (killed: exited) kept its old entry in the sleepers' set. The old code removed it on every state change (`set_state`); my `suspend_current` only added. Fix: an earlier sleep's entry goes first.

**2. The first queue looked into won.** `randomized_with_priorities_and_affinity`, step 498: "CPU 1 chose 9 over the more important 20". CPU 0 published the best key - thread 23, which may run only on CPU 0. CPU 1 locked CPU 0's queue, found 9 there (priority 1), and took it, because it was better than its own queue's best. CPU 2's first thread, 20 (priority -1), was better still. Fix: a queue looked into is judged by what was found in it, and taken from only if that beats every other queue's key; otherwise the next is looked into (`choose`).

**3. A test that could not fail.** The second look before idling ("Nobody sleeps with work waiting"), taken out on purpose, passed the CPUs-at-once tests every time. Three reasons, each in the test: every CPU's tick finds waiting work for an idle CPU; my idle CPUs chose at random, which no halted CPU does; and the thing a missed look loses is time, not a thread - a thread is always queued at a CPU that may run it, and that CPU sees its own queue exactly. So the invariant ("no runnable thread lost") cannot see it. Fixed in the tests: idle CPUs choose only when asked, a variant runs without idle ticks, and a deterministic test (`a_cpu_about_to_idle_takes_a_thread_woken_as_it_looked`) makes the race happen: a hook in the host tests (`race_window`) lets "CPU 0" wake a thread at the very moment CPU 1 has looked and not yet said it idles. It fails without the second look ("CPU 1 idles, not asked, while thread 3 waits in busy CPU 0's queue") and passes with it.

**4. Tests of mine that were wrong** (not the policy), all on the first runs of CPUs at once: a check that the running thread may run where it runs - but another CPU may change its affinity at any time, and it then runs until its CPU chooses again, which it has been asked to (moved into the stopped-CPUs check); a barrier that left the other CPUs waiting for ever when one failed (replaced by `Gate`); "every thread runs once woken" with strict priorities, where a busy important thread rightly keeps a less important one waiting (all at one priority for that phase); and "an idling CPU not asked has no thread it may run", which entry 124's one-offer-per-idler rule breaks on purpose (now: each queued thread has a CPU it may run on that is busy or asked).

**5. Testing while editing.** My first boot test of the policy alone (under the old lock) ran three tests in a row in this tree; I began editing `threads.rs` while the third, `aarch64-test`, was building, and it built my half-edited file and failed. The x86_64 two had built before the first edit (by their logs' times). Lesson kept: a tree under test is not edited.

**6. Passed over, a CPU kept its thread a whole slice.** The benchmark's "woken with every CPU busy" on four CPUs: 15, 17, 15 ms, against 1.4-7.3 on `main`, three runs. 15 ms is about what it costs if only *one* CPU ever takes the woken thread: the one whose queue it is in, which reads its own queue under its own lock. The others, at the end of their slices, tried that queue's lock and - when it was a lower-numbered CPU's, busy at that moment - passed it over after 64 tries, as designed; and then, with nothing better found, kept their own thread for a fresh slice. Under KVM a CPU holding a lock can be stopped by the host for a while, so passing over was not rare. The experiment that showed it: a million tries instead of 64 brought the row back to 5.9 and 6.6 ms. A million is no fix - the lower CPU may be waiting for this one's lock, and both would spin - so instead `schedule` chooses again at once, up to four times, when the policy passed a queue over (it asks its CPU to choose again; `schedule` sees that and lets go of its lock between tries, so the other CPU can finish). After: 5.0, 5.4, 5.1 ms.

**7. A check that counted lines from their start.** The second full `make test64` failed in `aarch64-keys-test`: "the new registry was not told fs's namespace". It was told - the log has `er[registry] fs may ask for: disk`: another program's line was cut in two on the serial line, and its end ("er") landed before the registry's line. `check-boot.sh` counted lines that *begin* with the announcement. Now it counts it anywhere in a line, as entry 104 did for cprimes' answers; both announcements are still required. With the CPUs truly at once more often, such cut lines may come more often too.

**8. `unsafe-check` (K112)** failed the first `make test64` of all: one SAFETY comment written above four unsafe blocks in `schedule`. Each has its own now. `cargo build` does not run that lint; `make unsafe-check` does, and I should have run it before the full suite.

**9. `join` returned before the stack was back.** The race hunt, first 20 rounds: 3 failed with the same panic in three different parts' boots - "threads on several CPUs leaked frames", 128095 against 128080, 127037 against 127022, 128453 against 128438. Always 15 frames: exactly one kernel stack (a 64 KiB slot, one page kept unmapped as its guard). The SMP self-test runs its work twice, joins every thread, and counts the free frames right after. `reap` used to have the policy forget an exited thread and take it out of the table under the table's lock, and free its stack after, outside it; `join` waits for the policy to forget the thread. So a joiner that looked in between - and it looks again at every exit, since every `join` waits in one queue - returned while the stack was still on its way back. The window was there before K193; CPUs truly at once made it a few microseconds wide often enough. Now the policy forgets a thread only after its stack is freed (`removable` asks, without changing anything; `remove` comes last), and of two CPUs reaping at once, the one that takes it out of the table frees it. No test can put a joiner in that gap on purpose; the evidence is the race hunt after the fix (How I checked it).

**10. Not explained: a kill that took 5 ticks, three times in one boot.** The same first race hunt, round 6: `e1000-64-test`'s 82574L boot, **one CPU**, panicked at "forever spin took 5 ticks to end, three times over" (up to 3 is allowed, and each kill is measured up to three times since entry 105). On one CPU the queues, the peeking and the passing over all do nothing, and the switch's only change is the `leaving` flag. In that boot the network services were running beside the self-test, and on one CPU a woken thread waits behind any busy thread of its priority, as it always did; the race hunt runs twelve machines at once on this laptop. I have no cause, and leave it open as K?-slow-kill unless it comes back.

**11. A date check that counted the wait for a CPU.** The second race hunt (after bug 9's fix), rounds 2 and 4 of 11: `basics64-test` failed with "TheTaaJ's date (27 November 2026, 17:03:23 UTC) is 404 s from the host's" and the same at 407 s (at most 300 allowed); nothing else failed. TheTaaJ's clock was right. I had run two other branches' boot tests (K130's, K46's) outside the test lock beside the hunt, and `boot-slots.sh` - which keeps the boots at once to the CPUs there are - made the hunt's boots wait about 6.7 minutes (the 400 s) for a slot before QEMU started. `basics-test.sh` took the host's time before that wait. Two fixes: the check now requires TheTaaJ's date to lie between the host's time before the boot and after it (5 s either side, the RTC's whole seconds), which holds however long a boot waits, and still fails a clock that is wrong by more than the test's own length; and boots during a hunt or `make test64` go under the lock (my mistake, not the test's).

## How I checked it

- **The policy on the host: 47 tests.** The 41 of entries 124-129 kept their meaning (the API is the same; only `&mut` became `&`); five new ones run CPUs at once (2, 4 and 8 host threads, 40 000-60 000 steps each, with and without idle CPUs' ticks): each host thread does what one CPU does - ticks, sleeps, blocks, wakes any thread, changes priorities and affinities, switches - and carries out each switch as `schedule` does (let go of the thread left, wait until no other CPU has the one taken, claim it; a thread two CPUs took fails the claim). Every 2000 steps all stop and `check_consistent` checks everything against everything: each runnable thread on one CPU or queued once, `rq` saying where, no waiting thread queued, the sleeper sets exact, each queue's published key its real first, and each queued thread with a CPU that is busy or asked. At the end every thread is woken and must run. And one deterministic test of the race the second look before idling closes (bug 3).
- **The tests were shown to catch what they claim**, by breaking the code on purpose and running them again:

| Broken on purpose | Caught |
|---|---|
| A wake-up does not take the CPU's lock before keeping a thread on it | Every run: "thread 1 is runnable but in no queue and on no CPU: lost" |
| No second look before idling | Only by the deterministic test (bug 3) |
| `choose` takes from the first queue it looks into | The randomized priorities-and-affinity test (bug 2 was this) |

- **In QEMU under KVM**: `smp64-test`, `aarch64-test` (all), `kernel64-test`, `schedbench64-test` six times (three before bug 6's fix, three after), and `make test64`: all passed on the third run, after bugs 7 and 8 (1152 s).
- **The race hunt** (`tools/race-hunt.sh`, entry 105; the 37-38 x86_64 parts of test64, under KVM, 12 at once): the first 20 rounds found bugs 9 and 10 (3 and 1 rounds). After bug 9's fix, 20 more rounds: 18 passed whole; the other 2 failed only in `basics64-test`'s date check, which was my own load (bug 11). Rounds 12-20, run after bug 11's fix and with `main` merged in (entry 140), all passed (567-639 s each). Bug 10 did not come back in those 20 rounds.
- **After `main` was merged in** (entry 140): `make test64` all passed (824 s).
- **Not tested**: more than four CPUs in QEMU (`schedbench` runs 1, 2 and 4); real hardware.

## What is still missing

- **K101's other half**: keeping a thread on its CPU while equals wait elsewhere - with this entry's locks in place, a CPU's own queue first among equals is again possible, with a test that reproduces entry 129's bug 1 on the host first.
- **The directory lock**: wake-ups still look a thread up by number under one lock (short, but shared). `WaitQueue` holding records instead of numbers would remove it.
- **The join counter** is one atomic every queueing CPU adds to: cheaper than a lock, still one shared cache line.
- More than 16 CPUs (K74), and idle CPUs that sleep deeper (K72): the published keys are what both would read.

## What I learned

- **A stress test is only as good as the bugs it is seen to catch.** The CPUs-at-once test passed at once - and so did the policy with a lock taken out, until I made the test's CPUs behave like the kernel's. Breaking the code on purpose, once per property, is what showed which properties it checks.
- **The lock across the switch was protecting one moment**, not the whole switch: between a thread being queued and its registers being saved. A flag on that thread protects the same moment, and only the CPU that wants that very thread waits.

## Testing note

```
make schedbench64-test
```

| Look for | You should see |
|---|---|
| `yield (a switch)` | On 2 and 4 CPUs about 300-500 ns - about half of the 1-CPU number, where before K193 it was more |
| `4 busy programs, 2 s: CPU each` | On 2 CPUs about 1000 ms each, all four |
| `wake, every CPU busy at nice 0: median/worst` | On 4 CPUs a median of a few ms - not about 15 000 us (bug 6) |
| `schedbench64-test passed` | |

On the host, the policy's tests, CPUs at once among them:

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched::
```

| Look for | You should see |
|---|---|
| `test result:` | `ok. 47 passed; 0 failed` |

Automated: `make test64`
