# The race hunt: twenty rounds under KVM, a SIGCHLD that came late, and tests that timed the host

*Journal entry #101 · MaK's step A, item K19 (the race hunt), second part. Written on MaK. Entry 100 made the tests choose KVM and fixed the typed boot test's typing races. Then the x86_64 half of `make test64` ran twenty times over under KVM. Four rounds failed, each in a different part. One failure was a real ordering bug in the kernel: a parent's `waitid` could return before its SIGCHLD handler had run. Two were checks that timed the host as well as TheTaaJ. One could not be explained, and its log was lost; the hunt now keeps the logs of a failed round.*

## Why

A race is a failure that depends on timing: on which CPU gets somewhere first. Emulated CPUs take turns in a way that hides most of them. KVM runs four CPUs at full speed and at once, the way a real PC does, so the same tests become a different experiment. One run proves little; twenty runs of the same tests show what fails only sometimes.

## How it fits together

```
tools/race-hunt.sh [ROUNDS]
  └─ round N: make -k test64-parts TEST64_PARTS=<the 30 x86_64 parts>
       ├─ build/race-hunt/round-N.log           make's output
       ├─ build/race-hunt/summary.txt           "round N: exit E, S s, failed: <parts>"
       └─ on a failure: build/race-hunt/round-N/   every file build/ got during the round
```

- **Only the x86_64 parts** by default: they are the ones KVM runs at full speed. AArch64 is emulated on every machine, and its parts are the slowest in `test64`. `ALL=1` runs them too.
- **A failed round's logs are kept.** The next round runs the same tests and writes the same file names, so a failure's serial log is overwritten within seconds. Round 6's was, before the tool kept them.

## The hunt

Twenty rounds of the 30 x86_64 parts under KVM, on MaK, 2026-11-26: **16 clean rounds** (415 s each on average), **4 rounds with one failure each**, every one in a different part.

