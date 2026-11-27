# Boot time, measured and cut: the loader was copying a byte at a time and talking to the screen

*Journal entry #144 · a step, MaK-WSL/boot's K23, with K128. Written on MaK-WSL/boot. A boot's time, stamped line by line from outside, with stage 2 now marking its own steps on the serial port. It showed where the loader's time went: copying memory one byte per turn of an unoptimised C loop (emulated, ten seconds), and printing on the VGA text screen (under KVM, most of a second). Both cut. An emulated boot reaches the kernel in under half a second instead of fourteen; one under KVM in about half the time.*

## Why

K23: "measure boot time on KVM and on the real PC, cut the slow parts". K131 (entry 144, just before this one) halved an emulated boot's way to the kernel by reading 16 sectors per BIOS call - but measured only the total. Where the rest went, nobody could see: stage 2 writes on the screen, and the screen is in no log.

## How it was measured

Two new tools, and markers:

- **`tools/boot-timeline.py`** boots an ISO and stamps every line of the serial port with the seconds since QEMU started, as it appears, until the shell says it is ready; then lists the ten biggest gaps. The line after each gap is what was waited for.
- **Stage 2's steps on the serial port** (`serial.inc`): eight lines - started, screen mode chosen, kernel read, kernel loaded, ramdisk, screen mode set, to the kernel. A few hundred bytes. Also useful the day a boot dies before the kernel: the log shows how far it got (2026-11-25's resets before the kernel had an empty log).
- `tools/boot-time.sh` (K131): QEMU start to the kernel's first line, N runs, the median.

## What it showed

The first timeline, emulated, after K131:

```
  1.381  [stage2] started
  1.415  [stage2] screen mode chosen; reading the kernel
  6.902  [stage2] kernel read                 5.5 s
 11.915  [stage2] kernel loaded (ELF)         5.0 s  - and no disk at all
 12.154  Hello from TheTaaJ!
```

**Loading the ELF does no I/O** - it copies the kernel's segments from the staging buffer into place and zeroes the `.bss` - and took five seconds. Stage 2 is compiled without optimisation, and `memcpy`, `memcpyb` and `memsetb` were C loops of one byte per turn: several instructions a byte, and under emulation every instruction is translated and run. Reading the kernel was mostly the same copying (the BIOS buffer into place, twice before K131).

**Fix 1: the x86's block instructions.** `rep movsd` copies four bytes a step and `rep stosd` fills four (`movsb`/`stosb` for the rest): one instruction for a whole block, which a CPU runs at memory speed and an emulator runs as one operation. Forward, as before; a count of 0 or less does nothing (the old `while (n--)` ran for ever on a negative `int`). The host tests run them (every ISO read and ELF load goes through them).

```
  0.316  [stage2] started
  0.347  [stage2] screen mode chosen; reading the kernel
  0.537  [stage2] kernel read                 0.19 s
  0.547  [stage2] kernel loaded (ELF)         0.01 s
  0.614  Hello from TheTaaJ!
```

**Under KVM, though, it did not move** - about 1.1 s to the kernel before and after - and now an emulated boot reached the kernel *sooner* than a KVM one. Its timeline:

```
  0.236  [stage2] screen mode chosen; reading the kernel
  1.001  [stage2] kernel read                 0.77 s
  1.280  [stage2] ramdisk done                0.24 s - to find there is no ramdisk
```

Two experiments, each undone after:

| Tried | Kernel read under KVM | So |
|---|---|---|
| 32 sectors per BIOS call instead of 16 | 0.53, 0.54 s (16: 0.55-0.77) | Not the number of calls |
| Stage 2's screen printing switched off | **0.12 s**; the ramdisk step **0** | The screen |

Under KVM, the VGA text screen is emulated memory: every byte written to it leaves the virtual machine for QEMU and comes back, and every scroll moves the whole screen that way, 4000 bytes read and 4000 written. Under emulation such a write is an ordinary one. And stage 2 printed a great deal: the ISO reader a line for **every directory entry it passed** ("Found entry: ..."), and for every search, match and read; the ELF loader one per segment; and `init_vfs`, on every boot, opened `SAAMPLE/RANDOM.TXT` - a test of the early reader, misspelled, never on the disc, walking the root directory and printing each entry - and then listed the `KERNEL` directory, entry by entry.

**Fix 2: stage 2 says what matters.** The test leftovers in `init_vfs` are gone, and so is the chatter: the per-entry, per-search, per-read and per-segment lines, and the root directory's sector in hex. What stays: the mount, the kernel's loading, the ramdisk, and **every error and refusal** - the host tests check those messages, and all of them still pass.

