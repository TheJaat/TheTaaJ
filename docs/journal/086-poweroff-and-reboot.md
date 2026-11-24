# Power-off and reboot: ACPI in user space, and an orderly shutdown

*Journal entry #86 · Era 7, step R3. TheTaaJ can now switch itself off and restart: `poweroff` and `reboot` in the shell. init first stops every service in an order that leaves the disk whole. Then a new program, `power`, switches the machine off through ACPI on a PC, or through the firmware (PSCI) on AArch64. In keeping with the microkernel, the kernel does not do the switching. It finds where the controls are and hands exactly those to `power`: the ACPI table with the AML in it (read-only), and the few I/O ports the FADT names. Reading the AML happens in ring 3. Testing it also turned up a bug from R1: a PC whose SATA controller has no disk left the file service waiting for ever. That is fixed too.*

## Why

A machine I use every day needs an off switch that does not cut the power under the disk: until now the only ways out were closing QEMU or pulling the plug. On a real PC, cutting the power in the middle of a write is how file systems get damaged. TaajFS's copy-on-write commits survive it, but a real SSD also wants to be told before power goes (entry 085's "shutdown notification"). And `reboot` is what every test cycle on real hardware will need.

How to do it was decided in the discussion before this step. TheTaaJ is a microkernel: drivers, the file system and the network all run in user space. So power control should too (option c). The kernel finds the controls; a program uses them.

## What it is

```
 kernel                          init                              power (user/src/bin)
 ------                          ----                              --------------------
 FADT: PM1 ports, reset, DSDT ─► "power" + PowerInfo + handles ─►  reads \_S5 from the DSDT (acpi crate)
 DSDT as read-only memory        keeps the only channel to power   waits for one byte: OFF or RESET
 (AArch64: a power handle)       shell: "power off" / "reboot" ─►  shut_down: stop services in order ─► OFF
```

- **Kernel (TheKernel):**
  - `kernel64::acpi::Fadt::parse`: the FADT's power fields;
  - `kernel64/src/power.rs`: hands the controls to init;
  - a new kind of object, `Object::Power`, and a system call, `POWER`, for AArch64, where only the kernel can call the firmware;
  - `abi::PowerInfo` and `abi::power`: the description and the requests.
- **`acpi` crate (TheTaaJ, beside `nvme/`):**
  - `sleep_type` reads `\_S5` out of AML;
  - `switch_off` and `reset` are the register sequences.

  All of it is host-tested against a simulated chipset.
- **`power` program:** starts at boot, reads `\_S5` at once (so a DSDT it cannot read shows up in the boot log, not when you try to switch off), then waits.
- **init:**
  - receives the "power" description and starts `power`;
  - holds the only channel to it;
  - on `poweroff` or `reboot`, stops the services in order, then asks `power`.
- **shell:** the `poweroff` and `reboot` commands.

## How ACPI switches a PC off

ACPI defines sleep states S0 (running) to S5 ("soft off": only the power button still works). Entering one is a write to the **PM1 control register**, an I/O port the FADT names (0x604 on QEMU):

| Bits | Name | Meaning |
|---|---|---|
| 0 | SCI_EN | power events go to the OS (ACPI mode) |
| 10-12 | SLP_TYP | which sleep state, in this chipset's numbering |
| 13 | SLP_EN | enter it now |

