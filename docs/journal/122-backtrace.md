# A panic that says where: backtraces in the kernel's own words

*Journal entry #122 · a kernel feature (K137, first part). Written on TheJat-WSL/ci. A kernel panic said what went wrong and in which source line, a CPU exception gave the registers, and neither said how the kernel got there. Now both print the thread and a backtrace - each caller, named: `kernel64::selftest::threads+0x603`, not `0xffffffff80132463`. The names come from the kernel's own symbol table, written into the kernel after it is linked.*

## Why

Every fault test, every panic, ended in an address. To learn which function it was in, I had to take the address to the host and look it up in the build that made it - if I still had that build. On real hardware, with only the screen, not at all. A backtrace in words is the first thing Era 22's observability needs (K137), and the one that every later debugging session leans on.

## What it is

What a page fault in the kernel prints now, after the registers:

```text
*** CPU EXCEPTION 14: page fault
    ...
    in thread "boot" (1)
    backtrace:
    #0  0xffffffff80121ac4  kernel64::run_fault_test+0x34
    #1  0xffffffff8012180e  kernel64::kernel_main+0x7ce
    #2  0xffffffff8010000e  __kernel_start+0xe
```

and a panic:

```text
*** KERNEL PANIC: panicked at src/selftest.rs:237:5:
rule 3 of threads.rs: a wake-up took a waiter out of its queue
    in thread "boot" (1)
    backtrace:
    #0  0xffffffff8013d70c  core::panicking::panic_fmt+0x2c
    #1  0xffffffff80132463  kernel64::selftest::threads+0x603
    #2  0xffffffff80120e1e  kernel64::kernel_main+0x79e
    #3  0xffffffff8010000e  __kernel_start+0xe
```

On AArch64 the same, with the link register too (`lr`), and a jump into data says so: `(past the code)+0x10a578`.

## How it works

Three pieces.

**Frame pointers.** `-C force-frame-pointers=yes` in the kernel's `.cargo/config.toml`, for both CPUs. Every function then begins by saving its caller's frame pointer and pointing its own at that record:

```text
 higher addresses    [ caller's frame ... ]
                     [ return address     ]  <- frame + 8
 rbp / x29  ------>  [ caller's rbp / x29 ]  <- frame
                     [ this function's locals ... ]
```

Following the first word from record to record climbs from callee to caller; the second word of each is where that caller resumes. The kernel's code did not grow past its page for it: the end of the code is at `0x80144000` with frame pointers and without.

**The walk** (`kernel64::symbols::walk`, tested on the host). A panic must never fault while it reports - a fault inside a panic is a triple fault, and the report is lost - so the walk reads a record only where `mm::may_read_stack` says it may: on the same thread's 64 KiB stack slot as the frame it came from, above that slot's guard page (a stack in use is mapped whole), or in the kernel image's data (the boot stack), not in the boot stack's guard page. And records only climb: a link that points down, or to itself, ends the walk, so it cannot loop. After a stack overflow, x86_64 does not walk at all: the stack that ran out is no place to.

**The kernel's own symbols** (`kernel64::symbols::Table`, `backtrace.rs`, `scripts/fill-symbols.py`). The kernel reserves a 64 KiB block in its read-only data, `TAAJ_SYMBOLS`, starting with the bytes `TAAJSYM1`. After `cargo build`, `fill-symbols.py`:

1. lists the kernel's functions with `nm -C` (which demangles Rust's names), skipping AArch64's mapping symbols (`$x`, `$d`);
2. drops generic arguments - `quicksort`, not `quicksort::<u64, <u64 as PartialOrd>::lt>` - which made the names half as long again (51 KB against 35) and the backtrace no clearer;
3. finds the block by its symbol, works out where it lies in the file from the ELF program headers, checks the magic, and writes the table there: a count, the sorted addresses with where each name starts, and the names.

The block's size never changes, so no address moves and nothing is linked again. Every place a kernel is built runs it: the `Makefile` (the PC kernel, the big one, the AArch64 one), `fault-test.sh` and `aarch64-test.sh`. A kernel built without it says `backtrace (no symbols in this kernel):` and prints addresses.

One trap: the compiler knows what `TAAJ_SYMBOLS` was compiled as - zeros after the magic - and could fold every read of it to "empty". The pointer goes through `core::hint::black_box`, which hides where it came from, so the bytes are read from memory: the ones the script wrote.

A return address is just past the call; the name is looked up one byte back, inside the calling function - when a call is the last instruction of a function, the address after it is already the next function's.

## Design decisions