| Round | Part | What failed | Cause |
|---|---|---|---|
| 2 | `nvme64-test` | kernel selftest: `forever wait took 17 ticks to end` | The host, not TheTaaJ (bug 2) |
| 6 | `bigmem64-test` | `the steps did not all run`; QEMU stopped by the time limit | Not known: the log was overwritten (bug 3) |
| 14 | `ground64-test` | `its end sent SIGCHLD, handled before waitid returned` | A real kernel ordering bug (bug 1) |
| 20 | `cxx64-test` | kernel selftest: `forever sleep took 4 ticks to end` | The host, as round 2 |

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Publish a process's exit code last, after its watchers' signals | Sending the signals first but publishing the code first, as before | What the waiting side's condition reads (the exit code) must change only after everything the woken side must find (its SIGCHLD) is in place: the rule `wait_any_until` already states for every waker |
| A separate `ending` flag decides which thread ends the process | The exit code's compare-and-swap, as before | The code now comes last, so something else must say "one thread does the ending" from the start |
| Upper time limits measured up to three times; lower limits checked once, strictly | Wider limits | A wait shorter than asked is a bug every time. A wait much longer can be the host stopping the virtual CPU (since entry 097 ticks follow the CPU's counter), which almost never happens three times running; a kernel that is really slow is slow every time. Wider limits would let a real slowness through |
| `bigmem64-test` left as it is | Lengthening its waits on a guess | One failure in twenty rounds, no log, not seen in five runs under full CPU load. The hunt now keeps the logs, so the next one will show its cause |
| The hunt as `tools/race-hunt.sh` | A script in `build/` on MaK | Both machines can run it; The-Jat's emulated rounds may find other things |

## Bugs

**1. A parent's `waitid` could return before its SIGCHLD handler ran.** Round 14:

```
ground: failed: its end sent SIGCHLD, handled before waitid returned
```

`ground.c` sets a SIGCHLD handler, starts a child that exits with 5, waits for it with `waitid`, and checks that the handler has already run. In the kernel, `end_current` (process.rs) did, in order: publish the exit code (the compare-and-swap that also chose the thread doing the ending), stop the other threads, close the handles, send the watchers' signals (SIGCHLD to the parent), wake those waiting (`EXITS`). The comment said the signals went "before those waiting are woken", which was true. But a waiting parent does not only return when woken: `PROCESS_WAIT` waits while `exit_code().is_none()` (ipc.rs), and a parent re-checking that on another CPU - after any wake-up, or just before sleeping - returns as soon as the code is there. Between the publish and the signal, it returned to user mode with no SIGCHLD pending, and the handler ran later.

Proved with a stronger check in `ground.c`: a hundred more children, each waited for, counting the SIGCHLDs handled only after their `waitid` returned (counted against a running total, so a late one is not counted again at the next child).

| Kernel | Runs of `ground64-test` (KVM) | Failed | Late SIGCHLDs in each run, of 100 |
|---|---|---|---|
| Before the fix | 5 | 5 | 1, 3, 4, 1, 1 |
| After the fix | 6 | 0 | 0, 0, 0, 0, 0, 0 |

Fixed in `end_current`: a separate `ending` flag (compare-and-swap) chooses the thread that does the ending; it stops the other threads, closes the handles, takes the watcher list (leaving it `None`) and sends the signals, and only then stores the exit code and wakes `EXITS`. `notify_on_end` checks under the same lock whether the list is still there, instead of whether the exit code is set: either the ending finds the new watcher, or the watcher finds the list gone and is told at once. SIGCHLD is The-Jat's item J10 (entry 099); the fix is small, so it went in the finder's branch (rule 11).

**2. Self-tests timed the host as well as TheTaaJ.** Rounds 2 and 20:

```
*** KERNEL PANIC: panicked at src/selftest/user.rs:169:13:
forever wait took 17 ticks to end
```

The kill selftest kills a program in five states and asks that each end within 3 ticks. It is not a lost wake-up: that wait has no time limit, so a lost one would hang the boot, not end it 170 ms later. Since entry 097, ticks follow the CPU's counter, so a moment in which the host does not run the virtual CPU counts as ticks. A full `test64` runs twelve machines of up to four CPUs each on MaK's sixteen threads. Measured, booting the ISO (every selftest runs at each boot):

| Host | Boots | Passed | Failed |
|---|---|---|---|
| Idle | 40 | 40 | 0 |
| Every host thread kept busy | 40 | 33 | 7: the kill check 4 times (in four different states: unborn, yield, sleep, yield), `echoc`'s timed wait 3 times (`the timed wait did not last its 3 ticks`: over 5) |
| Busy, with the fix below | 40 | 40 | 0 |

Every check with a tight upper limit had the same weakness: the kill selftest (at most 3 ticks), `sleep(5)` (5 to 7), `echoc`'s timed wait (3 to 5) and `fscheck`'s silent service (10 to 13). Each now checks its lower limit once, strictly, and its upper limit up to three times. `niccheck`'s "under a second" is far enough from anything a host does, and `hello` checks only a lower limit.

**3. `bigmem64-test` once did not finish its steps.** Round 6: `FAIL: the steps did not all run`, QEMU stopped by `qemu-boot.sh` ("terminating on signal 15"). Its serial log was overwritten by round 7 39 seconds later, before it could be copied. Five runs under full CPU load passed, in 72 to 106 s instead of 33 (the waits it may use add up to 270 s for its big step). The likeliest cause is the host's memory and CPU both being short in that round - twelve machines, one of them touching 3 GiB, on 11 GiB - but that is a guess. Left open as K49; the hunt now keeps a failed round's files.

**4. The first hunt script lost round 6's logs.** It saved only make's output. Rewritten (now `tools/race-hunt.sh`) to copy every file `build/` got during a failed round before the next one starts.

## How I checked it

- The SIGCHLD order: `ground64-test` under KVM, five times with the old `end_current` and six with the new one (table above).
- The time limits: 40 boots under full host load, before and after (table above).
- `bigmem64-test` five times under full host load: passed every time.
- `make test64` under KVM and `make test64 TAAJ_ACCEL=tcg`: see the testing note; both must end `test64: all passed`.
- `tools/race-hunt.sh 1`: one round, to check the tool itself.

## What is still missing

- **K49**: why `bigmem64-test` once did not finish (round 6). The next failure's files will say.
- **Upper limits still time the host a little**: three slow tries in a row on a very busy host would still fail. That has not been seen.
- **The hunt has run under KVM only.** Under emulation (The-Jat, or `TAAJ_ACCEL=tcg`) other races may show.
- **Ticks counting host stalls** is right for the time of day and for sleeps, but a kernel that wants to measure only its own work (K21's CPU time per process) must count when its threads run, not the ticks in between.

## What I learned

- **"Before those waiting are woken" is not enough.** A waiter does not only wake when woken; it checks its condition whenever it runs. The order that matters is against what the condition reads: everything the waiter must find first, the condition last.
- **One try of a race says little; a hundred in one boot say a lot.** The single SIGCHLD check failed once in twenty runs of `test64`; a hundred children in one boot failed every run with the old kernel.
- **A test that times something in a virtual machine also times the host.** Lower limits are about the system; upper limits are partly about whoever else is using the machine.
- **Keep the evidence first.** A failure seen once and then overwritten cannot be explained.

## Testing note

On MaK (KVM):

```
make ground64-test
```

| Look for | Meaning |
|---|---|
| `ground: 52 checks, all passed` | Every SIGCHLD of the hundred children was handled before its `waitid` returned |

On either machine:

```
make test64
make test64 TAAJ_ACCEL=tcg
```

| Look for | Meaning |
|---|---|
| `test64: all passed`, twice | Every part, at full speed and emulated |

To hunt for races (run `make test64-shared` first; each round takes about seven minutes on MaK):

```
tools/race-hunt.sh 20
```

| Look for in `build/race-hunt/summary.txt` | Meaning |
|---|---|
| `round N: exit 0, ... failed: none` | A clean round |
| `failed: <part>` | That part failed; `build/race-hunt/round-N/` holds every file the round wrote |
