# Who does what: The-Jat and MaK

TheTaaJ is built on two machines. Both push to the same three repositories (TheTaaJ, TheKernel, TheBootloader), so this file says which machine does which work. Each machine reads it after every `git pull`.

| Machine | What it is | Good at |
|---|---|---|
| **The-Jat** | An Ubuntu virtual machine inside VMware: 16 CPUs, 15 GB, no KVM (every QEMU boot is emulated) | Long builds (rustc, LLVM), work that doesn't need fast boots or real hardware |
| **MaK** | The real PC: Ubuntu under WSL2, with KVM | Fast boots, `make test64` in less time, timing races that only show at full speed, the real hardware itself (USB stick boot, R7) |

## The machines

Measured on MaK on 2026-11-25 and on The-Jat on 2026-11-26.

### Hardware and setup

| | **MaK** | **The-Jat** |
|---|---|---|
| Machine | A laptop: 13th Gen Intel Core i7-13620H | A VMware virtual machine on an Intel Core Ultra 9 275HX |
| CPU | 10 cores (6 performance + 4 efficient), 16 threads | 16 virtual CPUs |
| Vector instructions | AVX2, no AVX-512 | AVX2, no AVX-512 |
| Memory | 11 GiB given to WSL (WSL's default share; no `.wslconfig`) | 15 GB |
| Disk | 1 TB virtual disk, 917 GB free | 167 GB, 103 GB free |
| Virtualization | KVM in WSL2 (VT-x, nested) | None: every QEMU boot is emulated |
| OS | Ubuntu 26.04 on WSL2 (kernel 6.6) | Ubuntu 24.04.4 (kernel 7.0) |
| Host LLVM | 21 by default; Ubuntu 24.04's 18.1.3 unpacked locally for the byte-for-byte tests | 18.1.3, the reference |
| Host glibc | 2.43 (newer than the reference) | 2.39, the reference |
| AArch64 boots | Emulated | Emulated |

### Measured times

| Task | **MaK, KVM** | **MaK, emulated** | **The-Jat** |
|---|---|---|---|
| `make test64`, all 48 parts | 14 min 6 s | not measured | 13 min 55 s |
| `make test64`, the x86_64 parts only | 7 min 56 s | not measured | not measured |
| The ports (LLVM, Clang, lld, libc++ for both CPUs), first build | 39 min 3 s (compiling on the host: KVM doesn't change it) | | not measured (built before this table) |
| The i686 cross compiler (`toolchain_i686.sh`) | about 12 min | | not measured |
| `smp64-test` | 14 s | 43 s | 36 s |
| `thread64-test` | 25 s | 36 s | 33 s |
| `bigmem64-test` | 33 s | 48 s | 36 s |
| `boot64-test` (mostly typing keys and waiting) | 382 s | 385 s | not measured |
| Four `cprimes` copies at once (the busy part) | about 1.4 s | about 14 s | not measured |

The-Jat's emulated boots are a little faster than MaK's emulated ones (a newer CPU, and nothing else on it). The whole of `make test64` is about as long on both machines, because its slowest parts are the AArch64 ones, emulated everywhere. KVM makes the x86_64 half several times faster.

### What follows for the split

| Kind of work | Better on | Why |
|---|---|---|
| Fast edit, boot and test loops on x86_64 | MaK | KVM: a busy boot is up to ten times faster |
| Timing races, real-PC behaviour | MaK | KVM delivers interrupts in real time, as a PC does; it found the bootloader's protected-mode bug |
| Real hardware (USB stick, R7) | MaK | It is the real PC |
| Byte-for-byte comparisons with the host's tools (LLVM, Clang, glibc) | The-Jat | It has the reference versions; MaK's newer host needs local workarounds |
| Long unattended builds (rustc, LLVM 23) | The-Jat | MaK is a laptop: it can sleep, run on battery or slow down when hot, and WSL stops when idle; and it has 4 GB less memory |
| Big memory (building rustc, models of 1-3 GB) | The-Jat | 15 GB against 11 GiB |
| AArch64 | Either | Emulated on both, about as fast |

MaK's memory can grow: a `.wslconfig` gives WSL more of the laptop's memory, if the laptop has it to spare.

## How we work

**This file is the one place for the work items.** Every item, its owner, its order, what it depends on and whether it is done is written here and nowhere else. Each machine reads it after every `git pull` and changes it in the same branch as the work. How an item was built goes in its journal entry; this file only points to it.

### Rules for both machines

1. **A separate branch for everything**: every step, fix or document change, with the same name in every repository it touches (for example `r3-std`, `usb-hubs`). Nothing is committed straight to `main`.
2. **Nothing is merged into `main` without asking the user first**, and only after full testing:
   - pull `main` and merge it into the branch;
   - `make test64` passes on the branch with `main` merged in;
   - the step's own new tests pass, and its testing note was checked by a scripted boot;
   - the user is asked and says yes;
   - then merge and push, TheKernel and TheBootloader first, TheTaaJ last.
3. **The same commit author on both machines: `The-Jat`**, with the same email. Set it once in each repository (`git config user.name "The-Jat"`, and the email the user gives). Nothing else goes in a commit message's author or trailers: no "Claude" and no `Co-Authored-By` line.
4. **Work follows the dependencies.** An item is started only when the items it depends on are done. Among the items that are ready, the one that most other items wait for goes first (see "What waits for what" below). If an item turns out to block more work than this file says, move it up and write down why.
5. **Journal numbers are taken at merge time.** Whoever merges into `main` first takes the next number. The other renumbers their entry (file name, title, README row) before merging.
6. **Keep this file current**, in the step's own branch:
   - when a step starts, put it and its branch under "Now";
   - when an item is done, write **Done (entry NNN)** in its Notes;
   - when something new is found to do, add it to the right list with a new number (numbers are never reused);
   - when a dependency is found, add it to "What waits for what".
7. **Only the bootloader's owner (MaK) changes TheBootloader.** If The-Jat needs a change there, it's written down here and MaK does it.
8. **Files both machines touch often**, such as `Makefile`, `README.md`, `docs/testing-notes.md`, `libc/` and `kernel64/src/syscall*`: keep changes small and merge `main` in often, so conflicts stay small.
9. **Machine-local settings stay out of commits:** `kernel/headers/revision.h`, `bootloader/headers/revision.h`, `toolchain/`, and local tweaks to `qemu-boot.sh`.
10. **Everything is documented**: every feature, every bug fix (even a small one between steps), every test or tooling change and every change to how we work gets a journal entry in `docs/journal/` (why, how it fits together, design decisions, bugs, how it was checked, what is still missing, what I learned, a testing note), a row in `docs/journal/README.md`, and `python3 tools/testing-notes.py` run. A small fix gets a short entry, never none.
11. If a step on one machine finds a bug in the other machine's area, the fix goes in the finder's branch only if it's small. Otherwise it's added to the right list below.

### What waits for what

The items most others wait for, most first. This is why the order below is what it is.

| Item | Waiting for it | Owner |
|---|---|---|
| **J2 `getrandom`** | J15 R3, J40 `random_device`, K25 ASLR, K29 random TCP numbers, K37 TLS | The-Jat, step B |
| **J15 R3 Rust's std** | J16, J21 R4, J22 R5, J25 R6 | The-Jat, step F |
| **J14 LLVM 23** | J15 R3, J21 R4 | The-Jat, step E |
| **J1 ELF thread-local storage** | J15 R3 | The-Jat, step B |
| **J3 `poll`** | J15 R3, K33 non-blocking sockets | The-Jat, step C |
| **J17-J20 TaajFS version 4** | J7 `stat` with times, J21 (rustc's big files), E1 `fsck` (written for the new format) | The-Jat, step D |
| **K1 R7, the real PC** | K3 a day on it, K4 its fixes, MaK's step C drivers | MaK, step B |
| **K13 AML interpreter** | K14 `_PRT`, K15 power button, K17 battery, K18 suspend | MaK, step E |
| **J43 shell variables and `if`** | J45 rebuilding only what changed, J25 R6's build scripts | The-Jat, filler |
| **K44 pseudo-terminals** | K45 job control | MaK, step H |
| **K11 GPT** | K12 the installer | MaK, step B |
| **J51 test programs out of the kernel image** | Every kernel feature, once the image nears its 3 MiB limit (about 680 KiB left) | The-Jat, filler; it moves to the front if less than 200 KiB is left |

Sizes: **S** is a day or less, **M** a few days, **L** a week or more.

## Now

| Machine | Step | Branch |
|---|---|---|
| The-Jat | Entry 098, two machines (this file, MaK's three fixes documented); then step B | `two-machines` |
| MaK | A: the scheduler and timing step | `timing` |

## The decided order

Each step below is one branch, one journal entry and one test64 run. The items are grouped into big steps, as the project does. When a step is merged, the machine takes its next one.

### The-Jat

| Step | Items | Why in this order |
|---|---|---|
| **A** Merge the core basics | entry 097 | **Done (entry 097)**, merged 2026-11-26 |
| **B** Std's ground, part 1: randomness, real thread-local storage, signals std uses | J2 `getrandom`, J1 ELF TLS, J4 `sigaltstack`, J10 `waitid` and SIGCHLD | J2 first: MaK's security and network steps wait for it |
| **C** Std's ground, part 2: libc calls std uses | J3 `poll`, J5 `dl_iterate_phdr`, J6 pthread attributes, J7 `stat`, J8 `realpath` and `getcwd`, J9 `posix_spawn` with fd 3+, J12 `dlopen` stubs, J13 zlib, J37 environment, J38 `execvp` and `/bin`, J39 the real `argv[0]` | Everything R3 will ask for, so R3 is only Rust's side |
| **D** TaajFS version 4 | J17 file times, J18 files over 256 MiB, J19 long names, J20 faster writes, J35 a replacing `rename` | One format change instead of four |
| **E** LLVM 23 | J14 | Long; runs unattended on The-Jat |
| **F** R3 Rust's std | J15, J16 | Needs B-E |
| **G** Memory for big programs | J26-J31 | Before rustc's 182 MB runs |
| **H** R4 rustc on TheTaaJ | J21 | |
| **I** R5 the build without cargo | J22, J23 (nasm; the bootloader change is MaK's) | |
| **J** R6 the milestone | J24, J25 | |
| Fillers, between big steps | J32-J34, J36, J40-J54 | When a long build runs, or for a short break |

### MaK

| Step | Items | Why in this order |
|---|---|---|
| **A** Scheduler and timing | K19 the race hunt (test64 under KVM 20 times), K20 priorities and CPU choice, K21 CPU time in `ps`, K24 fine timers (one-shot timer, `setitimer`), J53 the ticks mutation test | Needs only KVM; races found now are cheaper than on the real PC |
| **B** The real PC | K1 R7 (USB stick, BIOS and UEFI), K39 native resolution, K11 GPT, K7 x2APIC, then K3 a day on it | Whenever you can be at the PC; it may jump ahead of A |
| **C** Drivers the real PC needs | K2 USB hubs, K5 Intel I219, K6 Realtek, and whatever B found | After B shows which ones matter |
| **D** Network for real networks | K30 DHCP, K31 DNS, K32 link state, K33 loopback and UDP from C, K34 | DHCP is needed on any real network |
| **E** ACPI | K13 AML, K14 `_PRT`, K15 the power button, K16 a clean shutdown, K28 who may switch off | Real PCs need `_PRT` for many cards |
| **F** Security | K25 ASLR, K26 stack protector, K29 random TCP numbers | Needs The-Jat's B (`getrandom`) |
| **G** Disks | K8 NVMe, K9 AHCI, K10 a queue per disk, K12 an installer | |
| **H** Desktop | K40-K46 | |
| **I** Later | K17, K18, K27, K35-K38, K47-K48 | TLS waits for `getrandom`; sound is Era 10 |

The E list (either machine) is taken by whoever finishes a step and is waiting for the other.

---|---|---|
| The-Jat | Entry 098, two machines (this file, MaK's three fixes documented); then step B | `two-machines` |
| MaK | (free; pick from the MaK list) | |

---

## The-Jat: the road to rustc on TheTaaJ (Era 9), and what it needs

The order matters here: each item is needed by the ones after it.

### Before R3: what Rust's std will ask libc and the kernel for

| # | Item | Size | Notes |
|---|---|---|---|
| J1 | **Real thread-local storage (ELF TLS)**: `PT_TLS` loaded per thread, the FS base / `TPIDR_EL0` pointing at a proper TLS block, `__tls_get_addr` for the general model | M | Today only emulated TLS works (`emutls.c`); Rust's std uses `#[thread_local]` |
| J2 | **`getrandom`** in the kernel: RDRAND/RDSEED on x86, `RNDR` or virtio-rng on AArch64, mixed with timer jitter into a ChaCha20 generator; `getrandom()`, `getentropy()`, `/dev/urandom`-like call in libc | M | Also needed by MaK's ASLR and TLS work, and by TCP sequence numbers |
| J3 | **`poll`** (and `select` on top of it) over pipes, files, sockets and the keyboard | M | Needs "writable" readiness in `object_wait` (an open issue) |
| J4 | **`sigaltstack`** and running a handler on it; SIGSEGV on a stack overflow reaches that handler | S | rustc installs one for its own stack overflow report |
| J5 | **`dl_iterate_phdr`** (for the program itself only, no shared libraries) | S | std's backtraces and unwinding look for it |
| J6 | **`pthread_getattr_np`**, `pthread_attr_getstack`, `pthread_sigmask` | S | std's thread guard pages |
| J7 | **`statx` or a full `stat`**: sizes, types, times (once TaajFS has them, see J17) | S | `std::fs::metadata` |
| J8 | **`realpath`, `getcwd`, `fchdir`, `opendir` on handles** | S | std paths |
| J9 | **`posix_spawn` file actions for fd 3+** (E11 left over from Era 6) | M | `std::process::Command` with pipes |
| J10 | **`waitid`, `WNOHANG` everywhere, SIGCHLD** | S | `Command::try_wait` |
| J11 | **Wide characters**: `wchar_t` functions, `mbrtowc`, `wcrtomb` in UTF-8 | M | Also helps libc++'s missing parts |
| J12 | **`dlopen`/`dlsym` stubs answering "unsupported"** honestly | S | rustc probes for codegen backends |
| J13 | **zlib** ported (for rustc's metadata compression) | S | |
| J14 | **LLVM 23** for TheTaaJ (our port is 18; rustc 1.100 needs 23) | L | Redo E1-E4's port on the new version, byte-for-byte checks again |

### Era 9 proper

| # | Item | Size | Notes |
|---|---|---|---|
| J15 | **R3 Rust's std**: `x86_64-unknown-taaj` and `aarch64-unknown-taaj`, std over our libc, `panic = "abort"` first | L | Done when files, env, `Command`, threads and time work on both arches |
| J16 | **R3b unwinding**, if rustc needs it (libunwind for TheTaaJ, `.eh_frame_hdr`) | M | |
| J17 | **TaajFS metadata**: modification times (now there is a real clock), sizes in directory entries, `ls -l` | M | Needs a format version 4; `tfstool` follows |
| J18 | **TaajFS files over 256 MiB** (one more level of indirection) | M | LLVM 23 and rustc may outgrow it |
| J19 | **TaajFS names over 27 bytes** | M | Some AArch64 `.td` files are 28 |
| J20 | **Faster writes**: a write buffer per open file, one commit per `fsync`/close instead of per `write()` | M | rustc writes many small pieces |
| J21 | **R4 rustc runs on TheTaaJ**, cross-built with `x.py`; `taajfs.rlib` byte for byte as the host's | L | |
| J22 | **R5 a Makefile driving rustc without cargo**, checked against cargo on the host | L | |
| J23 | **R5 nasm ported**; the BIOS loader built with clang (`--target=i686-elf`) | M | The bootloader change itself is MaK's (rule 5); The-Jat ports nasm |
| J24 | **R6 our own ISO 9660 / El Torito writer** (a Rust program) | M | |
| J25 | **R6 the milestone**: `make` on TheTaaJ builds everything, the image boots and passes test64, and builds itself again to the same bytes | L | |

### Memory, also needed for big builds

| # | Item | Size | Notes |
|---|---|---|---|
| J26 | **`mprotect` and `PROT_NONE`** really change page permissions | S | Guard pages, JITs |
| J27 | **`MADV_DONTNEED`**: give pages back from inside a reservation | S | Big allocators (jemalloc-like) |
| J28 | **`MAP_SHARED` file mappings that write back** | M | |
| J29 | **Programs loaded page by page from their file** (not whole, not twice in memory) | M | A 182 MB rustc |
| J30 | **The unmap race**: a system call copying into a page another thread is unmapping | S | Known open issue since entry 096 |
| J31 | **Small malloc classes give memory back** | S | |

### C and C++ library, left over from Era 6

| # | Item | Size | Notes |
|---|---|---|---|
| J32 | `scanf`/`fscanf` from streams | S | Only `sscanf` today |
| J33 | `%a` in printf; long double done properly (`strtold`, `%Lf`) | M | |
| J34 | `erf`, `erfc`, `tgamma`, `lgamma` and the long double math | S | Declared, not defined |
| J35 | `rename` replaces an existing file atomically | S | TaajFS operation |
| J36 | libc descriptors follow the file, not the name (rename/remove of an open file) | M | |
| J37 | `getenv`/`setenv` with a real environment passed to children | S | Always NULL today |
| J38 | `execvp`, `PATH` search; a `/bin` directory | S | |
| J39 | `argv[0]` is the program's real name (not "program") | S | Fixes clang finding itself |
| J40 | libc++ `<filesystem>`, `random_device` (after J2), wide streams | M | |
| J41 | Double `free` and overrun detection in `malloc` (a debug mode) | S | |
| J42 | Host tests for `malloc`'s family | S | |

### Developer tools on TheTaaJ

| # | Item | Size | Notes |
|---|---|---|---|
| J43 | Shell: variables, `if`, `for`, `&&`, `||`, `;`, wildcards | M | Build scripts need them |
| J44 | Shell: `2>`, `2>&1`, here-documents, `exec` for the last command | S | |
| J45 | Rebuild only what changed in `/src/build` (make with real dependencies) | S | |
| J46 | Install the built libc and programs over `/lib` and the root | S | |
| J47 | `cp -r`, `rm -r`, `mv`/`cp` with many names, `find`, `grep`, `diff`, `wc`, `sort`, `head`, `tail` | M | |
| J48 | A pager (`less`-like) and per-command help | S | `help` is at the screen's limit |
| J49 | Editor: undo, selection, copy and paste, search, go to line, line wrap | M | |
| J50 | `git`-like tool, or at least `patch` | L | Later |

### Test suite health

| # | Item | Size | Notes |
|---|---|---|---|
| J51 | Test programs out of the kernel image (onto the test disk) | M | The kernel image has ~680 KiB left under 3 MiB |
| J52 | A test that fails when rule 1 or rule 3 of `threads.rs` is broken | S | Known gap |
| J53 | A test for the ticks mutation from entry 097 (late timer interrupts) | S | Perhaps by stopping a CPU for a while |
| J54 | A fresh `git clone --recurse-submodules` builds and passes (in a clean container) | S | Never tried |

---

## MaK: real hardware, speed, and the machine itself

### Real hardware first (Era 7's milestone)

| # | Item | Size | Notes |
|---|---|---|---|
| K1 | **R7: boot from a USB stick on the real PC** (BIOS and UEFI), write down every failure | M | Needs you at the machine; the flushes and clock from entry 097 make it safer |
| K2 | **USB hubs** (xHCI behind a hub: ports, power, reset through the hub) | M | On real PCs keyboards and sticks are often behind one |
| K3 | **Use TheTaaJ for a day** on the real PC (Era 7's "done when"), noting what hurts | L | |
| K4 | Fix whatever K1-K3 find (likely: ACPI tables, interrupt routing, the screen mode, the disk controller) | L | |
| K5 | **Intel I219 and later onboard network cards** (e1000e) | M | The common onboard card |
| K6 | **Realtek RTL8111/8168** network card | M | The other common one |
| K7 | **x2APIC** and APIC ids above 255 | S | Bigger real machines |
| K8 | NVMe: shutdown notification (CC.SHN), more than one command at a time, 4K blocks | M | |
| K9 | AHCI: NCQ, more than one disk, 4Kn disks | M | |
| K10 | Only one disk request in flight → a queue per disk | M | Speed on real disks |
| K11 | GPT partitions, and finding the TaajFS partition on a real disk | S | |
| K12 | Install TheTaaJ onto the internal disk (an installer program) | M | After K11 |

### ACPI and power

| # | Item | Size | Notes |
|---|---|---|---|
| K13 | **An AML interpreter** (a small one, enough for `_S5`, `_PTS`, `_PRT`) | L | |
| K14 | **Interrupt routing from `_PRT`** for PCI cards | M | After K13 |
| K15 | **The power button** shuts down cleanly (services asked, disks flushed) | S | After K13 |
| K16 | Shutdown asks services to finish instead of killing them; NVMe and AHCI flushed | S | |
| K17 | Battery level (laptops), lid switch | M | Later |
| K18 | Suspend to RAM (S3) | L | Later |

### Speed and timing (KVM shows these first)

| # | Item | Size | Notes |
|---|---|---|---|
| K19 | **Run `make test64` under KVM repeatedly** (say 20 times) and fix every race found | M | KVM already found two |
| K20 | Process priorities and choosing a thread's CPU (affinity) | M | Four busy programs make typing sluggish |
| K21 | CPU time per process in `ps` | S | |
| K22 | A transmit-interrupt serial writer (AArch64 output capped at ~3200 bytes/s) | S | |
| K23 | The faster boots: measure boot time on KVM and on the real PC, cut the slow parts | S | |
| K24 | Timer: alarms and sleeps finer than a 10 ms tick (one-shot timer, `setitimer`, `timer_create`) | M | |

### Security

| # | Item | Size | Notes |
|---|---|---|---|
| K25 | **ASLR**: random program, stack and mmap addresses (after J2) | M | |
| K26 | Stack protector and stack probes in C programs | S | |
| K27 | An IOMMU (VT-d) confining each driver's device | L | |
| K28 | Only a chosen program may switch the machine off (not anyone with init's channel) | S | |
| K29 | A random TCP initial sequence number (after J2) | S | |

### Network (Era 12's ground)

| # | Item | Size | Notes |
|---|---|---|---|
| K30 | **DHCP** | S | Needed on any real network |
| K31 | **DNS** (`getaddrinfo`) | M | |
| K32 | Link state noticed (cable unplugged) | S | |
| K33 | Loopback (`127.0.0.1`), UDP and non-blocking sockets from C | M | |
| K34 | IP fragments, ICMP errors to programs | S | |
| K35 | TCP: window scaling, SACK, timestamps, keepalive; a lossy-network boot test | M | |
| K36 | IPv6 | L | Later |
| K37 | TLS 1.3 with its cryptography | L | After J2 |
| K38 | HTTP server: several connections, keep-alive | S | |

### Desktop

| # | Item | Size | Notes |
|---|---|---|---|
| K39 | The screen at the real PC's native resolution (UEFI GOP and VESA modes chosen well) | S | |
| K40 | Windows: resize, minimise, a task bar | M | |
| K41 | Copy and paste between windows | S | |
| K42 | Key up/down and lone Shift/Control reach windows | S | |
| K43 | A font with more than ASCII (UTF-8 drawn, not '?') | M | |
| K44 | The terminal as its own program; pseudo-terminals | M | |
| K45 | Job control: process groups, Ctrl+Z, `fg`, `bg`, SIGTSTP/SIGCONT | M | |
| K46 | Scroll wheel, more mouse buttons, pointer acceleration | S | |

### Sound (Era 10's ground)

| # | Item | Size | Notes |
|---|---|---|---|
| K47 | Intel HD Audio driver and an audio service | L | Testable in QEMU (`-device intel-hda`) |
| K48 | WAV playback, then a microphone recorded to a file | M | |

---

## Either machine: whoever is free

| # | Item | Size | Notes |
|---|---|---|---|
| E1 | `fsck` for TaajFS: check and repair a damaged disk | M | |
| E2 | Services that crash on one "poisoned" request again and again: give up on that request | S | |
| E3 | A watchdog for the shell, init and the registry | S | |
| E4 | A redirected file written in one commit, not one per message | S | Part of J20 |
| E5 | Swap to a disk file | L | Later |
| E6 | TheKernel standalone (not needing `heap`, `virtio`, `console`, `user/` from the parent) | M | |
| E7 | The 32-bit kernel booted again, or retired officially | S | Decide with the user |
| E8 | Users' documentation: a "how to use TheTaaJ" page for the website | S | |

## Waiting for you (the user)

| # | Item | Notes |
|---|---|---|
| U1 | Era 5's milestone: write one journal entry by hand in `edit` on TheTaaJ | |
| U2 | Merge `core-basics` into `main` (after test64 passes) | |
| U3 | Go-ahead for Era 9's R3 on The-Jat | |
| U4 | Be at the real PC for K1 (R7) | |
| U5 | Era 8, the AI engine: agree its plan (`docs/ai-assistant.md`) before any work | Then it splits: the engine (SIMD, tensors, GGUF) suits The-Jat's long runs; testing speed on the real CPU suits MaK |
| U6 | Raspberry Pi and RISC-V: only if you want them | |

## Later eras, as drafted in `docs/ai-assistant.md`

- **Era 8 · AI engine**: XSAVE/AVX2, a tensor crate, stories15M then Qwen3 on TheTaaJ.
- **Era 9 · Assistant** (it will need a new number, since Era 9 is now rustc): the `infer` service, tools published by services, confirmation, undo.
- **Era 10 · Sound**, **Era 11 · Voice**, **Era 12 · Web** (DNS, TLS, HTML to text).
