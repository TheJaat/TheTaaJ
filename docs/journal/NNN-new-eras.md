# Eras 13 and 14, and the network and threads items

*Journal entry #NNN · a work-list change, not a feature. Written on TheJat-VM, on the branch `new-eras`, which has `ipc-items` and `fs-items` merged into it. The user asked how TheTaaJ's network, threads and several-CPU support are, and for all the items found - IPC, TaajFS, network, threads - to be put into eras.*

## Why

Three reviews in a row (IPC, TaajFS, then the network and threads here) found the same thing: what is there is correct and carefully tested, and what it costs has never been measured. The items they produced did not fit any one machine's list, and the user asked for them as eras, so they have a milestone and a place in the order of the project.

## What the network review found

The `net` crate (about 5,000 lines: Ethernet, ARP, IPv4, ICMP, UDP, TCP, HTTP) is handed frames and the time and hands back frames, so all of it is tested on the host (78 tests, and 10 for the e1000 driver), and five boot tests check it in QEMU. Its TCP has RFC 9293's states, timeouts from the measured round trip, congestion control starting at 10 segments, and keeps what arrives ahead of a gap. Flow control goes all the way through: a program that does not read shrinks the window the other machine is offered.

What it lacks:

- **Reach**: one fixed address (QEMU's 10.0.2.15), no DHCP or DNS, TCP only from C, no loopback, IPv6 or TLS. All already items (K30-K38, K49).
- **Scale**: 16 connections, 10 sockets, 4 listeners, a backlog of 4, fixed because the service waits on all of them in one `object_wait`.
- **Speed**: a frame from the card to a program is copied about six times - into the driver, in two 1 KiB messages to the service (two copies each), in 1023-byte messages to the program (two copies again). Never measured.
- A restart of the service loses every socket; any program may listen on any port; one network and one router.

## What the threads review found

Up to 16 CPUs on both architectures; a program's threads run on all of them. The scheduler's decisions are tested on the host, `threads.rs` documents the four rules several CPUs need, futexes carry libc's locks with no system call when nobody else wants the lock, x86 has TLB shootdowns, ELF thread-local storage works, and twenty rounds of `make test64` under KVM passed (entries 104, 105).

What does not scale: one lock, `THREADS`, around one run queue that every CPU takes from; one lock each around the kernel heap (`mm.rs`) and the frame allocator (`FRAMES`); one list of futex waiters (`WAITERS`), and a wake that wakes a whole queue; on x86 a shootdown reloads all of CR3; and every service is a single thread. Also: round-robin with 20 ms slices and no priorities (K20, in progress on MaK-WSL), every idle CPU woken by each 10 ms tick, no semaphores, barriers or `pthread_cancel`. `fork` is refused with `ENOSYS` on purpose: TheTaaJ starts programs with `posix_spawn`.

## What changed in `docs/who-does-what.md`

- **Era 12 · Web** (its ground was already the "Network" section): an introduction to the section, and seven `K?-net-...` items - a benchmark, shared rings for frames, more sockets, a port filter, a routing table, offload, and sockets surviving a restart.
- **Era 13 · A fast core: IPC, threads and several CPUs**, new: the seven `K?-ipc-...` items (moved from MaK-WSL's list), and nine new ones for threads - `K?-smp-bench`, `-queues`, `-alloc`, `-futex`, `-tlb`, `-idle`, `-services`, `-max`, and `J?-threads-posix`.
- **Era 14 · TaajFS version 7**, new: the ten `J?-fs-...` items (moved from The-Jat's list).
- **Later eras**: 13 and 14 added; the assistant, drafted as Era 9 before Era 9 became rustc, takes the next free number, 15.

## Design decisions

- **Eras need a milestone you can see.** Era 13's: the IPC and threads benchmarks in `make test64`, a round trip and a context switch at least twice as fast as first measured, and a parallel job at least 3 times as fast on 4 CPUs as on 1. Era 14's: a 16 GiB volume with a rustc build tree mounting in under a second, a flipped bit reported, version 6 upgraded in place, the power-loss tests passing, `fsck` checking it. Numbers can be argued with, which is the point.
- **IPC and threads in one era.** They are the same work from two sides: a request's cost is two context switches and the scheduler's lock as much as the copies, and `channel_call`'s direct switch is a scheduler change.
- **The network items in Era 12, not a new era.** Era 12 was already the network's era; the new items are its scale and speed.
- **Every era starts with a benchmark.** Each section's first item measures; the rest are judged by it.
- **No owner given.** The eras are not put in either machine's decided order: that is for the user.
- **The two earlier branches are merged in, not rewritten**, so their entries stay as written: the IPC and TaajFS entries record those reviews; this one records the eras.

## How I checked it

- Each claim against the code: `MAX_CPUS = 16` (`smp.rs`), the one queue and `THREADS` (`sched.rs`, `threads.rs`), `SLICE = 2` ticks, the kernel heap's one `IrqLock` (`mm.rs`), `FRAMES`, the futex `WAITERS` list and its 64 queues (`futex.rs`), the CR3 reload (`tlb.rs`), `fork`'s `ENOSYS` (`libc/src/system.rs`), only `sched_yield` in `sched.h` and no `semaphore.h`; the network's limits (`TCP_CONNECTIONS` 16, `TCP_LISTENERS` 8, `TCP_BACKLOG` 4 in `net/src/stack.rs`; `SOCKETS` 10, `LISTENERS` 4 and the `WAIT_HANDLES_MAX` assertion in `user/src/bin/net.rs`), the fixed address there, what `tcp.rs` says it does not do, and the frame split into pieces (`user/src/nic.rs`).
- Nothing was built or run: this changes only documents.

## What is still missing

- Which machine takes Eras 13 and 14, and when; the numbers given to the new items at merge.

## What I learned

- **A microkernel's speed is its IPC and its scheduler together.** Every file read and every packet is a request to a service, so the cost of a message and the cost of a switch are paid on everything, and they are worth an era of their own.
- **Fixed tables hide in waits.** The network's socket limit isn't a setting: it is how many handles one wait can take.

## Testing note

Nothing to boot: this changes only `docs/`. To see the eras and items:

```
grep -n '^## Era\|K?-\|J?-' docs/who-does-what.md
```

| Look for | You should see |
|---|---|
| `## Era 13 · A fast core` | the IPC section (7 items) and the threads section (9 items) |
| `## Era 14 · TaajFS version 7` | 10 `J?-fs` items |
| `K?-net-bench` ... `K?-net-keep` | 7 rows in "Network (Era 12's ground)", after K49 |
| `## Later eras` | Eras 13 and 14, and the assistant as 15 |
