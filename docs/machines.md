# The machines, and the order decided for each

*Part of the work list: [`docs/who-does-what.md`](who-does-what.md).*

## The machines

Measured on MaK-WSL on 2026-11-25 and on TheJat-VM on 2026-11-26. TheJat-WSL's hardware was read on 2026-11-26; its times aren't measured yet (no built tree).

### Hardware and setup

| | **MaK-WSL** | **TheJat-VM** | **TheJat-WSL** |
|---|---|---|---|
| Machine | A laptop: 13th Gen Intel Core i7-13620H | A VMware virtual machine on an Intel Core Ultra 9 275HX | WSL2 on the Intel Core Ultra 9 275HX that also runs TheJat-VM |
| CPU | 10 cores (6 performance + 4 efficient), 16 threads | 16 virtual CPUs | 24 CPUs |
| Vector instructions | AVX2, no AVX-512 | AVX2, no AVX-512 | AVX2, no AVX-512 |
| Memory | 11 GiB given to WSL (WSL's default share; no `.wslconfig`) | 15 GB | 15 GiB given to WSL (no `.wslconfig`) |
| Disk | 1 TB virtual disk, 917 GB free | 167 GB, 103 GB free | 1 TB virtual disk, 912 GB free |
| Virtualization | KVM in WSL2 (VT-x, nested) | None: every QEMU boot is emulated | KVM in WSL2 |
| OS | Ubuntu 26.04 on WSL2 (kernel 6.6) | Ubuntu 24.04.4 (kernel 7.0) | Ubuntu 26.04 on WSL2 (kernel 6.6) |
| Host LLVM | 21 by default; Ubuntu 24.04's 18.1.3 unpacked locally for the byte-for-byte tests | 18.1.3, the reference | 21 (clang 21.1.8); no 18.1.3 yet, so the byte-for-byte tests need it unpacked first |
| Host glibc | 2.43 (newer than the reference) | 2.39, the reference | 2.43 (newer than the reference) |
| AArch64 boots | Emulated | Emulated | Emulated |

### Measured times

| Task | **MaK-WSL, KVM** | **MaK-WSL, emulated** | **TheJat-VM** |
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

TheJat-VM's emulated boots are a little faster than MaK-WSL's emulated ones (a newer CPU, and nothing else on it). The whole of `make test64` is about as long on both machines, because its slowest parts are the AArch64 ones, emulated everywhere. KVM makes the x86_64 half several times faster.

### What follows for the split

| Kind of work | Better on | Why |
|---|---|---|
| Fast edit, boot and test loops on x86_64 | MaK-WSL | KVM: a busy boot is up to ten times faster |
| Timing races, real-PC behaviour | MaK-WSL | KVM delivers interrupts in real time, as a PC does; it found the bootloader's protected-mode bug |
| Real hardware (USB stick, R7) | MaK-WSL | It is the real PC |
| Byte-for-byte comparisons with the host's tools (LLVM, Clang, glibc) | TheJat-VM | It has the reference versions; MaK-WSL's newer host needs local workarounds |
| Long unattended builds (rustc, LLVM 23) | TheJat-VM | MaK-WSL is a laptop: it can sleep, run on battery or slow down when hot, and WSL stops when idle; and it has 4 GB less memory |
| Big memory (building rustc, models of 1-3 GB) | TheJat-VM | 15 GB against 11 GiB |
| AArch64 | Either | Emulated on both, about as fast |

MaK-WSL's memory can grow: a `.wslconfig` gives WSL more of the laptop's memory, if the laptop has it to spare.

## The decided order

**From 2026-11-27 the order is per lane, in `docs/lanes.md`.** The steps below are what each machine did until then. Their unfinished items are in the lanes: MaK-WSL's step A continues as `sched`, B and C as `devices`' real-PC items, D as `network` (now TheJat-WSL's), E as `devices` (K28 in `services`), F as `boot`, `libc` and `network`, G as `devices`, H as `desktop`, I as `devices` and `network`. TheJat-VM's fillers are in `libc`, `ci` and `files`; its step G's J29 and J58 in `memory`. TheJat-VM's step E (LLVM 23) stays paused until the user says.

Each step below is one branch, one journal entry and one test64 run. The items are grouped into big steps, as the project does. When a step is merged, the machine takes its next one.

### TheJat-VM

| Step | Items | Why in this order |
|---|---|---|
| **A** Merge the core basics | entry 097 | **Done (entry 097)**, merged 2026-11-26 |
| **B** Std's ground, part 1: randomness, real thread-local storage, signals std uses | J2 `getrandom`, J1 ELF TLS, J4 `sigaltstack`, J10 `waitid` and SIGCHLD | **Done (entry 099)** |
| **C** Std's ground, part 2: libc calls std uses | J3 `poll`, J5 `dl_iterate_phdr`, J6 pthread attributes, J7 `stat`, J8 `realpath` and `getcwd`, J9 `posix_spawn` with fd 3+, J12 `dlopen` stubs, J13 zlib, J37 environment, J38 `execvp` and `/bin`, J39 the real `argv[0]` | **Done (entry 100)** |
| **D** TaajFS version 6 (versions 4 and 5 came in Era 6) | J17 file times, J18 files over 256 MiB, J19 long names (entry 094), J20 faster writes (entry 093), J35 a replacing `rename`, J55 `/bin` | **Done (entry 101)**; J55's moving of the programs stays open |
| **E** LLVM 23 | J14, and C++ (libc++, the LLVM port) moved from emulated to real thread-local storage | Long; runs unattended on TheJat-VM; it rebuilds the ports anyway. libc++ 23 needs a host clang of 21 or later, so a host LLVM 23.1.5 is built first (`tools/build-host-llvm.sh`, with cmake and ninja) |
| **F** R3 Rust's std | J15, J16 | Needs B-E |
| **G** Memory for big programs | J26-J31 | Before rustc's 182 MB runs. **Part 1 done (entry 102)**: J26, J27, J30, J31; **part 2 done (entry 103)**: J28. J29 needs sealed memory objects first (J58). Done before E because E waits |
| **H** R4 rustc on TheTaaJ | J21 | |
| **I** R5 the build without cargo | J22, J23 (nasm; the bootloader change is MaK-WSL's) | |
| **J** R6 the milestone | J24, J25 | |
| Fillers, between big steps | J32-J34, J36, J40-J54 | When a long build runs, or for a short break |

### MaK-WSL

| Step | Items | Why in this order |
|---|---|---|
| **A** Scheduler and timing | K19 the race hunt (test64 under KVM 20 times), K20 priorities and CPU choice, K21 CPU time in `ps`, K24 fine timers (one-shot timer, `setitimer`), J53 the ticks mutation test | Needs only KVM; races found now are cheaper than on the real PC |
| **B** The real PC | K1 R7 (USB stick, BIOS and UEFI), K39 native resolution, K11 GPT, K7 x2APIC, then K3 a day on it | Whenever you can be at the PC; it may jump ahead of A |
| **C** Drivers the real PC needs | K2 USB hubs, K5 Intel I219, K6 Realtek, and whatever B found | After B shows which ones matter |
| **D** Network for real networks | K30 DHCP, K31 DNS, K32 link state, K33 loopback and UDP from C, K34 | DHCP is needed on any real network |
| **E** ACPI | K13 AML, K14 `_PRT`, K15 the power button, K16 a clean shutdown, K28 who may switch off | Real PCs need `_PRT` for many cards |
| **F** Security | K25 ASLR, K26 stack protector, K29 random TCP numbers | Needs TheJat-VM's B (`getrandom`) |
| **G** Disks | K8 NVMe, K9 AHCI, K10 a queue per disk, K12 an installer | |
| **H** Desktop | K40-K46 | |
| **I** Later | K17, K18, K27 (Era 15), K35-K38, K47-K48 | TLS waits for `getrandom`; sound is Era 10 |

The E list (either machine) is taken by whoever finishes a step and is waiting for the other.