**K128 rides along** (the user's choice: with the next bootloader item): `abc.txt` and `random.txt`, sample files in TheBootloader's root - `RANDOM.TXT`'s namesake is what `init_vfs` went looking for - are removed.

## The numbers

MaK-WSL, the laptop idle. To the kernel: `tools/boot-time.sh`, medians of 5 runs emulated and 3 under KVM. To the shell: `tools/boot-timeline.py`, 4 CPUs, three runs each.

| | `main` | after K131 | after K23 |
|---|---|---|---|
| to the kernel, emulated | 13.73 s | 8.33 s | **0.34 s** |
| to the kernel, KVM | 1.29 s | 1.21 s | **0.50 s** |
| to the shell, emulated | 17.5, 18.0, 17.9 s | | **3.8, 3.9, 3.9 s** |
| to the shell, KVM | 4.1, 4.2, 3.9 s | | **3.2, 3.3, 3.3 s** |

An emulated boot reaches the shell four and a half times sooner - fourteen seconds a boot, on every machine without KVM and in every test that boots by BIOS. Under KVM the loader's part is half what it was, and what is left of the boot is the kernel's own start (What is still missing).

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Stamped from outside, by the host | Time stamps in the kernel's log | Stage 2 and the firmware have no clock worth the name; from outside, every line from QEMU's start has a time, and nothing in the system changes to get it |
| Markers on the serial port, kept | Taken out after measuring | A few hundred bytes; the boot log shows the loader's steps every boot, and how far a failed boot got |
| `rep movs`/`rep stos` in `mem.c` | Compiling stage 2 with `-O2` | The copying is where the time was. `-O2` changes every line of a loader whose timing-sensitive real-mode parts were written for `-O0`; one file's three functions is the change that can be checked |
| Quieter screen, not a faster screen | A RAM copy of the screen, written out at the end | The chatter helped nobody - it scrolled past in the time it took to print. What is worth showing still shows |
| 16 sectors per call kept | 32 | The experiment showed calls are not what costs under KVM; 16 keeps inside more BIOSes' limits (K131) |

## Bugs

**1. A copy of a negative length ran for ever.** `memcpy(..., int n)` and `memsetb(..., int n)` looped `while (n--)`: a negative `n` - a size worked out wrong somewhere - counted down through every number. Now 0 or less does nothing. Not seen happening; found rewriting them.

**2. Mine: a script that matched nothing.** My first edit of the chatter stopped halfway: its blank lines had no spaces, and the source's blank lines have four, so a pattern did not match. The script checked each match and stopped there; the half it had done was right, and the rest was done line by line.

## How I checked it

- **Host tests** (K129, K131): 167 checks, 0 failed, after each change - the copying runs under every ISO read and ELF load, and every message the tests expect is still printed.
- **Boots**: `stick64-test` (a USB stick: 512-byte sectors, 64 of them per call), `big64-test` (a kernel above 2 MiB, by BIOS and by UEFI), `smp64-test`, `boot64-test` (the whole typed session): all passed. Every timeline run, emulated and under KVM, reached the shell.
- `make test64`: RESULT.
- **Not tested**: a real PC's VGA and BIOS (K1) - on a real PC the screen is fast, and the copying was never slow; this entry is about emulators and KVM, where every test runs.

## What is still missing

- **The kernel's own start is now most of a boot**: under KVM about 3 of 3.3 seconds to the shell, much of it the boot self-tests - 40 000 hand-offs between threads (0.8 s), the file service asked to stall twice (0.2 s each), programs spinning to be killed. They run on every boot, `make run64` included. Running them only in tests would be a change to what a boot checks: the user's to decide (and the test-speed plan's tiers, idea 3).
- **SeaBIOS's own start** (0.1-0.3 s) is not ours.
- **A real PC** (K1): measure the same steps there.

## What I learned

- **Under KVM, what costs is leaving the virtual machine**, not computing: a byte to the VGA screen, a word from the CD's I/O port. Under emulation it is the other way round: computing is slow, and those are cheap. One loader, two opposite profiles - and a timeline of each is how to tell.
- **Measure before cutting, and test the guess.** I would have guessed the BIOS calls under KVM; two experiments said the screen.

## Testing note

```
python3 -I tools/boot-timeline.py build/image64.iso --accel tcg
```

| Look for | You should see |
|---|---|
| `[stage2] started` ... `Hello from TheTaaJ!` | Each of the loader's steps with its time; the kernel's first line well under a second after `[stage2] started` |
| `tcg:` | The whole boot to the shell, in seconds |

```
make run64
```

| Do | You should see |
|---|---|
| Watch the screen while it boots | Stage 2's few lines - the mount, the kernel loading - not a scroll of file names |

Automated: `make test64`
