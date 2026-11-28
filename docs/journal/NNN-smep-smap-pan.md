# SMEP, SMAP and PAN: the kernel cannot run or touch a program's memory by mistake

*Journal entry #NNN · K84, the `memory` lane's first item, done by TheJat-WSL/ci with the user's OK: `ci`'s own day work was done, and `memory`'s session had nothing in progress. Written by day, with quick checks only.*

## What they are

Three protections the CPU gives, which the kernel had not turned on:

- **SMEP** (x86, Supervisor Mode Execution Prevention): the kernel faults if it ever runs code from a page a program can reach. A classic attack on a kernel puts code in its own memory, then tricks the kernel into jumping there; with SMEP that is a fault, not a takeover.
- **SMAP** (x86, Supervisor Mode Access Prevention): the kernel faults if it reads or writes such a page, unless it says it means to (the AC flag). A kernel bug that follows a pointer a program gave it, without checking, then faults instead of reading or writing what the program chose.
- **PAN** (AArch64, Privileged Access Never): SMAP's equal.

## How

- **They needed nothing lifted.** The item expected the bit to be lifted around the kernel's explicit copies (`user_pages`, `copy_from_user`, `copy_to_user`). But those copies already go through the pages' frames, in the kernel's direct map, never through the program's addresses (`with_user_pages`; the debugger's `debug_memory` too). So SMAP and PAN apply everywhere, all the time. The boots below are the proof: with all three on, everything ran as before.
- **x86** (`arch/x86_64/mmu.rs`): `enable_user_protection` asks CPUID leaf 7 (EBX bit 7 for SMEP, bit 20 for SMAP) and sets CR4 bits 20 and 21 for the ones the CPU has, after the kernel's tables are loaded. The other CPUs copy the boot CPU's CR4 as they start (`control()`), so they have them too.
- **The AC flag is a program's to set.** With AC set, SMAP is off for the kernel, until it is cleared. `syscall` clears it already (its flag mask). Exceptions and interrupts do not, so their common handler starts with `clac`, when SMAP is on (`clac` does not exist on a CPU without SMAP).
- **AArch64** (`arch/aarch64/mmu.rs`): `enable_pan` asks `ID_AA64MMFR1_EL1` (bits 23:20) and, if the CPU has PAN, sets it on every CPU as its vectors are installed. It also clears `SCTLR_EL1.SPAN`, so that every exception taken to the kernel sets PAN again: a program cannot leave it off.
- **The kernel says which are on:** `[memory] SMEP and SMAP on: ...` or `[memory] SMEP off, SMAP off: this CPU does not have both`, and `[memory] PAN on: ...` or `off`.

## The tests

QEMU's default CPUs have none of the three, so every test until now ran without them, and only a test that asks for QEMU's `max` CPU sees them.

- **`fault64-smep-test`, `fault64-smap-test`** (two new fault parts, with `-cpu max`): the kernel maps a page a program could reach at 64 GiB (one `ret`) and runs it, or reads it. Each must be a protection fault there:
  - `kernel instruction fetch at 0x1000000000: protection violation`
  - `kernel read at 0x1000000000: protection violation`
- **`protect64-test`:** a PC and an AArch64 machine, both with `max` CPUs, say the protections are on and run the self-tests and typed programs: `ls`, `cat welcome`, `chello one two`, `ps`.
- **And once, by hand: the whole typed boot sessions with the protections on.** `QEMU_ARGS="-cpu max" make boot64-test` and `CPU=max make aarch64-keys-test` both passed: the C programs, files, pipes, jobs and signals, with SMEP and SMAP on, and with PAN on.

## Found on the way

**The SMEP test faulted with "page not present", on another CPU.** The test thread had moved to a CPU whose CR3 held a program's tables, which do not have the page mapped into the kernel's. Now the test turns interrupts off, so the thread cannot move, and loads the kernel's own tables before it touches the page. The SMAP test had passed only because it stayed on its CPU.

## Not done

**PAN has no fault test.** On AArch64, while the kernel's own tables are loaded, the half where programs live is not looked up at all (`TCR_EL1.EPD0`), so the kernel cannot map a page of a program's for itself as the x86 test does. A test would need a real program and a system call made to touch its memory wrongly. PAN being on, and the system running with it, is `protect64-test`'s.

## In other lanes' files

- `arch/x86_64/mmu.rs`, `arch/aarch64/mmu.rs`: the protections (the `memory` lane's item, its files).
- `arch/x86_64/idt.rs`: `clac`.
- `arch/aarch64/mod.rs`, `arch/aarch64/smp.rs`: PAN on each CPU.
- `arch/x86_64/mod.rs`: the fault tests' user page.
- `main.rs`: the two fault tests.
- `docs/lanes.md`: K84 out of `memory`'s table, with the user's OK.

## Checked

- **New:** `fault64-smep-test`, `fault64-smap-test`, `protect64-test`.
- **By hand:** `boot64-test` and `aarch64-keys-test` with the protections on (above).
- **Quick checks:**
  - `kernel64-test`, `coverage`, `unsafe-check`;
  - `boot64-test` and `aarch64-test` on the default CPUs;
  - `fault64-page-test` and `fault64-nx-test`;
  - `debug64-test`: the debugger writes programs' code through the frames;
  - `crash64-test`;
  - `gdb64-test`: the stub writes the kernel's code through the direct map.
- The full `make test64` is for the night.

## Testing note

| Do | You should see |
|---|---|
| `make run64` on a CPU with SMEP and SMAP (QEMU `-cpu max`; an Intel CPU from Broadwell, 2014, or an AMD one from Zen, 2017, on) | `[memory] SMEP and SMAP on: ...` |
| `make fault64-smep-test fault64-smap-test` | `fault-smep: ok (kernel instruction fetch at 0x1000000000: protection violation)`, and `fault-smap`'s read |

Automated: `make protect64-test`, `make fault64-smep-test`, `make fault64-smap-test` (all part of `make test64`).
