# C on AArch64

*Journal entry #57 · Era 3, step 3f, second half. The C library and four C programs build for AArch64 from the same sources, and the AArch64 kernel runs them at every boot. No line of the library had to change for the CPU; one line changed because of what `char` means on it.*

```
Hello from C
argc 3
argv[1] one
argv[2] two words
7.0 / 2 * 4 = 14
[process] 35 "chello" exited with code 3
strings: 28 checks, all passed
memory: 15 checks, all passed
cprimes: 25 primes below 100
[selftest] c: 8 runs of 4 C programs, twice - each ended with the code its main returned; nothing leaked
```

---

## Why

The milestone of Era 2 (entry 49) was a C program running on four CPUs. Era 3's promise is that *the same user space* runs on two kinds of CPU. Entry 56 covered the Rust programs. A C program is a harder test of that promise, because more people's assumptions are stacked up in it: the compiler's, the linker's, the C library's, and mine from entries 36 to 42 when I wrote that library with only x86 in mind.

I expected a list of things to port: `_start`, the system call path, the stack set-up. I wanted to find out how long the list was.

## How long the list was

It was empty. This is why:

```
  C program            main(argc, argv)
  libc/c/*.c           printf, stdio, ctype, stdlib          C
  libc/src/*.rs        _start, write, malloc, strings…       Rust
  user/src/lib.rs      taaj::write, taaj::arguments…         Rust
  taaj::syscall6       svc #0   /   syscall                  one function per CPU
```

The library's `_start` is not assembly. It is an ordinary `extern "C"` Rust function that takes the two numbers the kernel passes, and the kernel passes them where the calling convention puts a function's first two arguments - RDI and RSI on the PC, x0 and x1 here. Entry 56 made that true on AArch64 for Rust programs, so it was already true for C.

And the library never makes a system call itself. Every one goes through the `taaj` crate, and `taaj` has exactly one function with two bodies.

So the work was the build.

## The build

```
make cprogs-aarch64
```

| Step | PC | AArch64 |
|---|---|---|
| The Rust half | `cargo build` (x86_64-unknown-none) | the same with `--target aarch64-unknown-none-softfloat` |
| The C half | `clang --target=x86_64-unknown-none-elf … -mno-red-zone -mgeneral-regs-only` | `clang --target=aarch64-unknown-none-elf … -mgeneral-regs-only` |
| Link | `ld.lld … -T user/user.ld` | the same, plus `-z max-page-size=4096` |
| Where | `build/c/` | `build/c-aarch64/` |

Three differences, each with a reason:

**No `-mno-red-zone`.** The red zone is an x86-64 convention: a function may use 128 bytes *below* its stack pointer without moving it. AArch64's convention has no such thing, so there is nothing to switch off.

**`-mgeneral-regs-only` means something slightly different.** On both CPUs it says "use only the integer registers", and for the same reason - the kernel does not save the others when it switches programs. On AArch64 the kernel goes further and leaves them switched *off*, so a program that used one would be ended on the spot. Arithmetic on `double` still works: the compiler turns `7.0 / 2 * 4` into calls to functions that do it with integers (`__divdf3`, `__muldf3`), and those are in `libc.a`, which the Rust compiler fills in. `chello float` printing `14` is that whole chain working.

**`-z max-page-size=4096`.** The first AArch64 `chello` was 111 KB against 50 KB with the flag. Some AArch64 systems use 64 KiB pages, so the linker lines every segment up on 64 KiB *in the file* to be safe on all of them. This system has 4 KiB pages; the padding was 60 KB of zeroes per program.

## Where the programs live, for now

On the PC, C programs are files on the disk and the boot tests type their names at the shell. AArch64 has no disk and no shell yet (step 3g). So until it does, four programs are embedded in the kernel image and a self-test runs them:

```rust
pub static CHECK: &[(&str, &[u8], &[&str], i64)] = &[
    ("chello", CHELLO, &["one", "two words"], 0),
    ("chello", CHELLO, &["float"], 0),
    ("chello", CHELLO, &["fail"], 3),
    ("chello", CHELLO, &["exit"], 4),
    ("cmem", CMEM, &[], 0),
    ("cmem", CMEM, &["sort", "pear", "apple", "fig"], 0),
    ("cprint", CPRINT, &["one"], 0),
    ("cprimes", CPRIMES, &["100"], 0),
];
```

Each is started the way the shell starts a program: its arguments encoded into one message (`abi::arguments::encode`) waiting in a channel, the channel handed over as a startup handle. The test asserts the exit code - `return 3` from `main` and `exit(4)` both have to come back as numbers - and the boot script reads what the programs printed.

Embedding needs a C compiler to build the kernel, which the kernel never needed before. So it is a Cargo feature, `c-programs`, that the Makefile turns on only when Clang and lld are there. A *feature* in Cargo is a named switch: code marked `#[cfg(feature = "c-programs")]` exists only in builds that ask for it.

The four are the ones that need nothing but arguments and output. `cfile`, `cstdio`, `cwords` and `cask` need files or a keyboard; they build for AArch64 already and wait for 3g.

## The one line: is `char` signed?

Clippy, run for the AArch64 target, complained about this in the library's `strncmp`:

```rust
let (x, y) = unsafe { (*a.add(at) as u8, *b.add(at) as u8) };
//                     ^^^^^^^^^^^^^^^^ casting to the same type is unnecessary (`u8` -> `u8`)
```

On the PC that cast is necessary. C leaves it to each platform whether plain `char` is signed, and they chose differently: signed on x86-64, **unsigned on AArch64**. Rust's `c_char` follows - it is `i8` on one and `u8` on the other. So the same line is a real conversion on one CPU and a no-op on the other.

