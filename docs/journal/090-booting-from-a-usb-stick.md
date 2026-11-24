# Booting from a USB stick

*Journal entry #90 · Era 7, step R6b. `make stick64` makes `build/stick64.img`. Written to a USB stick, it boots TheTaaJ under a PC BIOS and under UEFI, and keeps the files on the stick. It is the CD image plus a few things:*

- *a master boot record of its own;*
- *the EFI system partition, as a partition;*
- *a TaajFS partition.*

*The BIOS loader now reads its disk through the BIOS (int 13h) instead of driving an IDE CD drive itself. That is what makes the stick readable, and it also lets the loader boot from a SATA CD drive, which it could not before. fs finds TaajFS in a partition.*

## Why

The goal of Era 7 is "it boots from a USB stick on my PC and I use it for a day". Entry 089 made a stick the system's disk once the kernel runs. This entry is the other half: getting from the firmware to the kernel with nothing but the stick plugged in.

The BIOS path had a deeper problem too. stage2 found the kernel by talking to an IDE CD drive through I/O ports 0x1F0 and 0x170 (ATAPI). That works in QEMU's default PC, which has an IDE controller. A real PC from the last fifteen years has no IDE at all: its CD drive, if it has one, is SATA (AHCI), and a USB stick is behind an xHCI controller. The old loader could boot neither, and the open list in PROJECT-CONTEXT had "BIOS loader via int 13h (SATA CD boot)" on it for that reason.

## What it is

```
 build/stick64.img (written to the stick as it is)

 sector 0        MBR: mbr.asm's code; at 0x1B0 where stage1 is; the partition table
 32 KiB ...      partition 1, type 0x17, active: the ISO 9660 file system (stage1, stage2,
                 kernel.elf, efiboot.img) - the same bytes the CD has
 ...             partition 2, type 0xEF: the EFI system partition (BOOTX64.EFI, kernel.elf)
 ...             partition 3, type 0x7F: a TaajFS volume (the programs, your files)
```

- **TheBootloader:**
  - `src/boot/stage1/mbr.asm` (new): a BIOS runs it from the stick's first sector. It moves itself out of the way, loads stage1's first 2 KiB from where the image says, and jumps to it: what El Torito does for a CD.
  - `headers/boot/disk.inc`: `DiskSectorSize` asks the BIOS how big the boot disk's sectors are: 2048 for a CD, 512 for a stick or a hard disk. When they are 512, every ISO 9660 sector read is four of the disk's.
  - stage1 calls `DiskSectorSize` before it reads anything.
  - stage2:
    - `bios_read` (in `stage2.asm`) is the way down to real mode and back, so 32-bit C can call int 13h;
    - `bios_disk.c` (new) has `read_sector` on top of it;
    - the ATAPI driver (`ata.c`, `ISO9660.c`, 1100 lines) is gone, and stage2.bin shrank from 30,561 to 20,293 bytes.
- **TheTaaJ:**
  - Makefile:
    - `hybrid64`, one xorriso command for both images;
    - `iso64` is now a hybrid too, so the CD image boots from a stick (without a TaajFS partition);
    - `stick64` adds the TaajFS partition, from `build/stick-disk.img`;
    - `run64-stick` and `run64-stick-uefi` boot it in QEMU from a USB stick;
    - `stick64-test` (in `make test64`).
  - `taajfs::partition` (new): `find_taajfs`, the first MBR partition of type 0x7F; 2 host tests.
  - fs: reads sector 0, and mounts TaajFS from the partition if sector 0 lists one, else from sector 0 as before.
  - tfstool: the same, so `tfstool get build/stick64.img name` works on a stick image.
- **TheKernel:** `stick-test.sh`, three machines (below). The kernel itself did not change.

## How a PC boots from a stick

**Under a BIOS.** The BIOS reads the disk's first sector, the master boot record, to 0x7C00 and jumps to it with the drive number in DL. That sector is 512 bytes:

- 446 bytes of code;
- the partition table, four 16-byte entries: an active flag, a type, the first sector, a count;
- 0x55 0xAA.

Some BIOSes boot only a disk whose table has an active partition, so partition 1 is marked active.

The image is an ISO 9660 CD image first. ISO 9660 leaves its first 32 KiB unused (the "system area"), which is why the same file can also start with an MBR. xorriso writes the partition table and stage1's position (at 0x1B0, in 512-byte sectors, ISOLINUX's convention), and takes the code from `mbr.bin`.

