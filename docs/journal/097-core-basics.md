# The core basics: the date, a finer clock, signals, and writes that reach the disk

*Journal entry #97 · a step of its own, on the branch `core-basics`. When I re-evaluated what an everyday system still lacked, four small things kept coming up. Programs on Linux take them for granted:
- **the date.** TheTaaJ counted from boot. `time()` said 1970 plus a few seconds;
- **a clock finer than a tick.** Every time was a multiple of 10 ms, and a `nanosleep` could end early;
- **signals.** `signal()` and `raise()` were only libc pretending. The kernel had no idea, so Ctrl+C could only kill a program, `alarm` did nothing, and `abort` could not be caught;
- **flushes.** The disk was never told to write its cache out, so a commit could reach the disk in any order.

Now:
- the kernel reads the real-time clock at boot (CMOS on a PC, a PL031 on AArch64);
- time is counted from the CPU's counter to the nanosecond;
- the kernel delivers signals: to handlers in libc, on the program's own stack, with every register given back, and through every wait;
- all four disk drivers flush, and TaajFS flushes before and after its superblock.

With the disk's write cache on, 4 MiB is written 1.9 times as fast as with every write going through.*

## Why

Every C program that prints a date, times something, catches Ctrl+C to tidy up, or sets an alarm meets these. rustc and cargo do too (Era 9): they handle SIGINT, read the time for incremental builds, and `fsync` what they write. The four parts are independent, but each is small. Together they are the gap between "runs programs" and "runs programs that behave as on Linux".

## How it fits together

```
 boot ── CMOS (x86) / PL031 (AArch64) ── seconds since 1970 ──► clock::start
          clock: EPOCH (ns), START (the counter then), HZ (its rate)
          monotonic() = (counter - START) * 1e9 / HZ      realtime() = EPOCH + monotonic()
          timer interrupt: ticks = counter's ticks (not how many interrupts came)

 program                      libc                          kernel
 ─────────────────            ──────────────────────        ─────────────────────────────────
 signal(SIGINT, h)  ────────► actions[SIGINT] = h
                              SIGNAL_SETUP(entry,           process: caught, ignored, entry
                                caught, ignored) ─────────►
 shell: Ctrl+C ──────────────────────────────── SIGNAL_SEND(child, 2) ─► caught? pending |= bit,
                                                                         wake its threads
                                                                         else: ended, exit code
                                                                         SIGNALLED - 2
 read() waiting ...                                         wait ends: Interrupted
                                                            on the way back to user mode:
                                                            Context (registers, sp, pc) pushed
                                                            on the program's stack; return to
                              __taaj_signal_entry ◄──────── entry(signal, &context)
                              fxsave / q0-q31 saved
                              dispatch: blocked? pending.
                              else h(SIGINT)
                              FP put back
                              SIGNAL_RETURN(&context) ────► registers from the context (only
                                                            what a program may set); iretq
 read() again (restarting) ◄────────────────────────────── back where it was
```

**The clock** (TheKernel, `rtc.rs`, `clock.rs`, `arch/x86_64/cmos.rs`):
- `rtc.rs` turns a date into seconds since 1970 and back: Howard Hinnant's `days_from_civil` and `civil_from_days`. It is plain arithmetic and host-tested on every day from 1970 to 2100.
- **The CMOS clock** (ports 0x70/0x71) is read until two reads in a row agree, never during its update. It may be in BCD or binary, and in 12- or 24-hour mode (register B says which). **The PL031**, on QEMU's AArch64 `virt`, is one register: seconds since 1970. The device tree says where (`arm,pl031`).
- `clock::start` keeps the date as nanoseconds (`EPOCH`), the counter's value at that moment (`START`) and its rate: the TSC on x86 (3.07 GHz here, measured at boot), the generic timer on AArch64 (62.5 MHz). `monotonic()` is `(counter - START) × 10⁹ / HZ` in 128-bit arithmetic, so it doesn't overflow for centuries. `realtime()` is `EPOCH + monotonic()`.
- **Ticks now come from the counter.** The timer interrupt used to add one to the tick count. Now it sets the count to the counter's ticks (never going back). An interrupt that comes late, or two merged into one, no longer makes time slow down. The self-test counts interrupts separately.
- **CLOCK (34)** `(which)` returns nanoseconds: `MONOTONIC` or `REALTIME`. In libc, `time`, `gettimeofday`, `clock_gettime`, `timespec_get` and `clock` use it.
- **`nanosleep` is never short.** It sleeps whole ticks, then the rest by the counter. It returns EINTR with the time left if a handler ran.