The code was correct on both; the cast only looked redundant on one. But a warning that appears on one CPU and not the other is a trap for later, so the line now reads the byte *as a byte*, whatever `char` is:

```rust
let (x, y) = unsafe { (*a.add(at).cast::<u8>(), *b.add(at).cast::<u8>()) };
```

`.cast::<u8>()` turns a `*const c_char` into a `*const u8` - the same address, read as a different type. The test that this matters at all was already there from entry 38: `cmem` checks `strcmp("\xe9", "z") > 0`, "strcmp compares as unsigned". It passes on both.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| C programs on AArch64 before there is a disk | embedded in the kernel, behind a feature | wait for 3g: the library would stay unproven on this CPU for two more steps |
| Which programs | the four that need only arguments and output | all eight: four would fail for want of files, proving nothing |
| How the test starts them | arguments in a channel, like the shell | no arguments: `argc`/`argv`, `exit(4)` and soft float would go untested |
| One at a time or all at once | one at a time | at once: their lines would interleave and the boot script could not read them |
| Segment alignment | 4 KiB | the linker's 64 KiB default: right for other systems, padding for this one |
| The `char` cast | read through a `u8` pointer | `#[allow]` on the lint: hides the next real one too |
| A separate `build/c-aarch64` | yes | sharing `build/c`: one `make` would overwrite the other's `libc.a` |

## Bugs

None found in the library or the kernel: the first AArch64 kernel built with the feature ran all sixteen runs.

That is suspicious enough that I checked what the result rests on. It rests on entry 56's `regs` program (the registers and the stack pointer a program starts with) and on the calling convention being followed by three compilers - rustc for the library, Clang for the C, rustc again for the kernel that sets x0 and x1. No part of this step could have worked by accident if one of them disagreed.

## How I checked it

**Automated** (`make test64`, ending `test64: all passed`): the twelve AArch64 boots that have threads must each print `argv[2] two words`, `7.0 / 2 * 4 = 14`, both `chello` exit codes (3 and 4), `strings: 28 checks, all passed`, `memory: 15 checks, all passed`, `apple fig pear`, the `printf` line with the largest 64-bit numbers, `cprimes: 25 primes below 100`, and the self-test's summary. The summary itself asserts each exit code, that no frame is lost between the first round and the second, and that no channel is left.

**Clippy**: clean for `libc/` on both targets, and for the kernel with and without the feature.

**Not tested:**
- No deliberate breakage this time. The checks are the ones entries 36 to 42 built, pointed at a new CPU; I did not re-prove that each can fail.
- `cfile`, `cstdio`, `cwords`, `cask` on AArch64: they compile and link, and have never run there.
- A C program that uses a floating-point register on purpose (it should be killed).
- The PC's C tests are unchanged and ran as before.

## What is still missing

- A disk, a shell and a keyboard on AArch64 - so that C programs are files again and the embedding can go. Step 3g.
- The C library's host tests run on the host's CPU only.
- `libc.a` for AArch64 exists only in `build/c-aarch64/`, as the PC's does in `build/c/`.

## What I learned

- **The best port is the one that already happened.** The library was portable because it was built on one narrow thing - a Rust crate with a single CPU-specific function - and not because I planned for a second CPU.
- **"Entry point" does not have to mean assembly.** If the kernel starts a program by the calling convention, `_start` is just a function.
- **C's `char` is not a type, it is a decision each platform made.** Code that reads text should say `unsigned` or `u8` and never find out.
- **Linker defaults encode someone else's system.** 64 KiB alignment is correct for a system I am not building.
- **A second target is a free review.** The same lint run for another CPU found the one place my code assumed something about the first.

## Testing note

**1. C programs on AArch64**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**. After the `[selftest] kill:` line:

| Look for | Meaning |
|---|---|
| `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two words` | a C program started, with its arguments |
| `7.0 / 2 * 4 = 14` | arithmetic with fractions, done without floating-point registers |
| `[process] … "chello" exited with code 3` | what `main` returned |
| `[process] … "chello" exited with code 4` | what was given to `exit()` |
| `strings: 28 checks, all passed` and `memory: 15 checks, all passed` | the library's string functions and `malloc` |
| `apple fig pear` | `cmem sort pear apple fig` |
| `printf: 42 -7 ff 0X2A [   hi] [hi   ] [00042] x 100%` | `printf` |
| `cprimes: 25 primes below 100` | a loop that only computes |
| `[selftest] c: 8 runs of 4 C programs, twice - each ended with the code its main returned; nothing leaked` | every exit code as expected, twice, nothing left behind |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

Each program's lines appear twice (the test runs two rounds).

**2. The files themselves**

```
make cprogs-aarch64
readelf -h build/c-aarch64/chello | grep Machine
readelf -lW build/c-aarch64/chello | grep LOAD
ls -l build/c-aarch64/chello build/c/chello
```

`Machine` should be `AArch64`; the three `LOAD` lines should end in `0x1000` (4 KiB alignment); the two files should be of similar size (about 50 KB and 54 KB), not 111 KB.

**3. The PC, unchanged**

```
make run64
```

| Type | You should see |
|---|---|
| `chello one "two words"` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two words` |
| `chello float` | `7.0 / 2 * 4 = 14` |
| `cmem` | `strings: 28 checks, all passed` and `memory: 15 checks, all passed` |

**4. Host checks**

```
cd libc && cargo clippy --release --target aarch64-unknown-none-softfloat
cd libc && cargo clippy --release
```

Both should print no warnings.

Automated: `make test64`