The catch is SLP_TYP. Every chipset numbers its states differently (S5 is 0 on QEMU's chipsets and 7 on many Intel ones), and the number is not in any fixed table. It is in the **DSDT**, written in **AML**, ACPI's byte code:

```
Name (\_S5, Package (4) { Zero, Zero, Zero, Zero })
08 5C 5F 53 35 5F 12 06 04 00 00 00 00
│  │  └─ _S5_ ─┘   │  │  │  └─ elements: ZeroOp ×4
│  └ root "\"      │  │  └ 4 elements
NameOp             │  └ package length (1 byte here)
                   PackageOp
```

The first element is SLP_TYPa (for PM1a), the second SLP_TYPb (for PM1b, a second register some old chipsets have). A full AML interpreter is thousands of lines. Reading one named constant needs only a few encodings:
- a number: `Zero` (0x00), `One` (0x01), or a prefix byte for a byte, word, double or quad word (0x0A-0x0E);
- the package length, which is 1 to 4 bytes (the lead byte's top two bits say how many follow);
- an old one-element form that packs both values in one number.

`sleep_type` decodes exactly that, and refuses everything else with a reason. One example is "`\_S5` is a method: needs an AML interpreter", which some firmware has.

The sequence (`switch_off`):
1. **ACPI mode.** If SCI_EN is 0, the firmware (System Management Mode) still owns power events. Writing ACPI_ENABLE to the SMI command port (0xB2 with value 0xF1 on QEMU) hands them over. Wait up to 3 s for SCI_EN.
2. **SLP_TYP** into each PM1 control register, keeping every other bit, and clearing the old SLP_TYP rather than OR-ing into it.
3. **SLP_EN** with it: this write is the moment the machine goes off.
4. Still running after 2 s? Say it did not work.

**Reset** is simpler: one byte to one port. The FADT may name a reset register (ACPI 2.0+; the q35's is 0xCF9 with value 0x0F). If it does not (the old PC's FADT is version 1), 0xCF9 with value 0x06 is the PC's reset control register, which every chipset since the 1990s has.

**On AArch64** there are no ports. PSCI, the same firmware interface the kernel already uses to start CPUs, has SYSTEM_OFF (0x84000008) and SYSTEM_RESET (0x84000009). These are `hvc` or `smc` instructions, which user space may not execute. So here the kernel does make the call, but only for the holder of a **power handle**, which only init receives and only `power` is given.

## The orderly shutdown

`poweroff` asks init (`abi::power::ASK_OFF` on init's request channel). init's `shut_down` then stops the services one by one, waiting up to 2 s for each to end:

1. **shell, hid, kbd/serial, net, nic:** everyone who could create new work.
2. **fs,** but only after it answers a ping. fs serves one request at a time, so an answer means it is between two requests. Every change it has acknowledged is already on disk: fs is write-through, and each request is one copy-on-write commit. Even if a stray program slips one more write in, a TaajFS commit lands whole or not at all.
3. **The disk driver** (blk, ahci or nvme), which now has nothing left to do.
4. **display, registry.**
5. **power** is asked last. It prints what it is about to do and waits 0.2 s. Program output is queued and written when a CPU is idle, so a machine switched off immediately would lose its last line. Then it writes the registers.

If `power` answers (it only answers on failure), init prints why and says "the services are stopped: it is safe to switch the machine off by hand". On a machine with no power control (no ACPI tables), `poweroff` is refused **before** anything is stopped, and the system carries on.

## The Rust in it

- **`#[cfg(...)]` on match arms.** In `power.rs` the ACPI arms of `act`'s `match` exist only on x86 (`#[cfg(target_arch = "x86_64")]`), because the `Ports` type that touches I/O ports only exists there. On AArch64 those arms are compiled out, and the catch-all `_` covers them. One source file serves both CPUs without dead code on either.
- **`#[cfg_attr(target_arch = "x86_64", allow(dead_code))]`.** `Object::Power` is never constructed on x86. Instead of hiding the warning everywhere, `cfg_attr` applies `allow(dead_code)` only when compiling for x86.
- **Closures that capture by reference.** In `Fadt::parse`, `gas` and `port` are small closures (`|at: usize| -> Option<(u8, u64)> { ... }`) that read the table `b` from the surrounding function. They make the "wide field if present and non-zero, else the 32-bit one" rule read as one line per register.
- **`Result` chains with `and_then`.** `Acpi::new(..).and_then(|acpi| acpi.find(..)).and_then(|t| Fadt::parse(&t))` stops at the first error and carries it out. The `?` operator does the same inside a function. `and_then` does it inside an expression.
- **A trait with a simulated twin, again.** `acpi::Ports` has `read16`, `write16`, `write8`, `pause` and `ticks`. The real one is `in`/`out` instructions; the test one is `Chipset`, which records every write and asserts that SLP_EN never comes before ACPI mode.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where AML is read | `power`, in ring 3 (the `acpi` crate) | In the kernel: AML is firmware-written byte code, the riskiest input there is. A mistake there should crash a program, not the system. Agreed in discussion as option (c) |
| What the kernel reads | The FADT only: fixed fields, like the MADT it already reads, needed to know *which ports* to hand on | Handing all I/O ports to `power`: it could then touch the disk controller or the PIC. It gets exactly the PM1, SMI and reset ports |
| AML support | One named constant (`\_S5`) by byte scan and a small decoder | A full AML interpreter (ACPICA is ~150,000 lines): needed later for battery, lid, buttons and `_PTS`, and then as its own user-space service |
| Who may switch off | Whoever holds init's request channel (the shell, and programs the shell lets start programs) | A separate capability per user: TheTaaJ has no users yet |
| Shutdown order | Stop the shell and the input drivers first, ping fs, then stop fs, the disk driver, and the rest | Killing everything at once: a disk write could be cut in half. Sending every service a "please stop" message: better, and the next step when services have state to save |
| AArch64 | A power handle and the POWER call (PSCI) | The kernel deciding by itself: then any process could ask the kernel |
| A failed switch-off | Say so, and leave the services stopped | Restarting everything: the user asked for the machine to stop; "safe to switch off by hand" is the honest answer |

## Bugs

- **init could not pass on the DSDT.** The first boot said `[init] could not start power: AccessDenied`. The kernel gave the DSDT handle READ only. init needs DUPLICATE and TRANSFER too, to hand a copy on; copies never carry more rights than the original. Now the kernel gives READ, DUPLICATE and TRANSFER, but never WRITE.
- **`power`'s last line was lost.** The log ended with init's "stopped registry", without `power`'s "switching off" line. A program's output is queued and written when a CPU is idle. The machine went off before that happened. `power` now waits 0.2 s after saying what it does.
- **fs waited for ever on a q35 with no SATA disk (a bug from R1).** The q35's own AHCI controller is always there, with only the CD on it. `ahci` finds no disk, ends, and after five restarts init gives up on it. But fs had already asked "how big is the disk?" and waited for an answer that would never come. During shutdown that showed up as "fs did not answer first". Now, when init gives up on the disk driver, it closes the disk channels' receiving ends. fs's pending question is dropped with its reply channel, fs hears "peer closed", and it carries on with files in memory, as with no disk.
- **The big-kernel test no longer linked.** `big64-test` builds a kernel with extra ballast, to check that the BIOS loader copies a kernel ending above 2 MiB byte for byte. With `power` embedded, the normal kernel ends at about 2.27 MiB. Adding 768 KiB of ballast went past 3 MiB, where the linker script stops the build, because the loader stages the file there. The kernel now passes 2 MiB without help, so the ballast is down to 512 KiB. The real lesson is a limit: **the kernel has about 750 KiB left before 3 MiB**, and every program embedded in it uses some of that. Before USB brings three more, either the programs move out of the kernel image (onto the boot disk) or the bootloader stages the file higher.
- **Two test mistakes:**
  - my "something failed" pattern matched `refused` in echoc's self-test line ("…were all refused");
  - in the kernel's FADT test, the memory-space PM1a address (0xFEC00004) was too big to be a port anyway, so the test passed without the address-space check. Mutation M10 showed that. The test now uses 0x1804 in memory space, which only the space check can refuse.

## How I checked it

On the host:
- `acpi`, 9 tests.
  - `\_S5` as QEMU writes it, surrounded by other AML (a scope, a device, a string that happens to contain `_S5_`, a method).
  - Every number encoding; a two-byte package length; the old one-element form; only three bits counting.
  - `_S3` not mistaken for `_S5`.
  - Refusals: a method, a non-package, a variable as an element, a cut-off package, an empty one.
  - The `Chipset` simulator:
    - the full off sequence (SMI write, SLP_TYP, then SLP_TYP|SLP_EN, other bits kept);
    - a machine already in ACPI mode;
    - PM1a and PM1b each getting their own type, with the old type cleared;
    - a chipset that never enters ACPI mode;
    - reset.
- Kernel: `Fadt::parse`, 3 tests, against FADTs of version 1 (116 bytes) and ACPI 5 (244 bytes):
  - the wide fields winning, and zero wide fields falling back;
  - the reset register only with its flag, and only in I/O space;
  - hardware-reduced ACPI, PM1 in memory, no PM1, no DSDT, short tables and wrong signatures refused.

  194 kernel tests in all.
- `abi`: `PowerInfo` round trip and handle count (28 tests).

By booting:
- **`power64-test`** (new, in `make test64`): five x86 machines at once.
  - **pc-off** (BIOS, virtio disk):
    - `write saved before-off`, then `poweroff`;
    - the services stopped in order (shell … fs, blk … registry);
    - the log's last line is `[power] switching off: sleep type 0/0 into PM1 control (ACPI S5)`;
    - QEMU ended by itself: its "terminating on signal" line, which the boot script's kill would leave, is absent;
    - the host reads `before-off` back from the disk.
  - **pc-reboot:** `reboot`, with QEMU told to reset instead of quitting (`-action reboot=reset`). The kernel starts a second time and reaches the shell.
  - **pc-none** (`acpi=off`): `[acpi] power: ... - no power control`; `poweroff` is refused; nothing is stopped; `echo still here` works afterwards.
  - **q35-off, q35-reboot** (UEFI): the FADT's own reset register (`0xcf9 <- 0xf`). `ahci` gives up on the diskless controller and fs carries on in memory.
- **`aarch64-power-test`** (new, in `make test64`): `poweroff` ends in the kernel's `[psci] SYSTEM_OFF` and QEMU ending by itself; `reboot` brings a second `[kernel] idle`.

Mutation experiments, on the host:

| Mutation | Result |
|---|---|
| M1: SLP_TYP not written before SLP_EN | 2 tests fail |
| M2: the old SLP_TYP OR-ed into, not cleared | 1 fails |
| M3: ACPI mode never switched on | 2 fail ("SLP_EN before ACPI mode") |
| M4: BytePrefix not understood | 2 fail |
| M5: package length always taken as one byte | 1 fails |
| M6: PM1b given PM1a's type | 1 fails |
| M7: the name's op not checked (a string's `_S5_` taken) | 3 fail |
| M8 (kernel): X_PM1a ignored | 2 fail |
| M9 (kernel): reset register taken without its flag | 1 fails |
| M10 (kernel): a memory-space PM1a taken as a port | **survived** at first (see Bugs); after the test was fixed, 1 fails |

And on QEMU (`make power64-test`):

| Mutation | Result |
|---|---|
| B1: SLP_EN never written | caught: `[init] could not finish switching off: the machine is still on after SLP_EN. The services are stopped: it is safe to switch the machine off by hand`, and QEMU still running |
| B2: init does not close the disk channels when it gives up on the driver (the R1 bug, put back) | caught on q35: "fs did not go on without the disk driver that gave up" |

clippy: `acpi` (all targets), `user`, the kernel (x86 and AArch64), `abi`; clean.

`make test64`: all passed, in 13 min 9 s.

The testing note below was run by scripted boots on fresh disks (`build/note-power.img`, made by the same `run-disk` rule, and `build/aarch64/note-power.img`), all four parts. Its outputs are copied from those boots.

Not tested:
- **On real hardware.** Only QEMU's chipsets (PIIX4 and ICH9), whose S5 type is 0. A real Intel chipset uses 7; the decoding is host-tested, but no real SLP_TYP 7 has been written.
- **The SMI handover.** QEMU's firmware leaves the machine in ACPI mode already, so the SMI write ran only against the simulator.
- **PM1b.** No QEMU machine has one.

## What is still missing

- **`_PTS` and `_GTS`.** ACPI says the OS should run the `\_PTS` ("prepare to sleep") method before entering S5. Some laptops need it, for example to turn off a device that would otherwise wake the machine. That needs an AML interpreter.
- **The power button.** Pressing it raises an SCI interrupt with a status bit in PM1 *event*. TheTaaJ does not listen yet, so a short press does nothing (a long press still cuts the power).
- **Telling services to stop.** Services are stopped by kill, not asked. For now that is safe (fs is write-through, between requests). An NVMe controller would like a shutdown notification (CC.SHN), and a future cache would want flushing: both need a "you are stopping" message before the kill.
- **Hardware-reduced ACPI** (most ARM laptops, some new x86 tablets): no PM1 registers, a sleep control register instead. Refused for now.
- **Room in the kernel image.** About 750 KiB is left below the BIOS loader's 3 MiB limit (see Bugs).
- **Anyone with init's request channel can switch off.** Fine without users; later, a capability of its own.

## What I learned

- **Microkernel means "the kernel knows where, not how".** The kernel's share of power-off was reading fixed fields, then handing on one memory object and four port ranges. Everything that interprets the firmware's program stays out of ring 0.
- **The last line is the hardest to keep.** A machine that switches itself off takes its unwritten log with it. Asynchronous output is right for performance, and wrong for a last word, so the last word now waits.
- **A new feature's tests find old bugs.** The diskless q35 had been "working" since R1: fs simply waited. Only when shutdown needed fs to *answer* did the hang show. Every feature that depends on a service responding is a test of that service.
- **A test can pass for the wrong reason.** M10 survived because the test's address failed a different check with the same message. Mutation testing is how I found out what my tests actually prove.

## Testing note

**1. Switch off, with a file written just before**

```
make run64 RUN_DISK=build/power-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[acpi] power: PM1a control 0x604, reset 0xcf9 <- 0x6 (the PC's; the FADT names none), DSDT ... bytes at 0x... - handed to init` | the kernel read the FADT and handed on the controls |
| `[init] started: registry display fs blk nic net shell kbd power; supervising` | init started `power` |
| `[power] ACPI: off is sleep type 0/0 (\_S5); reset is 0x6 into port 0xcf9; ready` | `power` read `\_S5` from the DSDT |

In the QEMU window:

| Type | You should see |
|---|---|
| `write note before-off` | `note: 11 bytes` |
| `cat note` | `before-off` |
| `poweroff` | `Switching off...`; in the log, `[init] switching off: stopping the services`, then `[init] stopped shell`, `kbd`, `net`, `nic`, `fs`, `blk`, `display`, `registry`, and last `[power] switching off: sleep type 0/0 into PM1 control (ACPI S5)`. The QEMU window closes by itself, and `make` returns |

Then on the host:

| Type | You should see |
|---|---|
| `tools/tfstool-rs/target/release/tfstool get build/power-disk.img note` | `before-off` |

**2. Reboot**

```
make run64 RUN_DISK=build/power-disk.img
```

| Type | You should see |
|---|---|
| `reboot` | `Resetting...`; in the log, `[init] resetting: stopping the services`, the services stopped, `[power] resetting: 0x6 into port 0xcf9`, then `Hello from TheTaaJ!` again: the machine starts over, in the same window |
| `cat note` (once the shell is back) | `before-off` |

**3. A q35 under UEFI: the FADT's own reset register**

```
qemu-system-x86_64 -machine q35 -m 512M -bios /usr/share/ovmf/OVMF.fd -cdrom build/image64.iso -serial stdio \
    -drive file=build/power-disk.img,format=raw,if=none,id=d0 -device virtio-blk-pci,drive=d0
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[acpi] power: PM1a control 0x604, reset 0xcf9 <- 0xf, DSDT ... - handed to init` (no "the PC's": this FADT names its reset register) |
| `cat note` | `before-off` |
| `poweroff` | the services stopped, `[power] switching off: ...`, and QEMU ends by itself |

**4. AArch64: PSCI**

```
make run-aarch64 RUN_A64_DISK=build/aarch64/power-disk.img
```

(A new disk of its own; your `build/aarch64/taaj-disk.img` is not touched.)

| Look for / Type | You should see |
|---|---|
| (the log) | `[psci] power: SYSTEM_OFF and SYSTEM_RESET - handed to init` and `[power] PSCI: SYSTEM_OFF and SYSTEM_RESET; ready` |
| `write note arm` | `note: 4 bytes` |
| `poweroff` | `Switching off...`, the services stopped, `[power] switching off (PSCI)`, `[psci] SYSTEM_OFF`, and QEMU ends by itself (no Ctrl+A X needed) |

**5. The tests by themselves**

```
make power64-test
make aarch64-power-test
make test64
```

| Look for | Meaning |
|---|---|
| `poweroff and reboot worked on a PC and a q35: ...` | the five x86 machines |
| `poweroff and reboot worked on AArch64: ...` | the two AArch64 machines |
| `test64: all passed` | every part |

Automated: make power64-test
