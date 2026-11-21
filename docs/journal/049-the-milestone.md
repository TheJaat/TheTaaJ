# The Milestone: A C Program on Four CPUs

*Journal entry #49 · the end of the era that began with "there is no C library". A C file is compiled on my laptop, put on TheTaaJ's disk with a tool, and typed at the shell four times. Four CPUs work at once, each copy prints the answer the same file prints on Linux, and the shell can now say so in numbers.*

```
cprimes 24000000 &
[4] cprimes 24000000
time wait
cprimes: 1507122 primes below 24000000
cprimes: 1507122 primes below 24000000
cprimes: 1507122 primes below 24000000
cprimes: 1507122 primes below 24000000
took 18.00 s; the 4 CPUs worked for 63.55 s in all
```

The roadmap's sixth step is one sentence:

> **The milestone.** Compile a C program on the host, `tfstool put` it, and run it under `-smp 4`.

Nearly everything in that sentence was built in the entries before this one. This entry adds the last small pieces, runs it, and looks back at what the sentence needed.

---

## What the sentence needed

Read it one phrase at a time and count the entries.

| Phrase | What had to exist | Entry |
|---|---|---|
| "Compile a C program on the host" | A C library for TheTaaJ: `crt0` (what runs before `main`), `printf`, `malloc`, files, `FILE` streams, `strtol`, `qsort` — and a recipe for the host's compiler to use them and nothing of Linux's | 36–42 |
| "`tfstool put` it" | A file system that lives on a disk, and a host tool that writes the same format | 14, 15, 26 |
| "and run it" | A shell that finds a program by name on the disk; a kernel that starts a process from a file's bytes; standard input and output; arguments | 12, 15, 16, 31, 33, 43 |
| "under `-smp 4`" | The other CPUs awake, each finding its own data, the scheduler on all of them, and a CPU that is told when work arrives | 44–47 |

So the program of this entry is not interesting. What carries it is.

## The program

`cprimes LIMIT` counts the primes below a number by trial division: for each number, try every odd divisor up to its square root.

```c
static int is_prime(long number) {
    if (number < 2) {
        return 0;
    }
    if (number % 2 == 0) {
        return number == 2;
    }
    for (long divisor = 3; divisor * divisor <= number; divisor += 2) {
        if (number % divisor == 0) {
            return 0;
        }
    }
    return 1;
}
```

It is a bad way to count primes, on purpose. I wanted a program that

- keeps one CPU busy for some seconds and touches nothing else — no files, no waiting;
- ends with one line that is either right or wrong;
- is standard C, so the same file compiles on Linux and the two answers can be compared.

Two details in it are about TheTaaJ and not about primes.

**`divisor * divisor <= number`, not `sqrt`.** The kernel does not save the floating-point registers when it switches threads. A C program that used them would have them changed under it by any other program that did. So C for TheTaaJ is compiled with whole-number registers only, and this loop is written so that it needs no square root.

**`strtol`, with all three of its answers checked.**

```c
char *end;
errno = 0;
long limit = strtol(argv[1], &end, 10);
if (errno != 0 || end == argv[1] || *end != '\0' || limit < 0 || limit > 1000000000L) {
```

`strtol` reports through three channels: the number it returns, where it stopped reading (`end`), and whether the number overflowed (`errno`). `cprimes 12x` must be refused, not read as 12; `end` pointing at the `x` is how the program knows.

## From my laptop to a CPU

Here is everything that happens between `make cput C=cprimes` and the answer.

```
on the host                                   in TheTaaJ
-----------                                   ----------
cprimes.c
   | clang --target=x86_64-unknown-none,
   |   no host headers, no SSE
   v
cprimes.o
   | ld.lld, with crt0 first and libc.a
   v
cprimes  (a static ELF file)
   | tfstool put disk.img cprimes
   v
disk.img  ==== the virtio disk ===========>  blk (the disk driver, ring 3)
                                                 |
                                              fs (TaajFS, ring 3)
                                                 |  "cprimes": 54 KiB
   you type: cprimes 24000000 &                  v
   ---------------------------------------->  shell: not one of mine;
                                              is there a file called that?
                                                 |  reads it into a memory object
                                                 v
                                              kernel: process_spawn_image
                                                 |  checks the ELF, maps its
                                                 |  segments, makes a thread
                                                 v
                                              the ready queue  --offer-->  an idle CPU,
                                                                           told by interrupt
                                                 v
                                              crt0: argc, argv, then main()
```