**Flushes** (TheTaaJ: `virtio`, `ahci`, `nvme`, `usb`, `user/src/block_service.rs`, `taajfs`):
- **block::FLUSH (7)**, a one-byte request, only on the writers' channel. `Device::flush` has no default: every driver must say what it does.
- **virtio**: the driver now accepts `FEATURE_FLUSH` (bit 9) if offered. Before, it accepted no feature at all, so QEMU made the disk write-through. A flush is a request of type 4 with no data. **AHCI**: FLUSH CACHE EXT (0xEA), a command with no PRD. **NVMe**: the I/O command FLUSH (0x00). **USB sticks**: SYNCHRONIZE CACHE(10) (0x35); a stick that says "illegal request" has no cache to flush, which is fine.
- **TaajFS's commit**: the new blocks, **flush**, the superblock, **flush**. A disk may write its cache in any order. Without the first flush the new superblock can land before the blocks it points to, and a power cut then leaves a file system pointing at garbage. Without the second, a commit that has returned is not yet safe. `format` flushes too.
- `blk` says at start whether it has a write cache: `..., a write cache (flushed on commit), ...`.

**Signals** (TheKernel `process.rs`, `signals.rs`, `alarms.rs`, the three entry paths; libc `signal.c`, `signals.rs`, `spawn.rs`):
- **Each process has** pending, caught and ignored masks, the address of its signal entry, and the exit code a kill will have.
- **SIGNAL_SETUP (35)** `(entry, caught, ignored)`: libc calls it whenever a handler or the blocked mask changes. A blocked signal is caught by the kernel and held by libc, so the kernel needs to know only three things per signal. **SIGNAL_SEND (36)** `(process handle or SELF, signal)`: the handle needs the KILL right. **SIGNAL_RETURN (37)** `(context)`. **ALARM (38)** `(nanoseconds)`, returning what the one before had left.
- **Sending**:
  - caught: the bit goes into pending and every thread of the process is woken;
  - ignored (or ignored by default: SIGCHLD, SIGURG, SIGWINCH, SIGCONT): nothing happens;
  - otherwise the process is ended as a kill does, with exit code `SIGNALLED - n` (-1100 - n). The kernel log says `ended by signal 2 (SIGINT)`, and `waitpid` gives `WIFSIGNALED` and `WTERMSIG`.

  SIGKILL can't be caught or ignored.
- **Waits are interrupted.** `threads::interrupted()` is "being killed, or a caught signal is pending", checked where the kill was before. The wait returns `Interrupted`, and the system call returns `Error::Interrupted`.
- **Delivery** (`signals.rs`) happens on every return to user mode: from a system call, an interrupt (a program that computes and never calls the kernel gets its signal at the next timer tick) or a fault. Delivery:
  1. builds a `Context` of the saved registers: 31 general registers, sp, pc, flags, the signal;
  2. writes it on the program's stack, 128 bytes below its stack pointer (the red zone), aligned to 16;
  3. points the saved pc at the entry, with the signal and the context's address as arguments.

  The `UserFrame` trait hides the three layouts of saved registers: x86's system call frame, x86's interrupt frame, and AArch64's exception frame.
- **The return**: SIGNAL_RETURN reads the context back. It takes the registers and only the flags a program may set (on x86 the arithmetic flags, DF and AC; on AArch64 NZCV), and refuses a pc outside user space.
  - On x86 this needs a new exit from the system call path. `sysretq` sets rcx and r11 from rip and rflags, so it can't restore them. The system call entry now saves rcx and r11 as two more slots (18 pushes). For SIGNAL_RETURN the handler returns 1 instead of 0, and the assembly leaves with `iretq`, which restores every register.
