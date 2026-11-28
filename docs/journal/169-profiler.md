# A profiler: where the CPUs spend their time

*Journal entry #169 · K140 (Era 22). Written on TheJat-WSL/ci, by day, with quick checks only; it stands on K139, tracing. The item: "the timer samples where each CPU is (program and address); a report per function, with symbols".*

## What it shows

`forever spin &`, then `profile -s 2` on a PC with two CPUs (`profile64-test`):

```
profile: 1.99 s on 2 CPUs: 846 samples (0 lost)
  by program:
     50.1%    2010 ms      0 kernel
     49.8%    1995 ms     48 forever
  the 5 busiest places:
     49.1%    1971 ms  kernel  kernel64::threads::idle_loop
     48.4%    1939 ms  forever (48)  at 0x40043b
      1.3%      55 ms  forever (48)  at 0x400439
      0.6%      26 ms  kernel  kernel64::threads::wait_any_until_ns
      0.3%      12 ms  kernel  kernel64::threads::sleep_until_ns
```

One CPU spun in `forever`'s loop at `0x40043b` and `0x400439`; the other sat in the kernel's idle loop.

## How

- **The sample** is tracing's seventh kind of event, `sample`, with the address the CPU was at.
  - On a PC, it is written at every timer interrupt, from the interrupt's frame (`idt.rs`, before `handle_interrupt`).
  - On AArch64, the frame is at the exception's entry, which does not know which interrupt came, so it is written at every interrupt (`vectors.rs`), mostly the timer's.
  - It goes into the same rings, with the same handle: `trace` shows samples too.
- **The kernel's symbols.** The table the build fills in after linking (entry 122) was the kernel's alone. Its format, parsing and lookup moved to `abi::symbols`, which the kernel re-exports, so the backtraces did not change. A copy of the table, 64 KiB, comes with tracing's rings in the kernel's `trace` offer. init registers it as `trace.symbols`, and the shell gives it to `trace` and `profile`.
- **`profile`** (on the disk) records like `trace`, reading every tenth of a second, and keeps only the samples.
  - The kernel's addresses (the top half) are named by function.
  - Programs' addresses are grouped by program and address. Programs are built stripped (`strip = true` in `user/Cargo.toml`), so there are no names for them.

## Found on the way: counting samples overstates idle

The first `profile` counted samples: `forever` had 24% of them, though it kept one CPU of two busy. There were 838 samples in 2 s on 2 CPUs, twice the 100 Hz tick. A CPU that sleeps is woken by one-shot timers for the sleeps it waits on as well as by the tick, so idle is sampled more often than busy code. So each sample now stands for the time since its CPU's last sample: 10 ms for the first, at most 20 ms. `profile` reports time, in milliseconds and percent. `forever` then had 49.9%: one CPU's two seconds.

## In other lanes' files

A line or a few each:
- `arch/x86_64/idt.rs`, `arch/aarch64/vectors.rs`: the samples;
- `tracing.rs` (K139's): the symbols with the offer;
- `backtrace.rs`: `block()`;
- `symbols.rs`: the table moved to `abi`;
- `init.rs` (`services`): the second handle of the offer, grant 59, the name `trace.symbols`;
- `shell.rs`: `profile` in `TRACE_PROGRAMS`, and the symbols' name.

The boot test's expected list of the shell's names has `trace.symbols`. `ROOT_FILES` is 44, or 32 without the C compiler.

## Checked

- **`profile64-test`, new:**
  - two seconds on both CPUs, with the kernel's symbols, none lost;
  - `forever` about half the time (40% to 60%), by program and at its place;
  - the kernel's idle loop named;
  - `profile -x` says how to use it.
- **`trace64-test`** now also expects `sample` events.
- **Host tests:** `abi::symbols`, the two table tests that moved with it; the stack walk's tests stay in `symbols.rs`.
- **Quick checks:**
  - `kernel64-test`, `coverage` (`kernel/abi` at 90.6%) and `unsafe-check`;
  - `boot64-test`, `boot64-uefi-test`, `aarch64-test`;
  - `crash64-test` and `fault64-page-test`: the backtraces still have the kernel's names, from the table's new crate;
  - `top64-test`, `log64-test`, and the timing tests `schedbench64-test` and `times64-test`.
- The full `make test64` is for the night.

## For later

- **Programs' names:** build them with symbols (`strip = false`) where the disk has room, or keep a symbol file beside each program, and let `profile` read it.
- **A faster sample** than the 100 Hz tick, for short spans.
- **Call stacks**, once programs have frame pointers (K138's note).

## Testing note

| Do | You should see |
|---|---|
| `forever spin &`, then `profile` | `forever` about half the time on two CPUs, at one or two addresses; `kernel64::threads::idle_loop` the other half |
| `profile -s 10 -n 30` | Ten seconds, the 30 busiest places |

Automated: `make profile64-test` (part of `make test64`).
