# The IPC benchmark: what a message costs, and where the time goes on two CPUs

*Journal entry #NNN · K60, the `ipc` lane's first item, done by TheJat-WSL/ci with the user's OK (`ipc`'s session had nothing in progress). Written by day, with quick checks only. The item: "round-trip time of a small request and reply, and bulk throughput through messages and through a shared memory object; on both CPUs, under KVM and emulated; the numbers kept in the journal". K61 (`channel_call`) and K62 (one copy per message) are to be measured against it.*

## The numbers

`ipcbench64-test`, on TheJat-WSL (24 CPUs, KVM), with the machines given 1 and 2 CPUs:

| Machine | CPUs | Round trip, two programs (median, 99%) | Round trip, two threads | Messages, 1 KiB | Shared memory |
|---|---|---|---|---|---|
| PC (KVM) | 1 | 2756 ns, 2936 ns | 2382 ns, 2574 ns | 1057 MB/s | 14768 MB/s |
| PC (KVM) | 2 | 21439 ns, 35045 ns | 20995 ns, 30560 ns | 591 MB/s | 12422 MB/s |
| AArch64 (emulated) | 1 | 53744 ns, 135920 ns | 17984 ns, 35648 ns | 115 MB/s | 950 MB/s |
| AArch64 (emulated) | 2 | 57408 ns, 139552 ns | 39120 ns, 96736 ns | 173 MB/s | 1139 MB/s |

A round trip is a 16-byte request and the same 16 bytes back: two system calls each way, and two switches. "Two programs" adds a change of address space each way; "two threads" does not.

The numbers vary from run to run. An earlier run by hand on 2 CPUs gave 45810 ns for the programs' round trip, and the host's load moves them. They are a baseline, not a constant.

## What they say

- **On one CPU, a round trip between programs is 2.8 us**, and between threads 2.4 us: the address space's change costs about 0.4 us. L4-family kernels do it in well under a microsecond. K61's `channel_call`, which goes straight from client to server and back in one system call, is where most of the rest goes.
- **On two CPUs it is eight times slower: 21 us, programs and threads alike.** The address space is no longer what matters. The server waits on the other CPU, so each message must wake a thread there. The time most likely goes in that wake-up and in that CPU coming out of its halt - not measured yet: `trace` and `profile` can say. That is `sched`'s K192 ("cross-CPU wake-ups under KVM", entry 128's bug 5), now measured. Messages lose half their throughput for the same reason.
- **Shared memory moves 12 to 15 GB/s** against 0.6 to 1 GB/s through messages. That is the case for K63 (bigger messages, or rings) and `network`'s K54 (frames through shared rings).
- **Emulated (AArch64), everything is 10 to 20 times slower**, and the cross-CPU wake-up matters less: everything else is slow too.

## How

- **`ipcbench`** (`user/src/bin/ipcbench.rs`), a program on the disk, run as `with programs ipcbench all`.
  - **The other program** is `ipcbench serve`, a copy of itself started through init. Its input and output are two channels: requests come in on one, and replies go out on the other.
  - **The other thread** runs the same `serve`, with two channels of its own.
  - **Round trips:** 5000, each timed; the median and the 99th percentile.
  - **Messages:** 8 MiB in messages of 1 KiB, one way, waiting while the queue is full. The other end counts the bytes and says how many arrived.
  - **Shared memory:** a memory object of 1 MiB, lent to the other end (mapped there to read). Sixteen times: written here, then one short message there and its answer back. The other end sums what it reads, and the sum must be what was written.
- **`taaj::start`** (`user/src/start.rs`, new): starting a program from the disk, with given channels as its output and input. It moved out of K142's `debug`, which now uses it too.
- **`ipcbench64-test`:** four boots at once, a PC and AArch64, each on 1 and 2 CPUs. It checks that every measurement ran and nothing failed, never how fast: a busy host or an emulated CPU is slower, and that is not a failure. The table above is the test's `build/ipcbench64.log.summary`, printed at the end.

## In other lanes' files

`user/src/lib.rs` gets `pub mod start` (`user/src/` is everyone's). `docs/lanes.md` has K60 out of `ipc`'s table, with the user's OK.

## Checked

- **`ipcbench64-test`, new:** the table above, in 24 seconds.
- **Quick checks:** `debug64-test` (it starts its program through `taaj::start` now), `boot64-test`, `unsafe-check`.
- The full `make test64` is for the night.

## For later

- **K61 and K62** (`ipc`): run `ipcbench64-test` before and after, and put both tables in their entries.
- **K192** (`sched`): the two-CPU round trip is the number to bring down.
- **With `trace` and `profile`** (K139, K140): where a round trip's microseconds go, system call by system call.

## Testing note

| Do | You should see |
|---|---|
| `with programs ipcbench all`, with `ipcbench` put on the disk (`tfstool put DISK ipcbench user/target/x86_64-unknown-none/release/ipcbench`) | Four measurements and `done in ... ms` |
| `make ipcbench64-test` | The table, for 1 and 2 CPUs, a PC and AArch64 |

Automated: `make ipcbench64-test` (part of `make test64`).
