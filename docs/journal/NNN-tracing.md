# Tracing: what the kernel does, on every CPU, as it does it

*Journal entry #NNN · K139 (Era 22). Written on TheJat-WSL/ci, by day, with quick checks only. The item: "trace points in the kernel (system calls, IPC sends and receives, switches, faults, interrupts) written to a per-CPU ring that a tracing service maps, with a handle for it; a `trace` command that records a span and shows it". It is the data the benchmarks of Eras 13 and 17 want, and what K140's profiler will stand on.*

## What it shows

```
trace: 1.99 s on 2 CPUs: 3994 events (0 lost)
  by kind:  syscall 1973  send 165  receive 164  switch 620  fault 35  interrupt 1037
  by CPU:  cpu0 2287  cpu1 1707
  by process:
        0 kernel       949
       43 e1000        815
       49 ?            786
       37 display      373
       38 fs           282
       42 blk          248
  the last 12 events (ms from the start, CPU, process, what, value):
      1995.089  cpu0      0 kernel       switch    0x23
      1995.094  cpu1      0 kernel       interrupt 0xf0
      1995.100  cpu0     35 init         interrupt 0x20
      1995.105  cpu1      0 kernel       switch    0x2b
      ...
```

That is two seconds of `trace -s 2` while `ls` ran (`trace64-test`): a program started, files read through `fs`, its pages filled. A `?` is a process that ended before `trace` asked for the names: `ls`. A switch's value is the process switched to (`0x23`, 35, is init).

## How

- **The rings** (`kernel/abi/src/trace.rs`, shared by the kernel and `trace`, host-tested). There is one per CPU, each 4096 events of 32 bytes: a sequence number, the time, the kind, the CPU, the process and a value. A system call's value is its number; a send's or receive's, the message's length; a switch's, the process of the thread switched to (the event's own process is the one leaving); a fault's, the address; an interrupt's, its vector (or GIC id).
  - **No locks.** A trace point may be in an interrupt or in the middle of a switch. Each CPU writes only its own ring. On one CPU, an interrupt in the middle of an event takes the next number from the CPU's own atomic counter, so the two take different slots. A slot's sequence is written last, so the reader takes an event only when the sequence it expected is there before and after copying. If it finds a later sequence, the ring came round, and those events count as lost.
  - The header has one word that its holder writes: on or off.
- **The handle is the memory.** The kernel makes the rings once the CPUs are running (`tracing.rs`, new) and offers them to init as `trace`, to read and to write. Holding them is what tracing takes, so no new system call was needed. With tracing off, a trace point is one load and a branch. The kernel keeps its own reference, as with the log's ring.
- **Six trace points**, a line each: `syscall::dispatch`, channel `send` and `receive` (after the channel's locks are let go), the switch in `threads`, `process::fault_in` (programs' page faults), and the interrupt dispatch on each architecture.
- **init** takes the `trace` offer and registers the name `trace`, which only the shell's namespace has. A name whose handle is missing (no memory for tracing) is not registered, so the registry still starts. **The shell** gives `trace` the name, and `system` to name the processes (`TRACE_PROGRAMS`).
- **`trace`** (on the disk) switches tracing on, reads every tenth of a second, and switches it off after `-s SECONDS` (1 to 60). It counts by kind, CPU and process as it goes, and keeps only the last `-e` events to show. If another `trace` is recording, it says so and leaves.

## In other lanes' files

A line each:
- `syscall.rs`, `ipc.rs` (`ipc`), `threads.rs` (`sched`): trace points;
- `process.rs`: the fault point;
- `arch/x86_64/platform.rs`, `arch/aarch64/mod.rs`: interrupt points;
- `main.rs`: the offer;
- `init.rs` (`services`): the offer, grant 58, the name, and skipping names without a handle;
- `shell.rs`: `TRACE_PROGRAMS`.

The boot test's expected list of the shell's names has `trace` at the end. The disk's root has one file more: `ROOT_FILES` is 43, or 31 without the C compiler.

## Found on the way

- **Read at the end, a span lost events.** The first `trace` read its rings once, after the span: 730 events of about 4000 were lost, because a ring of 2048 held about one second of a busy boot. Now `Trace::read` moves each CPU's starting place on, so `trace` reads every tenth of a second, and a span can be as long as it likes. The rings are 4096 events, for margin.

- **A switch's value was an address.** The switch first gave the thread switched to, but in `threads` a thread is known by its record's address, and the events showed `0xffffc000000080f0`. Now the point comes before the CPU's current thread changes: the event's process is the one leaving, and its value is the process arriving.

## Checked

- **`trace64-test`, new:** a PC with two CPUs runs `trace -s 2 -e 12 &`, then `ls`.
  - Every kind of event is there, on both CPUs, with none lost.
  - The kernel's own events are counted.
  - The last 12 events each have their time, CPU, process, kind and value.
  - `trace -x` says how to use it.
- **Host tests:** `abi::trace`, four: events back as written, CPU by CPU, and read again from where the last read stopped; a ring that came round says how much was lost; two events numbered out of order on one CPU (an interrupt in the middle) both arrive; what is not a trace is not opened.
- **Quick checks:**
  - `kernel64-test`;
  - `coverage`: `kernel/abi` went from 89.5% to 90.5%;
  - `unsafe-check`;
  - `boot64-test` and `boot64-uefi-test`, with the names and the root's count;
  - `aarch64-test`, `top64-test`, `log64-test`;
  - `schedbench64-test` and `times64-test`, the timing tests: the trace points sit on the hottest paths, and while off they cost one load each.
- The full `make test64` is for the night.

## For later

- **K140, the profiler:** the timer samples where each CPU is. A seventh kind, the interrupted address, makes it.
- **Names:** system call names for the numbers, from `abi::call`.
- **More points:** futex waits, memory object maps, process starts and ends.

## Testing note

| Do | You should see |
|---|---|
| `trace` | One second: every kind counted, by CPU and by process, and the last 20 events |
| `trace -s 2 &`, then `ls` | `ls` among the processes; sends and receives with `fs` |
| Two `trace` at once | The second says tracing is on already |

Automated: `make trace64-test` (part of `make test64`).