- **libc's entry** (`__taaj_signal_entry`, assembly, both architectures) saves the floating-point registers that the kernel doesn't (`fxsave64` on x86; q0–q31, FPSR and FPCR on AArch64) and calls `__taaj_signal_dispatch`. Dispatch either:
  - holds a blocked signal as pending; or
  - runs the handler, with its `sa_mask` and the signal itself blocked, `SA_RESETHAND` and `SA_SIGINFO` (the third argument is the kernel's context).

  Then it restores the FP registers and asks for SIGNAL_RETURN.
- **System calls are restarted.** The `taaj` crate's waits (receive, wait, futex, object waits) go round again when they come back `Interrupted`. That is `SA_RESTART`: a `read` interrupted by a handler goes on reading. Sleeps, `pause` and `sigsuspend` return EINTR instead, as POSIX says.
- **Faults**: a page fault, an invalid instruction, a division by zero or an alignment fault in a process that catches SIGSEGV, SIGILL, SIGFPE or SIGBUS is delivered as that signal, and the handler can `siglongjmp` out. Uncaught, the process ends as before: `killed: page fault`, which `waitpid` reports as SIGSEGV.
- **Alarms** (`alarms.rs`): one per process, kept in a list. A kernel thread, "alarms", started with the first alarm, sleeps until the earliest is due and sends SIGALRM. A new alarm wakes it to look again.
- **In libc**:
  - `raise` and `kill` go through the kernel. `kill` reaches only one's own children, by their process handles; another number is EPERM.
  - `alarm`, `pause`, `sigpending` and `sigsuspend` are new;
  - `sigsetjmp` and `siglongjmp` are new, needed to jump out of a handler with its mask put back;
  - `abort` raises SIGABRT, so a handler runs. If the handler returns, `abort` sets the default action and raises it again, and the program ends by SIGABRT, not exit code 134.
- **The shell's Ctrl+C** sends SIGINT instead of killing. A program that doesn't catch it ends ("was stopped", as before); one that catches it decides. The `kill` command still kills.

**The test** (`basics.c` and `basics-test.sh`, `make basics64-test` and `make aarch64-basics-test`): below, under "How I checked it".

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where blocked signals are held | In libc: the kernel only knows caught / ignored / default | A blocked mask in the kernel: one more system call for every `sigprocmask`, which libc calls around every handler. Holding them in libc costs a SIGNAL_SETUP only when the set of caught signals changes |
| Who saves the FP registers | libc's trampoline | The kernel: on x86 it would need the XSAVE area's size, and every delivery would pay for 512+ bytes even for handlers with no FP. The trampoline knows it runs C |
| Where the context goes | The program's stack, below the red zone | Kernel memory per thread: nested signals need a stack of them, and the kernel would have to track it. On the program's stack, nesting is free and `siglongjmp` just abandons it |
| What signal_return trusts | Registers yes; flags masked; pc must be in user space | Trusting the whole context: a program could set IOPL or the interrupt flag on x86 by writing its own context |
| Interrupted waits | Restart in the user library (SA_RESTART), except sleeps | EINTR everywhere: every read in every program would need a retry loop, which C programs famously forget |
| The time of day | Read once at boot, then counted | Reading the RTC each time: slow (port I/O, an update to wait out) and only to the second |
| Ticks | The counter's | Counting interrupts: QEMU without KVM delays or merges them under load, and time then runs slow |
| Alarms | A kernel thread with a list | The timer interrupt: sending a signal can end a process and takes the scheduler's lock, which the interrupt may already hold |
| A disk with a cache | Take it, and flush on commit | Leaving it write-through: safe, but half the speed. Flushing after every write: as slow as write-through |
| Ctrl+C in the shell | SIGINT | The kill as before: a program could never clean up |

## Bugs

- **Killed processes were never freed.** On the first boot, the kernel's own self-test stopped it: `killed processes leaked frames`, 14 frames each. My new `stop_if_killed` read the kill code inside `if let Some(process) = current()...`, and called `end_current` (which never returns) while `process`, an `Arc`, was still held. Its count never went down, so the process, its tables and its pages stayed. Now the code is read and the `Arc` dropped first. The same rule came up in entry 096: nothing held across a function that doesn't return.
- **`nanosleep` slept short**, and `boot64-test`'s clib caught it (`nanosleep sleeps at least that long`). The kernel's SLEEP n waits for n tick *boundaries*, so the first tick may be almost over. Now libc sleeps whole ticks and then waits out the rest against the counter.
- **The first flush mutation was not caught.** I took out the flush before the superblock, and the power-cut test still passed: my simulated disk with a cache (`CachingDisk`) could only lose power at writes. A flush is a moment too, the one where the cache empties. With flushes counted as cut points, both mutations (no flush before, no flush after) fail the test.
- **AArch64 couldn't assemble the trampoline**: `stp x, x, [sp, #512]` is out of range for a pair store (its offset is 7 bits times 8). FPSR and FPCR now go at offset 0, and the q registers after them.
- **`pause` slept a day at a time**, and the kernel refuses sleeps over ten minutes. It now sleeps 5 minutes at a time until a handler has run.
- **libc had no `sigsetjmp`.** The test's SIGSEGV handler couldn't get out with its mask put back. It is a macro over `setjmp` (setjmp must be called by the function to return to) and a saved mask.
- **TheTaaJ could no longer build its own libc.** The first `make test64` failed in `tcc64-test` and `libmake64-test` (and their AArch64 twins). On TheTaaJ, TCC compiles libc's C files, and the new `signal.c` had three things TCC doesn't know:
  - the `__atomic_load_n`, `__atomic_store_n` and `__ATOMIC_SEQ_CST` of clang and GCC. TCC's `<stdatomic.h>` defines those as macros over its own builtins, so `signal.c` now includes it, and uses `__atomic_fetch_or`/`__atomic_fetch_and`, which both compilers have, instead of the `or_fetch` forms;
  - `fxsave64` in TCC's x86 assembler;
  - the q registers, FPSR/FPCR, `svc` and `brk` in its AArch64 assembler.

  Those instructions are now written as the bytes clang assembles them to (`.byte`, `.int`), each with its instruction named beside it.
- **`aarch64-test` looked for the self-test's old line** `timer: 10 ticks in ...`, which now says `10 interrupts in ...` (ticks come from the counter). Its pattern is updated.
- **A test bug**: the check that the monotonic clock never goes back was called once for each of 1000 readings, which made the count "1031 checks". It is one check now, counting the readings that went back.

## How I checked it

**Host tests** (all in `make test64`'s `kernel64-test`):
- `rtc`: day 0 is 1 January 1970; known dates; every day from 1970 to 2100 follows the one before; seconds turn back into the same dates; impossible dates refused; the CMOS registers in BCD and in binary, in 24- and 12-hour mode (12 AM is midnight, 12 PM noon);
- `abi::signal`: exit codes both ways, and a context through bytes and back;
- the device tree: the PL031 found on the small tree and on QEMU's;
- TaajFS: `a_disk_that_reorders_its_cache_still_leaves_old_or_new_state`. A disk that keeps writes in a cache, and loses a random subset of them at a cut, is cut at every write *and every flush* of a run of commits. After each cut the volume must mount and hold the old state or the new, never a mix;
- each driver's simulator sees a flush: AHCI a command with no data (`a_flush_is_a_command_with_no_data`), NVMe the flush opcode with no data, and a USB stick SYNCHRONIZE CACHE, while one that has no cache says "illegal request" and that is fine (`a_stick_is_told_to_empty_its_cache_and_one_without_is_fine`).

**The new boot test, `make basics64-test` and `make aarch64-basics-test`**:
- **the date**: the kernel says `[clock] the CMOS clock says 2026-11-25 15:32:42 UTC; time is counted from here at 3073929600 Hz` (on AArch64: `the PL031 says ...`, at 62500000 Hz). `basics date` says `Wednesday 25 November 2026, 15:32:46 UTC; up 4.333 s`. The script compares that with the host's clock: it must be within 5 minutes after it.
- **`with programs basics`, 33 checks**:
  - `time()` is this year; the two clocks agree; the monotonic clock moves by less than a millisecond and never goes back; the time of day and the time since boot move together; 20 nanosleeps of 20 ms, none short;
  - `raise` runs the handler; SIGCHLD is ignored; a blocked signal waits and comes when unblocked;
  - **an alarm while computing**. The program steps a random-number generator and two floating-point series in a loop until the handler has run. The handler does its own floating-point and integer work. Then the program does the same number of steps again and compares. Every value must match, so every register came back;
  - an alarm while waiting in `read`: the handler writes to the pipe being read, and the read goes on and gets that byte, about a second later;
  - `pause`, `sigsuspend` (which also puts the mask back) and `nanosleep` (EINTR with about 4 of 5 seconds left);
  - `alarm` returns what the one before had left;
  - a write to address 16 runs the SIGSEGV handler, with the context; `siglongjmp` gets out, and SIGSEGV is unblocked again;
  - children, started with `posix_spawn` and a pipe to say "ready":
    - one is ended by SIGTERM (`WTERMSIG` 15);
    - one catches SIGTERM and exits 7;
    - one calls `abort` with a SIGABRT handler: the handler runs, and SIGABRT ends it;
    - one that faults ends by SIGSEGV;
  - `kill` of another program's process is EPERM;
  - a 256 KiB file written, `fsync`'d, read back the same.
- **`basics catch`, then Ctrl+C**: the program waits in a read of standard input, and its SIGINT handler says `caught SIGINT while waiting for a line - tidying up` and exits 0.
- **`with files basics speed`**: 64 writes of 64 KiB, each a commit with two flushes. The script also checks that `blk` took the disk's write cache.

**Writes with and without the disk's cache** (x86, QEMU, the same `basics speed`):

| The disk | 4 MiB in 64 commits |
|---|---|
| write-through: no FEATURE_FLUSH, as before this step | 818 ms (5.0 MB/s) |
| a write cache, flushed twice per commit | 435 ms (9.4 MB/s) |

**Mutation experiments**, each undone. Each was run with `make basics64-test`:

| What I broke | What happened |
|---|---|
| SIGNAL_RETURN leaves by `sysretq` instead of `iretq` | Caught: "every register, integer and floating point, as it was after the handler". rcx and r11 come back wrong |
| The trampoline doesn't save the FP registers (no `fxsave64`/`fxrstor64`) | Caught: the same check. The handler's floating-point work leaked into the loop |
| A caught signal doesn't interrupt a wait | Caught: the program hung in `read` waiting for the alarm's byte, and the steps never finished |
| The user library doesn't restart an interrupted call | Caught: "a read interrupted by a handler goes on, and reads what it wrote" |
| `siglongjmp` doesn't put the mask back | Caught: "siglongjmp out of the handler unblocks SIGSEGV again" (a check I added for this) |
| Setting an alarm doesn't wake the alarms thread | Caught: the second alarm never rang, and the program hung |
| The virtio driver refuses FEATURE_FLUSH | Caught: "the disk's write cache was not taken: no flushes" (the speed run above) |
| No flush before, or after, the superblock in a commit | Caught by the host test above, once flushes were cut points (see Bugs) |
| Ticks counted from interrupts again, not from the counter | **Not caught.** On an idle QEMU the timer interrupts come on time, so the two counts agree. It matters on a loaded host, or a VM that pauses, and I have no test that can make interrupts late on purpose |

With the two new parts, the expectations changed: `boot64-test`'s clib now expects `clib was ended by signal 6 (SIGABRT)`, the C++ test `"cxxlib" ended by signal 6 (SIGABRT)`, and the `forever` test `ended by signal 2 (SIGINT)`. `make test64` passed on its second run, in 13 min 55 s. The first failed in `tcc64-test`, `libmake64-test` and `aarch64-test` (see Bugs); I reran those parts until they passed before running the whole again. I also typed the testing note's steps below by script into a boot of the run image on a scratch disk made by `make run-disk`, and every row came out as the table says. Ctrl+C on `basics catch` printed its handler's line; `forever` was stopped by signal 2; `basics speed` took 477 ms. One thing I noticed on the way, which this step didn't cause: `with files clib` fails 12 of its 115 checks on the run disk, because they read `welcome` and `docs/`, which only the test disk has.

## What is still missing

- **Signals go to the process, not to a thread**: any thread may take one. There is no `pthread_kill` or `pthread_sigmask` per thread (libc's mask is the process's).
- **No `sigaltstack`**: a handler for a stack overflow has no stack to run on. rustc installs one (R3's list).
- **No SIGCHLD** when a child ends, **no SIGPIPE** on writing a pipe with no reader (the write returns EPIPE), **no job control** (SIGTSTP, SIGCONT, process groups), and no `kill` of a process that isn't one's own child.
- **Every interrupted call restarts** (SA_RESTART always). A program can't ask for EINTR from `read`.
- **An uncaught fault** still says `killed: page fault`, not `ended by signal 11`. `waitpid` reports it as SIGSEGV.
- **The time of day can't be set** (no `settimeofday`, no writing the RTC), there are **no time zones** (`localtime` is UTC), and nothing corrects drift: a machine up for weeks drifts as its counter does.
- **Alarms and sleeps are as fine as a tick** (10 ms); `timer_create` and `setitimer` aren't there.
- **The ticks mutation went uncaught** (above).
- **The flushes of AHCI, NVMe and USB** are checked only in their simulators and by their boot tests passing with them; only virtio's cache is measured.

## What I learned

- **A signal is a function call the program didn't make.** The kernel fakes one: it saves what the program was doing on its own stack, makes the program "return" into the handler, and on the way back puts everything back. Every register matters, because the program was between two instructions and expects all of them. That is why `sysretq`, which destroys rcx and r11, can't be the way back. The mutation proved it.
- **Who saves what is a contract.** The kernel saves the integer registers, libc the floating-point ones, and the handler's own calls save the rest by the calling convention. A missing piece shows only when the interrupted code had something in that register. The test computes in a tight loop so that it always does.
- **Interrupting a wait is half the work.** The other half is what the program sees. EINTR everywhere is correct, and it breaks every program that forgot to retry. Restarting in the library keeps them working.
- **A disk's cache reorders.** "Write A, then write B" doesn't mean A is on the medium first. A flush is the only ordering a disk promises, and a test of power loss has to be allowed to lose power at a flush too.
- **The clock is a counter and a date.** The date is read once. Everything after is the counter, which never jumps, so the time of day and the time since boot can't disagree.

**Rust, as used here:**
- **A trait over three layouts.** `UserFrame` has `context`, `restore`, `enter` and `stack`. x86's two frames and AArch64's each implement it, and `signals.rs` is written once against `impl UserFrame`. Each call site is compiled for its own frame type, with no dynamic dispatch.
- **`#[repr(u32)]` on an enum returned to assembly.** `Exit::Sysret = 0, Exit::Iret = 1`: the assembly compares the return value to choose its exit, and the enum keeps the Rust side from returning anything else.
- **`u128` for one multiplication.** `(counter - START) * 1_000_000_000 / HZ` overflows `u64` after about 6 seconds at 3 GHz. Widened to `u128` and narrowed back, it is exact for centuries.
- **`compare_exchange` in a loop** to take one bit from a mask: `take_signal` takes the lowest pending signal so that two threads can't both take it.
- **`Weak<Process>` in the alarm list**: an alarm doesn't keep a process alive. `upgrade()` gives `None` for one that has ended, and the list drops it.
- **Drop doesn't happen in a function that never returns.** A `-> !` call is the end of the stack frame, and the values in it are never dropped. The leak in Bugs was exactly that.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `basics` on the disk.

| Type | You should see |
|---|---|
| (at boot, in the terminal) | `[clock] the CMOS clock says <today's date and time, UTC>; time is counted from here at N Hz` |
| `basics date` | today's date and time in UTC, e.g. `Wednesday 25 November 2026, 15:32:46 UTC; up 4.333 s` |
| `with programs basics` | `basics: the date is ...`, a few seconds of alarms, then `basics: 33 checks, all passed`. In the terminal, four children: `ended by signal 15 (SIGTERM)`, `exited with code 7`, `ended by signal 6 (SIGABRT)`, `killed: page fault` |
| `basics catch`, then Ctrl+C | `basics: waiting for a line (Ctrl+C to interrupt)`, then `basics: caught SIGINT while waiting for a line - tidying up` |
| `forever`, then Ctrl+C | `forever was stopped`; in the terminal `ended by signal 2 (SIGINT)` |
| `with files basics speed` | `basics: 4 MiB in 64 writes took N ms` (a few hundred) |
| `clib assert` | `libc/programs/clib.c:461: main: assertion failed: number == 2`, then `clib was ended by signal 6 (SIGABRT)` (it was `exited with code 134`) |

In the terminal, `[blk] virtio disk at ...: ..., a write cache (flushed on commit), ...`.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1. At boot: `[clock] the PL031 says ...`, at 62500000 Hz.

**3. The tests by themselves**

```
make basics64-test
make aarch64-basics-test
make test64
```

| Look for | Meaning |
|---|---|
| `the core basics (x86_64): the date read at boot is the host's (N s apart); 33 checks of the clock, signals and fsync passed; Ctrl+C reached a waiting program's handler; 4 MiB written with flushes` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

Automated: make basics64-test, make aarch64-basics-test, make test64
