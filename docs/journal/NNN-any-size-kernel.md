# A kernel of any size: the 3 MiB wall is gone

*Journal entry #NNN · a step, MaK-WSL/boot's K130. Written on MaK-WSL/boot. The BIOS loader read the whole kernel file to physical 3 MiB and copied its segments down from there, so a kernel had to end below 3 MiB - and with about 680 KiB left, every lane adding kernel tests was nearing it. Now a 64-bit kernel is not staged at all: each segment is read from the file straight to where it goes, anywhere in usable memory from 1 MiB to 1 GiB. A test kernel ending at 6.6 MiB boots by BIOS and by UEFI.*

## Why

Entry 122 had already met the wall: the big-image test (a kernel padded to end above 2 MiB) had to shrink its padding from 512 KiB to 256 KiB to stay under 3 MiB, and the linker script made crossing it a build error. `lanes.md` listed it across lanes: J51 (test programs out of the kernel image) or K130, first wave both, because every lane's new tests need room.

## Where the wall was

```
 before:   1 MiB          3 MiB                 3 MiB + file
           [ segments ...  ][ the whole kernel file, staged ]
           copied down from here ─┘   so they must end below 3 MiB

 after:    1 MiB                                   up to 1 GiB
           [ segment ][ segment ............ ][ ramdisk ]
             each read from the file straight to its place
```

The BIOS itself was never the limit: stage 2 reads through a buffer below 1 MiB and copies out in protected mode, which reaches anywhere (K131). The wall was the staging - the whole file in one place, read before anything was known about it.

## How it works

- **`load_kernel`** (`vfs.c`) opens the file and reads its first 64 bytes: the ELF header, of either class. A 32-bit kernel is still read whole to 3 MiB, as before - it has its addresses compiled in, and E7 may retire it. A 64-bit one is read no further.
- **`kernel_read(dest, size, offset)`** (`vfs.c`) reads any part of the open file. The ELF loader reads through it and knows nothing of ISO 9660; the host tests give their own, over a file in memory.
- **`load_elf64`** reads each program header from the file, checks the segment, reads its bytes from the file to its physical address, and zeroes its `.bss`. A segment must be:
  - above 1 MiB (the loader is below) and ending by 1 GiB - the kernel window's size, which `longmode.c` maps, and within this code's 32-bit pointers;
  - identity- or window-mapped;
  - inside the file, with no addition that can wrap (K129's checks);
  - **all in memory the firmware calls usable** - new: `usable_memory` walks the E820 map stage 2 collected in real mode (`BootHeader`, now `global` for C), joins usable regions that touch, and refuses anything overlapping a region called something else. With the wall gone, this is what stands between a big kernel and a device's memory or ACPI's tables.
- **The ramdisk** went where the file was staged; there is no staging now, so it goes on the page after the kernel's end - also checked against the map.
- **The linker script's guard** moves with it: from "ends above 3 MiB" to "ends above 1 GiB", still a build error with a reason rather than a boot that never starts.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Each segment read from the file to its place | Staging the file higher (at 16 MiB, say) | Any fixed place is the next wall; read in place there is none, and the bytes are copied once less |
| The loader reads through `kernel_read` | Calling the ISO reader | The ELF loader stays about ELF; the tests give it a file without making a disc |
| A program header read at a time | All of them into a buffer | The table can be anywhere in the file; a few BIOS reads more, against a fixed-size buffer and its limit |
| Usable memory checked against the E820 map | Trusting the link addresses | Under 3 MiB the PC always had RAM; up to 1 GiB it may not (a 64 MiB machine, a hole at 15 MiB on old boards) |
| The 32-bit kernel unchanged | Reading it in place too | Its ramdisk and bitmap addresses are compiled in around the staging; E7 decides its future |
| 1 GiB as the limit | 4 GiB (the identity map) | The kernel is linked in the window, which is 1 GiB; and 32-bit pointers |

## Bugs

**1. The linker script's guard, right until now.** The first big-image build with 4 MiB of padding failed to link: "the kernel image ends above 3 MiB, where the bootloader refuses to load it". Exactly what the guard was for; it moved to 1 GiB with the loader.

**2. Mine: a refusal test damaging the wrong buffer.** "Not an ELF file" set a byte in the old staging buffer, where a 64-bit file no longer is - the loader read an undamaged header and loaded it, and the test failed. It damages the file now.

**3. Mine: a rebuild that did not happen.** `global BootHeader` went into an include file, and stage 2's Makefile rebuilds `stage2.o` only when `stage2.asm` changes: the link failed with `BootHeader` undefined until it was rebuilt.

## How I checked it

- **Host tests**: 181 checks, 0 failed, with the build's ISO and kernel. New: a kernel with a 4 MiB segment at 8 MiB - every byte read from the file to its place, the `.bss` zeroed and nothing past it, the kernel's end right, a read per program header and segment; a segment over reserved memory, and one past the end of memory, refused; a segment or a program header the disk cannot give, refused with its message; `usable_memory` itself - regions that touch joined, a gap, a reserved region inside a usable one, past the end. The ELF tests now load as stage 2 does: the header copied as `load_kernel` would, the file read through `kernel_read`. The build's real kernel loads from its file in place. On the old loader the new tests fail at once: it refused anything ending past 3 MiB.
- **Boots**: `big64-test` with the padding raised from 256 KiB to 4 MiB - a 5.6 MB kernel ending at 0x69c000 (6.6 MiB), loaded by BIOS with its 4 MiB checked byte for byte by the kernel, the ramdisk on the next page (0x69c000), and by UEFI; `stick64-test`, `smp64-test`, `boot64-test`. These ran beside the race hunt, unlocked (small boots, throttled by `boot-slots.sh`).
- `make test64`: RESULT.
- **Not tested**: a real PC's memory map (K1); a kernel near 1 GiB (QEMU here has 512 MiB).

## What is still missing

- **J51** (test programs out of the kernel image) is no longer forced by a wall - it is `ci`'s to order now.
- **The UEFI loader** had no 3 MiB wall; whether its own checks match these (usable memory, 1 GiB) is worth a look with K132.
- **A real PC** (K1).

## What I learned

- **A wall is often a copy.** Nothing in the hardware stopped at 3 MiB; one buffer did, because the whole file was put in one place before the loader knew what it held.

## Testing note

```
make big64-test
```

| Look for | You should see |
|---|---|
| `[test] big image:` | `the kernel ends at 0x69c000, above 3 MiB; 4096 KiB of it ... is byte for byte what was built`, in both the BIOS and the UEFI log |
| `ramdisk` (BIOS log) | `0x2000 bytes at 0x69c000` - on the page after the kernel |
| `big64-test passed` | |

```
make stage2-test
```

| Look for | You should see |
|---|---|
| `stage2 host tests:` | `181 checks, 0 failed` |

Automated: `make test64`
