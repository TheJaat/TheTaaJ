# Priorities and affinity: who runs first, and where

*Journal entry #NNN · a step, MaK-WSL/sched's K20 with J80. Written on MaK-WSL/sched. Until now every thread was equal: one queue, first come first served, each running a slice of 20 ms before the next. So a key press waited behind every busy program. Now each thread has a priority and a set of CPUs it may run on; a thread made ready that is more important than a running one takes its CPU at once; init decides that the services between the keyboard and the screen come first; and C programs get `nice`, `setpriority`, `sched_setaffinity` and the rest.*

## Why

The benchmark (entry 116) measured it: with every CPU busy, a woken thread waited a median of **20 ms** on one CPU (a whole slice) and 6-8 ms on two or four. Typing into the shell while a compile runs is exactly that: the keyboard driver's thread is woken, and waits its turn. K20's note in `who-does-what.md` said it plainly: "Four busy programs make typing sluggish."

## What a priority is here

Two ideas, kept apart:

- **A priority** says who runs first. It is `nice`'s number: **-20** the most important, **19** the least, **0** the default, so a C program's `nice 10` means what it means on Linux.
- **An affinity** says where a thread may run: one bit per CPU. A thread pinned to CPU 0 never runs on CPU 1, whatever is idle there.

And one rule about who may change them:

- **A ceiling**: the most important priority a process may use, for itself, its threads, or a process it started. init's is -20. Every other process's is 0: it can make itself less important, never more. A program cannot escape by starting a child and making the child important either, because nothing above the caller's own ceiling can be given to anyone. This is the capability idea again: the kernel does not know "root"; it knows who holds what.

## How it works

### The policy (`sched.rs`, tested on the host)

```
 ready: one FIFO queue per priority     -20 ... -5 ... 0 ... 10 ... 19
                                          │      │     │      │
        a CPU takes the first allowed  ───┘  (most important, longest waiting)
```

- **Choosing**: a CPU that needs a thread takes the most important one allowed on it, and of those the one that has waited longest. Within a priority it is round-robin, as before; between priorities it is **strict**: a less important thread never takes the CPU from a more important one. A busy important thread that never waits leaves nothing for the less important ones. That is the trade I chose (below).
- **Taking a CPU at once**: when a thread is made ready - woken, started, given a better priority - the scheduler first looks for an idle CPU it may use, as before. If there is none, it looks at the CPUs it may use for the one running the **least important** thread; if that thread is less important than the new one, that CPU is asked to choose again. On the same CPU that happens when the system call returns; on another, an inter-processor interrupt ("a poke") makes it happen now.
- **Affinity**: a CPU takes only threads whose affinity includes it, and is offered only those. A running thread whose affinity no longer includes its CPU leaves it at once.

The idea that makes the test suite useful: the policy is pure bookkeeping, with no hardware in it, so every rule is a host test. Eight new ones, among them a randomized run of 40 000 steps on four CPUs with mixed priorities and affinities that checks after every step: nobody is queued twice or while on a CPU; no CPU runs a thread outside its affinity; and after a CPU chooses, no thread waiting for it is more important than the one it chose.

### The kernel's side

- `Process` keeps a priority, an affinity and a ceiling; a program's new threads take its priority and affinity.
- Two system calls, **`SCHED_SET` (45)** and **`SCHED_GET` (46)**: a target (a thread of my own, or a process), what (priority, affinity, ceiling; and the calling thread's number), and a value. Priorities travel as **levels**, 0 to 39, so that no value looks like an error code.
- To change another process, the caller needs its handle with **`KILL`**, the right whoever started it holds. Who may end a process may as well say when it runs; no new right was needed.
- **The end of every system call** now checks whether the call asked this CPU to choose again (a futex wake-up, a message), and switches before going back to user mode. Before, a wake-up on the same CPU was noticed only at the next timer tick, up to 10 ms later.

### init's policy

`init` gives **priority -5** to the services on the way from a key to the screen: `kbd`, `serial`, `usb`, `hid`, `display` and `shell`. It is init's decision, in user space; the kernel only keeps the priorities it is given. Everything else, and everything the shell or `posix_spawn` starts, runs at 0.

### libc (J80)

| Call | What it does here |
|---|---|
| `nice(increment)` | The process's priority moved; EPERM past its ceiling |
| `getpriority`/`setpriority(PRIO_PROCESS, who, prio)` | `who` 0 or the program itself, or a child not yet waited for; EACCES above the ceiling; PRIO_PGRP and PRIO_USER are EINVAL (no groups or users) |
| `sched_setaffinity`/`sched_getaffinity(pid, size, set)` | `cpu_set_t` with the `CPU_*` macros; pid 0 is the calling thread, a child's pid all its threads; a set with no CPU there is EINVAL |
| `pthread_setaffinity_np`/`pthread_getaffinity_np` | One thread's CPUs |
| `sched_setscheduler`, `sched_getscheduler`, `sched_*param`, `sched_get_priority_*`, `pthread_*schedparam` | Always SCHED_OTHER, priority 0; SCHED_FIFO and SCHED_RR are EPERM until the kernel has real-time classes (K99) |

For a call on *another* thread, libc needs that thread's number in the kernel, which `pthread_create` used to throw away. Each thread now writes it into its own record as it starts (`SCHED_GET`'s THREAD_ID), and `pthread_create` writes it too when the kernel answers: whichever comes first, the same number.

