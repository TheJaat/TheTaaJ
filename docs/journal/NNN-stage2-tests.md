# Stage 2 on the host: tests for the ISO reader, the ELF loader and the page tables

*Journal entry #NNN · a step, MaK-WSL/boot's K129. Written on MaK-WSL/boot. The kernel's crates have had host tests for a long time; the bootloader's C code had none - only whole boots. Now the three parts the boot lane is about to change - the ISO 9660 reader, the kernel's ELF loader, and the page tables and boot info for a 64-bit kernel - are run on the host, unchanged, as 32-bit programs at stage 2's own fixed addresses. 152 checks; they found eight bugs, fixed here.*

## Why

The boot lane's next items change exactly these three: a fast BIOS read (K131) replaces how sectors come in, a kernel of any size (K130) changes the loader's limits, a relocatable kernel (K135) changes the page tables. A whole boot tells me *that* something broke, not what. And some things no boot shows at all: a damaged disc, a file whose header lies about its sizes. Those are what a host test can make on purpose.

## The idea: run the code that boots, at its own addresses

Stage 2's C code is written for one place: 32-bit protected mode, paging off, with fixed physical addresses wired in - the ISO reader's buffers at `0x80000` and `0x90000`, the kernel file staged at `0x300000`, the kernel's segments copied to between 1 and 3 MiB, and pointers cast to `uint32_t` throughout (`(uint32_t)IdentityPdpt` in an entry of the page tables).

Two ways to test that on a 64-bit host:

| | How | Cost |
|---|---|---|
| Change the code: every fixed address through a macro, `PHYS(a)`, mapped to a host buffer in tests | Tests as 64-bit host programs | Every address in stage 2 rewritten; the `uint32_t` casts break; the code tested is no longer quite the code that boots |
| **Run it as it is** (chosen) | Tests as 32-bit programs, which may map those low addresses | Linux must run 32-bit programs (it does, here and on WSL), and the programs bring their own start |

A 32-bit Linux process can have memory almost anywhere: its own code starts at `0x08048000`, and only the first 64 KiB are refused (`vm.mmap_min_addr`). So the tests ask for `0x80000` to 16 MiB, exactly where stage 2 works (`MAP_FIXED_NOREPLACE`, so a clash is an error, not a silent overlap), and then call stage 2's own functions, compiled from stage 2's own files with stage 2's own compiler (`i686-elf-gcc`) and flags.

That compiler has no C library - neither has stage 2 - so the tests have none either. `harness.c` is their whole runtime:

```c
/* int 0x80 with up to six arguments. The sixth goes in EBP, which the
 * compiler may be using as the frame pointer: saved and restored here. */
static int32_t sys(int32_t n, int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f)
```

`_start`, five system calls (`mmap2`, `open`, `read`, `write`, `exit`), a printer, and `CHECK(condition, "why it matters")`, which counts and goes on - one bug, every symptom. What touches hardware is replaced: `read_sector` reads from a disk image in memory (and counts its reads, and can be told to fail one), and `boot_print` keeps what is printed, so that a test checks *which* message a refusal gave.

The one address a test cannot have is the BIOS data area at `0x40E` (below 64 KiB): `FindRsdp` reads it, so `prepare_long_mode` is not run whole. Its steps are: `longmode.c`'s functions are `static`, so `longmode_test.c` includes the file and calls them one by one.

```
 bootloader/tests/
   harness.c / .h     the runtime: start, system calls, checks, the disk
   iso_test.c         ISO9660FS.c on images made sector by sector, and a real ISO
   elf_test.c         elf.c on ELF files made at 0x300000, and the real kernel
   longmode_test.c    longmode.c's page tables, walked as the CPU walks them
   Makefile           stage 2's files with stage 2's flags; the tests with -Wall
```

## What the tests do

