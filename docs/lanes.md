# Lanes: Eras 12 to 29 worked on in parallel

Fourteen **lanes**, each worked by one session on one machine, so that the eras of `docs/who-does-what.md` move forward at the same time. A lane is a set of items that change the same files; two lanes rarely change the same file, so their branches rarely conflict.

A new session reads `docs/start-here.md` first; it says how to work. This file says **what** each lane does, in which order, which files are its own, and what it waits for from other lanes.

**Status lives in `docs/who-does-what.md`, not here.** An item's Notes there say when it is done ("**Done (entry NNN)**"). This file holds each lane's order, and a lane's table is changed only by that lane's session (rule 6). `python3 tools/lanes-check.py` says which open items are in no lane, or in two.

## The week (from 2026-11-27)

There are about 270 open items, and many are L-sized (a week or more alone), so not all of them can be finished in a week. What a week can do:

- every lane started, its benchmark (where it has one) merged first, so every speed item after it says what it moved;
- each lane's **first wave** (the rows marked **1** below) merged: the ground the rest of its era builds on;
- the items other lanes wait for done first (see "Across lanes").

The second and later waves follow on the same lanes in the weeks after, in the same order.

## The machines and their lanes

| Machine | Lanes (one session each) | Why there |
|---|---|---|
| **TheJat-WSL** (KVM, 24 CPUs, 15 GiB; `make test64` 389 s, entry 109) | `ipc`, `memory`, `ci`, `network`, `services` | The kernel's hot paths and the network need fast, real-time boots; the most CPUs for five sessions; Era 25's CI suits it |
| **MaK-WSL** (KVM, laptop, the real PC, owns TheBootloader) | `sched`, `boot`, `devices`, `desktop` | `sched` continues step A (K20); `boot` is TheBootloader's (rule 7); `devices` and `desktop` need the real PC now and then |
| **TheJat-VM** (no KVM, 16 CPUs, 15 GB, the reference host versions) | `files`, `taajfs`, `libc`, `crypto`, `arch` | Mostly user space and host-tested crates, where emulated boots cost least; `arch`'s new CPUs are emulated on every machine |

**Starting a session.** Open a new session on the machine, in its first TheTaaJ checkout, and type one line:

```
You are TheJat-WSL/ipc. Read docs/start-here.md.
```

with the machine and lane changed. The session makes its own tree (`sh tools/new-session.sh ipc`), reads its lane below and starts on the lane's first open item. It asks the user only before merging.

A machine may run fewer sessions than it has lanes: then it starts them in the order listed, and a session that finishes its lane's first wave may start the next lane of its machine (in its own worktree).

**Not in a lane this week:**
- Era 8 (the AI engine), waiting for U5;
- Era 9 (rustc: J14-J16, J21, J22, J24, J25), paused at step E (`llvm-23`): to be finished when the user says;
- Era 10 and 11 (sound and voice), apart from the two sound items in `devices`' last wave;
- Era 30 (the assistant);
- E6-E8, J50, J97, which come later or wait for the user.
- J51 (test programs out of the kernel image): not needed if K130 comes - the user's decision (U7).
- K186 (files after the disk driver is restarted), found by `ci` in entry 117: for the `services` lane to take into its table.

## Across lanes

What one lane waits for from another. When an item's dependency isn't merged yet, the session takes the next ready item of its own lane; it never does the other lane's item in its branch (rule 11 allows only small fixes).

