# Std's ground, part 1: random numbers, real thread-local storage, signal stacks, SIGCHLD

*Journal entry #99 · The-Jat's step B (`docs/who-does-what.md`), on the branch `std-ground-1`. Era 9's next big step is R3: Rust's standard library on TheTaaJ, built on our libc. Before that can start, libc has to offer what std asks a Unix for. This step is the first half of that list, the four items most other work waits for:
- **random numbers** (`getrandom`): std's hash maps seed themselves with it, and MaK's later steps (address randomization, random TCP numbers, TLS) need it too;
- **real thread-local storage**: std keeps per-thread data in `#[thread_local]` variables, which the compiler reaches through the thread pointer. Until now TheTaaJ only had the emulated kind, a function call per access;
- **signal stacks** (`sigaltstack`): std installs one in every thread so that a stack overflow prints "thread has overflowed its stack" instead of a bare crash;
- **SIGCHLD and `waitid`**: `std::process::Child::try_wait` and friends.*

## Why

Each of these is something a Unix program can count on and TheTaaJ couldn't offer:
- **No random numbers at all.** `rand()` is a fixed sequence from a seed, and there was nothing unpredictable to seed it with. Programs that need secrets, unique names or hash seeds had nowhere to get them.
- **Thread-local variables cost a call each.** `-femulated-tls` makes every access to a `_Thread_local` variable a call to `__emutls_get_address`, which looks the variable up in the thread's table. Rust's std on a Unix target expects the native kind: an offset from the thread pointer, one instruction.
- **A stack overflow killed the program without a word.** The kernel pushes a signal's context onto the stack, and an overflowed stack has no room for it. The only way out is a second stack, set aside beforehand.
- **A program never heard of its children ending.** Its children are started by init (posix_spawn asks init), so the kernel doesn't see the program as their parent, and nothing sent it SIGCHLD.

## How it fits together

```
 kernel                                      libc                         a C or Rust program
 ──────────────────────────────────────      ─────────────────────────    ─────────────────────
 chacha.rs   ChaCha20 + Generator             random.rs                    getrandom(buf, n, 0)
 random.rs   seed at boot: RDSEED/RDRAND/RNDR   getrandom, getentropy,     arc4random_uniform(6)
             + 256 loop timings + the date      arc4random*
             every tick: counter -> pool
   RANDOM 39 ──────────────────────────────►

 elf.rs      keeps the program headers        threads.rs                   _Thread_local int x;
   PROGRAM_HEADERS 40 ─────────────────────►   PT_TLS -> each thread's      x = 5;  (fs:-0x1100,
                                                area: TLS block + TCB        or tpidr_el0+0x40)
                                               THREAD_POINTER -> the TCB

 threads.rs  a signal stack per thread        signals.rs  sigaltstack      sigaltstack(&ss, 0);
 signals.rs  deliver on it if asked           signal.c    SA_ONSTACK ->     sigaction(SIGSEGV,
   SIGNAL_STACK 41, SIGNAL_SETUP's 4th arg ◄─   the on-stack mask             SA_ONSTACK ...)

 process.rs  at a process's end: signal       spawn.rs   posix_spawn ->     signal(SIGCHLD, h);
             everyone who asked                 PROCESS_NOTIFY(child,       waitid(P_PID, id,
   PROCESS_NOTIFY 42 ◄──────────────────────    SIGCHLD); waitid              &info, WEXITED)
```

### Random numbers

The generator is ChaCha20, the stream cipher from RFC 8439, used the way OpenBSD's arc4random and newer Linux kernels use it:
- the generator keeps one 256-bit key;
- **stirring in a sample** (`stir`) replaces the key with the first half of a ChaCha20 block made from the old key, with the sample in the nonce. Whatever the sample is worth is now in the key, and nothing that was in the key is lost;
- **a request** (`split`) makes one block from the key, with the pool of samples gathered since the last request in the nonce. Its first 32 bytes become the generator's next key, and its last 32 bytes become a key used only for this request. Then the request's bytes are the ChaCha20 stream of that key, made with no lock held.

Replacing the key on every request is called *fast key erasure*: someone who reads the kernel's memory later can't work back to bytes already handed out.

What goes in:
- **at boot**, right after the clock starts:
  - 8 numbers from the CPU's random instruction, if it has one (RDSEED or RDRAND on x86, RNDR on AArch64);
  - the date and the counter;
  - **256 timings of a small loop** that writes scattered words of a buffer. How many counter steps the loop takes varies with caches, the memory bus, the host's other work and interrupts, and the low bits of each timing are hard to predict;
