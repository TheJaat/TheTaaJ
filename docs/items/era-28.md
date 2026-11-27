# Era 28 · Portability: every architecture

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**The goal (the user's, 2026-11-26): TheTaaJ runs on every architecture in the world that can run it - fully portable.** "Can run it" means a CPU with a memory management unit: a microkernel's isolation is separate address spaces, and a CPU without an MMU (most microcontrollers) cannot give them. Within that, nothing in TheTaaJ may assume one CPU's word size, byte order, page size, page-table shape, firmware, interrupt controller, timer or cache behaviour.

**Done when**: (1) the kernel, libc and every service build for each architecture below from the same sources, with what differs only behind `arch/` (kernel), `libc`'s small per-CPU files, and the toolchain's target; (2) each tier's architectures pass what their tier promises, in our CI (Era 25), every time; (3) at least one big-endian architecture and one with a page size other than 4 KiB pass, proving those assumptions gone; (4) a written porting guide lets a new architecture be added without changing anything outside `arch/` and the per-CPU files. Whose it is, and when, is for the user to say. Pointed to: U6 (a real board), E7 (the 32-bit kernel), K135 (Era 21), J93 (Era 25).

**Tiers**, each tested in QEMU (which emulates every one of them) and on real hardware where there is some:

| Tier | Architectures | What it promises |
|---|---|---|
| **1** | x86_64, AArch64 | All of `make test64`, before every merge (today's state) |
| **2** | RISC-V 64 (RV64GC), LoongArch64, PowerPC64 (little-endian) | Boot to the shell, the core tests (threads, memory, IPC, files, network), in CI |
| **3** | s390x (big-endian), PowerPC64 big-endian, MIPS64, SPARC64 | Boot to the shell and the core tests in CI; they prove byte order, page size and firmware are behind `arch/` |
| **32-bit** | i686, ARMv7-A, RISC-V 32 | Only if the user decides (E7): the vision is 64-bit, and a 32-bit address space limits what TheTaaJ can do; but the kernel can be made word-size clean so it is possible |

What is right: one kernel for x86_64 and AArch64, with what differs behind `arch/`; one page-table implementation with a `Format` per CPU (`paging.rs`); the TaajBoot protocol and the device tree keep the kernel ignorant of firmware; every on-disk format and every protocol is little-endian at explicit offsets, so files and messages are already the same on any CPU; virtio drivers over PCI and MMIO; Rust and LLVM target nearly every architecture; about half of `make test64` runs on both CPUs.

What stands in the way, found in the code:

- **4 KiB pages are fixed**: `PAGE_SIZE = 4096` in `abi` and in the kernel, and 4096 written in `user/` (the allocator, threads, windows). AArch64 machines often use 16 or 64 KiB, PowerPC 64 KiB, LoongArch 16 KiB.
- **Four page-table levels of 512 entries are fixed** (`paging.rs`): RISC-V's Sv39 has three, Sv57 and x86's LA57 five; PowerPC's hashed page table and s390x's region tables have other shapes.
- **Little-endian is assumed in places** besides the formats (code that reads a word in memory as bytes).
- **Two CPUs' assembly in libc** (`setjmp.c`, `fenv.c`, the thread pointer) and in the runtime, each ending in `#error` for anything else.
- **Interrupts, timers and SMP start-up** are written for the APIC and the GIC (and PSCI); RISC-V has the PLIC or AIA with SBI, LoongArch its own, PowerPC XIVE, s390x neither.
- **DMA is assumed cache-coherent**, true on PCs, not on many ARM and RISC-V boards: drivers must flush and invalidate.
- **The LLVM port** keeps only the ARM, PowerPC and RISC-V targets' descriptions (`tools/import-llvm.sh`), and the toolchain is built for two.
- **The x86-only parts**: the PCI drivers and some tests (AArch64's PCIe bus is unused).

All of it stays the kernel's mechanism behind `arch/` and a few per-CPU files; nothing above the kernel may know which CPU it is on, beyond what `abi` tells it (the page size among it).

| # | Item | Size | Notes |
|---|---|---|---|
| K160 | **The port contract written down**: what `arch/` must provide (entry, context switch, page-table format and levels, TLB, interrupts, timer, SMP start, cache maintenance, the console), what libc's per-CPU files must, and a checklist for a new architecture | S | First. The rest is measured against it |
| K161 | **The page size a parameter**: read from `abi` at run time by programs, a constant per architecture in the kernel; every 4096 in `user/`, libc and the services removed; tested at 16 KiB and 64 KiB on AArch64 | M | Early: touches everything |
| K162 | **Page tables of any shape**: the number of levels and entries per level part of each CPU's `Format` (3 for Sv39, 5 for LA57 and Sv57), and room for CPUs whose tables are not a tree | M | With K161 |
| K163 | **Byte-order clean**: every place that reads memory as bytes goes through the explicit little-endian helpers; a big-endian architecture (s390x or PowerPC64 BE) in CI to keep it so | M | Proven by Tier 3 |
| K164 | **Non-coherent DMA**: memory objects for DMA say whether the device is coherent; drivers flush before a device reads and invalidate after it writes, through one `arch/` call | M | With Era 15's K80; for ARM and RISC-V boards |
| K165 | **Interrupt controllers and timers behind one interface** in `arch/`, so a new one (PLIC/AIA, LoongArch's, XIVE) is one file | M | With Era 15's K79 |
| K166 | **The toolchain for every target**: host LLVM and our port built with every Tier 1-3 target, Rust's `no_std` targets for each, libc's per-CPU files (`setjmp`, `fenv`, the thread pointer) for each | M | |
| K167 | **PCIe on AArch64** (and on every architecture with it): the bus from the device tree, so AHCI, NVMe, e1000 and xHCI run there with their tests | M | After K75 (Era 15) |
| K168 | **Every Tier 1 test on both Tier 1 CPUs**, or a written reason why not | M | After K167 |
| K169 | **RISC-V 64**: `arch/riscv64` (Sv39/Sv48, the PLIC or AIA, SBI timers and SMP), booted by OpenSBI with U-Boot or our UEFI loader | L | The first Tier 2 port; it tests the contract |
| K170 | **LoongArch64**: `arch/loongarch64`, booted by UEFI | L | Tier 2 |
| K171 | **PowerPC64 (little-endian)**: `arch/ppc64` (radix page tables on POWER9 and later, XIVE), booted by QEMU's `pseries` or OPAL | L | Tier 2; the big-endian build is Tier 3 |
| K172 | **s390x**: big-endian, its own I/O (channel subsystem, virtio-ccw), its own boot (IPL) | L | Tier 3; the strongest test of the contract |
| K173 | **MIPS64 and SPARC64** | L | Tier 3, later |
| K174 | **Real boards**: a Raspberry Pi 4 or 5, a RISC-V board (VisionFive 2 or similar), whatever the user has | L | U6: only if the user wants them |
| K175 | **A 32-bit build** (i686, ARMv7-A, RV32) of the same kernel, if the user decides to (E7) | L | Decide with the user |
