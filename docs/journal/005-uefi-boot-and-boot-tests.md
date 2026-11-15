# One ISO, Two Firmwares: Booting by UEFI Too

*Journal entry #5 · and boot tests that stop when the kernel is done*

The vision (entry #1) says TheTaaJ must boot from both legacy BIOS and UEFI. Until this step it booted one way. This entry adds the second — without the kernel changing — and tightens the boot tests so that running them stopped being something I avoided.

---

## Why the kernel did not have to change

Entry #1 put one idea at the centre of booting: **every bootloader fills in the same `TaajBootInfo` structure**, and the kernel starts from that structure and never knows how it was booted.

This step is the first real test of that idea. The BIOS path and the UEFI path have almost nothing in common before the handoff:

| | BIOS | UEFI |
|---|---|---|
| Starts in | 16-bit real mode | 64-bit mode already |
| Loader is | raw boot sectors and a second stage | a PE executable, `BOOTX64.EFI` |
| Reads files via | its own disk and file system code | firmware services |
| Memory map from | BIOS call `E820` | `GetMemoryMap` |
| Screen from | VBE | Graphics Output Protocol |

And after it they are identical: `RDI` holds a `TaajBootInfo`, jump to the kernel. The only kernel change was *naming a new memory type* in the printed memory map — memory the UEFI loader itself occupies — and reporting which method was used. That it was so little is the result I wanted.

The UEFI loader is written in C, like the BIOS one, and shares the ELF loading and boot-info code with it. It needs Clang and `lld-link` to build, because a UEFI application is a Windows-style PE file.

## One image that boots both ways

I did not want two ISOs. El Torito, the CD boot standard, allows **several boot entries**:

- BIOS firmware takes the first: my stage 1 loader.
- UEFI firmware looks for the entry marked EFI: a small **FAT image** embedded in the ISO, holding the loader at `EFI/BOOT/BOOTX64.EFI` — the path firmware tries when nothing else is configured — plus its own copy of the kernel.

So `image64.iso` is a hybrid. If Clang is not installed, the build makes a BIOS-only ISO and says so, rather than failing.

## Checking that the test tests what it claims

`make boot64-uefi-test` boots the same ISO under OVMF, QEMU's UEFI firmware. There is a trap here: if the UEFI entry were broken, could the test pass anyway by quietly booting through BIOS? To rule that out, the kernel prints the boot method it was told, and the test **requires** `(UEFI)` in that line (and the BIOS test requires `(BIOS)`). A test that can pass for the wrong reason is worse than no test.

The checks both tests share moved into one script, `check-boot.sh`, so the two cannot drift apart.

## Boot tests that do not waste time

The first boot test ran QEMU for a fixed number of seconds and then looked at the log. That is wrong in both directions: too short and a slow machine fails a good kernel; too long and every test run costs that long, so I run them less.

The kernel never exits, so "done" has to be read from what it prints. `qemu-boot.sh` now watches the serial log and stops QEMU the moment it sees the kernel reach its end state (or a panic, or a CPU exception). The time limit remains only as a guard against a kernel that hangs silently.

A boot test went from a fixed wait to about as long as the boot actually takes. That sounds like a convenience. In practice it is what makes "boot every configuration on every change" a habit rather than an intention.

## What I learned

- **A handoff structure is a real abstraction only once a second producer exists.** With one bootloader it was a claim. With two it is a fact.
- **Make tests prove the path, not just the outcome.** Requiring the boot method in the log is one line, and it is the difference between "it booted" and "it booted *via UEFI*".
- **Slow tests do not get run.** Time spent making the test loop fast is repaid on every later step.

---

## Testing note

```
make boot64-test        # BIOS
make boot64-uefi-test   # UEFI (needs clang, lld and the ovmf package)
```

Each should end with `… passed`. To see it yourself:

```
make run64-uefi
```

| Look for | Meaning |
|---|---|
| A firmware splash (TianoCore) in the QEMU window before TheTaaJ appears | It really is UEFI firmware booting |
| A loader line in the terminal log containing `(UEFI)` | The kernel was told it was booted by UEFI |
| The same line with `(BIOS)` under `make run64` | Same ISO, other path |
| Everything after that line identical in both | The kernel does not care how it was booted |

If `make run64-uefi` says there is no UEFI loader in the ISO, install the tools it names (`sudo apt install clang lld`) and rebuild.