- **ISO 9660**: images built sector by sector, laid out by the standard's byte offsets (not the header's structs, so that a mistake in a struct shows; a first test checks the structs against the standard too). A volume with nested directories, a directory spanning two sectors, files whose every byte says where it came from. Mount, refusal of what is not ISO 9660, paths with and without leading and repeated slashes, names matched whole, a file used as a directory, read errors, reads at every kind of offset; then the damage (below). With a real ISO given, the build's own: it mounts, and `KERNEL/KERNEL.ELF` - stage 2's name for it - is found and begins as an ELF file.
- **ELF**: files built at the staging address. A good 64-bit kernel as the real one is linked (code and data in the kernel window, 1 MiB physical, a `.bss`): the bytes land where they should, the `.bss` is zeroed and nothing past it touched, the span and entry are right. Then each refusal, by its message: not ELF, big-endian, AArch64, `ET_DYN`, no program headers, headers too small, headers past the file, a segment on the loader, one reaching the staging buffer, one wrapping past 2^64, one neither identity- nor window-mapped, `p_filesz > p_memsz`, bytes past the file, no `PT_LOAD`, an entry outside. The same for 32-bit. With a real kernel given, the build's own: it loads as a 64-bit higher-half kernel between 1 MiB and the staging buffer, and fits the 3 MiB the BIOS path stages (K130's wall).
- **Long mode**: the page tables built over garbage (stage 2's `.bss` is not in its flat binary, so it starts as whatever was in memory), then **walked as the CPU walks them** - four levels, 9 bits each, a 2 MiB page ending at the third. Identity from 0 to 4 GiB, not at 4 GiB; the kernel window `0xFFFFFFFF80000000 + x` to `x` for its 1 GiB, not past it; non-canonical never; every other entry zero. The memory map (an empty entry dropped, unknown types reserved, 64-bit bases kept, 80 entries cut at 64 without overrunning), the framebuffer (nothing in text mode, the pitch from the BIOS not width times four), the RSDP checksums (1.0 and 2.0, a length past any RSDP refused before it is summed).

## Bugs

The tests were written first, against the code as it was: 142 checks, 10 failed. Each failure was one of these, and each fix is in this branch, next to a comment naming its test.

**1. A directory size short of a whole sector read on and on.** `iso9660_find_in_directory` counted the bytes left by subtracting 2048 per sector from an `unsigned int`. A size that is not a whole number of sectors - not what a mastering tool writes, but what a damaged disc can say - went below zero and wrapped to about four thousand million, and the search read sector after sector. Now it stops at 0.

**2. A damaged directory record was parsed past itself.** A record's length was trusted: one shorter than its own fields and name, or running past the sector's end, was read anyway, into whatever followed. Now such a record ends its sector's records (the next sector is still read), with a message.

**3. A long name in a path ran over the stack.** `iso9660_open` copied each name of a path into `char token[64]` with no limit. A 149-character name was written over the rest of the stack - silently, this time: the check that it was refused passed by luck, and only the missing message showed. Now a name of 64 characters or more is refused.

**4. A read whose offset and size passed 4 GiB came out small.** `iso9660_read` cut a read at the file's end with `if (offset + size > node->size)`; with a size near 4 GiB the sum wrapped to a small number, the read was not cut, and it ran past the caller's buffer. Now it compares `size > node->size - offset`, which cannot wrap (offset is already below the size).

**5-8. Four ELF bounds checks that wrapped.** The same mistake four times: "do the program headers fit in the file?" as `e_phoff + count * size > FileSize`, and "do a segment's bytes?" as `p_offset + p_filesz > FileSize` - in ELF32 and ELF64 both. An offset near the top of the number range makes the sum wrap to something small, which passes; the loader then read its program headers from before the file, or copied a segment from there. Each is now two comparisons that cannot wrap: the offset within the file, then the size within what is left.

Also fixed, found reading the code: the mount's message printed the root directory's sector plus 2 (`*(unsigned int *)pvd->root + 2` - the 2 added to the value, not the pointer); and three functions nothing calls are gone - `iso9660_open1` (which looped for ever on its first record), `iso9660_open_old` and `iso9660_list2`, each with bugs 1 and 2 in them.

**Mine, along the way**: a check written as `... || 1`, which could not fail (now it checks that a name longer than its record ends the sector's records); a "size that wraps" that did not wrap (`0xFFFFFFF0` plus 10 is still below 4 GiB; `0xFFFFFFFF` is the one); a declaration missing; and a check that the real ISO has `RAMDISK.MDR` - it has none: the 64-bit ISO carries stage 1, stage 2 and the kernel, and stage 2 goes on without a ramdisk.

**The cross compiler's PATH.** `bootloader/config.mk` puts `../toolchain/i686/bin` on the PATH - relative to the directory make runs in. From `bootloader/` that is TheTaaJ's toolchain; from `bootloader/tests/` it is nothing. `tests/Makefile` puts it there from `ROOT_DIR` instead, and `bootloader/Makefile`'s new `test` target runs it from `bootloader/`.

**Making this tree.** `tools/new-session.sh` fetches first, and the SSH key was locked, so I made `TheTaaJ-boot` by hand from the `main` fetched earlier that day. My first try hid the commands' output; the submodule step failed silently - git (2.38 and later) will not clone a submodule from a local path unless `protocol.file.allow` says so - and my next commands, meant for the empty `kernel/` and `bootloader/`, ran in TheTaaJ itself. Worktrees share one config, so for a few minutes every tree's `origin` pointed at TheBootloader, and the first checkout's submodule URLs were gone. Both were put back, checked before and after. The lesson is the plainest one: output is not hidden from a step that can fail.

## How I checked it

- The tests against the code as it was: 142 checks, 10 failed - the ten that bugs 1-8 fail (5-8 fail two checks each in places, a refusal with the wrong message counting as one). After the fixes: 142, 0 failed.
- With the build's real ISO and kernel: 152 checks, 0 failed.
- `make stage2-test` (TheTaaJ), the new part of `test64`, beside the boot parts: RESULT.
- `make test64`: RESULT.
- **Not tested**: the AArch64 UEFI loader (`uefi/aarch64.c`) and the x86_64 one (`uefi/main.c`) - Windows-format programs built with clang, a different harness; stage 1 and the assembly; `FindRsdp`'s search itself.

## What is still missing

- The UEFI loaders' C code under the same kind of test.
- `stage2_malloc` never frees: each `iso9660_open` takes a node per name in the path from 64 KiB, so a few hundred opens exhaust it. Stage 2 opens two files; noted, not changed.
- K128 (two stray files in TheBootloader) waits, by the user's choice, for the next bootloader item.

## What I learned

- **Freestanding code can be tested freestanding.** No C library is needed to run C on Linux: five system calls and a `_start`. And a 32-bit process is the right shape for code with 32-bit addresses wired in - it can simply have them.
- **Every addition of a length to an offset read from a file is a question**: can it wrap? Four of the eight bugs were one line each, the same line.

## Testing note

```
make stage2-test
```

| Look for | You should see |
|---|---|
| `stage2 host tests:` | `152 checks, 0 failed` |
| `stage2-test passed` | |

On their own, without TheTaaJ's build (no real ISO or kernel: their checks are skipped and say so):

```
make -C bootloader test
```

| Look for | You should see |
|---|---|
| `iso: no real ISO given, its checks skipped` | |
| `stage2 host tests:` | `142 checks, 0 failed` |

Automated: `make test64`
