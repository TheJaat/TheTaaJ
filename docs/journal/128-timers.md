# Timers finer than a tick: one-shot timers, sorted sleepers, and late interrupts

*Journal entry #128 · a step, MaK-WSL/sched's K24 with K96 and J53. Written on MaK-WSL/sched. Until now the kernel's only sense of time for sleepers was the 10 ms tick: a program asking for 1 ms slept 10 to 20, an alarm rang at the next tick after its time, and every tick looked at every thread to find the sleepers. Now each CPU's timer is one-shot, set for its next tick or for the first sleeper due before it; sleepers wait in sorted sets; and a self-test makes the timer interrupts late on purpose, which catches a mistake no test caught before.*

## Why

- **Sleeps were tick-sized.** `nanosleep(1 ms)` took a whole tick or two: the kernel's SLEEP counted ticks, and libc rounded up to the next one. A program that paces itself (a game loop, an audio buffer, a retry with a short back-off) could not wait less than 10 ms.
- **Alarms were tick-sized**, and there was no `setitimer`, `timer_create` or `clock_nanosleep` at all (K24's row).
- **Every tick looked at every thread** (K96): the boot CPU's tick went through the whole state map to find sleepers whose time had come, and collected them in a new `Vec` - an allocation in an interrupt, a hundred times a second, however many threads were only blocked.
- **Entry 097's ticks mutation went uncaught** (J53): counting ticks from interrupts again instead of from the counter changed nothing any test saw, because on an idle QEMU interrupts are never late.

## How it works

### Sleepers, sorted (K96)

```
 per CPU:  sleepers[cpu] = { (when, thread), ... }   sorted by when
           a tick (or an early interrupt):  wake the first ones that are due - and stop
```

- The scheduler keeps the sleepers in **one sorted set per CPU** - the CPU each went to sleep on - next to the state map. `set_state` is now the only way an added thread's state changes, and it keeps the sets right: a thread woken early by a futex or a message leaves its set there, whatever woke it.
- A tick wakes the first sleepers of each set while they are due, and stops at the first that is not: no thread that is not due is looked at, and nothing is allocated.
- Host tests: the order, equal times, an early wake, a sleep taken back; a sleeper on CPU 1 woken by CPU 0's interrupt; and both randomized runs (40 000 steps on four CPUs) now check after every step that the sets hold exactly the Sleeping threads, each in the set its CPU says.

### One-shot timers (K24)

```
 CPU's timer:   ---- tick ------------ tick ------------ tick ----      every 10 ms, as before
 a sleeper:              ^ due at 3.2 ms
 the timer is set for:   ^ (an early interrupt: wake it, count no tick) then the next tick
```

- **Each CPU's timer interrupts once**, and is set again at every interrupt: for its next tick (stepped from the last tick's time, so ticks do not drift), or sooner for the first thread that went to sleep on this CPU. On x86 the local APIC timer is now one-shot (it was periodic), its count worked out from the time-stamp counter; on AArch64 the timer always was a compare value, which is now set to the earlier of the two.
- **An early interrupt is no tick**: it wakes the sleepers due and sets the timer again, and counts no slice or tick. The logic that tells the two apart (`after_interrupt`) is a host-tested function shared by both CPUs.
- **Sleepers' deadlines are nanoseconds** of the clock, not ticks. A tick-based sleep (the old SLEEP call, `OBJECT_WAIT_UNTIL`, futex timeouts) is the same deadline written in nanoseconds - tick *k* is exactly *k* × 10 ms - so it means what it meant.
- **Any CPU's interrupt wakes whoever is due**, not only the boot CPU's. A sleep costs one extra interrupt, on its own CPU: why the sets are per CPU, instead of every CPU setting its timer for the first sleeper of all.
- **The tick count is read from the counter by every reader** (`interrupts::ticks()`), never going back. A sleeper woken by its own CPU's timer a hair before the boot CPU's tick must not find the count a tick short: the boot self-test's `sleep(5)` would have seen 4.

### The calls

| Call | What |
|---|---|
| **`SLEEP_UNTIL` (47)** | Sleeps until the monotonic clock reads a nanosecond. Interrupted by a caught signal or a kill |
| `ALARM` (38) | Two more arguments: an **interval** (it rings again every interval, stepped from when it was due; intervals missed ring once) and **keep** (change nothing, only say what is left). Old callers pass zeros: unchanged |

In libc (`timers.rs`, and `nanosleep` and `usleep` in `system.rs`):