- **at every timer tick**, the counter's exact value is rotated and XOR-ed into a pool word. It's the moment the tick was handled, which wobbles. Every RANDOM call adds the counter too. The pool goes into the key at the next request.

The kernel logs which sources it used: `[random] seeded from 256 timings (no random instruction on this CPU)`, or on a CPU that has one, `[random] seeded from 8 numbers from RDSEED and 256 timings`.

### Thread-local storage

A program's `_Thread_local` variables have a **template** in its file: the PT_TLS program header points at the initial bytes (`.tdata`) and says how big the whole block is with the zeros after them (`.tbss`) and how it must be aligned. The linker gives every variable a fixed offset from the thread pointer. Each thread's copy has to be laid out exactly as the linker assumed, and the two CPUs' ELF ABIs assume different things:

```
 x86_64  (variant II)   [ TLS block            ][ TCB: self, record ]
                        ^ TP - round_up(size, align)    ^ TP = FS base
 AArch64 (variant I)    [ TCB: record, 0 ][ pad ][ TLS block ]
                        ^ TP = TPIDR_EL0         ^ TP + round_up(16, align)
```

For `ground.c` the template is 0x10e4 bytes aligned to 64, and clang's code reads `answer` with `cmpl $0x2a, -0x1100(%rbx)` after `mov %fs:0, %rbx`. 0x1100 is 0x10e4 rounded up to 64, which is where `tls_area` puts the block.

How libc builds it:
- the kernel keeps each program's headers and gives them on request (PROGRAM_HEADERS). libc reads them at the very start of `_start` and keeps the PT_TLS fields;
- `tls_area` allocates one piece of memory per thread for the block and the 16-byte TCB, copies the initial bytes in (the rest stays zero), and fills in the TCB. On x86_64 the TCB's first word is its own address, because the x86_64 ABI says `fs:0` holds the thread pointer, and its second word is the thread's record. On AArch64 its first word is the record;
- the main thread's area is made in `_start`; every other thread's in `pthread_create`, whose thread pointer for the kernel is now the TCB instead of the record. Joining or detaching frees the area with the stack;
- `current()`, which errno uses, now reads `fs:8` on x86_64, or the word at TPIDR_EL0 on AArch64.

Both linker scripts (`user/user.ld`, `user/cxx.ld`) gained a `tls PT_TLS` program header, with `.tdata` and `.tbss` in both the data segment and it. Without that, a script that names its program headers produces no PT_TLS at all.

C programs no longer need `-femulated-tls`: the threads test (`threads.c`) is now compiled without it, and its `_Thread_local` is a real one. C++ is still built with it; see "What is still missing".

### Signal stacks

- **The kernel** keeps a signal stack per thread (SIGNAL_STACK) and, from SIGNAL_SETUP's new fourth argument, the set of signals whose handlers asked for SA_ONSTACK. `deliver` puts the context on the signal stack when the signal is in that set, the thread has a signal stack, and isn't already on it. A second signal during such a handler stacks below the first, on the same signal stack;
- **libc's `sigaltstack`** checks the request, tells the kernel, and remembers it in the thread's record. It answers what the stack is, including SS_ONSTACK while running on it, and refuses a change while on it (EPERM), a stack below MINSIGSTKSZ (ENOMEM) and unknown flags (EINVAL). `signal.c` builds the SA_ONSTACK mask from the actions, next to the caught and ignored masks.

A stack overflow now goes like this. The thread touches the page below its stack, which belongs to no reservation, so it page-faults in user mode. If SIGSEGV is caught, the kernel delivers it. The thread's stack pointer is still in the overflowed area, but SIGSEGV is marked on-stack, so the context goes on the signal stack, and the handler runs there.

### SIGCHLD and waitid

The kernel has no parent-child link for programs: init starts every program, and the asker gets a handle. So the kernel offers a general mechanism instead: **PROCESS_NOTIFY(handle, signal)**, "send me `signal` when that process ends". Any right to the handle will do. The process keeps a list of `(Weak<Process>, signal)` pairs, and its end goes through the list:
- **before** it wakes whoever waits for it. So a program that catches SIGCHLD and waits for the child has its handler run before the wait returns;
- if the process has already ended when PROCESS_NOTIFY is called, the signal is sent at once. The check and the push happen under the list's lock, and the end takes the list after setting the exit code, so nothing is missed.