## The numbers

`schedbench` (entry 116), MaK-WSL, under KVM. Before is `main` at entry 116; after is this branch; each column measured three times back to back, one run shown.

| | 1 CPU | 2 CPUs | 4 CPUs |
|---|---|---|---|
| **woken, every CPU busy at nice 0** (as before: equals take turns) | 19 974 us | 6 131 us | 7 694 us |
| **woken, every CPU busy at nice 10** | **2 us** | **4 us** | **21 us** |
| four busy programs, two at nice 10: CPU each in 2 s | 993, 999, **0, 0** ms | 1990, 1990, **0, 0** ms | 1996, 1996, 1991, 1994 ms |
| two busy threads pinned to CPU 0 / free | - | **1000** / 1999 ms | **1000** / 2000 ms |
| yield, before → after (3 runs each) | 286-312 → 337-352 ns | 277-379 → 412-448 ns | 397-535 → 451-485 ns |
| hand-off, wake-to-run | the same within their spread (±20 %) | | |

- **A thread more important than the busy ones takes a CPU in microseconds**, not at the end of a slice: 2 µs on one CPU, ten thousand times sooner than the 20 ms of equals.
- **Strict means strict**: on one or two CPUs, two programs at nice 0 leave none for two at nice 10. On four, all four run.
- **Pinning holds**: pinned to one CPU, two busy threads get one CPU's worth.
- **The cost**: about **45 ns more per switch** on one CPU (of about 300), the bookkeeping of priorities and affinities on the hottest path. See bug 2 for how it was 130 ns first.

## Design decisions

| Decision | Instead of | Why (and what it costs) |
|---|---|---|
| Strict priorities | Shares in proportion (a fair scheduler weighting each by its priority) | It is what makes the woken important thread run *now*, which is what typing needs (*responsive*). The cost is starvation: a busy important thread that never waits leaves nothing below it. Here only init can make anything more important than 0, and it raises only services that mostly wait. Shares in proportion are K100's (*fair*) |
| A ceiling per process, init's the highest | Privileged users, or a right to raise | No users exist (*capabilities*); a ceiling is a capability measured in priorities. It also stops "start a child and raise it", the obvious escape |
| `KILL` for changing another process | A new right | Who can end a process can already deny it the CPU completely; no new power, and no new ABI number |
| The policy (who is important) in init | A table in the kernel | The kernel keeps mechanism, user space decides (*pure microkernel*) |
| Priorities as levels 0-39 in the calls | Signed values | A negative value would look like an error code in the kernel's return convention |
| New processes start at 0, whatever their parent's priority | Inheriting the parent's priority | Programs the shell starts are loaded by init on its behalf; "inherit from whoever loads you" would give every one init's settings. Inheriting `nice` the way entry 096 passes the memory limit (through who made the program file) is K?-nice-inherit |
| SCHED_FIFO and SCHED_RR refused (EPERM) | Mapping them onto priorities | They promise real-time semantics the kernel does not give yet (K99); refusing is honest |
| J80 with K20 | J80 after K97, as its row said | The user's decision: one branch for the kernel side and the C calls. When K97 makes scheduling contexts capabilities, these calls map onto them |

## Bugs