`mbr.asm` is short, but it has one trap. The BIOS loads it at 0x7C00, and stage1 must go to 0x7C00 too. So the first thing it does is copy itself to 0x0600 and jump to the copy. Then it reads stage1 over the place it came from.

**Two sector sizes.** On a CD a sector is 2048 bytes, and ISO 9660 counts in those. On a stick a sector is 512 bytes, and the BIOS counts in those. stage1 and stage2 still think in ISO sectors. `DiskSectorSize` asks int 13h function 48h (bytes 0x18–0x19 of its answer). When the answer is 512, `ReadFromDiskUsingExtendedBIOSFunction` multiplies the sector number and the count by four. That one change in one shared routine is all stage1 needed.

**int 13h from 32-bit code.** stage2's C code runs in 32-bit protected mode, and the BIOS only works in real mode. `bios_read` therefore goes:

1. 32-bit, to 16-bit protected mode (CODE16_DESC), because real mode can only be entered from 16-bit code;
2. protection off, a far jump to set CS to 0, the segment registers 0, the real-mode interrupt table (`lidt Idt16`), interrupts on;
3. `int 0x13`, function 42h, into a 2 KiB buffer at 0x60000 (the BIOS can only reach the first megabyte);
4. interrupts off, protection on, a far jump back into 32-bit code, the old ESP back.

Two constraints make this work. The stack is below 64 KiB, so in real mode SS = 0 and SP = the low half of ESP point at the same bytes. And `bios_read` itself is below 64 KiB (it is at 0xE254), because real mode with CS = 0 cannot reach further. `read_sector` copies each 2048 bytes from 0x60000 to where they go, 0x300000 for the kernel. That is one mode switch per 2 KiB; I have not measured what it costs.

**Under UEFI.** UEFI firmware looks on a disk for a partition of type 0xEF (MBR) or the EFI system partition GUID (GPT), reads its FAT file system, and runs `\EFI\BOOT\BOOTX64.EFI`. The CD already had that FAT image, for El Torito's UEFI entry. xorriso now also appends it as partition 2, so the loader and its `\kernel\kernel.elf` are found the same way on a stick. The UEFI loader did not change at all.

**Partitions that do not overlap.** `-partition_offset 16` makes xorriso start partition 1 at 32 KiB, not at sector 0, and write a second set of ISO 9660 descriptors there. That way partition 1 doesn't overlap the MBR or the other partitions. UEFI firmware rejects an MBR whose partitions overlap. Linux distributions build their install images the same way.

## TaajFS in a partition

Until now a TaajFS disk was TaajFS from sector 0. On the stick, sector 0 is the MBR, so the volume is partition 3, type 0x7F ("for experimental operating systems", an agreed-upon type for exactly this). `taajfs::partition::find_taajfs` reads the table and returns the first 0x7F partition that fits on the disk. A partition that starts at 0, is empty, or runs past the end is ignored, so a garbage table can't send fs off the end of the disk.

fs reads sector 0 before mounting. If it lists a TaajFS partition, fs logs `TaajFS partition: sectors 20480 to 53247` and adds the start to every sector it asks for. Otherwise nothing changes. tfstool does the same on the host.

## The Rust in it

- **`find_map`** walks the four entries and returns the first `Some` the closure gives: one line in place of a loop with a `return`.
- **`bool::then_some`.** `(entry[4] == TAAJFS_TYPE && fits).then_some((first, count))` turns a condition into an `Option`: `Some` when true, `None` when false.
- **`Option::filter`.** Used to write one of the mutations below, `find_taajfs(..).filter(|_| false)`: it keeps a `Some` only when the closure says so. My first try at that mutation (`Some(..) if false =>` in a `match`) did not compile. A guard makes Rust treat the arm as possibly not matching, so the `match` was no longer exhaustive. The compiler checks that every case is handled, even in code written to be wrong on purpose.
- **Fixed-size arrays in signatures.** `find_taajfs(sector0: &[u8; 512], ..)` takes exactly one sector. The indexing inside (`sector0[510]`) can't go out of bounds for any caller, and a caller with a slice must convert it first.
- **`io::Result` and `?`** in tfstool's `FileDevice::find`: every read and seek that can fail is one `?`, and the error goes up to the command line.

## Design decisions

