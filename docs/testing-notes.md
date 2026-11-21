# Testing notes, collected

The testing note from the end of each journal entry, from entry 044 on, one after another, so that a run of steps can be tested in one sitting. Each is copied from its entry word for word; the entry has the story. For entries before 044, see the entry itself.

Build once and everything below runs from the root of TheTaaJ. `make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from. `make test64` runs every automated test and should end `test64: all passed`.

---

## 044 · Waking the Other CPUs

[The entry](journal/044-waking-the-other-cpus.md)

```
make run64
```

Nothing new to type: what changed is in the kernel's log, in the terminal `make run64` was started from. Scroll up to just after the `[selftest] timer` line.

**In the log**

| Look for | You should see |
|---|---|
| `[acpi] 4 CPU(s)` | `[acpi] 4 CPU(s), APIC ids [0, 1, 2, 3]` |
| `[smp] CPU 1 awake` | Three lines, for CPU 1, 2 and 3, in that order, each with its APIC id and a different stack address |
| `[smp] 4 CPUs awake` | `[smp] 4 CPUs awake: CPU 0 runs everything; CPU 1, 2, 3 started through the page at 0x1000` |
| `[smp] counting together` | `4 CPUs reached 400000 of 400000 with a lock; without one NNNNNN, NNNN lost`. The first two numbers must be 400000. The last two differ on every boot; "0 lost" is possible but I have not seen it |

**In the window**

| Type | You should see |
|---|---|
| `primes`, or any command you like | Everything works as before: the system still runs on CPU 0 alone |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | `[smp] 1 CPU: no others to start`, and a working system |
| `make run64 RUN_CPUS=2` | One `awake` line, `2 CPUs awake`, `reached 200000 of 200000 with a lock` |
| `make run64 RUN_CPUS=20` | `[smp] 4 more CPU(s) than the 16 this kernel uses: left asleep`, fifteen `awake` lines, `reached 1600000 of 1600000` *(slow to start in QEMU: twenty emulated CPUs on a few real ones)* |
| `make run64-uefi` | The same four lines as `make run64` |

**On the host**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu smp
```

should end `9 passed`.

```
make smp64-test
```

boots with 1, 2 and 20 CPUs and should end `smp64-test passed`.

Automated: `make test64`

---

## 045 · Each CPU Finds Itself: GS and `swapgs`

[The entry](journal/045-each-cpu-finds-itself.md)

```
make run64
```

Nothing new to type. In the kernel's log (the terminal `make run64` was started from):

| Look for | You should see |
|---|---|
| `[smp] CPU 1 awake` | The three `awake` lines as in entry 44, numbered 1, 2, 3 — each number now read by that CPU through its own GS |
| `[gs] system calls` | Twice: `[gs] system calls, timer interrupts, then a read through GS...`, each followed a few lines later by `[process] page fault at 0x… in user mode`, `user read at 0x5a5: page not present`, and `[process] N "gs" killed: page fault` |
| `[gs] FAILED` | Nothing. This line must not be there |
| `[selftest] user:` | `hello exited 42, spin and heapcheck exited 0, peek, cli, port, memro and gs were killed; 16 processes, nothing leaked` |

**In the window**

| Type | You should see |
|---|---|
| `primes`, `ps`, `cask` and a line, anything | Everything as before. Every key pressed is an interrupt and every command dozens of system calls, all through `swapgs` |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | The same `[gs]` lines and a working system: GS is used with one CPU too |

Automated: `make test64`

---

## 046 · Threads on Every CPU

[The entry](journal/046-threads-on-every-cpu.md)

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

---

## 047 · One CPU Taps Another on the Shoulder: the Reschedule Interrupt

[The entry](journal/047-one-cpu-taps-another.md)

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

---

## 048 · The Kernel Outgrows Its Megabyte

[The entry](journal/048-the-kernel-outgrows-its-megabyte.md)

```
make big64-test
```

It builds a second kernel (a minute the first time) and boots it twice. You should see, at the end:

| Look for | You should see |
|---|---|
| The last line | `big64-test passed` |

and in the two logs:

```
grep -a "big image\|ramdisk  " build/big64-bios.log build/big64-uefi.log
```

| Look for | You should see |
|---|---|
| `build/big64-bios.log` … `ramdisk` | `ramdisk        0x2000 bytes at 0x300000` — at 3 MiB, above the kernel |
| `build/big64-bios.log` … `big image` | `[test] big image: the kernel ends at 0x2c…, above 2 MiB; 768 KiB of it at 0x…-0x… is byte for byte what was built` |
| `build/big64-uefi.log` … `big image` | The same line (and `ramdisk        none`: the UEFI loader carries no ramdisk) |
| `FAILED` | Nothing |

**The ordinary system** — it is loaded by the changed loader too

```
make run64
```

| Type | You should see |
|---|---|
| nothing | The system starts as before, to the shell's prompt |
| `ls`, `primes`, `ps` | As before |

**To see the wall itself** (optional): in `kernel/kernel64/linker.ld` change `KERNEL_LOAD_LIMIT = 0x300000;` to `0x100000;` and run `make kernel64`. The build should stop with `the kernel image ends above 3 MiB, where the bootloader refuses to load it`. Change it back.

Automated: `make test64`

---

## 049 · The Milestone: A C Program on Four CPUs

[The entry](journal/049-the-milestone.md)

**1. Compile the C program on your machine and put it on the disk**

```
make cput C=cprimes
```

should end `cprimes is on build/taaj-disk.img: make run64, and type cprimes`.

**2. Run it, on four CPUs**

```
make run64
```

| Type | You should see |
|---|---|
| `cprimes 100` | `cprimes: 25 primes below 100` |
| `cprimes 12x` | `cprimes: not a limit between 0 and 1000000000: 12x` |
| `cprimes` | `usage: cprimes LIMIT` |
| `time cprimes 4000000` | `cprimes: 283146 primes below 4000000`, then `took 1.… s; the 4 CPUs worked for 1.… s in all` (mine: 1.05 and 1.08) — the two numbers about the same: one program, one CPU's work |
| `uptime` | `up … s (… ticks), on 4 CPUs` |
| `cprimes 60000000 &` and Enter, four times, quickly — each takes about a minute | `[1] cprimes 60000000` … `[4] cprimes 60000000` |
| `jobs` | Four jobs, running |
| `time wait` | Four lines `cprimes: 3562115 primes below 60000000`, then `took T s; the 4 CPUs worked for W s in all` with **W three to four times T** |

**3. The same on one CPU**

```
make run64 RUN_CPUS=1
```

| Type | You should see |
|---|---|
| `uptime` | `… on one CPU` |
| `cprimes 24000000 &` four times, then `time wait` | The same four answers (`1507122 primes below 24000000`), and `took T s; the one CPU worked for W s in all` with **W equal to T**, or a hair under. Typing will feel slow while they run - each key waits its turn behind four busy programs - so `time wait` may be read late and T may be much less than the minute the four really take. `uptime` before and after shows the real time. The ratio is the result. |

**4. Compare with your own machine** (optional)

```
build/c/host-cprimes 24000000
```

prints the line TheTaaJ printed. It is the same `libc/programs/cprimes.c`, compiled by `cc` against Linux's C library.

Automated: `make test64`