**1. An affinity read back as 64 CPUs.** A new thread's affinity is "any CPU", kept as every bit set. `sched_getaffinity` would have returned 64 CPUs, and setting that back (which the kernel limits to the CPUs there are) would not have read back the same. Found by reading my own code against clib's new check, before any test ran. `SCHED_GET` now reports an affinity limited to the CPUs that exist.

**2. Priorities first cost 130 ns a switch.** The benchmark three times on `main` and three on the branch, back to back: yield on one CPU went from 286-312 ns to 428-435 ns. My first guess was the extra lock at the end of every system call; I made that check a per-CPU hint (`ASKED`), so the lock is taken only when a call asked this CPU to choose again - and measured again: no change (421-442 ns). The guess was wrong. The cost was in the policy: every switch looked up the chosen and the leaving thread's priority and affinity in a map, several times, and scanned forty mostly empty queues. Now each queued thread carries its affinity, each CPU keeps its current thread's priority and affinity, `take` hands back the chosen thread's, and a 64-bit mask of non-empty levels finds the first in one instruction: a switch looks nothing up. Yield: 337-352 ns. The hint stayed; it costs nothing.

**3. My first randomized test blamed the wrong CPU.** It checked that no CPU interrupts itself, collecting the interrupts once per step, so it charged CPU 0's tick work to whichever CPU the step chose. Fixed by checking after each thing a CPU does, as the older randomized test does.

## How I checked it

- `cargo test` of the policy: 35 tests (27 before, unchanged; 8 new).
- `abi`: a new test, every priority survives its level and back.
- clib on x86_64 and AArch64: 135 checks, 16 of them J80's: `nice` down and the EPERM back up, `setpriority`'s EACCES, `getpriority` of nobody (ESRCH) and of a group (EINVAL), the affinity set to CPU 0 and read back, an impossible affinity (EINVAL), SCHED_OTHER and SCHED_FIFO's EPERM, `pthread_getschedparam`.
- `schedbench64-test`: now checks priorities and affinity as behaviour, with generous limits: on one or two CPUs the two programs at nice 10 get under a quarter of what the two at 0 get; pinned, two threads get at most three quarters of what they get free.
- `boot64-test`, `aarch64-keys-test`: the whole typed session with init's priorities in place.
- The benchmark, three runs before and three after, for bug 2.
- `make test64`.

## What is still missing

- **K?-nice-inherit**: a child taking its parent's `nice`, through `posix_spawn` and the shell (as the memory limit travels with the program file, entry 096).
- **Shares in proportion** (K100) and **real-time classes** with SCHED_FIFO/RR (K99); **scheduling contexts** as capabilities (K97), which these calls will map onto.
- **Priority inheritance** across a call to a service (K65/K98): a client at -5 waiting for a server at 0 still waits behind the server's other work.
- **`sched_getcpu`**: TheTaaJ does not tell a thread which CPU it is on.
- **A way to see priorities**: `ps` shows CPU time (entry 114) but not priority.
- The remaining **45 ns a switch**: K68 (a run queue per CPU) rebuilds this path anyway.

## What I learned

- **Measure before fixing, and again after.** The obvious culprit (a lock at every system call's end) was innocent; only measuring the fix showed it.
- **Strict priorities are a promise and a danger.** The same rule that wakes the keyboard's thread in 2 µs starves a program at nice 10 completely. The ceiling is what keeps the danger in init's hands.
- **Responsiveness is a wake-up path, not a speed.** Nothing got faster; a woken thread just stopped waiting behind threads that matter less.

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| `cprimes 24000000 &` four times, then type at the prompt | The letters appear at once: the shell and the keyboard driver run before the busy programs |
| `ps` | The four `cprimes` with growing CPU times |

```
make schedbench64-test
```

| Look for | You should see |
|---|---|
| `same, two at nice 10: CPU each (nice)` | With 1 and 2 CPUs: about 1000 (1 CPU) or 2000 ms (2 CPUs) for each at nice 0, and 0 ms for each at nice 10 |
| `2 threads pinned to CPU 0 / free: CPU` | About 1000 ms pinned, 2000 free |
| `wake, every CPU busy at nice 10` | A median of a few microseconds; at nice 0, milliseconds |
| `schedbench64-test passed` | |

```
make boot64-test
```

| Look for | Meaning |
|---|---|
| `library: 135 checks, all passed` | J80's calls, with the rest |

Automated: `make test64`