libc's `posix_spawn` (and `system`, `popen` and pipelines, through the same `adopt`) asks for SIGCHLD for every child. SIGCHLD does nothing unless caught, as before.

**`waitid`** is `waitpid` with more detail. It supports:
- P_ALL or P_PID; WEXITED is required;
- WNOHANG: returns 0 with `si_pid` 0 if no child has ended;
- WNOWAIT: reports the child but leaves it to be waited for again;
- the result: `si_code` CLD_EXITED or CLD_KILLED, and `si_status` the exit code or the signal.

`siginfo_t` gained `si_uid` and `si_status`, after the fields it had.

## Design decisions

- **ChaCha20, not a hash of the samples.** One primitive does both the mixing and the output, it needs no special instructions, and its correctness is checkable against RFC 8439's vectors. The design is close to well-reviewed ones, but **nobody has reviewed this one as a cryptographer would**. The journal says so, and so does the code.
- **Never block.** Linux's getrandom blocks until the kernel estimates it has 256 bits of entropy. Estimating entropy is guesswork, and a program that blocks at boot is worse than useless. TheTaaJ seeds before any program runs and never blocks. The risk is a machine with no random instruction and very regular timings. QEMU's emulation still produced different bytes on every boot.
- **The TLS block is allocated next to its TCB, not inside the thread's record.** The record is a Rust struct whose size libc chooses. The TLS block's size and alignment come from the program, and on AArch64 the block must start right after the TCB. A separate allocation per thread keeps both simple.
- **The thread pointer points at the TCB, not the record.** The ABI decides what's at the thread pointer, and the variables' offsets depend on it. The record moved one indirection away: `fs:8`.
- **Program headers from the kernel, not from `__ehdr_start`.** lld defines `__ehdr_start` only when the ELF header is loaded, and ours isn't. Programs linked by TCC on TheTaaJ have no linker-script symbols at all. The kernel already reads the headers, so keeping 56 bytes per header costs nothing, and dl_iterate_phdr (step C) needs them too.
- **PROCESS_NOTIFY instead of a parent pointer.** A parent pointer would make init everyone's parent. Asking "tell me when this ends" works with handles: whoever holds a process handle may ask, and nobody else can.
- **Signal stacks are per thread, the on-stack mask per process.** That's POSIX: sigaltstack is a thread's, sigaction a process's.

## Bugs

Two in the test itself, found by typing the testing note in a scripted boot:
- **`ground` was killed after its last check.** It printed `ground: 51 checks, all passed`, then died fetching an instruction at `0x1a0cb4eb0ef3f4fd`. That address was random bytes: `getentropy(a, 256)` wrote 256 bytes into the 64-byte array `a`, over `main`'s return address. libc was right to write 256 bytes; the test's buffer was wrong. It now has a buffer of its own, of 257 bytes;
- **and the test script didn't notice.** It checked for `"ground" exited with code 0`, which a child's exit also matches. It now requires exactly one `"ground" killed` (the child without a signal stack) and no "ground was killed" from the shell. Against the bad run's log, that check fails as it should.

