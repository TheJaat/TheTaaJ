# The Kernel Outgrows Its Megabyte

*Journal entry #48 · not on the roadmap. The kernel reached the size the BIOS bootloader would load, in the middle of entry 47. This entry moves the wall from 2 MiB to 3 MiB without touching how the old 32-bit kernel is loaded, and adds a test that boots a kernel made too big on purpose.*

```
[test] big image: the kernel ends at 0x2c3000, above 2 MiB; 768 KiB of it at 0x1d56d4-0x2956d4 is byte for byte what was built
```

Entry 47 ended with a list, and the first line of it was:

> **The kernel has about 40 KiB left to grow** before the bootloader's 2 MiB limit stops it again.

That is not a list item to carry around. The next thing I add to the kernel would have hit it.

---

## Where the megabyte came from

Nobody chose "the kernel may be one megabyte". It fell out of three addresses in the BIOS loader (stage 2), chosen long ago for the 32-bit kernel:

```
physical memory, as the BIOS loader uses it

0          1 MiB            2 MiB            3 MiB
|-----------|----------------|----------------|-----------------------> 
 the loader   the kernel's     the ramdisk      the kernel's FILE,
 itself,      segments are     is read to       as read from the CD
 BIOS data    copied to here   here             ("staging buffer")
```

The loader works in three steps, in this order:

1. **Read the file.** `kernel.elf` is read whole from the CD to 3 MiB. It is an ELF file: headers, then the segments, in the order the linker wrote them. It cannot be run where it lies.
2. **Place the segments.** Each segment is copied from the file to the address the kernel was linked for — from 1 MiB up.
3. **Read the ramdisk** to 2 MiB.

Step 3 is the wall. A kernel that reached past 2 MiB would be loaded correctly and then have a ramdisk written through the middle of it. So the loader refuses such a kernel. I met that refusal once before, in entry 22, as a boot that silently never started; since then the linker script refuses first, with a reason:

```
ASSERT(__kernel_end - KERNEL_WINDOW <= KERNEL_LOAD_LIMIT,
    "the kernel image ends above 2 MiB, where the bootloader refuses to load it - see linker.ld")
```

The UEFI loader has no such wall. It reads the file into memory the firmware gives it, then *asks* the firmware for exactly the pages the kernel's segments cover, wherever they end.

## What is in the megabyte

