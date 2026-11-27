# A fast BIOS read: sixteen sectors per trip to real mode

*Journal entry #NNN · a step, MaK-WSL/boot's K131 (which is K51). Written on MaK-WSL/boot. Stage 2 read the kernel through the BIOS one 2 KiB sector at a time, dropping from 32-bit protected mode to real mode and back for every one: about 830 round trips for the 1.7 MB kernel. Now whole runs of sectors are read sixteen per call, straight into place. An emulated BIOS boot reaches the kernel in about half the time.*

## Why

Entry 105 measured it (K51): of an 18-second emulated boot, 14 seconds passed before the kernel's first byte - and stage 2 spent them in `bios_read`. Each call is a trip: from 32-bit protected mode down to 16-bit, protection off, real mode with the BIOS's interrupt table, `int 13h`, and all the way back up. On a PC that is a few microseconds. Emulated, every mode switch makes QEMU throw away the code it has translated, so each trip costs far more than the sector it brings - and the kernel took 830 of them. The-Jat's machines have no KVM: every BIOS boot of every test there pays this.

## How it works

Two changes, one in each layer that asked for one sector at a time:

```
 before:  iso9660_read ─▶ read_sector ─▶ bios_read(1) ─┐  x 830
                         copy to a stack buffer ◀──────┘
                         copy to the kernel's place
 after:   iso9660_read ─▶ part of the first sector  ─▶ read_sector
                       ─▶ every whole sector         ─▶ read_sectors ─▶ bios_read(16) x 52
                       ─▶ part of the last sector   ─▶ read_sector
```

- **`read_sectors(lba, count, buffer)`** (`bios_disk.c`): up to `READ_MAX`, 16 ISO sectors, per `int 13h` call. Sixteen is 32 KiB, which keeps inside three limits at once: the BIOS buffer (`0x60000`, up to the ISO reader's buffers at `0x80000`), one 64 KiB block (some BIOSes' DMA cannot cross one), and 127 sectors per call (many BIOSes' most) - on a stick, whose sectors are 512 bytes, 16 ISO sectors are 64 of its own. The two tries for a USB device waking up stay, per call.
- **`iso9660_read`** reads a part of the first sector, then every whole sector at once **straight into the caller's buffer**, then a part of the last. A whole sector is also copied once now, not twice.

## The numbers

`tools/boot-time.sh` (new): from starting QEMU to the kernel's first line on the serial port, `Hello from TheTaaJ!` - firmware, stage 1, stage 2 reading the kernel, the ELF loader, long mode. One CPU, the ISO alone. MaK-WSL, with another tree's build running (so slower than idle - entry 105's 14 s was idle); before and after the same way:

| | before | after |
|---|---|---|
| emulated (TCG), 3 runs | 47.2, 35.8, 38.3 s - median **38.3** | 21.3, 22.7, 20.0 s - median **21.3** |
| KVM, 3 runs | 4.0, 4.8, 3.9 s - median **4.0** | 3.5, 3.0, 4.2 s - median **3.5** |

Again with the laptop idle (5 runs emulated, 3 under KVM, medians): emulated **13.73 s before, 8.33 s after**; KVM 1.29 s and 1.21 s - 14 s idle is what entry 105 measured too.

What was left of the emulated boot I took for SeaBIOS and stage 2's own start. It was not: K23 (the next entry) measured each step and found it - copying memory a byte at a time, and printing on the screen.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| 16 sectors per call | As many as the buffer holds (64); staying in real mode for the whole read | 16 keeps inside every BIOS limit I know of, including the 64 KiB DMA block; 52 calls instead of 830 is most of the gain. Staying in real mode would mean the ISO reader in 16-bit code, or unreal mode - much more change for the last few percent |
| Whole sectors straight into the caller's buffer | Through a sector buffer | One copy instead of two, and no 2 KiB-at-a-time loop |
| The tests replace `bios_read`, not `read_sector` | Keeping K129's stand-in | Then `bios_disk.c` itself is tested: how it splits a read, its retry, both sector sizes, and every call checked against the BIOS's limits |

## Bugs

None in the new code that the tests or the boots found. Run against the old code, the new checks fail where they should - 5 of 157: the number of BIOS calls (100 for the big file instead of 8), and the new message for a failed read; the content checks pass on both, since one sector at a time was correct, only slow.

## How I checked it

- **Host tests** (K129's, now with the real `bios_disk.c` under them): 157 checks, 0 failed. New: a 100-sector file read in 8 BIOS calls of at most 16 sectors, every byte from its place; a read from inside one sector to inside another (a first part, 3 calls, a last part); the same file from a disk of 512-byte sectors (the same 8 calls, 64 of its sectors each); a sector that fails once read again, one that always fails failing the read with its message; and on every call, that a BIOS would accept it (127 sectors, the buffer's size, one 64 KiB block).
- **Boots**: `tools/boot-time.sh`, three emulated and three under KVM, before and after: every one reached the kernel.
- `make test64`: RESULT.
- **Not tested**: a real BIOS. QEMU's SeaBIOS accepts much more than this asks for; the limits kept to are those of older BIOSes - which a real PC (K1) would show.

## What is still missing

- **A real PC** (K1): the limits above, met by a real BIOS and a real USB stick.
- **The rest of the emulated boot**: K23's.

## What I learned

- **Under emulation, a mode switch is the expensive thing, not the work.** One `int 13h` for 2 KiB or for 32 KiB costs nearly the same; the trip there and back is the cost.

## Testing note

```
tools/boot-time.sh build/image64.iso tcg
```

| Look for | You should see |
|---|---|
| `boot-time (tcg): median` | About half of what the same command gives on the ISO of `main` before this change |

```
make stage2-test
```

| Look for | You should see |
|---|---|
| `stage2 host tests:` | `167 checks, 0 failed` (with the build's ISO and kernel) |

Automated: `make test64`
