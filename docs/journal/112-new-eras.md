# Goals, and Eras 13 to 29: every part of TheTaaJ reviewed and given an era

*Journal entry #112 · a work-list change, not a feature. Written on TheJat-VM, on the branch `new-eras`, which has `ipc-items` and `fs-items` (entries 110 and 111) merged into it. The user asked how TheTaaJ's network, threads, several-CPU support, VFS, device drivers, memory management, the scheduler, security, the desktop, the shell and libc and the bootloader are, for all the items found - IPC, TaajFS, network, threads, VFS, drivers, memory, scheduling, security, desktop, command line, boot - to be put into eras, and then for every area not yet reviewed (observability, services, protocols, build and tests, packages, time and power, portability, configuration) to be given its era too, and for the project's goals to be written down: "always remember our goal as pure microkernel, secure, reliable, modular, scalable, portable, maintainable, extensible, performance, stable".*

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

## What the VFS review found

There is no VFS. The kernel knows no files - right for a microkernel - but nothing has taken a VFS's place:

- one `fs` service serves one volume as the whole tree: no mounts, no second disk or USB stick beside `/`, no `/tmp`, `/dev` or `/proc`;
- an open file lives only in libc (`libc/src/descriptors.rs`): a path, an offset and flags. The service is stateless and every request names the whole path from the root, so a descriptor follows the name, not the file (J36), and an open file can't be handed to another program;
- what a descriptor can be is a closed list (`Open`: the three standard ones, a file, a socket, a pipe's ends);
- access is all or nothing: the read-write channel or the read-only one;
- the gaps already show as workarounds: LLVM's `/proc/self/exe` is patched to call `__taaj_executable()`, and `isatty` is a special case;
- Rust programs (`taaj::Files`) and C programs (libc) each have their own client of the file protocol.

## A VFS with the kernel left out

The question the user asked next: would a VFS still leave TheTaaJ a pure microkernel? Yes, done as Fuchsia does it, not as Linux or even Plan 9 do:

- **An open file is a handle**, a channel to the service that owns it, and requests go straight there. No central VFS server, so no extra hop on every request (MINIX 3 and the Hurd have one) and no single process whose crash takes every file away.
- **The mount table lives in each process, in libc.** On Linux and Plan 9 the kernel keeps it because it is the security boundary. Here the handles are: a program can write anything into its own table and still reach only the directories whose handles it was given. Something that need not be trusted need not be in the kernel.
- **One protocol for every file service**, so `/tmp`, `/dev`, `/proc` and FAT are each another service.
- What the kernel may still gain is generic: `channel_call`, a wait on many handles, sealed memory objects, a pager filling a memory object from a channel. Nothing that knows what a file is.

The cost is a message per operation where a monolithic kernel makes a function call - which is why Era 13 (a fast core) comes before Era 14.

## What the drivers review found

Every driver is a user-space program, handed its registers, a contiguous DMA memory object with its physical address, and its interrupt as capabilities; the logic of most is in a crate tested on the host against a simulated device; init restarts a driver that crashes, and keeps its DMA memory so a restart never frees what the device may still write. Architecturally right. Against the Goals, two real flaws and some smaller ones:

- **The kernel decides which devices exist.** `offered()` in `devices.rs` has a list - AHCI, NVMe and xHCI by class, the e1000 by Intel's numbers - and knows things about each kind (which BAR, MSI-X or not, NVMe's smallest register size); `mmio_devices.rs` identifies virtio devices. Policy in the kernel, and every new driver a kernel change.
- **Nothing confines DMA.** No IOMMU, and bus mastering, turned on at hand-out, is never turned off. The CPU confines a driver; the bus does not.
- init has fixed slots for one disk, one card and one USB controller, with each driver's handles numbered by hand; one `usb` program is the controller and all its devices; one interrupt vector per device; data copied through one fixed DMA region; only virtio's legacy interface; no test that kills a driver mid-I/O.