| | KiB |
|---|---|
| Code | 178 |
| Read-only data | 672 |
| …of which programs carried inside the kernel (shell, file service, display, init and fifteen more) | 643 |
| Data | 10 |
| Zeroed memory (the boot stack, the debug log's queue, tables) | 154 |
| **Together** | **1014 of 1024** |

Nearly two thirds is programs. That is a debt from before there was a disk to load them from, and the real cure is to carry fewer of them. But the kernel must be able to start a file service *before* it can read a file, so some will always ride along — and a wall I can hit by adding a self-test is too close whatever is inside it.

## The observation that makes it cheap

Look at the three steps again and ask what is still needed after each.

After step 2 the staging buffer is dead. The kernel has been copied out of it; nobody reads the file again. That megabyte-and-more at 3 MiB is free — at exactly the moment the loader needs somewhere to put a ramdisk.

So for a 64-bit kernel:

```
before:   [ kernel 1..2 MiB ][ ramdisk 2..3 ][ file being copied from 3.. ]
after:    [ kernel 1.................3 MiB  ][ ramdisk, where the file was ]
```

- the kernel may fill everything from 1 MiB to 3 MiB — up to the buffer it is being copied *from*, which it must not overwrite while the copy is going on;
- the ramdisk is read to 3 MiB, over the dead file.

Two megabytes instead of one, and nothing new to reserve.

```c
/* elf.c */
#define LOAD_MAX_ADDRESS   KERNEL_ELF_READ          /* was RAMDISK_LOAD_ADDRESS */

/* vfs.c, load_ramdisk */
unsigned int address = g_kernelIs64 ? KERNEL_LOAD_START : RAMDISK_LOAD_ADDRESS;
```

The 64-bit kernel never knew where its ramdisk was: it is told, in the boot information the loader hands over. So it needs no change at all. That is the boot contract paying for itself — an address passed is an address that can move.

### Why only for the 64-bit kernel

The 32-bit kernel has `0x200000` compiled into it as the place to find its ramdisk, and `0x300000` as the place for its memory bitmap. It is the kernel this project started with, it still builds, and it has no automated test. Changing where its ramdisk goes would be changing code I cannot check.

So `g_kernelIs64` decides, and for a 32-bit kernel every instruction the loader executes is the one it executed before.

## A test that is too big on purpose

How do I test that a kernel bigger than 2 MiB loads, when mine is 1 MiB?

By making one. A build feature, `big-image`, adds 768 KiB of ballast to the kernel. The build script writes it:

```rust
// build.rs
if std::env::var_os("CARGO_FEATURE_BIG_IMAGE").is_some() {
    let ballast: Vec<u8> = (0..768 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    std::fs::write(format!("{out}/ballast.bin"), ballast).expect("writing the ballast");
}
```

and the kernel carries it and, once booted, checks every byte against the same rule:

```rust
static BALLAST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ballast.bin"));
let ballast = core::hint::black_box(BALLAST);
let wrong = ballast
    .iter()
    .enumerate()
    .filter(|&(i, &byte)| byte != ((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8)
    .count();
```

Three things here are worth a sentence each.

- **Why a formula and not zeroes.** Zeroes would pass if the loader skipped the bytes entirely and the memory happened to be clear. The multiply-and-shift (a well-known way to scramble a counter) gives bytes that look random and that I can recompute anywhere.
- **`include_bytes!`** puts a file's contents into the program as a byte array, at compile time. `env!("OUT_DIR")` is the directory cargo gives a build script to write into.
- **`black_box`** tells the compiler "pretend you know nothing about this value". Without it, an optimizer that could see both the data and the check might work out `wrong == 0` while compiling, and the kernel would test nothing at run time.

`make big64-test` builds that kernel, puts it on a CD image together with a ramdisk of 8 KiB of random bytes, and boots it twice:

| Boot | Must print |
|---|---|
| BIOS | the `[test] big image` line, and `ramdisk 0x2000 bytes at 0x300000` |
| UEFI | the `[test] big image` line |

The ballast runs from `0x1d56d4` to `0x2956d4`: it *straddles* the old wall at `0x200000`. I did not arrange that; it is where the linker put it. But it is the best place it could be — an old-style ramdisk at 2 MiB lands inside it.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How to get room | The ramdisk moves into the dead staging buffer; the kernel may reach 3 MiB | *Stage the file at 16 MiB and let the kernel grow to there.* Fourteen megabytes instead of two — and it changes where the 32-bit kernel's file is read to, on a path with no test. Worth doing the day the 32-bit kernel is retired or tested. |
| | | *Shrink the kernel instead.* I did, as a stopgap in entry 47 (half the log queue). It buys weeks. |
| | | *Load segments straight from the CD to their places, no staging.* The right design; the file system code reads whole files, and that is a bigger change than this problem. |
| The 32-bit kernel | Loaded exactly as before | *The same new layout for both.* Its ramdisk address is compiled in. |
| Where the limit is checked | Still in the linker script, now 3 MiB | *Only in the loader.* The loader's refusal is a line on a screen nobody is looking at in a test. |
| Testing it | A real kernel with ballast, booted both ways | *A unit test of the loader's arithmetic.* The loader is C that runs in 32-bit protected mode with no test harness; and what I care about is that the bytes arrive. |
| The ballast's contents | A formula, checked byte by byte | *A checksum.* A count of wrong bytes says more when it fails: 8153 wrong is a ramdisk; 786432 wrong is something else. |

## Bugs

None in the change: it booted and passed the first time, which for two changed lines of C and one of linker script is not much of a boast.

What the test found when I broke the loader on purpose is below, and one of those results taught me something about my own tools.

## How I checked it

**Automated** (`make test64`, which now includes `big64-test`):

- The big kernel, BIOS and UEFI, as described.
- Everything else, unchanged, with the ordinary kernel — which is loaded by the same changed loader. The debug log's queue is back to 64 KiB.

**By hand** — undoing each half of the change and running `make big64-test`:

| Change | What happened |
|---|---|
| The old limit back in `elf.c` | The loader refuses the kernel; nothing on the serial port; `FAIL: BIOS: the big kernel did not boot, or was not loaded whole` |
| The ramdisk back at 2 MiB | The kernel boots — the ramdisk landed in data, not code — and reports `[test] FAILED - big image: ends at 0x2c3000, 8153 of 786432 bytes wrong` |

8153 wrong, for a ramdisk of 8192 random bytes: a random byte equals the one it replaced one time in 256, so about 32 of them should go unnoticed, and 39 did. The test did not just fail; it measured the damage.

The first row has a wrinkle. A loader that refuses a kernel prints its reason on the screen and stops — and to `qemu-boot.sh` that looks exactly like the firmware dying before the kernel ran, which since entry 46 it answers by starting QEMU again. So a refused kernel is now retried twice before it fails. The result is right and the message is not: it says *did not boot* where it could say *refused*.

**Not tested:**

- **The 32-bit kernel.** The claim is that nothing changes for it, and I have not booted it to see. The claim rests on reading: both changed lines choose the old value when `g_kernelIs64` is 0.
- **A real ramdisk with a 64-bit kernel.** The 64-bit kernel does not use one; the test's is random bytes, and the kernel only prints where it is. The address is right; nothing reads the contents.
- **A machine with little memory.** The loader writes to 3 MiB and above without asking the BIOS's memory map whether that is RAM. It did before, too.
- **A kernel between 3 MiB and the next wall.** The linker refuses to build one.

## What is still missing

- **The wall is at 3 MiB, not gone.** The kernel is 1 MiB today, so that is two megabytes of room — years at the rate so far, weeks if I start embedding things carelessly.
- **The programs inside the kernel.** Nearly two thirds of the image. Now that there is a disk and a file service, most of them could be loaded from it; only what is needed to reach the disk has to ride along.
- **The boot test cannot tell "refused" from "never started".** The loader would have to say its reason on the serial port too.
- **The slow start.** Nine and a half seconds pass before the kernel's first line (entry 47). Most of it is the loader reading the CD a sector at a time.

## What I learned

- **Limits nobody chose are the ones that surprise you.** "1 MiB" was written nowhere. It was the distance between two constants.
- **Ask what is dead.** The room I needed was already there, holding a file nobody would read again. Seeing it took drawing the three steps in order and asking, after each, what was still in use.
- **An address that is passed can move; an address that is compiled in cannot.** The 64-bit kernel needed no change because it is *told* where its ramdisk is. The 32-bit kernel pins the old layout in place because it *knows*.
- **Check the limit where the error can be read.** The assertion in the linker script turned a boot that silently never starts into one line with a reason. That line is why this took a morning and not a week.
- **Test with data that cannot pass by accident.** Zeroes pass when nothing was loaded. A formula does not.
- **`black_box`**: when a test's data and its check are both known at compile time, the optimizer may run the test for you, once, at build time.

## Testing note

```
make big64-test
```

It builds a second kernel (a minute the first time) and boots it twice. You should see, at the end:

| Look for | You should see |
|---|---|
| The last line | `big64-test passed` |

and in the two logs:

```
grep -a "big image\|ramdisk  " build/big64-bios.log build/big64-uefi.log
```

| Look for | You should see |
|---|---|
| `build/big64-bios.log` … `ramdisk` | `ramdisk        0x2000 bytes at 0x300000` — at 3 MiB, above the kernel |
| `build/big64-bios.log` … `big image` | `[test] big image: the kernel ends at 0x2c…, above 2 MiB; 768 KiB of it at 0x…-0x… is byte for byte what was built` |
| `build/big64-uefi.log` … `big image` | The same line (and `ramdisk        none`: the UEFI loader carries no ramdisk) |
| `FAILED` | Nothing |

**The ordinary system** — it is loaded by the changed loader too

```
make run64
```

| Type | You should see |
|---|---|
| nothing | The system starts as before, to the shell's prompt |
| `ls`, `primes`, `ps` | As before |

**To see the wall itself** (optional): in `kernel/kernel64/linker.ld` change `KERNEL_LOAD_LIMIT = 0x300000;` to `0x100000;` and run `make kernel64`. The build should stop with `the kernel image ends above 3 MiB, where the bootloader refuses to load it`. Change it back.

Automated: `make test64`
