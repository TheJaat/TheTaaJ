# Era 21 · Boot: recoverable, verified and unlimited

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: a deliberately broken kernel update boots the previous kernel by itself; a kernel twice today's size boots from BIOS; an emulated BIOS boot reaches the kernel under a time the first measurement sets; the user can choose a kernel and edit its command line at boot; and, with Era 18's K107, the boot is verified. **The bootloader is MaK-WSL's (rule 7)**: these items are MaK-WSL's to do, or to agree before anyone else does them. Whose it is, and when, is for the user to say. Pointed to, not moved: K51 (the slow BIOS loader), J51 (test programs out of the kernel image), K12 (the installer), K25 (ASLR), K107 (Era 18), U6 (Raspberry Pi and RISC-V).

What is right: **the TaajBoot protocol** (`bootloader/headers/boot/taajboot.h`) - one 136-byte structure every loader fills (memory map, kernel place, ramdisk, ACPI RSDP, device tree, command line, framebuffer, loader name), fixed-width, append-only, with a `Size` field for older loaders, its layout checked in C and in the kernel's Rust mirror. "A new way to boot is then a new loader, never a change to the kernel." Three loaders behind it: BIOS (a hybrid MBR for USB sticks, stage1, stage2 in 32-bit assembly and C, ISO 9660, E820, VESA, long mode), UEFI x86_64 (`ExitBootServices` done right: nothing allocates between reading the map and exiting, and it retries), and UEFI AArch64 (a Linux-style `Image` with a device tree, the firmware's keep-out ranges copied into the tree, so the kernel never learns UEFI). About 5,000 lines.

What is missing:

- **The BIOS loader takes a kernel only between 1 and 3 MiB** (`stage2/elf.c`; the kernel's `linker.ld` checks it), with about 670 KiB left - the wall J51 works around.
- **It is slow emulated**: 14 of an 18 s boot, an `int 13h` read at a time (K51).
- **No recovery**: one kernel, no fallback when a new one fails to boot. TaajFS has A/B superblocks; the boot path has nothing like them.
- **No menu**: no timeout, no choice of kernel, no way to give a command line at boot (the protocol has the field).
- **No verification**: Secure Boot and measured boot are Era 18's K107.
- **The kernel lives outside TaajFS** - ISO 9660 for BIOS, the FAT EFI partition for UEFI - and nothing decides how TheTaaJ updates it.
- **A fixed physical place** in the first 1 GiB: no kernel ASLR.
- **No tests of its own**: the ISO 9660 reader, the ELF loader and the page-table builder are tested only by booting everything.
- **Two stray files** in the public repository: `abc.txt` and `random.txt`.

| # | Item | Size | Notes |
|---|---|---|---|
| K128 | **`abc.txt` and `random.txt` removed** from TheBootloader | S | MaK-WSL's. First: trivial |
| K129 | **Host tests** for the ISO 9660 reader, the ELF loader and the page-table builder, as the kernel's crates have | S | **Done (entry 142)**: `bootloader/tests/`, stage 2's own files run unchanged as 32-bit programs at its own addresses; 152 checks with the build's ISO and kernel (`make stage2-test`); eight bugs found and fixed (four wrap-arounds in the ELF loader's bounds, four in the ISO reader). Left: the UEFI loaders' C code |
| K130 | **A kernel of any size from BIOS**: read in pieces to anywhere above 1 MiB (unreal mode, or a copy out of a buffer below 1 MiB after each read) | M | Ends the 3 MiB wall; J51 then is no longer urgent |
| K131 | **A fast BIOS read**: as many sectors per `int 13h` as the BIOS takes, or staying in real mode for the whole read; measured before and after | S | **Done (entry 143)**: 16 ISO sectors per call (inside every BIOS limit: 127 sectors, one 64 KiB block), whole sectors straight into place; an emulated boot reaches the kernel in about half the time (`tools/boot-time.sh`). Left: a real BIOS (K1) |
| K132 | **Two kernels, A and B**: a new one is tried once, TheTaaJ marks it good after it has booted, and the loader goes back to the other one if it was not marked | M | The era's milestone. With K134 |
| K133 | **A boot menu** with a timeout: which kernel, the command line edited, a recovery entry (a shell with nothing started but what is needed) | M | The protocol's `CommandLine` already exists |
| K134 | **Where kernels live and how they are updated**: on the EFI partition (written from TheTaaJ through Era 14's J68), or a loader that reads TaajFS; one answer for BIOS and UEFI | M | With K12 (the installer) |
| K135 | **A relocatable kernel**, placed by the loader where there is RAM, at a random place when asked | M | For K25's kernel half; machines with odd memory maps |
| K107 | Secure Boot and measured boot | L | In Era 18; listed here because it is the same code |
