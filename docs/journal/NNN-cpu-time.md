# CPU time: what each process has used, in `ps`, `clock()` and `CLOCK_PROCESS_CPUTIME_ID`

*Journal entry #NNN · MaK-WSL's step A, item K21. Written on MaK-WSL. Until now TheTaaJ knew how much the CPUs worked in all (`busy_ticks`, what the shell's `time` prints), but not who used it. Now the kernel counts, for each process, the time its threads were on a CPU; `ps` shows it, and a C program gets it from `clock()` and `clock_gettime(CLOCK_PROCESS_CPUTIME_ID)`.*

## Why

"Which program is using the CPU?" is the first question when the machine is slow, and `ps` could not answer it. And `clock()` - C's "processor time used by the program" - returned the time since boot, as its own comment admitted: measured twice, it counted every other program's time too. Rust's `std` (R3) and many C programs measure with it. It is also the ground K20 (priorities) stands on: to see that a priority works, one must see who got the CPU.

## How it fits together

```
 a CPU switches threads (schedule)  ─┐
 a timer tick on any CPU            ─┴─ charge(cpu, now):
                                         its current thread's process += now - RUNNING_SINCE[cpu]
                                         RUNNING_SINCE[cpu] = now
 CLOCK(PROCESS_CPU)  ── threads::process_cpu_ns: the process's count + the caller's time since
                        its own last switch or tick
 PROCESS_LIST        ── ProcessEntry.cpu_ns (a fifth word)
```

- **The count** is `Process::cpu_ns`, an atomic in nanoseconds, added to by `threads::charge`.
- **When**: at every switch, in `schedule`, *before* the policy picks the next thread - after `pick_next` the CPU's current thread is already the next one, and the time would go to the wrong process. And at every timer tick (`on_tick`, `on_tick_elsewhere`), so a thread that keeps its CPU is counted every 10 ms, not all at once when it finally leaves.
- **What counts**: the time a program's thread was the CPU's current thread - in user mode or in a system call. A thread asleep or waiting is on no CPU; an idle thread belongs to no process.
- **The time** is the CPU's counter (`clock::monotonic`), as for the other clocks since entry 097. So a moment in which the host stops a virtual CPU counts to the thread that was on it.
- **Reading it** (`clock::PROCESS_CPU`, through the existing `CLOCK` call) adds the caller's own time since its last switch or tick, so two reads in a row grow by what was used between them. Another of its threads running on another CPU at that moment is counted up to its last tick: at most 10 ms behind.
- **`ps`** has a CPU column, in seconds to the millisecond; an ended process shows all it ever used.
- **libc**: `CLOCK_PROCESS_CPUTIME_ID` (2, as glibc's) for `clock_gettime`, and `clock()` in microseconds of the same time.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Charge at each switch and each tick, with the counter's nanoseconds | Counting ticks in which a process was running (as `busy_ticks` samples the CPUs) | Exact to the counter, not a sample: a program that runs for 3 ms and sleeps is counted 3 ms, not 0 or 10 |
| Charge before `pick_next` | After the switch | Afterwards the CPU's current thread is the next one; the leaving one's time would be charged to it |
| A process's total, not each thread's | Per thread as well | `ps`, `clock()` and `CLOCK_PROCESS_CPUTIME_ID` ask for the process. Per-thread time (`CLOCK_THREAD_CPUTIME_ID`) is easy to add on the same counting when something needs it |
| One more word in `ProcessEntry` | A new system call for the list | `ps` already gets everything else about a process from it; the ABI's two users (the shell and `taaj::process_list`) change with it, in this item |
| `CLOCK` answers a third clock | A new system call | The existing call already means "nanoseconds on a clock"; libc's `clock_gettime` maps onto it |

## Bugs

**1. `aarch64-keys-test`, run on its own, booted an old kernel image.** Its first run here failed with `FAIL: no output matching "init +running +[0-9]+\.[0-9]{3} s"`, and its `ps` printed the old three columns. `build/aarch64/Image` was from 09:51, hours before this item: the target depended on `aarch64-test-disk` but not on `kernel-aarch64`, and the image carries the user programs - the shell among them. Inside `make test64` this never showed, because `test64-shared` builds `kernel-aarch64` first; run alone, the test quietly tested old code. Fixed by the missing prerequisite; it was the only AArch64 test target that boots the image without it (all of them checked). Run again: passed, with the new column.

## How I checked it

- `abi`: a new host test, `a_process_entry_survives_its_words`: an entry with a CPU time, running and ended, comes back the same from its five words.
- The kernel builds for x86_64 and AArch64; `kernel64-test` (the kernel's host tests, libc's host tests) passes.
- `clib.c`, which the boot tests run on both architectures, checks it: busy for 200 ms of the monotonic clock, the process's CPU time grew by at least half of that and at most 20 ms more; `clock()` grew too; asleep for 200 ms, it grew by less than 50 ms. The limits follow the timing rules ("Before merging"): generous upper and lower bounds, since a busy host or another program can take some of the time.
- `boot64-test` under KVM: passed (499 s), clib's 119 checks all passed, and `ps` printed:
  ```
    PID  NAME        STATE           CPU
     35  init        running         0.043 s
     36  registry    running         0.002 s
     37  display     running         0.031 s
  ```
- `aarch64-keys-test` (emulated), after bug 1's fix: passed, clib's 119 checks all passed, `ps` with the CPU column.
- `make test64`, as rule 2 asks.

## What is still missing

- **Per-thread CPU time** (`CLOCK_THREAD_CPUTIME_ID`, `pthread_getcpuclockid`): the same counting, kept per thread.
- **User and system time apart** (`times()`, `getrusage`): the count does not tell user mode from system calls.
- **A child's time** for its parent (`RUSAGE_CHILDREN`, the shell's `time` per command).
- **Live time of other CPUs' threads** when reading: up to 10 ms behind, as above.

## What I learned

- **Where the accounting goes matters more than how.** One line too late - after `pick_next` rather than before - and every slice is charged to the wrong process, with nothing to show it but numbers that look plausible.

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| `ps` | A CPU column: `init`, `shell` and the services with small times, each `N.NNN s` |
| `cprimes 24000000 &` then, after a few seconds, `ps` | `cprimes` with a CPU time that grows each time you look |

```
make boot64-test
```

| Look for | Meaning |
|---|---|
| `library: N checks, all passed` | clib's CPU-time checks passed with the rest |
| `boot64-test passed` | `ps` printed the CPU column the test expects |

Automated: `make test64`