Every box on the right is a program I wrote, and every arrow between two of them is a message on a channel. The kernel appears twice: it copies the file's bytes into a new address space, and it decides which CPU.

## Four at once, and how to know

Typing `cprimes 24000000 &` four times starts four processes. With four CPUs each should get one. How do I *know* they did, and did not take turns on one CPU?

The answer the copies print cannot tell: it is the same either way. Time can — but "it felt quick" is not a result.

There are two clocks worth reading:

- **the time that passed** — ticks of the one clock, which the boot CPU keeps;
- **the work that was done** — for every tick on every CPU: was that CPU running something, or idling?

On one CPU the second can never exceed the first: one CPU does at most one second of work per second. On four, four busy programs make it four seconds of work per second. So the *ratio* is a measurement of "at once", and it is not about how fast the host is.

### Counting work

Each CPU has its own timer tick (entry 46). One line added to both tick handlers:

```rust
fn count_if_busy(t: &Threads) {
    if !t.policy.is_idle(t.policy.current(arch::cpu())) {
        BUSY_TICKS.fetch_add(1, Ordering::Relaxed);
    }
}
```

This is **sampling**. A hundred times a second, on each CPU, the kernel looks at what is running and makes a mark if it is not the idle thread. A thread that ran for nine milliseconds and slept just before the tick is missed; one that woke just before it is counted for a whole tick. Over a second those errors cancel. It is how `top` works on every Unix, and it costs one comparison per tick.

The count goes out through `system_info`, the call `mem` and `uptime` already use, which gains two numbers: how many CPUs, and the busy ticks.

### `time` reads both clocks

```
took 1.25 s; the 4 CPUs worked for 1.25 s in all        <- one program
took 18.00 s; the 4 CPUs worked for 63.55 s in all        <- four
```

Unix's `time` prints `real` and `user`; this is the same idea with a sentence around it. The shell reads the busy count before and after the command and prints the difference.

`time wait` is the measurement of this entry: `wait` returns when every background job is over, so `time wait` is "how long until they were all done, and how much work was done meanwhile".

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How to show "at once" | Work done against time passed, both in the guest's own ticks | *A time limit* ("four copies must finish in under N seconds"). Entry 47 taught me what that tests: the host. *Which CPU each copy ran on.* True and unconvincing — four programs can each sit on a different CPU and still take turns. |
| What is counted | System-wide busy ticks | *Per-process CPU time.* The better number, and what `ps` should show one day; it needs a counter per thread and a place in the process list. For "did four CPUs work" the total is enough. |
| How it is counted | Sampled at the tick | *Measured at every switch* with the time-stamp counter. Exact, and a read of the counter on every context switch. Sampling is the usual trade. |
| Where the numbers go | Two more words in `SystemInfo` | *A new system call.* It is the kernel's state, and there is a call for that. |
| Starting four at once | Four lines, each ending in `&` | *`a & b & c & d` on one line.* I refused that in entry 33 — a `&` in the middle of a line means things in Unix that this shell does not do — and wanting it for a demo is not a reason to change my mind. The price: the program must run long enough to type four lines. |
| The program | Trial division | *A sieve.* Quicker, and done before anyone could see four CPUs busy. |

## Bugs

**The copies were over before I could start them all.** My first try used `cprimes 300000`. It took 0.09 seconds. By the time the test had typed the second line the first copy had long finished, and `time wait` reported `took 0.00 s`. Four programs "at once" needs each to last longer than it takes to type four lines — at a real keyboard too. The limit in the test is now 24 000 000: about seventeen seconds for the four together.

That is a property of my shell, not of the kernel: one line starts one job. See the design table for why I left it so.

**The harness would not wait that long.** `qemu-boot.sh` gives the shell ten seconds to finish a line before it types on. A line that is *meant* to take eighteen needed a way to say so: `~~` in the key list now waits up to ninety.

