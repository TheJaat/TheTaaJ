# Era 13 · A fast core: IPC, threads and several CPUs

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


The core is correct and secure by construction; this era makes it fast and lets it use a real PC's CPUs. **Done when**: the IPC and threads benchmarks run in `make test64`; a request's round trip and a context switch are at least twice as fast as the first measurement; and on KVM with 4 CPUs a parallel job (several compilers at once) runs at least 3 times as fast as on one. Whose it is, and when, is for the user to say.

### IPC: speed and the cycle leak

The design - channels, handles with rights, bounded inboxes, handles carried in messages, memory objects for bulk data, `PeerClosed` when a service dies - is sound, and secure by construction. What it costs is not yet looked at: a request and its reply are four system calls and two context switches, and each message is copied twice through a kernel allocation. Every file and network operation is such a request. Measure first, then change what the numbers show.

| # | Item | Size | Notes |
|---|---|---|---|
| K60 | **An IPC benchmark**: round-trip time of a small request and reply, and bulk throughput through messages and through a shared memory object; on both CPUs, under KVM and emulated; the numbers kept in the journal | S | **Done (entry NNN)**, by `ci` for `ipc` with the user's OK: `ipcbench` (round trips of 16 bytes to another program and to another thread; 8 MiB through 1 KiB messages; 16 MiB through a shared memory object, every byte checked), `ipcbench64-test` on 1 and 2 CPUs, on a PC (KVM) and AArch64 (emulated); the first numbers are in the entry. One CPU: 2.8 us a round trip between programs; two: 21 us - waking a thread on the other CPU (`sched`'s K192) |
| K61 | **`channel_call`**: send a request and wait for its reply in one system call, switching straight to the server's waiting thread and back (as L4, seL4 and Zircon do) | M | After K60 |
| K62 | One copy per message: straight from the sender into a receiver that is already waiting, and no kernel allocation for each message | M | After K60 |
| K63 | Bigger messages (today 1 KiB; 4 to 64 KiB) or shared rings for streams - fs and net cut streams into 1023-byte pieces | M | After K60; changes the `abi` with the services and libc in step |
| K64 | Waiting until a channel can be sent to (`object_wait` for "writable"), instead of each sender retrying on `QueueFull` | S | |
| K65 | Priority inheritance across a call: a server works at the priority of the client it answers | M | After K20 and K61; done by Era 17's K98 if that comes first |
| K?-port | **A port**: one wait queue for many channels' readiness, so a service with hundreds of open files (J60) or sockets waits at a cost that does not grow with them; then `WAIT_HANDLES_MAX` (256 since entry 115) stops being a limit on open files | M | Generic: it knows channels, never files |
| K66 | **Channel cycles leak**: end A in B's inbox and B in A's keep both alive once their owners close every handle (`ipc.rs` says so; only the one-step cycle is refused) - a cycle collector, or a rule that stops cycles forming | M | A process can make kernel memory it can't give back |

### Threads and several CPUs

TheTaaJ runs on up to 16 CPUs (`smp.rs`), on both architectures, and a program's threads run on all of them at once. What is right: the scheduler's decisions are separate from the switching and tested on the host (`sched.rs`); `threads.rs` writes down the four rules several CPUs need and keeps them; futexes under libc's mutexes, condition variables, read-write locks and `pthread_once`, with an uncontended lock never entering the kernel; TLB shootdowns on x86 (`tlb.rs`) and broadcast invalidation on AArch64; thread-local storage the ELF way (entry 099); twenty rounds of `make test64` under KVM without a race (entries 104, 105). What does not scale: one lock (`THREADS`) around one run queue that every CPU takes from; one lock each around the kernel heap and the frame allocator; one list of futex waiters; on x86 a shootdown reloads the whole address space; and every service is one thread. Fine on 4 CPUs in QEMU; on a 16-thread real PC the CPUs will queue for the same locks.

| # | Item | Size | Notes |
|---|---|---|---|
| K67 | **A threads benchmark**: a context switch, a futex hand-off between two threads, thread creation, `posix_spawn` to exit, and the same work on 1, 2, 4 ... CPUs to see how it scales; KVM and emulated, the numbers kept in the journal | S | **Done (entry 116)**: `schedbench` (`make schedbench64-test`, in `make test64`) on 1, 2 and 4 CPUs, KVM and emulated; still missing: AArch64, and more than 4 CPUs |
| K68 | **A run queue per CPU**, with an idle CPU taking work from a busy one, instead of one queue under one lock | M | **Done (entry 129)**, part 1: a queue per CPU in the policy; placing by the idle deciding CPU, the last CPU, the home with the fewest waiting; choosing strict and FIFO across queues (an idle CPU takes work from a busy one). The lock per queue is K193 |
| K69 | **Per-CPU caches** in the kernel heap and the frame allocator, so most allocations take no shared lock | M | After K67 |
| K70 | **Futexes without one shared list**: a lock per bucket, a wake that touches only its own address's waiters, `FUTEX_REQUEUE` (so `pthread_cond_broadcast` doesn't wake everyone to fight for the mutex) | M | Today a wake wakes its whole queue and those whose flag is clear sleep again |
| K71 | **Cheaper shootdowns**: PCID on x86 (ASIDs on AArch64) so a switch keeps the TLB, and only the pages taken away invalidated rather than the whole address space | M | |
| K72 | **Idle CPUs that sleep** until they are needed, not woken by every 10 ms tick | S | With K24 (the one-shot timer) |
| K73 | **Services with several threads**: the file and network services serve more than one request at a time | M | With J78; after K61 |
| K74 | **More than 16 CPUs** (`MAX_CPUS`) | S | With K7 (x2APIC) |
| J59 | The POSIX thread calls still missing: semaphores (`sem_*`, `semaphore.h`), barriers, spin locks, `pthread_kill`, `pthread_cancel`, `sched_setaffinity` (after K20), robust mutexes | M | Ports ask for them; Rust's std needs none of them |

Processes: TheTaaJ starts programs with `posix_spawn`, not `fork` (on purpose: a microkernel copies nothing it isn't asked to), and `fork` says so with `ENOSYS`. What is missing for processes is on the list already: process groups and job control (K44, K45), CPU time per process (K21), priorities (K20).
