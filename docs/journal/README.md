# TheTaaJ Journal

The story of building TheTaaJ, one step at a time: what each step is, why it was needed, how it works, what went wrong on the way, and how to test it. Each entry ends with a **testing note** — the exact things to type, and what should appear.

Since entry 027 the Rust kernel and `abi` live in the kernel repository: where an older entry says `kernel64/…` or `abi/…`, the path is now `kernel/kernel64/…` or `kernel/abi/…`.

Entries are numbered in the order the work happened. Entries 002–015 were written after the work, from the code and the commit history; each is listed with the commit that holds the work.

| # | Entry | Status | Commit |
|---|---|---|---|
| 001 | [Where it is, where it's going — vision and architecture](001-vision-and-architecture.md) | written | `0ccd11e` |
| 002 | [The 64-bit Rust kernel, booted by the same bootloader](002-the-64-bit-rust-kernel.md) | written | `e1b9aae` |
| 003 | [Own GDT, TSS and IDT; exception handlers](003-gdt-tss-idt-and-exceptions.md) | written | `379ce73` |
| 004 | [Page tables, the frame allocator and the heap — tested in three layers](004-memory-page-tables-frames-heap.md) | written | `eb2e6e3` |
| 005 | [Booting via UEFI too, and boot tests that stop when the kernel is done](005-uefi-boot-and-boot-tests.md) | written | `d8f978b`, `633fa10` |
| 006 | [Interrupts: ACPI, the APIC timer, the I/O APIC and the keyboard](006-interrupts.md) | written | `ab7e2cd` |
| 007 | [Threads: context switch, preemptive round-robin, wait queues](007-threads.md) | written | `531371a` |
| 008 | [User mode: ring 3 processes, system calls, ELF loading](008-user-mode.md) | written | `ed9f9c5` |
| 009 | [IPC: capabilities, handle tables and channels](009-ipc-capabilities-and-channels.md) | written | `7546291` |
| 010 | [The keyboard driver moves to user mode; init, a supervisor that restarts crashed services](010-user-mode-keyboard-driver-and-init.md) | written | `f5a92b1`, `598114a` |
| 011 | [Memory objects, and a text console drawn in ring 3](011-memory-objects-and-a-ring-3-console.md) | written | `004600a`, `3490095` |
| 012 | [A shell with commands](012-a-shell-with-commands.md) | written | `5e971db` |
| 013 | [A heap for user programs, and a file service in memory](013-a-user-heap-and-a-file-service.md) | written | `779ff6d` |
| 014 | [The disk: PCI, MSI-X, DMA memory and a virtio block driver in user space](014-the-disk.md) | written | `f0d7718` |
| 015 | [Files on the disk (TaajFS in the file service), and programs loaded from it](015-files-on-the-disk-and-programs-from-it.md) | written | `9d1702b`, `3fc4bf1` |
| 016 | [Programs that take arguments and use files](016-programs-with-arguments-and-files.md) | written | `ca4ac6d`, `ceb5780` |
| 017 | [Kill: ending a program from outside](017-kill.md) | written | `5755c9e` |
| 018 | [A faster disk: shared memory and a block cache](018-a-faster-disk.md) | written | `f11bbb2` |
| 019 | [Faster text: a display that draws only what changed](019-faster-text.md) | written | `fb49716` |
| 020 | [The console fills the screen](020-the-console-fills-the-screen.md) | written | `d6beacd` |
| 021 | [What a program is given, and a client that does not wait forever](021-what-a-program-is-given.md) | written | `9039a12` |
| 022 | [Read-only files, a service stalled on purpose, and a kernel too big to boot](022-read-only-files-a-stalled-service-and-a-kernel-too-big-to-boot.md) | written | `4fe68a7` |
| 023 | [A watchdog: replacing a service that has stopped answering](023-a-watchdog.md) | written | `d4ff7ed` |
| 024 | [Every service watched: pings for the display and the keyboard driver, and four hangs on purpose](024-every-service-watched.md) | written | `6dc5831` |
| 025 | [Last words: the kernel says why it stopped, and a screen that survives its display](025-last-words.md) | written | `5d8c055` |
| 026 | [Directories: a tree of names, changed without ever writing over anything](026-directories.md) | written | `34c5aca` |
| 027 | [The kernel goes home: three repositories, each with its own work](027-the-kernel-goes-home.md) | written | `9a9a52c` |
| 028 | [The current directory: a place to stand that the file service never hears about](028-the-current-directory.md) | written | `09caaef` (TheKernel `73b2ab2`) |
| 029 | [Moving and copying: one is a single commit, the other cannot be](029-moving-and-copying.md) | written | `bdb77dc` (TheKernel `4a18915`) |
| 030 | [Quoting: who decides where one argument ends](030-quoting.md) | written | `6a1d21d` (TheKernel `9ec61a9`) |
| 031 | [Standard output, standard input, and redirection: a program that does not know where its words go](031-standard-output-input-and-redirection.md) | written | `7bb1162` (TheKernel `4330b55`) |
| 032 | [Pipes: two programs, one channel, and a shell that lets go](032-pipes.md) | written | `7a2e05d` (TheKernel `36b4ffd`) |
| 033 | [Background jobs: everything the shell runs is a job, and one loop looks after them all](033-background-jobs.md) | written | `f79925e` (TheKernel `41dd0ba`) |
| 034 | [The service registry: asking for a service by name, on a channel that decides what names exist](034-the-service-registry.md) | written | `2206f6b` (TheKernel `8e3ff1c`) |
| 035 | [Namespaces for programs: `ls` asks for "files", and the shell decides what that means](035-namespaces-for-programs.md) | written | `3b02b2f` (TheKernel `fab4327`) |
| 036 | [The first C program: what has to exist before `main`](036-the-first-c-program.md) | written | `fc11db8` (TheKernel `055c353`) |
| 037 | [Standard error: a second way out](037-standard-error.md) | written | `e85dc51` (TheKernel `2142a4a`) |
| 038 | [Memory and strings: `malloc`, `free`, and the sixteen bytes in front](038-memory-and-strings.md) | written | `a78fa19` |
| 039 | [`printf`: the first C in the C library](039-printf.md) | written | `78c7404` (TheKernel `ca699d7`) |
| 040 | [File descriptors: where an open file lives when nobody keeps it](040-file-descriptors.md) | written | `5c5c43c` (TheKernel `895dc42`) |
| 041 | [Streams: a buffer in front of a descriptor](041-stdio-on-files.md) | written | this step (TheKernel `8145921`) |

## How to run what the entries describe

```
make run64        # boot in QEMU (BIOS), with a persistent disk: build/taaj-disk.img
make run64-uefi   # the same, booted by UEFI firmware
make test64       # every automated test: host unit tests, BIOS and UEFI boots, fault boots
```

`make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from.