| Waiting | For | From lane |
|---|---|---|
| `sched` K98 (and K65) | K61 `channel_call` | `ipc` |
| `ipc` K73 services with several threads; `taajfs` J78 | K61; then K10 (disk queue, `devices`) and J71 (`taajfs`) | `ipc`, `devices`, `taajfs` |
| `network` K54 shared rings | K63 bigger messages or rings | `ipc` |
| `taajfs` J79 a shared page cache | K88 a generic pager, J58 sealed objects | `memory` |
| `libc` J90 a shared libc | J29, K87 (shared code pages) | `memory` |
| `files` J63-J67, `services` K105, `crypto` J99 | J62 a namespace per process | `files` |
| `taajfs` J76/J77 (owners, links) | the file protocol's new operations (J61) | `files` |
| `ipc` K152 the file protocol on the IDL | J61 one file protocol | `files` |
| `desktop` K123 virtio-gpu | K81 virtio 1.x | `devices` |
| `arch` K167 PCIe on AArch64 | K75 every device handed out the same way | `devices` |
| `arch` K161/K162 (page size, page-table shape) | `memory`'s K90/K91 merged (both change `paging.rs`) | `memory` |
| `network` K37 TLS, `crypto` K106 signed programs, `boot` K107 | K176/K177 the crypto libraries; K106 also K105 | `crypto`, `services` |
| `network` K156 NTP | K30 DHCP, K33 UDP (its own) | - |
| `crypto` J102 system updates | K132 kernels A and B | `boot` |
| `files` K108 users | J62 (its own), J76 owners | `taajfs` |
| `ci` K141 `top` | K21 CPU time per process | `sched` |
| `desktop` K45 job control, `libc` J84/J85 | K44 pseudo-terminals | `desktop` |
| **Every lane that adds kernel tests** | J51 test programs out of the kernel image, or K130 a kernel of any size: the image has about 680 KiB left under its 3 MiB limit | `ci`, `boot` - both first wave |

## Lanes

Each lane's table is its order. **Wave 1** is this week's aim. An item marked "with" another goes in the same branch when that is smaller than two.

### `ipc` - TheJat-WSL/ipc · Era 13's IPC, then Era 24 (the IDL)

Its files: `kernel/kernel64/src/channel.rs`, `ipc.rs`, `cap.rs`, the channel and handle parts of `kernel/abi`, `user/src/services.rs`, and a new `tools/idl` for Era 24. Repos: TheKernel, TheTaaJ.