- **int 13h for every BIOS boot, not just sticks.** One path for the CD, a stick and a hard disk, behind any controller the BIOS knows. The ATAPI driver is deleted, not kept as a fallback. It was the only code in stage2 that probed hardware directly, and on a real PC without IDE that probing is the riskiest thing the loader did.
- **The image is the CD image.** No second loader, no separate disk layout. The stick's partition 1 is byte for byte the ISO 9660 file system the CD has. There is one thing to build and one thing to test.
- **xorriso writes the table, the MBR is ours.** ISOLINUX's `isohdpfx.bin` would have done the same job, but it is someone else's binary. About 100 lines of assembly, each explained, fit the project better.
- **`iso64` is hybrid too.** `dd` of `image64.iso` to a stick boots it (with files in memory, as there is no TaajFS partition). Nothing about CD boots changes: `make test64` boots that ISO dozens of times.
- **A 2 KiB bounce buffer, one mode switch per ISO sector.** Bigger reads would be faster. Simple first.

## Bugs

1. **A mutation that "survived" but was never applied.** The first run of M1 (below) passed. The reason was the bootloader's Makefile, not the test: the `.asm` files are rebuilt only when they change, not when a `.inc` they include changes. M1 changed `disk.inc` and nothing was rebuilt. Touching the `.asm` files made M1 fail as it should. The Makefile still has this gap (it's the bootloader's own Makefile); I noted it under What is still missing.
2. **A stash that would not pop.** To build the old loader for M3, I first stashed my bootloader changes. The build then regenerated `headers/revision.h`, and `git stash pop` refused to overwrite it. The stash had the original working copy, so I discarded the regenerated one and popped. For M3 itself I then built the old loader from a separate worktree, which leaves the working copy alone.

## How I checked it

**Host tests:** `cd taajfs && cargo test`, 64 passed (2 new, `partition`); `cd tools/tfstool-rs && cargo test`, 4 passed.

**By booting (`make stick64-test`, in `make test64`): three machines.**

| Machine | Boots from | What it checks |
|---|---|---|
| bios | SeaBIOS, a USB stick on qemu-xhci (`bootindex=0`), a blank CD nothing can boot from | `loader TheTaaJBoot (BIOS)`; `[usb] port 1: disk "QEMU QEMU HARDDISK" ..., 29272 sectors (14 MiB) ready`; `[fs] TaajFS partition: sectors 20480 to 28671`; `[fs] ready: TaajFS on the disk`; `cat`, `wc long`, `cat docs/readme`; `write`, `cp`; `wc` loaded from the stick; no other disk driver ran |
| uefi | OVMF, the same stick | the same, with `loader TheTaaJBoot (UEFI)` |
| sata | SeaBIOS on q35: the ISO in its CD drive, which is on the chipset's SATA (AHCI) controller | `loader TheTaaJBoot (BIOS)`, a file read from the virtio disk |

After the boots, on the host, `tfstool get` on each stick's image finds `saved` = `kept` and `copy` = `long`.

`boot64-test` and `boot64-uefi-test` passed with the new loader and the hybrid ISO before anything else was tried. `make test64`: all passed, in 14 minutes 4 seconds.

**Testing note, as a scripted boot:** `make stick64`, then the stick image booted with only a blank CD, a USB keyboard and mouse. The log showed:

- `loader TheTaaJBoot (BIOS)`;
- the stick (`53848 sectors (26 MiB)`) and the keyboard and mouse;
- `[fs] TaajFS partition: sectors 20480 to 53247`, then `[fs] ready: TaajFS on the disk, 2052 of 4096 blocks free`.

`write hello from the stick`, typed on the USB keyboard, and then `poweroff`: fs stopped, then usb. On the host, `tfstool get build/stick-note.img hello` printed `from the stick`.

**Mutation experiments:**

| Mutation | Result |
|---|---|
| M1: stage1/stage2 read a stick in 2048-byte units (`DiskSectorSize` ignored) | caught: `stick-64.bios: the steps did not all run` (the BIOS machine never reached the kernel) |
| M2: fs ignores the partition table | caught: `stick-64.bios: fs did not find the TaajFS partition` |
| M3: the old ATAPI loader, on the q35's SATA CD | its serial log stayed empty: nothing booted. With the new loader the same machine boots. This is the open "SATA CD boot" item, fixed. |

Clippy is clean for `taajfs`, `tfstool-rs`, `user` (x86_64 and the AArch64 programs).