**One file more.** The boot test knows how many files are on its disk (`2 directories, 24 files`). There are 25 now. The test failed on that line, with the milestone itself already passed above it — which is the right way round to find it.

**I guessed how slow the emulator is, and was wrong by more than ten times.** I sized the first run expecting QEMU to be thirty times slower than the host. For this loop it is about twice as slow: `cprimes 4000000` takes 0.54 s on the host and 1.05 to 1.25 s in TheTaaJ.

## How I checked it

**Automated** (`make test64`): the boot test — BIOS and UEFI, four CPUs — now ends its C section by typing `cprimes 100`, then `cprimes 24000000 &` four times, then `time wait`. `check-boot.sh` requires:

- `cprimes: 25 primes below 100`;
- four lines, each exactly what the *same C file compiled for Linux* prints for 24000000 — the host program is run by the test, the number is not written in it;
- that the shell waited more than a second (the copies were still running when `time wait` was typed);
- that in that wait the CPUs worked at least **one and a half times** as long as the wait took.

One and a half, of a possible four. I did not ask for three because the number is not what the test is for: anything clearly above one cannot happen on one CPU. In my runs it was between 3.5 and 3.8 in six boots.

**By hand:**

- **One CPU** (`SMP=1`, the same disk and the same four lines): `took 20.84 s; the one CPU worked for 20.84 s in all` — the work equal to the time, as it must be, and the same four answers. (Why 20 and not 60 is told just below.)
- `cprimes 12x`, `cprimes` with no argument, `cprimes 100`, and `time cprimes 4000000` (`took 1.05 s; the 4 CPUs worked for 1.08 s in all`) at the prompt.
- `uptime` on four CPUs and on one: `… on 4 CPUs`, `… on one CPU`.

**What the clock says.** One copy alone takes 13 to 15 seconds in the emulator. On four CPUs, four copies were all done about 22 seconds after the first was started. On one CPU they took about a minute: `uptime` read 3 seconds before I started them and 66 after. So four CPUs finished the work nearly three times sooner — not four, because QEMU without KVM runs each emulated CPU as a thread on my four-core laptop, which was also running a desktop.

**And something I did not expect on one CPU.** `time wait` there reported only 20.84 seconds, though the copies ran for a minute. The shell had not *read* the line for the first half minute. Every key I typed had to pass from the keyboard driver to the shell to the display, and each of those three programs waited its turn behind four that never stop: 80 milliseconds a turn, several turns a key. The scheduler is fair, and fair is wrong here — whoever is typing should go first. There are no priorities yet, and this is the clearest reason so far to want them.

**Not tested:**

- **Real hardware.** Everything above is QEMU without KVM.
- **A C program that uses floating point while another does.** It would break; C programs are compiled not to.
- **More programs than CPUs for long.** Four on four. The scheduler's fairness with forty is tested by the host tests of entry 46, not by a boot.

## What is still missing

- **No priorities.** With more busy programs than CPUs, the keyboard waits in the same queue as everyone. Seen above.
- **`ps` does not show CPU time or CPUs.** The kernel now has the sample; nothing per process is kept.
- **Floating point.** The kernel must save and restore the FPU and SSE registers per thread before C programs can use `double` without a library doing the arithmetic by hand.
- **The compiler runs on the host.** "Compile a C program" on TheTaaJ itself is a different era.
- **`fork`, `exec`, signals, a POSIX layer.** A C program here starts from `main` and can do what `libc` offers; it cannot start another program yet.

## What I learned

- **A milestone is a sentence that was hard to earn, not hard to run.** The program is forty lines. Each phrase of the sentence it satisfies names several entries.
- **Two clocks.** "How long did it take" and "how much work was done" are different questions, and their ratio is a third: how many CPUs were really in use. That is what `real` and `user` in Unix's `time` have always been saying.
- **Ask for the weakest thing that proves the point.** One and a half times is a weak requirement and an unbreakable proof. Four would have been a strong requirement and a test of my laptop.
- **Sampling.** You do not have to watch something all the time to measure it; you have to look at moments that it cannot predict.
- **The same file on two systems is the best test of a C library.** `cprimes.c` has no `#ifdef`. If it prints one thing on Linux and another on TheTaaJ, my library is wrong, and nothing else can be.

## Testing note

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
