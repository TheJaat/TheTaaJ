# Two machines: The-Jat and MaK, and what the second one found at once

*Journal entry #98 · a working step, not a feature. TheTaaJ is now built on two machines:
- **The-Jat**, the Ubuntu virtual machine in VMware where everything so far was written;
- **MaK**, my real PC, with Ubuntu under WSL2 and KVM.

Both push to the same three repositories. So I wrote down who does what, and the rules for working side by side, in one file: `docs/who-does-what.md`. Running at full speed under KVM, MaK found three bugs on its first day that every emulated boot before had missed. This entry explains them too.*

## Why

One machine, one line of work: until now, each step waited for the one before it. A second machine doubles what can be done in parallel, but only if the two don't step on each other:
- two machines editing the same file make conflicts;
- two "entry 098"s make a muddle;
- a merge into `main` that wasn't tested breaks the other machine's next pull.

The machines are also different in useful ways. MaK has KVM, so its x86_64 boots run at the CPU's own speed. The-Jat has more memory and the reference versions of the host's tools (Ubuntu 24.04's LLVM 18.1.3 and glibc 2.39), which the byte-for-byte tests compare against.

## How it fits together

```
                  GitHub: TheTaaJ, TheKernel, TheBootloader
                   main  ◄── merged only after test64 and the user's yes
                  ╱    ╲
   The-Jat (VMware, no KVM)          MaK (the real PC, WSL2 + KVM)
   branch per step: std-ground-1 …   branch per step: timing …
   Era 9: the road to rustc          real hardware, races, ACPI, network
   (J items)                         (K items), owns TheBootloader
                  ╲    ╱
          docs/who-does-what.md: every item, owner, order, what
          waits for what, Done (entry NNN) - read after every pull
```

`docs/who-does-what.md` has:
- **the machines**: their hardware, and times measured on both;
- **the rules**: a separate branch for everything; nothing merged into `main` without the user's yes and a full test; the same commit author, `The-Jat`, on both; work in dependency order; journal numbers taken when a step is merged;
- **what waits for what**: the items others depend on, most-waited-for first;
- **the order**: The-Jat's steps A to J, toward rustc on TheTaaJ, and MaK's steps A to I;
- **every item**: J1-J54 for The-Jat, K1-K48 for MaK, E1-E8 for either, U1-U6 waiting for me.

Two small build changes came with it:
- **`make run64` (and `run64-uefi`, `run64-stick`, `run64-stick-uefi`) use KVM when `/dev/kvm` can be opened**, and emulate otherwise. `RUN_ACCEL=tcg` forces emulation; `TAAJ_ACCEL` sets it too.
- **`tools/setup-wsl.sh`** makes a fresh WSL2 Ubuntu ready: packages, Rust, `/dev/kvm` for the user, a clone, the i686 cross compiler, a first ISO. It's how MaK was set up.

## Design decisions

- **Split by kind of work, not by part of the code.** The-Jat takes the long road to rustc, because its builds run for hours unattended and it has the reference tools. MaK takes what needs speed or real hardware: timing races, the real PC, the bootloader. A split by directory (say, "kernel" and "libc") would make both machines wait on each other for every feature, since most features cross both.
- **One file for all work items.** A list in two places drifts. The file is changed in the same branch as the work, so when a step is merged, the plan and the code arrive together.
- **Journal numbers are taken at merge time, not reserved in ranges.** Ranges would leave gaps and put the journal out of order. The cost is a rename now and then.
- **MaK owns TheBootloader.** It's the part closest to the hardware, and both of its bugs so far were found there.
- **Order by dependency, with the most-waited-for item first.** `getrandom` is small, but five items on both machines wait for it (Rust's std, address randomization, random TCP numbers, TLS, `random_device`), so it's The-Jat's first item.

## Bugs

All three were found by MaK, by running `make test64` under KVM for the first time.

**1. The bootloader triple-faulted on 5 boots out of 8 under KVM.** Stage 2 switches from real mode to protected mode twice: once to copy the kernel above 1 MiB, and once for good. Both switches set the PE bit in CR0 and then far-jump into 32-bit code. Between those two instructions, the CPU is in protected mode but its interrupt table is still the real-mode one, a list of 4-byte segment:offset pairs at address 0. If an interrupt arrives in that window, the CPU reads that table as protected-mode gate descriptors. That's garbage, which causes a fault, then a double fault, then a triple fault, and the machine resets. Interrupts were on at the second switch, because `VesaFinish` ran with them on just before. The fix is a `cli` before each `mov cr0` (TheBootloader `73d962c`). Emulated, QEMU only delivers interrupts between blocks of translated code, and the window is a few instructions inside one block, so it almost never hit. Under KVM, the timer is real time, and the window is real too. VMware and a real PC can hit it as well.

**2. `check-boot.sh` missed `cprimes` answers on a fast machine.** The test starts four `cprimes` at once in the background and counts four answers in the log. It looked for each answer at the start of a line. Under KVM, the copies finish while the test is still typing the next command, and the typed echo comes first on the line an answer ends. The fix counts the answers wherever they are (TheKernel `4928804`). It was a test bug, not a kernel bug. A slow machine had hidden it, because the answers always came after the typing.

**3. `sscanf("0x", "%x")` read a 0.** C says a conversion reads the longest prefix that *could start* a number. For `%x` on `"0x"`, that prefix is `"0x"` itself, which isn't a number, so it's a matching failure and the count is 0. Ours used `strtoull`, which takes the `0`. MaK's host has glibc 2.43, which does what C says. The-Jat's glibc 2.39 also takes the `0`, so `scan_test`, which compares ours with the host's, passed on one machine and failed on the other. Ours now returns a matching failure for a bare `0x`, `%x` and `%i` alike. `scan_test` checks those inputs against C instead of the host (`408eba9`).

## How I checked it

- `make test64` passed on The-Jat with all three fixes merged into `core-basics`.
- MaK ran its timing comparisons under KVM and emulated. The-Jat ran `smp64-test`, `thread64-test` and `bigmem64-test` emulated. The times are in `docs/who-does-what.md`.
- `make run64` on The-Jat (no `/dev/kvm`) still starts QEMU with `-accel tcg`.
- `make libc-test` prints `scan: N checks, all the same as the host's sscanf (a bare 0x: as C says)` on both glibc versions.

## What is still missing

- **MaK's three commits carry a `Co-Authored-By` line** and the author name `TheJat`, without the hyphen. They are pushed, so they stay; from now on both machines commit as `The-Jat`.
- **The bootloader fix has no test that fails without it** under emulation. Only a KVM boot loop shows it (5 of 8 boots).
- **Nothing checks that the two machines follow the rules**: no hook rejects a direct commit to `main`, for example. It's discipline.
- AArch64 boots are emulated on both machines. Neither can run them at full speed.

## What I learned

- **The window between `mov cr0` and the far jump.** Protected mode begins the instant PE is set, but the CPU keeps the old segment caches until the jump reloads CS. It also keeps the old IDT until `lidt` gives it a new one. Anything that consults the IDT in that window (an interrupt, an NMI, an exception) sees the wrong format. Hence the rule in every OS's switch code: `cli` first. An NMI can't be masked this way, which is why some code also disables NMIs through port 0x70.
- **Emulators hide races.** QEMU's TCG checks for interrupts only between translated blocks, so a race inside a few instructions practically never happens there. KVM runs the real CPU, and interrupts arrive whenever the hardware says. A suite that passes emulated has only shown that it passes *at block boundaries*.
- **A test that compares with the host inherits the host's bugs.** `scan_test` was "ours equals glibc", so it was as right as glibc 2.39, which wasn't right for `0x`. Where the standard and the host disagree, the test has to state the standard's answer itself.
- **Two machines need written rules more than one machine does.** One person on one machine keeps the plan in their head. Two machines, each with its own assistant session, don't share a head. The file is the shared memory.

## Testing note

On The-Jat (no KVM):

```
make run64
```

| Look for | You should see |
|---|---|
| The QEMU command printed by make | `qemu-system-x86_64 -accel tcg ...` |
| The system | It boots as before |

On MaK (KVM), the same command prints `-accel kvm`, and the boot to the shell takes a second or two instead of several. To force emulation there, use `make run64 RUN_ACCEL=tcg`.

```
make libc-test
```

| Look for | Meaning |
|---|---|
| `scan: N checks, all the same as the host's sscanf (a bare 0x: as C says)` | `sscanf` agrees with the host everywhere, and with C on a bare `0x` |

```
make test64
```

| Look for | Meaning |
|---|---|
| `test64: all passed` | Every part, with the bootloader and `check-boot.sh` fixes |

The work split itself is in `docs/who-does-what.md`.

Automated: make libc-test, make test64