| Item | Wave | Note |
|---|---|---|
| K60 | 1 | The IPC benchmark: first |
| K64 | 1 | Waiting until a channel is writable |
| K66 | 1 | The cycle leak |
| K61 | 1 | `channel_call`: `sched`'s K98 and this lane's K73 wait for it |
| K62 | 1 | One copy per message |
| K?-port | 2 | A port: one wait for many channels (the file service's open files need it, entry 115) |
| K63 | 2 | Bigger messages or rings; `network`'s K54 waits for it |
| K111 | 2 | `DEBUG_WRITE` behind a capability |
| K73 | 2 | Services with several threads |
| K151 | 2 | The IDL |
| K153 | 3 | Versions |
| K155 | 3 | Protocols documented from the IDL |
| K154 | 3 | The ABI check before merging |
| K152 | 3 | The protocols moved onto the IDL, the file protocol first (after `files`' J61) |

### `memory` - TheJat-WSL/memory · Era 16, and the memory parts of Eras 9 and 18

Its files: `kernel/kernel64/src/memory/`, `mm.rs`, `memory_object.rs`, `paging.rs`, `protections.rs`, `regions.rs`, the memory part of `kernel/abi`. Repos: TheKernel, TheTaaJ (tests).

| Item | Wave | Note |
|---|---|---|
| K84 | 1 | SMEP, SMAP, PAN: small, first |
| K85 | 1 | No kernel panic for want of memory |
| K103 | 1 | Kernel objects counted against their creator (Era 18's milestone; with K85) |
| K86 | 1 | The memory-pressure signal |
| J58 | 1 | Sealed memory objects: J29 and `taajfs`' J79 wait for it |
| J29 | 2 | Programs loaded page by page (Era 9's; after J58) |
| K90 | 2 | A buddy allocator |
| K69 | 2 | Per-CPU caches (Era 13; with K90) |
| K91 | 2 | 2 MiB pages |
| K93 | 2 | Memory objects over 512 MiB |
| K87 | 2 | Copy-on-write objects |
| K92 | 3 | The shared zero page |
| K71 | 3 | PCID and ASIDs (Era 13) |
| K94 | 3 | Separate kernel tables |
| K110 | 3 | Speculation mitigations (with K94) |
| K88 | 3 | The generic pager |
| K89 | 3 | Pages given back under pressure |
| E5 | 3 | Swap to a disk file (with K89) |

### `ci` - TheJat-WSL/ci · Era 25, the test suite, then Era 22 (observability)

Its files: the test rules of the `Makefile`, `kernel/kernel64/scripts/*-test.sh`, `tools/`, CI scripts; for Era 22 `logring.rs`, `lastwords.rs` and a new log service. Repos: TheTaaJ, TheKernel.

| Item | Wave | Note |
|---|---|---|
| J93 | 1 | Our own CI: `main` built and tested after every merge |
| J95 | 2 | No flaky tests (every part 50 times) |
| K50 | 2 | `bigmem64-test` once did not finish under KVM |
| K52 | 2 | `aarch64-tcp-test`'s late segment |
| J96 | 2 | Coverage of the host tests |
| K189 | 2 | `cargo fuzz` on the host for every parser (K104's second half) |
| K136 | 3 | The log service |
| K188 | 3 | A crash record the next boot finds (K137's second half) |
| K138 | 3 | A crash report per program |
| K141 | 3 | `top` (after `sched`'s K21) |
| K139 | 3 | Tracing |
| K140 | 3 | A profiler |
| K142 | 3 | A debugger |

### `network` - TheJat-WSL/network · Era 12's network items

Its files: the `net` crate, `user/src/bin/net.rs`, `user/src/net.rs`, `nic.rs`, `nic_service.rs`, `e1000/`, `libc/src/sockets.rs`. Repos: TheTaaJ, TheKernel (tests). Taken over from MaK-WSL's step D, so that MaK-WSL's sessions are free for the real PC, the bootloader and the scheduler.

| Item | Wave | Note |
|---|---|---|
| K53 | 1 | The network benchmark: first |
| K33 | 1 | Loopback, UDP and non-blocking sockets from C |
| K49 | 1 | Socket readiness for `poll` |
| K55 | 1 | More sockets |
| K30 | 1 | DHCP |
| K31 | 2 | DNS |
| K32 | 2 | Link state |
| K34 | 2 | IP fragments, ICMP errors |
| K29 | 2 | A random initial sequence number |
| K56 | 2 | Who may listen on which port |
| K58 | 2 | Checksum and segmentation offload |
| K156 | 2 | Network time (after K30, K33) |
| K54 | 3 | Frames through shared rings (after `ipc`'s K63) |
| K35 | 3 | TCP's extensions |
| K59 | 3 | Sockets that survive a restart (with `services`' K146) |
| K57 | 3 | A routing table |
| K38 | 3 | HTTP server: several connections |
| K36 | 3 | IPv6 |
| K37 | 3 | TLS 1.3 (after `crypto`'s K176/K177) |

### `services` - TheJat-WSL/services · Era 23, and the service parts of Era 18

Its files: `user/src/bin/init.rs`, `registry.rs`, the `registry` crate, `servers/init`, `servers/regsrv`, service description files. `user/src/services.rs` is shared with `ipc`: keep changes there small. Repos: TheTaaJ.

| Item | Wave | Note |
|---|---|---|
| K143 | 1 | Services described in files: first |
| K144 | 1 | `service` commands |
| K145 | 1 | A watchdog per service (covers E3) |
| E3 | 1 | Done by K145 |
| K148 | 1 | A poisoned request refused (is E2) |
| E2 | 1 | Done by K148 |
| K149 | 1 | Shutdown in order (is K16) |
| K28 | 2 | Only a chosen program may switch off |
| K105 | 2 | A manifest per program (after `files`' J62 for directories; the format shared with K143) |
| K147 | 2 | init and the registry restartable |
| K146 | 2 | State that survives a restart |
| K150 | 3 | Settings |
| K113 | 3 | The audit log |

### `sched` - MaK-WSL/sched · step A, Era 13's threads, then Era 17

Its files: `kernel/kernel64/src/sched.rs`, `smp.rs`, `futex.rs`, `alarms.rs`, `clock.rs`, the thread parts of `kernel/abi` and of `libc/src/threads.rs`. Repos: TheKernel, TheTaaJ.

| Item | Wave | Note |
|---|---|---|
| K191 | 2 | More POSIX timers, fine timeouts (after K24) |
| K192 | 1 | Cross-CPU wake-ups under KVM (entry 128's bug 5) |
| K193 | 1 | A lock per CPU's run queue (K68's other half) |
| K101 | 2 | A thread prefers its last CPU |
| K70 | 2 | Futexes without one shared list |
| K72 | 2 | Idle CPUs that sleep |
| K74 | 2 | More than 16 CPUs (with `devices`' K7) |
| K102 | 2 | More than 64 CPUs |
| J59 | 2 | The missing POSIX thread calls (semaphores, barriers, cancellation) |
| K97 | 2 | Scheduling contexts as capabilities |
| K99 | 3 | Classes |
| K98 | 3 | A server on its client's context (after `ipc`'s K61; replaces K65) |
| K65 | 3 | Done by K98, or alone if K98 is far |
| K100 | 3 | Fairness between processes |
| K157 | 3 | CPU idle states and frequency (Era 27) |

### `boot` - MaK-WSL/boot · Era 21, the bootloader (MaK-WSL's alone, rule 7)

Its files: all of TheBootloader; `kernel/kernel64/src/bootinfo.rs`, `elf.rs`, the linker scripts. Repos: TheBootloader, TheKernel, TheTaaJ.

| Item | Wave | Note |
|---|---|---|
| K128 | 1 | `abc.txt` and `random.txt` removed |
| K129 | 1 | Host tests for the ISO reader, the ELF loader, the page tables |
| K131 | 1 | A fast BIOS read (is K51) |
| K51 | 1 | Done by K131 |
| K130 | 1 | A kernel of any size: ends the 3 MiB wall every lane is near |
| K23 | 1 | Boot time measured and cut |
| K135 | 2 | A relocatable kernel |
| K25 | 2 | ASLR (the kernel's half after K135) |
| K133 | 2 | A boot menu |
| K132 | 2 | Kernels A and B: `crypto`'s J102 waits for it |
| K134 | 3 | Where kernels live, and their updates (with `devices`' K12) |
| K107 | 3 | Secure Boot and measured boot (after `crypto`'s K176/K177) |
| J23 | 3 | The BIOS loader built with clang (Era 9's R5; the bootloader half) |

### `devices` - MaK-WSL/devices · Era 15, the real PC, ACPI, then sound

Its files: `kernel/kernel64/src/pci.rs`, `devices.rs`, `interrupts.rs`, `mmio_devices.rs`, `acpi.rs`, `devicetree.rs`, `power.rs`, and the drivers: `ahci/`, `nvme/`, `virtio/`, `usb/`, `e1000/` (shared with `network`: the card's protocol is `network`'s, the hardware is `devices`'), `acpi/`. Repos: TheKernel, TheTaaJ.

The real-PC items need the user at the PC (U4): done whenever the user is there, ahead of the rest.

| Item | Wave | Note |
|---|---|---|
| K82 | 1 | A test that kills each driver mid-I/O: measures the era |
| K75 | 1 | Every device handed out the same way; `arch`'s K167 waits for it |
| K77 | 1 | Bus mastering tied to a handle |
| K76 | 1 | The device manager |
| K81 | 1 | virtio 1.x; `desktop`'s K123 waits for it |
| K1 | real PC | R7: boot from a USB stick |
| K39 | real PC | The native resolution (with `desktop`'s K124) |
| K11 | real PC | GPT |
| K7 | real PC | x2APIC |
| K3 | real PC | A day on the real PC |
| K4 | real PC | What K1-K3 find |
| K10 | 2 | A queue per disk; `taajfs`' J78 waits for it |
| K8 | 2 | NVMe |
| K9 | 2 | AHCI |
| K2 | 2 | USB hubs (with K78) |
| K78 | 2 | USB as a bus |
| K5 | 2 | Intel I219 |
| K6 | 2 | Realtek |
| K79 | 2 | MSI-X queues (K68 done, entry 129) |
| K22 | 2 | A transmit-interrupt serial writer |
| K13 | 2 | The AML interpreter |
| K14 | 2 | `_PRT` |
| K15 | 2 | The power button |
| K16 | 2 | A clean shutdown's driver half (`services`' K149 has the rest) |
| K159 | 3 | Power button, lid and battery as events (is K15, K17) |
| K17 | 3 | Battery and lid |
| K12 | 3 | The installer (with `boot`'s K134) |
| K27 | 3 | An IOMMU |
| K80 | 3 | DMA into a program's memory (with K27) |
| K83 | 3 | Device power states |
| K158 | 3 | Suspend and resume (is K18) |
| K18 | 3 | Suspend to RAM |
| K47 | 3 | HD Audio and an audio service (Era 10) |
| K48 | 3 | WAV playback and a microphone (Era 10) |

### `desktop` - MaK-WSL/desktop · Era 19, the terminal and job control

Its files: `gui/`, `gfx/`, `input/`, `console/`, `user/src/window.rs`, `user/src/bin/display.rs`, `edit.rs`, `shell.rs`'s terminal side. Repos: TheTaaJ, TheKernel (tests).

| Item | Wave | Note |
|---|---|---|
| K114 | 1 | The desktop benchmark: first |
| K115 | 1 | `display` split: compositor, window manager, terminal |
| K44 | 1 | The terminal as its own program; pseudo-terminals; `libc`'s J85 waits for it |
| K116 | 1 | Real key events (with K42) |
| K42 | 1 | Key up and down, lone Shift and Control |
| K120 | 1 | Copy and paste (with K41) |
| K41 | 1 | Copy and paste between windows |
| K119 | 2 | A panel (with K40) |
| K40 | 2 | Resize, minimise, a task bar |
| K46 | 2 | Scroll wheel and more buttons |
| K45 | 2 | Job control (after K44; with `libc`'s J84) |
| K117 | 2 | Text for every language (with K43) |
| K43 | 2 | A font beyond ASCII |
| K124 | 2 | Native resolution and scaling |
| J49 | 2 | The editor: undo, selection, search |
| K118 | 3 | The toolkit |
| K122 | 3 | PNG and JPEG |
| K121 | 3 | The first applications |
| K123 | 3 | virtio-gpu (after `devices`' K81) |
| K125 | 3 | A trusted path |
| K126 | 3 | Notifications |
| K127 | 3 | Accessibility |

### `files` - TheJat-VM/files · Era 14's VFS, then users

Its files: `libc/src/descriptors.rs`, `user/src/files.rs`, the `fileserver` crate, `user/src/bin/fs.rs`'s protocol side, the file-protocol part of `kernel/abi`, `servers/fat32`, new services for `/tmp`, `/dev`, `/proc`. Repos: TheTaaJ, TheKernel (`abi`, tests).

| Item | Wave | Note |
|---|---|---|
| J55 | 2 | Programs moved into `/bin` |
| J66 | 2 | `/proc` |
| J67 | 2 | A second disk (with `devices`' K11) |
| J69 | 2 | Change notification |
| J68 | 3 | FAT32 as a service |
| K108 | 3 | Users and login (after J62 and `taajfs`' J76) |
| K109 | 3 | Disk encryption (after K108; `crypto`'s libraries) |

### `taajfs` - TheJat-VM/taajfs · TaajFS version 7 (Era 14's other half)

Its files: the `taajfs` crate, `tools/tfstool-rs`, the store side of `user/src/bin/fs.rs`. The file protocol is `files`': a new operation (links, owners) is added there, or in this lane's branch after asking that session through who-does-what.md. Repos: TheTaaJ, TheKernel (tests).

| Item | Wave | Note |
|---|---|---|
| J70 | 1 | The file-system benchmark: first |
| J71 | 1 | Group commits (covers E4) |
| E4 | 1 | Done by J71 |
| J73 | 1 | Version 7: the free map on disk |
| J72 | 1 | Version 7: hashed directories |
| J74 | 1 | Version 7: checksums |
| J75 | 1 | Version 7: 64-bit sizes and block numbers |
| J76 | 2 | Version 7: owners, permissions, times (the protocol after `files`' J61) |
| J77 | 2 | Version 7: symbolic and hard links |
| E1 | 2 | `fsck` for version 7 |
| K184 | 3 | Per-file encryption (after `crypto`'s K176/K177) |
| J78 | 3 | Several requests at a time (after J71, `devices`' K10, `ipc`'s K73) |
| J79 | 3 | A shared page cache (after `memory`'s K88 and J58) |

Version 7 is one format change: J72-J77 are one or two branches (the fewer-bigger-steps rule), with one version number and one upgrade path from version 6.

### `libc` - TheJat-VM/libc · Era 20, and the libc items of Era 9's list

Its files: `libc/src/` except `descriptors.rs` (`files`') and `sockets.rs` (`network`'s), `libc/c`, `libc/include`, `libc/programs`, `servers/sh`, `libc/src/sh*.rs`, the C ports in `ports/` other than LLVM. Repos: TheTaaJ, TheKernel (tests).

| Item | Wave | Note |
|---|---|---|
| J81 | 1 | `lib.rs`'s overview brought up to date: first |
| J89 | 1 | A conformance suite: measures the era |
| J11 | 1 | Wide characters |
| J87 | 1 | A UTF-8 locale (with J11) |
| J32 | 1 | `scanf` from streams |
| J33 | 1 | `%a`, long double |
| J34 | 1 | `erf`, `tgamma` and the rest |
| J88 | 1 | A faster `malloc` (covers J41, J42) |
| J41 | 1 | Done by J88 |
| J42 | 1 | Done by J88 |
| K26 | 1 | Stack protector and stack probes |
| J103 | 2 | Time zones (Era 27) |
| J86 | 2 | `regex`, `iconv` and the other missing headers |
| J82 | 2 | One shell language (supersedes J43, J44) |
| J43 | 2 | Done by J82 |
| J44 | 2 | Done by J82 |
| J83 | 2 | Line editing |
| J85 | 2 | `termios` (after `desktop`'s K44) |
| J84 | 2 | `&` and job control (with `desktop`'s K45) |
| J91 | 3 | The everyday tools (covers J47, J48) |
| J47 | 3 | Done by J91 |
| J48 | 3 | Done by J91 |
| J45 | 3 | Rebuild only what changed |
| J46 | 3 | Install over `/lib` |
| J40 | 3 | libc++'s `<filesystem>`, `random_device` |
| J57 | 3 | exec without the go-between |
| J56 | 3 | zlib on TheTaaJ's disk |
| J90 | 3 | A shared libc (after `memory`'s J29, K87) |

### `crypto` - TheJat-VM/crypto · Era 29, then Era 26 (packages)

Its files: new crates (`crypto/`, `pkg/`) and their tools. It is the lane least in anyone's way: start it on any machine with a free slot. Repos: TheTaaJ.

| Item | Wave | Note |
|---|---|---|
| K176 | 1 | Our crypto library: first |
| K177 | 1 | The audited library beside it |
| K178 | 1 | Differential tests |
| K179 | 1 | Constant time and wiping, checked |
| J98 | 2 | A package format |
| J100 | 2 | Packages built on the host |
| K106 | 2 | Signed programs (after `services`' K105) |
| J99 | 2 | The package service (after `files`' J62) |
| J101 | 3 | A repository (after `network`'s K31, K37) |
| J102 | 3 | System updates (after `boot`'s K132) |
| K182 | 3 | The key service (after `files`' K108) |
| K183 | 3 | Encrypted swap (with `memory`'s E5) |
| K185 | 3 | SSH (after `network`'s K37 and K182) |
| K181 | 3 | The CPU's crypto instructions (after Era 8's XSAVE) |
| K180 | 3 | Ours takes over: last |

### `arch` - TheJat-VM/arch · Era 28, every architecture

Its files: `kernel/kernel64/src/arch/`, new `arch/` directories, the linker scripts, the per-CPU files of libc, the Makefile's per-CPU rules, the toolchain scripts. `paging.rs` is `memory`'s until its K90 and K91 are merged. Repos: TheKernel, TheTaaJ.

| Item | Wave | Note |
|---|---|---|
| K160 | 1 | The port contract written down: first |
| K166 | 1 | The toolchain for every target |
| K163 | 1 | Byte-order clean, with host tests |
| K169 | 1 | RISC-V 64, started: the contract's first test, in new files |
| K165 | 2 | Interrupt controllers and timers behind one interface |
| K161 | 2 | The page size a parameter (after `memory`'s K90/K91) |
| K162 | 2 | Page tables of any shape (with K161) |
| K164 | 2 | Non-coherent DMA (with `devices`' K80) |
| K167 | 2 | PCIe on AArch64 (after `devices`' K75) |
| K168 | 2 | Every Tier 1 test on both Tier 1 CPUs |
| K170 | 3 | LoongArch64 |
| K171 | 3 | PowerPC64 |
| K172 | 3 | s390x |
| K173 | 3 | MIPS64 and SPARC64 |
| K174 | 3 | Real boards: only if the user wants them (U6) |
| K175 | 3 | A 32-bit build: decide with the user (E7) |