Three were caught before running, and one in the plan file:
- `pthread_create`'s new error path unmapped the stack twice: once itself, once inside `free_thread`. Found while reading the change over;
- clang refused the test's endless recursion (`-Winfinite-recursion`), so the recursion now ends on a `volatile` flag that is always 1;
- **In `docs/who-does-what.md` (entry 098's file, already on `main`)**: a leftover piece of an old table, three rows below "The decided order". The script that replaced the "Now" table cut it at the first `---` after the heading, and the table's own `|---|` line matched first. Removed in this step.
- `crate::descriptors::fail(EINVAL) as isize` didn't compile: `fail` is generic over its return type, and `as` doesn't tell it which. It's now `fail::<isize>(EINVAL)`.

## How I checked it

**Host tests**: `chacha.rs`:
- **RFC 8439's block test vector** (section 2.3.2) and **appendix A.1's first vector**, both checked independently with a Python implementation first;
- a stream that ends mid-block;
- every split giving a new key;
- the same samples giving the same keys;
- a 64 KiB stream with half its bits set, to within 0.5%.

**The new boot tests, `make ground64-test` and `make aarch64-ground-test`.** Each boots twice:
1. **QEMU's usual CPU, which has no random instruction.** The kernel must say `[random] seeded from 256 timings (no random instruction on this CPU)`. `ground random` prints 16 bytes, and **`with programs ground` runs 51 checks**:
   - **random numbers:**
     - getrandom gives what's asked for and differs between calls;
     - a megabyte has half its bits set (within 20 standard deviations) and every byte value about equally often (chi-squared under 450, where about 255 is normal);
     - no 16-byte block of the megabyte appears twice;
     - getentropy of 256 bytes works and of 257 bytes gives EIO;
     - an unknown flag gives EINVAL and a null buffer EFAULT;
     - `arc4random_uniform(6)` gives each of 0-5 between 800 and 1200 times in 6000 draws;
   - **thread-local storage:**
     - the main thread starts with the template's values (`.tdata` 42 and "template", `.tbss` zeros);
     - a 64-byte-aligned variable is aligned;
     - four threads start after the main thread changed its own values. Each sees the template's values, not the main thread's; zeros; its own aligned address; and keeps its own values and errno while the others change theirs;
     - the main thread's values are untouched afterwards;
   - **signal stacks:**
     - none at first; ENOMEM, EINVAL;
     - an SA_ONSTACK handler runs inside the signal stack, sees SS_ONSTACK and gets EPERM when changing it;
     - without SA_ONSTACK the handler runs on the normal stack;
     - SS_DISABLE removes the signal stack;
     - **a thread with a 64 KiB stack overflows it**, its SIGSEGV handler runs on the thread's own signal stack and `siglongjmp`s out, and the program goes on;
   - **children:**
     - ECHILD with none;
     - `waitid` sees a child exit with 5 (CLD_EXITED), and **its SIGCHLD was handled before waitid returned**;
     - WNOHANG with a child still running;
     - **`pause` ends at the child's SIGCHLD**;
     - WNOWAIT, then `waitpid` still finds the child;
     - CLD_KILLED with SIGTERM;
     - EINVAL for P_PGID or no WEXITED;
     - **a child that overflows its main stack** with a SIGSEGV handler on a signal stack prints `ground: caught the stack overflow on the signal stack` and exits 3;
     - one without a signal stack is ended by SIGSEGV.
2. **`-cpu max`**, which has the instruction. The kernel must say `[random] seeded from 8 numbers from RDSEED and 256 timings` (RNDR on AArch64), and `ground random` must print different bytes from the first boot's.

On x86_64 the two boots printed `626ea490…691d` and `eb5e6a55…fb9b`. On AArch64 they printed `1bb2cb61…f479` and `135ddad2…4640`.

**The threads test** (`thread64-test`, `aarch64-thread-test`) is now compiled without `-femulated-tls`. Its `_Thread_local` check (8 threads, each its own copy from its starting value) passes with the real kind.

**Mutation experiments**, each undone, each run with `make ground64-test`:

| What I broke | What happened |
|---|---|
| x86_64 TLS block 16 bytes off from where the linker put it | **caught**: "the main thread's thread-locals start as the template says", and the same for new threads |
| the template's initial bytes not copied (a block of zeros) | **caught**: the same two checks |
| the kernel ignores the signal stack | **caught**: the SA_ONSTACK handler isn't on it, SS_ONSTACK and EPERM fail |
| libc no longer asks for SIGCHLD | **caught**, as a hang: `pause` waits for a SIGCHLD that never comes, and the script says "the steps did not all run" |
| each request's key not taken from the generator (always the same) | **caught**: two getrandom calls equal, arc4random constant, `arc4random_uniform(6)` lopsided |
| WNOWAIT ignored (the child reaped anyway) | **caught**: "after WNOWAIT the child can still be waited for" |
| SIGCHLD sent *after* waking the waiters | **not caught**. The parent's thread is only made ready by the wake; the dying thread sends the signal before the parent runs again, almost always. The order is kept as a guarantee, but no test forces the race |
| no boot seeding at all (no CPU numbers, no timings, no date) | **caught only by the log line** (`seeded from 8 numbers from RDSEED` missing on `-cpu max`). The two boots' bytes still differed (`93d7f344…` and `07322a5b…`): the timer ticks and the counter at each call are enough to make them differ. So the tests show the generator isn't stuck. They can't show how much it is worth |

## What is still missing

- **C++ still uses emulated TLS.** libc++, libc++abi and the LLVM port are built with `-femulated-tls`. Switching them means rebuilding the ports and checking LLVM's byte-for-byte tests again. That fits step E (LLVM 23), which rebuilds them anyway.
- **The randomness isn't measured, only checked for gross faults.** The tests catch a stuck or repeating generator and a seed that is the same on every boot. They can't tell good entropy from mediocre: with every boot sample removed, only the log line noticed (above).
- **No `/dev/urandom`** as a file: TheTaaJ has no `/dev`. Programs use `getrandom`.
- **TLS for dynamically loaded code** (`__tls_get_addr`, the general-dynamic model) isn't there: there are no shared libraries. Static programs use local-exec and initial-exec, which work.
- **SIGCHLD carries no information** about which child ended (a handler with SA_SIGINFO gets `si_pid` 0 for it). `waitid` says which.
- **The rest of R3's list** is step C: `poll`, `dl_iterate_phdr`, pthread attributes, `stat`, `realpath`, `posix_spawn` with descriptors 3 and up, an environment, `execvp`, zlib, `argv[0]`.

## What I learned

- **Thread-local variables are a three-way contract.** The compiler emits `fs:-0x1100`, the linker chose that 0x1100, and the C library must put the variable there in every thread. Each CPU's ABI fixes where the block sits relative to the thread pointer: below it on x86_64, after a 16-byte TCB on AArch64. A layout off by 16 bytes reads the neighbouring variable, and nothing complains.
- **Why x86_64 has `fs:0` point to itself.** Code that needs the thread pointer as a number (to compute `&x`) reads `fs:0`. There is no instruction to read the FS base cheaply on older CPUs, so the ABI asks for the pointer to be stored where FS points.
- **A signal stack is needed because of where the context goes.** The kernel saves a signal's context on the stack it interrupts. When that stack is the problem, delivery itself fails. A second stack, set aside before anything goes wrong, is the only way to run code after an overflow.
- **"Parent" is a Unix assumption.** With init starting everything, a parent pointer is meaningless. "Tell whoever asked" fits a capability system better: the right to be told comes from holding the handle.
- **Fast key erasure is cheap forward secrecy.** One extra block per request means that a later look at the generator's state reveals nothing about earlier output.

**Rust, as used here:**
- **`Layout` and `alloc_zeroed`/`dealloc`** for memory whose size and alignment are only known at run time (the TLS template's). The same `Layout` must be given back when freeing, so the record keeps it.
- **`#[cfg(target_arch = ...)]` on a `let`**: one statement per architecture, each giving the same tuple `(size, tp_offset, block_offset)`, so the rest of `tls_area` is written once.
- **`AtomicU64::try_update`** (it was `fetch_update`, renamed in this Rust): a compare-and-swap loop around a closure, for the pool word that every timer tick changes.
- **`Weak` in the notify list**, as in the alarm list: being told of a child's end doesn't keep the watcher alive.
- **`core::mem::take` on a locked `Vec`** takes the list out and leaves an empty one, so the lock is released before the signals are sent, since sending takes other locks.
- **`chunks_exact(56)` over the program headers**, and `u64::from_le_bytes(h[a..b].try_into()...)` to read their fields without `unsafe`.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `ground` on the disk.

| Type | You should see |
|---|---|
| (at boot, in the terminal) | `[random] seeded from 256 timings (no random instruction on this CPU)` |
| `ground random` | `ground: random ` and 32 hex digits, different every time |
| `with programs ground` | after a few seconds `ground: caught the stack overflow on the signal stack`, then `ground: 51 checks, all passed`; in the terminal, its children `exited with code 5`, `exited with code 0`, `ended by signal 15 (SIGTERM)`, `exited with code 3` and `killed: page fault`, then `"ground" exited with code 0` |
| `with files threads` | `threads: 30 checks, all passed`, as before, now with real thread-local storage |

To see the CPU's random instruction used, start QEMU yourself with `-cpu max` added to the `make run64` command line. The kernel then says `[random] seeded from 8 numbers from RDSEED and 256 timings`.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1. With `-cpu max` instead of `cortex-a72`, the kernel says `... from RNDR ...`.

**3. The tests by themselves**

```
make ground64-test
make aarch64-ground-test
make test64
```

| Look for | Meaning |
|---|---|
| `std's ground (x86_64): the kernel's random numbers seeded with and without a random instruction, different on two boots; 51 checks of getrandom, thread-local storage, signal stacks and SIGCHLD passed; a stack overflow caught on the signal stack` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

Automated: make ground64-test, make aarch64-ground-test, make test64