| Function | Here |
|---|---|
| `nanosleep`, `usleep` | To the nanosecond the timer gives (SLEEP_UNTIL) |
| `clock_nanosleep` | CLOCK_MONOTONIC and CLOCK_REALTIME, relative or TIMER_ABSTIME; an error number, as POSIX says |
| `setitimer`, `getitimer` | ITIMER_REAL; ITIMER_VIRTUAL and ITIMER_PROF are EINVAL |
| `timer_create`, `timer_settime`, `timer_gettime`, `timer_getoverrun`, `timer_delete` | One timer per program, SIGALRM; a second is EAGAIN |

A process has one alarm in the kernel, and `alarm`, `setitimer` and the `timer_create` timer are all it: setting one replaces the others. Linux keeps them apart; a program using two at once is rare, and the header says so.

### The alarms thread starts at boot

It used to start with the first alarm, so that a system none set never had it. clib now sets alarms, and the boot test's "`ps` before and after running programs" saw one thread more after (14, then 15) - the alarms thread, which stays. It is not a leak, but the check is right to want the counts equal, so the thread now starts at boot, before init. Waiting, it costs nothing: a tick looks only at the sleepers due.

## The numbers

MaK-WSL under KVM. clib, on both CPUs:

| | Before | After |
|---|---|---|
| `nanosleep(1 ms)`, the least of twenty | 10-20 ms (a tick, rounded up) | **1.025 ms** (x86), **1.095 ms** (AArch64) |
| `setitimer` every 5 ms, for 60 ms | - (no setitimer; alarms to the tick) | **12** signals |

`schedbench` (entry 116), `main` and this branch measured back to back on the same machine (which since a restart runs everything about 1.5 times slower than in entry 124: arithmetic 66 ms against 44, on both):

| | `main` (KVM) | branch (KVM) | `main` (TCG) | branch (TCG) |
|---|---|---|---|---|
| yield, 1 / 2 / 4 CPUs | 671 / 709 / 783 ns | 672 / 735 / 711 ns | 5779 / 4010 / 4825 ns | 4230 / 4109 / 4777 ns |
| hand-off, 1 / 2 / 4 CPUs | 1464 / 8296 / 9076 ns | 1845 / 8094 / 11462 ns | 12902 / 59955 / 81790 ns | 14177 / 55266 / 80508 ns |
| **wake-to-run median**, 1 / 2 / 4 CPUs | 4 / 4 / 40 us | 5 / 46 / 47 us | 61 / 140 / 202 us | 84 / 122 / 127 us |
| woken with every CPU busy at nice 10, median | 3 / 22 / 37 us | 2 / 27 / 35 us | 48 / 112 / 189 us | 66 / 110 / 117 us |

- **Switches and hand-offs cost the same.** The sorted sets and the timer are not on that path.
- **Wake-to-run on two CPUs is slower under KVM, and only there** - a median of 46 us against 4 (bug 5). Under TCG the branch is the same or quicker. See bug 5 and K192.

## Design decisions

| Decision | Instead of | Why (and what it costs) |
|---|---|---|
| One-shot timers, set each interrupt for the next tick or a sleeper | A tickless kernel (no tick while idle) | A sleep gets its own interrupt; everything else - slices, the console's flush, CPU time - still counts ticks, unchanged. Tickless (an idle CPU sleeping until its next event) is the next step, and needs slices kept another way |
| A sorted set per CPU | One global sorted set | With one set every CPU would set its timer for the first sleeper of all: a sleep would cost an interrupt on every CPU |
| Any CPU's interrupt wakes anyone due | Only the boot CPU's | A sleeper's own CPU's early interrupt must be able to wake it |
| Deadlines in nanoseconds, tick deadlines converted | Two kinds of deadline | One scale for the policy; a tick deadline converts exactly |
| `ticks()` read from the counter | Written only by the boot CPU's tick | A sleeper woken early must see the count its deadline means. It is a counter read and a division per call |
| One alarm per process for alarm, setitimer and timer_create | Several timers per process | The kernel's alarm list stays one entry per process. Several timers (and SIGEV_THREAD) are K191 |
| SLEEP_UNTIL a new call | SLEEP in nanoseconds | SLEEP's ticks are in the ABI, and programs use them |

## Bugs

**1. The tree changed between my test run and my commit.** The K96 commit (`c996206`) had `let _ = until;` where `set_state` takes a woken sleeper out of its set - a line I did not write, the shape of a mutation experiment. The tests I had just run cannot have run against it: put back on purpose, four of them hang (`wake_sleepers` loops for ever on a sleeper that never leaves its set). Two other sessions were running on the machine; which of them, or what, changed the file, I could not tell. Fixed in the next commit (`a15b60e`), and nothing else in the commit differed from what I wrote. Since then I read the diff of every commit before making it. Every session must keep to its own tree (`docs/lanes.md`).

**2. A thread left behind after the boot test's programs.** `boot64-test` failed "running programs left processes, threads or channels behind": `ps` said 14 threads before and 15 after. The new clib checks set the first alarm, which started the kernel's alarms thread - which stays. Not a leak, but the check rightly wants the same counts: the alarms thread starts at boot now.