## What is still missing

- **A real PC (R7).** QEMU's SeaBIOS and OVMF are well behaved. On real machines, the things most likely to differ:
  - a BIOS that boots a stick as a floppy ("USB-FDD") when its MBR looks wrong;
  - a firmware that wants GPT and not MBR under UEFI;
  - a firmware's USB keyboard support giving up when the kernel takes the controller (handled by entry 088's BIOS handoff, untested on hardware).
- **Files written on a stick image are in `build/stick64.img`, not `build/stick-disk.img`.** `make stick64` copies `stick-disk.img` in again, so files written in a previous run are lost. Copy them out first with `tfstool get`, or keep the image.
- **The bootloader's Makefile doesn't rebuild on `.inc` changes** (Bug 1). It's harmless for a clean build, but surprising.
- **No GPT.** The table is MBR only. That reaches 2 TiB and every firmware boots it, so it is enough for a stick.
- **One mode switch per 2 KiB.** Faster reads (64 KiB at a time) would cut boot time on slow USB 2 sticks.
- **SYNCHRONIZE CACHE before power-off** (from entry 089) matters more now that the stick is where the files live.

## What I learned

- An ISO image can be a disk image at the same time, because ISO 9660 leaves its first 32 KiB empty. One file can carry three boot paths: El Torito for CDs, the MBR for BIOS disks, and an EFI system partition for UEFI disks.
- Asking the firmware to read the disk is more portable than a driver of my own. The BIOS already knows the machine's IDE, SATA, USB and NVMe controllers. The loader doesn't need to.
- Going from 32-bit protected mode to real mode and back is about ten instructions. All the care is in where things are: below 64 KiB for the code and the stack, below 1 MiB for the buffer.
- A mutation that survives needs a second look at whether it was applied at all, before anyone concludes that the tests are weak.

## Testing note

**1. Booting from a USB stick, in QEMU**

```
make run64-stick
```

This makes `build/stick64.img`: the boot image, plus a 16 MiB TaajFS partition from `build/stick-disk.img` (made the first time, with the programs on it; your `build/taaj-disk.img` is not touched). It then boots it as a USB stick, with no CD and no other disk, and a USB keyboard and mouse. In the terminal's log:

| Look for | Meaning |
|---|---|
| `  loader         TheTaaJBoot (BIOS)` | SeaBIOS ran the stick's MBR, which loaded stage1, and stage2 read the kernel through int 13h |
| `[usb] port 1: disk "QEMU QEMU HARDDISK" (46f4:0001, SuperSpeed), 53848 sectors (26 MiB) ready` | the kernel's usb driver found the same stick |
| `[fs] TaajFS partition: sectors 20480 to 53247` | fs found partition 3 in the stick's MBR |
| `[fs] ready: TaajFS on the disk, ... blocks free` | and mounted it |

In the QEMU window (typing goes to the USB keyboard):

| Type | You should see |
|---|---|
| `ls` | the programs, `welcome`, ... |
| `write hello from the stick`, then `cat hello` | `from the stick` |
| `poweroff` | `[init] stopped fs`, then `[init] stopped usb` |

Then, on the host:

```
tools/tfstool-rs/target/release/tfstool get build/stick64.img hello
```

prints `from the stick`: tfstool finds the TaajFS partition in the image too.

**2. The same stick under UEFI**

```
make run64-stick-uefi
```

The log shows `  loader         TheTaaJBoot (UEFI)`, and the rest as in 1. (`make run64-stick-uefi` makes the image again from `build/stick-disk.img`; see What is still missing.)

**3. On a real USB stick** (all data on the stick is lost; check the device name twice)

```
make stick64
lsblk                                   # find the stick, say /dev/sdX
sudo dd if=build/stick64.img of=/dev/sdX bs=4M conv=fsync
```

Then boot the PC from the stick (its boot menu: often F12, F11 or Esc). This is R7. Nothing here has run on real hardware yet.

**4. The tests by themselves**

```
cd taajfs && cargo test
make stick64-test
make test64
```

| Look for | Meaning |
|---|---|
| `test result: ok. 64 passed` | TaajFS, including the partition table |
| `Booting from a USB stick worked: the MBR, stage1 and stage2 through int 13h (BIOS), the EFI system partition (UEFI), TaajFS in the stick's partition; and the loader read a SATA CD` | the three machines of `stick64-test` |
| `test64: all passed` | every part |
