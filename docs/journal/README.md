# TheTaaJ Journal

The story of building TheTaaJ, one step at a time: what each step is, why it was needed, how it works, what went wrong on the way, and how to test it. Each entry ends with a **testing note** — the exact things to type, and what should appear.

Entries are numbered in the order the work happened. Some earlier steps were built before their entries were written; they are listed here with the commit that holds the work, and will be filled in.

| # | Entry | Status | Commit |
|---|---|---|---|
| 001 | [Where it is, where it's going — vision and architecture](001-vision-and-architecture.md) | written | `0ccd11e` |
| 002 | The 64-bit Rust kernel, booted by the same bootloader | to write | `e1b9aae` |
| 003 | Own GDT, TSS and IDT; exception handlers | to write | `379ce73` |
| 004 | Page tables, the frame allocator and the heap — tested in three layers | to write | `eb2e6e3` |
| 005 | Booting via UEFI too, and boot tests that stop when the kernel is done | to write | `d8f978b`, `633fa10` |
| 006 | Interrupts: ACPI, the APIC timer, the I/O APIC and the keyboard | to write | `ab7e2cd` |
| 007 | Threads: context switch, preemptive round-robin, wait queues | to write | `531371a` |
| 008 | User mode: ring 3 processes, system calls, ELF loading | to write | `ed9f9c5` |
| 009 | IPC: capabilities, handle tables and channels | to write | `7546291` |
| 010 | The keyboard driver moves to user mode; init, a supervisor that restarts crashed services | to write | `f5a92b1`, `598114a` |
| 011 | Memory objects, and a text console drawn in ring 3 | to write | `004600a`, `3490095` |
| 012 | A shell with commands | to write | `5e971db` |
| 013 | A heap for user programs, and a file service in memory | to write | `779ff6d` |
| 014 | The disk: PCI, MSI-X, DMA memory and a virtio block driver in user space | to write | `f0d7718` |
| 015 | Files on the disk (TaajFS in the file service), and programs loaded from it | to write | `9d1702b`, `3fc4bf1` |
| 016 | [Programs that take arguments and use files](016-programs-with-arguments-and-files.md) | written | `ca4ac6d`, `ceb5780` |
| 017 | [Kill: ending a program from outside](017-kill.md) | written | `5755c9e` |
| 018 | [A faster disk: shared memory and a block cache](018-a-faster-disk.md) | written | `f11bbb2` |

## How to run what the entries describe

```
make run64        # boot in QEMU (BIOS), with a persistent disk: build/taaj-disk.img
make run64-uefi   # the same, booted by UEFI firmware
make test64       # every automated test: host unit tests, BIOS and UEFI boots, fault boots
```

`make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from.