| Decision | Instead of | Why (and the goals) |
|---|---|---|
| A table filled in after linking | Linking twice with a generated table | One link; the table's size is fixed, so filling it moves nothing (Maintainable) |
| Names in the kernel | Addresses, looked up on the host | Real hardware has only a screen; the build that made the kernel may be gone |
| Generic arguments dropped | Full names | 642 functions in a 44 KiB table, its names 35 KB instead of 51; the function is what tells you where |
| Frame pointers | Unwind tables (DWARF CFI) | A few instructions per function against an unwinder in the kernel; and an unwinder reading tables during a panic is one more thing that can fault |
| A read allowed only on the walk's own stack or the image | Reading wherever a link points | A panic must not fault (Reliable) |
| `lr` printed on AArch64 | Only the frame records | A function that calls nothing never saves its return in a record; the nx test branches into data from `kernel_main`, and only x30 says so |

What it costs: the kernel image grew by 52 KiB - the 64 KiB block, in read-only data that had some room in its last page - leaving 508 KiB under the BIOS loader's 3 MiB (J51, K130).

## Bugs

- **The x86 nx test's frame was one further down than I expected**: `run_fault_test` was not inlined into `kernel_main` in that build, so `kernel_main` was frame 2, not 1. The fault tests now look for it in the first four frames.
- **AArch64's nx test had no `kernel_main` at all** in the first version - only `(past the code)` and the entry point - until the link register was printed.
- **The big-image test crossed the 3 MiB wall.** `big64-test` builds a kernel with 512 KiB of ballast, to check that one above 2 MiB loads whole; with the table, that kernel ended above 3 MiB and did not link. The kernel itself now ends above 2 MiB (`0x281000`), so the ballast is 256 KiB: the big image still tests what it was for, and fails first - while the kernel still has 256 KiB left under the wall, close to where J51 says to hurry.
- **A section boundary named a fault in data**: `__rodata_start+0x126260`. It is the first symbol after the code, which the linker marks as code; the table now calls that address `(past the code)`.

## How I checked it

- Host tests of `symbols` (4): a lookup between functions, at their starts and below the first; an unfilled or broken table; a walk that climbs, stops where it may not read, at a loop, at 32 frames, and at a misaligned or forbidden first record. All 229 of the kernel's host tests pass.
- Every fault kernel, both CPUs: the backtrace names `kernel_main` (x86_64: page, page@20, double, rodata, nx; AArch64: null, page, rodata, nx); the stack overflows walk nothing and stop cleanly. `fault-test.sh` and `aarch64-test.sh` now require it.
- With the fill step taken out, `fault-page` printed `backtrace (no symbols in this kernel):` with bare addresses, and its check failed.
- A deliberate panic (rule 3 of `threads.rs` broken, as in entry 119), on 2 CPUs: the backtrace above.
- `make unsafe-check` (the new code has unsafe blocks, each with its reason), and `make test64`.

## What is still missing

- **The crash record**: the panic's report kept where the next boot finds it (memory that survives a warm reboot, or a reserved spot on the disk) and handed to the log service. It needs K136, the log service; it is K137's second half, now an item of its own.
- Frames in `core` and `alloc`: they are precompiled without frame pointers, so a call through them is skipped - the walk goes from our function to our function. In the panic above, `panic_fmt` is there because it was the panic's own caller.
- Programs' backtraces: a program that crashes gets its registers, not its calls (K138).

## What I learned

- **Fill the table in after the link, at a fixed size.** It looked like a chicken-and-egg problem - the table's contents depend on where everything is, and where everything is depends on the table's size - until the size was fixed.
- **A panic is the worst place to fault.** Every read in the report is decided before it is made.
- **The first frame on AArch64 lives in a register.** Frame records describe the callers; the function that stopped may not have made one.

## Testing note

`make run64` boots as before: nothing is printed until something goes wrong. To see a backtrace:

| Do | You should see |
|---|---|
| `make fault64-test` | `fault-page: ok`, and in `build/fault64-page.log`, after the registers: `backtrace:`, then `#0 ... kernel64::run_fault_test+0x...` and `#1 ... kernel64::kernel_main+0x...` |
| In `kernel/kernel64/src/threads.rs`, `WaitQueue::wake_all`, take the waiters out (`core::mem::take(&mut *self.waiters.lock())`), then `make smp64-test` | `KERNEL PANIC ... rule 3 of threads.rs ...`, then `in thread "boot" (1)` and a backtrace through `kernel64::selftest::threads` and `kernel64::kernel_main`. Change it back |
| `python3 kernel/kernel64/scripts/fill-symbols.py kernel/kernel64/target/x86_64-unknown-none/release/kernel64` | `SYMBOLS 642 functions` (about that many) |

Automated: `make test64` (`fault64-test` and `aarch64-test` require the backtraces).