## What the memory review found

One of the cleanest parts of the kernel: one page-table implementation for both CPUs, tested on the host; physical and virtual addresses as different types; a frame bitmap that starts "in use", so a wrong memory map wastes memory rather than inventing it; kernel code read-only and data never executable; memory objects shared without copying, a mapping counted as a reference; reserved memory filled on first touch; a guard page after every mapping; every page a process takes counted against a limit that only goes down; a documented lock order.

What it lacks is everything above "map this": nothing shared but explicit memory objects (programs loaded whole into each process, no copy-on-write); nothing reclaimed (no dropping of clean pages, no swap, no pressure signal); the kernel heap turns a failed allocation into a panic and never shrinks; SMEP, SMAP and PAN are off, and there is no KPTI; the frame allocator scans a bitmap under one lock; no huge pages for programs; memory objects at most 512 MiB.

## What the scheduler review found

Simple, correct and tested, with no policy yet. `sched.rs` decides and `threads.rs` switches, so every decision is tested on the host. Round-robin over one queue for every CPU, 20 ms slices; the kernel preemptible in system calls; idle CPUs poked as soon as work arrives; the rules for several CPUs kept.

What it lacks: priorities or classes (every thread equal; K20 is under way on MaK-WSL); fairness per process rather than per thread; a tick that walks every thread's state and allocates, under the global lock, to find due sleepers; 10 ms ticks; every CPU woken each tick; no regard for a CPU's cache; no priority inheritance; no CPU budgets; a 64-CPU cap in `pokes`; no `nice` or `sched_setscheduler`; no latency ever measured.

## What the security review found

TheTaaJ's strongest point. Capabilities from the bottom up: handles with rights that only shrink when passed on; no kill by process number, starting a program needs a launcher handle, the process list needs the system handle; the shell narrows the registry for every program it runs, so `ls` gets read-only files, `ping` only the network and an unknown program nothing; init's namespace table is the policy in one place; drivers hold only their own hardware; the kernel is Rust.

What surrounds that core is thin. Kernel objects - channels, queued messages, handles, processes - are not counted against anyone, and the kernel heap panics when it cannot grow: the one way a program with no grants can bring the machine down. SMEP, SMAP, PAN, KPTI, the IOMMU, ASLR, the stack protector and speculation mitigations are all missing. A program's services come from lists of names in the shell, nothing is signed, nothing verifies the boot. No users, no encryption, no fuzzing, no audit of `unsafe`, no audit log; `DEBUG_WRITE` is open to every program.

## What the desktop review found

The window server's design is right for a microkernel and close to Wayland's: a window is a channel and a memory object, the program draws in its own memory and says what changed, pixels never travel in messages, and a crash closes its windows by itself. The server draws every frame, so titles can't be faked, and keys go only to the focused window. The window manager, drawing, the editor and the protocol are tested on the host.

As a desktop it is at the start. `display` is the compositor, window manager, terminal and console in one process; keys reach programs as terminal bytes; the font is ASCII at one size; no resizing, panel, launcher or clipboard; two programs with windows (`counter` and `edit`); a toolkit of a frame, text and buttons; CPU composition at the boot resolution; any program with `windows` may cover the screen, and no key always reaches the system; no notifications, accessibility or image formats; key-to-screen time never measured. Of the vision's "a desktop people can use daily", the furthest from done.

## What the shell and libc review found

libc is well built: the system half in Rust on the `taaj` runtime, shared with Rust programs; printf, stdio, math, locale and fenv in C; musl's libm; Linux's ABI values; host tests for both halves; proven by tcc, LLVM, clang and libc++. Its gaps: no `termios`, no `regex`, `iconv`, `wctype`, `threads.h` and others; the "C" locale only; a linked-list `malloc`; static linking only; no conformance suite ever run; and its own overview (`libc/src/lib.rs`) out of date, still saying a process has one thread.