**3. `setitimer` said no interval while its timer was running** - only in clib compiled by tcc on TheTaaJ, whose slower code ended the 60 ms wait just as the alarm was due. A repeating alarm that is due but not yet rung by the alarms thread had "0 left", which libc (as POSIX) reads as "no timer", so its interval was reported as 0. An alarm still to ring now says at least 1 ns left.

**4. The AArch64 masked-interrupts self-test counted ticks.** It checks that no timer interrupt arrives while interrupts are masked, by watching the tick count; the count now follows the counter, so it moved on (3 ticks in 35 ms) and the boot panicked. It counts interrupts now, and still checks that the ticks were all counted once unmasked.

**5. (A finding, open: K192.) Waking a thread on another CPU is slower under KVM.** The wake-to-run median on two CPUs went from 4 us (`main`) to 46-109 us under KVM. Found by measuring `main` and the branch back to back. What it is not:
- not the shorter gap between rounds (with one-shot timers, `nanosleep(2 ms)` really is 2 ms): with 20 ms gaps it was 53 us;
- not the scheduler's sorted sets: bisected by commit, K96 and the per-CPU sets give 7 us;
- not the timer logic at all: under TCG the branch is the same as `main` or quicker (122 against 140 us on two CPUs).
With the x86 timer put back to periodic mode (an experiment, undone), it was 25 us: part of it is the one-shot local APIC timer under KVM, most likely in how the host wakes a halted virtual CPU. I have not found the rest. Left as K192, with these measurements; it does not show under TCG, where The-Jat's machines run.

**6. (In K20's merge, entry 124.)** A script of mine opened `docs/lanes.md` for writing before reading it, and emptied it. Caught before the commit by its line count, restored from the merge commit, and the one edit made again.

## How I checked it

- `cargo test` of the kernel's host-tested code: 240 tests (the scheduler's 37: two new, and the set check in both randomized runs; `after_interrupt`'s).
- clib on x86_64 and AArch64, and compiled by tcc on TheTaaJ on both: 156 checks, 21 of them new - a 1 ms sleep's length, `clock_nanosleep` relative and absolute on both clocks and its EINVALs, `setitimer` ringing about twelve times in 60 ms and then stopping, `getitimer`, `timer_create`'s one timer and the EAGAIN for a second, a time already past ringing at once.
- The self-tests on both CPUs, with the new late-interrupts check; `basics64-test` and `aarch64-basics-test` (alarms and signals); `boot64-test`, `aarch64-keys-test`, `smp64-test`, `aarch64-test`, `tcc64-test`, `aarch64-tcc-test`.
- **The mutation of entry 097** (ticks counted from interrupts again), applied and undone: on x86 the late-interrupts self-test fails - "after 35 ms without interrupts the tick count moved on by 1, not the 3 ticks that passed". On AArch64 it passes, rightly: its timer calls the tick handler once for every tick that passed, so counting interrupts there already keeps the time.
- **The mutation of K96** (a sleeper woken early left in its set), which happened by accident (bug 1): four tests hang.
- `schedbench` on `main` and the branch, back to back; and a bisection of the wake-up numbers (above).
- `make test64`.

## What is still missing

- **K192**: waking a thread on another CPU under KVM (bug 5).
- **K191**: several POSIX timers per process, SIGEV_THREAD and SIGEV_NONE, and `timer_getoverrun` counting.
- **A tickless idle**: an idle CPU still takes its 100 ticks a second.
- **Fine timeouts for futexes and `OBJECT_WAIT_UNTIL`**: `pthread_cond_timedwait` and `poll` still wait to the tick, as their calls take ticks.
- **ITIMER_VIRTUAL and ITIMER_PROF** (CPU-time timers).

## What I learned

- **A count read where it is needed, not where it is written.** Ticks written by one CPU's interrupt were right as long as only that interrupt woke anyone. The moment another CPU's timer could wake a sleeper, the count had to be read from the clock.
- **A mutation that changes nothing on one CPU can be right there.** AArch64 counts every tick a late interrupt stands for; the same mutation is a real bug only on x86.

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| `with files clib` | Among the checks: `clib: nanosleep(1 ms) took 1... us at least, of twenty` (about a millisecond, not ten), `clib: setitimer every 5 ms rang 12 times in 60 ms` (about twelve), and `library: 156 checks, all passed` |

```
make smp64-test
```

| Look for | You should see |
|---|---|
| `[selftest] late interrupts` in `build/smp64-*.out` | `35 ms without any, then 1 came; the tick count moved on by 3` (or 4): the time kept, however late the interrupt |

Automated: `make test64`