There are two shells. `sh` (`libc/src/sh.rs`) has nearly all of POSIX's language and 33 host tests, but no `&`; `shell`, at the keyboard, grants each program only its services and runs background jobs, but has no variables, `if` or `for`, and no history, Tab completion or arrow keys. Two parsers for two languages: a script that works in one doesn't in the other.

## What the bootloader review found

The bootloader is MaK-WSL's (rule 7): it was read, not changed. Its core is excellent: the TaajBoot protocol, one fixed-width, append-only structure every loader fills, its layout checked in C and in Rust, so the kernel cannot tell BIOS from UEFI and a new way to boot is a new loader, never a kernel change. Three loaders: BIOS (MBR, stage1, stage2), UEFI x86_64 (`ExitBootServices` done right), UEFI AArch64 (the firmware's reserved memory copied into the device tree).

The loaders are a working minimum. The BIOS loader takes a kernel only between 1 and 3 MiB (about 670 KiB left) and is slow emulated (K51). There is one kernel and no fallback when a new one fails; no menu or command line at boot; no verified boot; the kernel lives on ISO 9660 or the EFI partition, outside TaajFS, with no plan for updating it; it sits at a fixed physical place; the loaders have no tests of their own; and two stray files (`abc.txt`, `random.txt`) sit in the public repository.

## What the last reviews found

Asked whether everything was covered, I listed eight areas not yet reviewed; the user asked for all of them as eras. Each was checked against the code first:

- **Observability**: one debug log on the serial line, from a fixed ring that drops whole messages and counts them; lines kept whole; the screen taken back at a panic. But nothing keeps the log (on a real PC it is gone at the next boot), no backtrace at a panic, no crash record or crash reports, no tracing, profiler, `top` or debugger.
- **Services and recovery**: init is a real supervisor (wiring, least authority, restarts with the same channels, giving up after 5 in 30 s, a watchdog for `fs` and the disk driver). But nothing restarts init or the registry, a restarted service starts empty, and everything is in init's code - there is no configuration; `ramdisk-src/config.cfg` is a leftover nothing reads.
- **Protocols and the ABI**: one `abi` crate shared by everyone, explicit little-endian layouts, host tests. But every protocol is written by hand (and twice, for C), nothing has a version, and nothing checks that a change keeps the ABI. The vision asked for an IDL.
- **Build and tests**: `make test64` in full before every merge, the race hunt, our own toolchain. But no CI, a fresh clone never tried, two flaky parts, no coverage numbers, and TheTaaJ can't build its own kernel yet.
- **Packages and updates**: none. Everything on the disk is put there from the host.
- **Time and power**: a nanosecond clock, Unix time from the RTC, power-off and reset with `\_S5` read without an interpreter. But no time zones, no network time, no AML interpreter, so no power button, battery or suspend.
- **Portability**: one kernel for both CPUs behind `arch/`, about half the tests on both. But the PCI drivers and some tests are x86-only (AArch64's PCIe bus is unused), no real ARM board, no RISC-V. Then the user set the goal higher: "i want it to run on every architecture available in the world, fully portable". Checked again for that: 4 KiB pages fixed (`PAGE_SIZE` in `abi` and the kernel, 4096 in `user/`), four page-table levels fixed (`paging.rs`), libc's `setjmp` and `fenv` written for two CPUs and `#error` otherwise, interrupts and timers for the APIC and GIC only, DMA assumed coherent, the toolchain built for two targets.
- **Configuration**: none; it went into Era 23 with services, since settings are what services read.

## What the encryption review found

One piece of cryptography, and a good one: the kernel's random generator, ChaCha20 with fast key erasure, seeded from the CPU's random instruction, jitter and the clock. Nothing else: no hash, cipher, key exchange or signature, so no disk encryption, TLS, signed programs, SSH or key storage; no use of the CPU's crypto instructions; no constant-time checks.

Asked whether to write the algorithms or port audited ones, the user chose both: ours for learning, checked against the RFC vectors, Wycheproof and an audited library on every test run; the audited one guarding real secrets until ours has earned it.

## What changed in `docs/who-does-what.md`

- **Era 12 · Web** (its ground was already the "Network" section): an introduction to the section, and seven K53-K59 items - a benchmark, shared rings for frames, more sockets, a port filter, a routing table, offload, and sockets surviving a restart.
- **Era 13 · A fast core: IPC, threads and several CPUs**, new: the seven K60-K66 items (moved from MaK-WSL's list), and nine new ones for threads - K67, K68, K69, K70, K71, K72, K73, K74, and `J59`.
- **Goals**, a new section at the top: a pure microkernel, secure, reliable, modular, scalable, portable, maintainable, extensible, performant, stable - each with what it means here. Every design, review and item is judged against them.
- **Before merging** gains a check: the kernel stays pure - nothing new in it knows about files, paths, mounts, protocols or devices, and anything added is a generic mechanism whose entry says why it could not live in user space.
- **Era 14 · Files: a VFS and TaajFS version 7**, new: ten J60-J69 items (handles, one protocol and one client library, a namespace per process, `openat` and one-directory sandboxes, `/tmp`, `/dev`, `/proc`, a second disk or USB stick, FAT32, change notification), and the ten J70-J79 items (moved from The-Jat's list).
- **Era 15 · Devices: a device manager and confined drivers**, new: nine K75-K83 items (every device handed out the same way, a device manager, bus mastering tied to a handle, USB as a bus with class drivers, several vectors and queues, DMA into a program's memory, virtio 1.x, a test that kills drivers mid-I/O, device power states), and K27, the IOMMU, moved there from Security.
- **Era 16 · Memory: sharing, reclaim and hardening**, new: eleven K84-K94 items (SMEP, SMAP and PAN; no kernel panic for want of memory; a pressure signal; copy-on-write objects; a generic pager; reclaim; a buddy allocator; huge pages; a zero page; objects over 512 MiB; KPTI where needed). J29, J58, K25 and E5 stay where they are, pointed to.
- **Era 17 · Scheduling: contexts, classes and fairness**, new: nine items (K95, K96, K97, K98, K99, K100, K101, K102, and `J80`), building on K20, K21, K24 and Era 13's queue, idle and IPC items. K65 notes that K98 does its job if it comes first.
- **Era 18 · Security: hardening, trust and people**, new: eleven K103-K113 items (kernel objects counted against their creator, fuzzing, a manifest per program, signed programs, verified boot, users as capability sets, disk encryption, speculation mitigations, `DEBUG_WRITE` behind a capability, an `unsafe` audit with Miri, an audit log). K25, K26, K28, K29 and the security items of Eras 12 to 17 are pointed to, not moved.
- **Era 19 · Desktop: applications people can use daily**, new: fourteen K114-K127 items (a benchmark, `display` split into four programs, real key events, text for every language, a toolkit, a panel, a clipboard, the first applications, PNG and JPEG, virtio-gpu, native resolution and scaling, a trusted path and screen lock, notifications, accessibility). K39-K46 are pointed to.
- **Era 20 · The command line and libc**, new: eleven items (`J81`, `J82`, `J83`, `J84`, `J85`, `J86`, `J87`, `J88`, `J89`, `J90`, `J91`). J43 and J44 note that `sh` has what they ask for and J82 brings it to the keyboard; J32-J50, J57, K31, K33, K44 and K45 are pointed to.
- **Era 21 · Boot: recoverable, verified and unlimited**, new, MaK-WSL's: eight K128-K135 items (stray files removed, host tests, a kernel of any size from BIOS, a fast BIOS read, A/B kernels with fallback, a boot menu, where kernels live and how they are updated, a relocatable kernel), with K107 listed beside them. K51, J51, K12, K25 and U6 are pointed to.
- **Eras 22 to 28**, new: Observability (seven K136-K142 items), Services, recovery and configuration (eight K143-K150), Protocols and the ABI (five K151-K155), Build, toolchain and tests (six J92-J97), Packages and updates (five J98-J102), Time and power (five: `J103`, `K156`, K157-K159), Portability: every architecture (sixteen K160-K175: the port contract, page size as a parameter, page tables of any shape, byte-order clean, non-coherent DMA, one interrupt and timer interface, the toolchain for every target, PCIe on AArch64, test parity, RISC-V, LoongArch, PowerPC64, s390x, MIPS and SPARC, real boards, a 32-bit build if decided). The Goals' Portable row now says every architecture with an MMU.
- **Era 29 · Cryptography and encryption**, new: ten K176-K185 items (our library, the audited one beside it, differential tests, constant time and wiping, ours taking over, the CPU's crypto instructions, a key service, encrypted swap, encryption in TaajFS version 7, SSH). K37 now says its cryptography comes from Era 29. E2 and E3 now point to Era 23's items that do them.
- **Later eras**: 13 to 29 added; the assistant, drafted as Era 9 before Era 9 became rustc, takes the next free number, 30.

## Design decisions

- **Eras need a milestone you can see.** Era 13's: the IPC and threads benchmarks in `make test64`, a round trip and a context switch at least twice as fast as first measured, and a parallel job at least 3 times as fast on 4 CPUs as on 1. Era 14's: files from several services under one tree with no file code in the kernel, an open file passed between programs and following its file through a rename, and a 16 GiB volume with a rustc build tree mounting in under a second, a flipped bit reported, version 6 upgraded in place, the power-loss tests passing, `fsck` checking it. Numbers can be argued with, which is the point.
- **IPC and threads in one era.** They are the same work from two sides: a request's cost is two context switches and the scheduler's lock as much as the copies, and `channel_call`'s direct switch is a scheduler change.
- **The VFS and TaajFS version 7 in one era.** Handles change the file protocol and version 7 changes the format; together the file service is rebuilt once. The handles and the namespace come first: J36, the real PC's USB sticks and sandboxing all wait on them.
- **The goals in the repository, not only in a session's memory.** All three machines read this file after every pull; the rule that keeps the kernel pure has to be where each of them will see it, and in the checklist they each go through before merging.
- **Drivers as their own era, not in Era 7.** Era 7 is the real PC's drivers (hubs, I219, Realtek); Era 15 is how every driver is found, started, confined and restarted - the same for a virtual card as for a real one. Its milestone: no device kinds in the kernel, a new driver without a kernel change, two disks and two cards at once, the IOMMU on the real PC, every driver killed mid-I/O and recovered from.
- **The device manager is Zircon's answer**: the kernel hands out every device the same way, and one table in user space decides which driver gets it. The IOMMU stays mechanism too: the kernel programs it from the memory objects a driver holds.
- **Memory as its own era, with policy kept out of the kernel.** The kernel shares, copies on write and drops pages; which pages to evict and which program to stop when memory runs out is decided in user space, as the Goals ask. Its milestone: shared code pages, copy-on-write private mappings, running out of memory without a kernel panic, SMEP, SMAP and PAN on. Hardening first: K84 is small, and K85 is what the milestone tests.
- **Scheduling contexts, seL4's MCS way.** The scheduler is the one piece of policy every microkernel keeps, but it can be kept to mechanism: the kernel enforces a context's priority, budget and period; user space decides who gets which context and hands it out as a capability, so a program can't raise its own priority; and a server runs on its caller's context during a call, which makes priority inheritance and "who pays for this request" the same mechanism. Linux's `nice`, trusted from any process, would fit the Goals worse. Its milestone: a key press on time with four busy programs, real-time threads never late behind a compile, 64 threads no more than one, servers on their clients' budgets, no priority raised without a capability.
- **Security as its own era, though it runs through every other.** Each era already has its security items (the IOMMU, SMAP, contexts as capabilities, sandboxes); Era 18 has what belongs to none of them - accounting for kernel memory, trust in what runs and boots, users, encryption, fuzzing - and points to the rest. Its milestone: a program with no grants can't crash the machine, the fuzzers find nothing in a set time, only signed programs get grants, a verified boot, two users kept apart.
- **Users as capability sets, not user ids.** A login service hands a user's session its home directory handle and its allowed services; the kernel never checks a uid. That keeps the kernel pure and makes a user the same thing as everything else in TheTaaJ: what you were handed.
- **Kernel-memory accounting first**, because it is the one hole a powerless program can use against the whole machine.
- **The desktop's milestone is a day without the terminal.** The vision is a desktop used daily; the test is whether a day's ordinary work - files, editing, copy and paste, another language, a locked screen - can be done in applications alone, quickly enough (the benchmark sets the key-to-screen limit).
- **Split `display` first.** Four jobs in one process is the one thing about the desktop that goes against Modular and Reliable; every other item builds on the pieces it becomes.
- **A trusted path.** A key combination that always reaches the system is what makes a screen lock and a login trustworthy when any program may draw anything in its window.
- **One shell language, not two.** Rather than teach the keyboard's shell what `sh` already knows (J43, J44), the keyboard's shell uses `sh.rs` - one parser, one set of tests, scripts and typed lines the same. What only the keyboard's shell does (grants, jobs, the keyboard) stays in it.
- **A conformance suite as libc's yardstick**, as the benchmarks are for speed: libc's coverage is measured, not guessed.
- **Boot's milestone is recovery.** A machine used daily must survive a bad kernel update by itself, as TaajFS survives a power cut: A/B kernels, a new one tried once and kept only when TheTaaJ says it booted. The 3 MiB wall comes next, since every kernel feature moves it closer.
- **The bootloader's era is MaK-WSL's.** Rule 7 gives TheBootloader to MaK-WSL; the era says so at its top, and its items are MaK-WSL's to do or to agree.
- **Crypto: ours and an audited one.** Crypto written for the first time is where serious bugs hide, and they don't show in ordinary tests: a timing leak, a wrong edge case. So ours is checked three ways on every run, and an audited library protects real secrets until ours has passed and its timing has been checked. That keeps both goals: learning every concept, and secure.
- **Keys as capabilities.** A program is given the right to use a key, never the key: what a hardware security module does, as a service - natural where everything is a capability already.
- **"Every architecture" means every one with an MMU.** A microkernel's isolation is separate address spaces; a CPU without an MMU can't give them, so most microcontrollers are out by the design's nature, not by choice. Within that, tiers: x86_64 and AArch64 fully (today), RISC-V, LoongArch and PowerPC64 LE to the shell and the core tests, s390x, PowerPC BE, MIPS and SPARC as proof that byte order, page size and firmware are behind `arch/`. 32-bit only if the user decides (E7), since the vision is 64-bit.
- **A big-endian CPU and a page size other than 4 KiB in CI.** Portability claimed is not portability; the two assumptions most likely to hide are kept away by running where they are false.
- **The port contract first**: written down before a third architecture, so each port tests the contract rather than inventing it.
- **Configuration inside Era 23, not an era of its own.** Today's "configuration" is init's code; describing services in files and keeping settings in a service are the same work.
- **Observability's kernel part is trace points and a ring.** The kernel can see what nobody else can - switches, faults, every IPC - so it writes events to a ring handed out as a memory object; keeping, searching and showing them is a service's.
- **Our own CI, on our own machine.** No outside service builds the code; TheJat-WSL suits it, and it never builds anyone else's pull request (the rule for public repositories).
- **An IDL of our own size.** What TheTaaJ's protocols use, generating `no_std` Rust and C, not a general framework.
- **The network items in Era 12, not a new era.** Era 12 was already the network's era; the new items are its scale and speed.
- **Every era starts with a benchmark.** Each section's first item measures; the rest are judged by it.
- **No owner given.** The eras are not put in either machine's decided order: that is for the user.
- **The two earlier branches are merged in, not rewritten**, so their entries stay as written: the IPC and TaajFS entries record those reviews; this one records the eras.

## How I checked it

- Each claim against the code: `MAX_CPUS = 16` (`smp.rs`), the one queue and `THREADS` (`sched.rs`, `threads.rs`), `SLICE = 2` ticks, the kernel heap's one `IrqLock` (`mm.rs`), `FRAMES`, the futex `WAITERS` list and its 64 queues (`futex.rs`), the CR3 reload (`tlb.rs`), `fork`'s `ENOSYS` (`libc/src/system.rs`), only `sched_yield` in `sched.h` and no `semaphore.h`; the network's limits (`TCP_CONNECTIONS` 16, `TCP_LISTENERS` 8, `TCP_BACKLOG` 4 in `net/src/stack.rs`; `SOCKETS` 10, `LISTENERS` 4 and the `WAIT_HANDLES_MAX` assertion in `user/src/bin/net.rs`), the fixed address there, what `tcp.rs` says it does not do, and the frame split into pieces (`user/src/nic.rs`).
- The VFS claims: the stateless whole-path protocol and its 15 operations (`fileserver/src/lib.rs`), one store per `fs` service (`user/src/bin/fs.rs`), the `Open` enum and the path-and-offset open file (`libc/src/descriptors.rs`), `__taaj_executable` (`libc/src/lib.rs`), `isatty`'s cases, and init's namespaces table (`user/src/bin/init.rs`).
- The driver claims: `offered()` and its class and vendor matches, `prepare`'s per-kind choices and the bus-master bit set (`kernel64/src/devices.rs`), the virtio identification (`mmio_devices.rs`), the DMA note at the top of `devices.rs`, init's fixed grants for one disk, card and USB controller and its keeping of their DMA memory (`user/src/bin/init.rs`), and what `usb.rs` serves.
- The memory claims: `paging.rs`'s `Format`, `frame.rs`'s bitmap and lowest-first search, the panic for a null `alloc` in `mm.rs`, the protections `enable_protection` turns on (NXE, CR0.WP, CR4.PGE - not SMEP or SMAP) and PXN and UXN on AArch64, `fault_in` and the account (`process.rs`, `account.rs`), `MEMORY_MAX` 512 MiB and `RESERVE_MAX` 64 GiB (`abi`).
- The scheduler claims: `SLICE`, the one `ready` queue and the `states` map, `tick`'s walk over every state and its `Vec`, `offer` and `pokes` (a `u64`) in `sched.rs`; `preempt_if_needed` from the timer interrupt and the preemption selftest; `HZ` from `abi::TICKS_PER_SECOND`. K21's branch (`k21-cpu-time`) exists unmerged; K20's is not pushed yet.
- The security claims: the rights in `abi` (SEND to KILL), `PROCESS_KILL` and `SIGNAL_SEND` needing KILL, `PROCESS_SPAWN`'s launcher, `SYSTEM_INFO` and `PROCESS_LIST` needing the system handle, `DEBUG_WRITE` with no handle (`abi`, `syscall.rs`); the shell's grants by name (`user/src/bin/shell.rs`, `narrow`); what `account.rs` does not count; `random.rs`'s sources; no fuzz targets in the repository.
- The desktop claims: what `display.rs` holds and its startup handles, the window protocol (`gui/src/protocol.rs`), the window manager (`gui/src/desktop.rs`), `gfx`'s font from `console`, `WINDOW_PROGRAMS` (`counter`) and `EDIT_PROGRAMS` (`edit`) in `user/src/bin/shell.rs`.
- The shell and libc claims: `sh.rs`'s header and its refusal of `&`, its 33 tests; `shell.rs`'s header (jobs, grants, no history or completion found in it); `libc/src/lib.rs`'s overview against `threads.rs` and the missing `emutls.c`; the headers in `libc/include`; musl's libm in `libc/c/libm`.
- The boot claims: `taajboot.h` (the protocol, its size checks), `stage2/elf.c`'s "Segment outside 1 MiB .. 3 MiB, refusing" and the kernel's `linker.ld` check, journal entry 089's 672 KiB left, `uefi/main.c` and `uefi/aarch64.c`'s headers, `abc.txt` and `random.txt` at TheBootloader's root, and no test files in it.
- The encryption claims: `chacha.rs` and `random.rs`, and no other cryptographic code in TheTaaJ's own sources (a search for SHA, BLAKE, AES, Poly1305, Ed25519, X25519 and HMAC outside `ports/`).
- The last reviews' claims: `logring.rs`, `logmerge.rs`, `lastwords.rs` and the panic handler in `main.rs` (no backtrace); the shell's built-in commands; init's header, `MAX_RESTARTS` and `RESTART_WINDOW`; `ramdisk-src/config.cfg` read by nothing in the 64-bit code; the `abi` crate's modules and no version on calls or protocols; `TEST64_PARTS` and no CI files; no package code; `libc/c/time.c` (localtime is UTC), `rtc.rs`, the `acpi` crate; the `aarch64-...` test parts against the x86-only ones.
- Nothing was built or run: this changes only documents.

## What is still missing

- Which machine takes each of Eras 13 to 29, and in what order (Era 21 is MaK-WSL's, as TheBootloader's owner); the numbers given to the new items at merge.

## What I learned

- **A microkernel's speed is its IPC and its scheduler together.** Every file read and every packet is a request to a service, so the cost of a message and the cost of a switch are paid on everything, and they are worth an era of their own.
- **What must be trusted decides what must be in the kernel.** A mount table is in the Unix kernel because it is where access is decided; with capabilities it decides nothing, so it can live in the program.
- **Fixed tables hide in waits.** The network's socket limit isn't a setting: it is how many handles one wait can take.

## Testing note

Nothing to boot: this changes only `docs/`. To see the eras and items:

```
grep -n '^## Goals\|^## Era\|kernel stays pure' docs/who-does-what.md
```

| Look for | You should see |
|---|---|
| `## Era 13 · A fast core` | the IPC section (7 items) and the threads section (9 items) |
| `## Goals` | ten goals, before "The machines" |
| `The kernel stays pure` | a line in "Before merging" |
| `## Era 14 · Files: a VFS and TaajFS version 7` | 10 items (J60-J69), then 10 items (J70-J79) |
| `K53` ... `K59` | 7 rows in "Network (Era 12's ground)", after K49 |
| `## Era 15 · Devices: a device manager and confined drivers` | 9 items (K75-K83) and K27 |
| `## Era 16 · Memory: sharing, reclaim and hardening` | 11 items (K84-K94) |
| `## Era 17 · Scheduling: contexts, classes and fairness` | 8 items (K95-K102) and `J80` |
| `## Era 18 · Security: hardening, trust and people` | 11 items (K103-K113) |
| `## Era 19 · Desktop: applications people can use daily` | 14 items (K114-K127) |
| `## Era 20 · The command line and libc` | 11 items (J81-J90 and J82-J91) |
| `## Era 21 · Boot: recoverable, verified and unlimited` | 8 items (K128-K135), MaK-WSL's |
| `## Era 22` ... `## Era 28` | Observability; Services, recovery and configuration; Protocols and the ABI; Build, toolchain and tests; Packages and updates; Time and power; Portability: every architecture (with its tiers table) |
| `## Era 29 · Cryptography and encryption` | 10 items (K176-K185) |
| `## Later eras` | Eras 13 to 29, and the assistant as 30 |
