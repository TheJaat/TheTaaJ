# Every device handed out the same way

*Journal entry #NNN · a step, MaK-WSL/devices' K75, Era 15's first item. Written on MaK-WSL/devices. Until now the kernel chose which devices got a driver - AHCI, NVMe and xHCI controllers by their class, Intel's e1000 cards by their numbers, virtio's by theirs - and knew for each which BAR its registers are in, which kind of interrupt suits it, and how much DMA memory its driver wants. Every new driver was a kernel change. Now the kernel hands every PCI function and every virtio place to init the same way - a description and a **device handle** - and a driver asks the handle for what its device needs. The kernel holds no list of device kinds or vendors.*

## Why

Era 15's "Done when" begins: "the kernel holds no list of device kinds or vendors, and a new driver is added without changing it". K75 is that half; K76 (a device manager) moves the choosing out of init into its own table and program.

## How it works

### The device handle

One new kernel object, `Object::Device` (`kernel/kernel64/src/device.rs`), and one system call, `DEVICE` (49: entry 170's debugger merged first, with 48), whose operations are `abi::device`'s:

| Operation | What the caller gets | The kernel keeps |
|---|---|---|
| `CONFIG_READ` | its configuration space, a byte, word or dword of the first 256 (PCI) | writing: the command register, BARs and MSI structures are the kernel's |
| `BAR n` | an I/O-ports object or an uncached memory object, and where it is | a memory BAR stops short of any MSI-X structure in it - unless the interrupt is MSI, given before (MSI-X is then off for good, its table inert) |
| `INTERRUPT kind` | `MSIX` or `MSI` (PCI), `WIRED` (a place in memory: the line the device tree names) | one interrupt per device, for good: asked again, the same one; another kind, refused |
| `DMA pages` | contiguous memory and its physical address | at most 512 pages per device |

The first BAR or interrupt asked for turns the function's decoding and bus mastering on (K77 will tie that to a handle). A BAR is sized at the bus scan, before any driver runs: sizing switches decoding off for a moment, which a device in use must never see.

### Who decides what

| Decision | Before | Now |
|---|---|---|
| Which devices get a driver | the kernel (`offered()`: classes, Intel's IDs, virtio's) and init again | init's table (`user/src/bin/init.rs`), the kernel hands everything |
| Which BAR, which interrupt kind | the kernel's `Kind` per device | the driver (`taaj::device::claim_pci`): blk and nic BAR 0 and MSI-X; ahci BAR 5, MSI-X else MSI; nvme BAR 0, MSI-X else MSI; e1000 BAR 0, MSI only (an 82574L's MSI-X mode needs IVAR setup it does not do); usb BAR 0, MSI else MSI-X |
| How much DMA memory | the kernel's `*_DMA_PAGES` | init's `DRIVER_DMA_PAGES`, by driver; init asks and keeps it across restarts |
| The boot log's names for classes | the kernel (`pci::Device::kind`) | gone: the kernel logs numbers, the drivers say what they took |

On AArch64 the kernel still looks at each place the device tree describes, to see whether a virtio device is there at all - that is the bus's own protocol, as PCI's scan is - and hands over every one it finds, not the first disk, the first card and four inputs. The `virtio` crate is an AArch64-only dependency of the kernel now; the PC's kernel has none.

### What the drivers changed

Their first startup handle is the device's handle, no longer its registers, and the interrupt handle is gone: each asks with `taaj::device::claim_pci` or `claim_mmio` right after reading its description, which fill in the description's register and interrupt fields as the kernel used to. Everything after - the drivers' own starts - is as it was. Each says what it took:

```
[ahci] 00:1f.2 8086:2922: registers 0xfebd5000+0x1000 (BAR 5), 132 KiB DMA at 0x..., MSI
[blk] 0xa003e00 virtio kind 2: registers in the page at 0xa003000, 192 KiB DMA at 0x4..., an interrupt
```

## Bugs

**1. Mine, caught before running: two interrupt lines for one AArch64 disk.** The kernel's own disk self-test drives the first virtio disk before init does; my first version made it a device object of its own. Both would have asked for interrupt 79, the kernel would have set up two lines for it, and an interrupt goes to the first line with that number - the self-test's, long finished: init's blk would never have heard its disk. One device object per place now, made once (`mmio_devices::hand_out`), the self-test using init's.

**2. A restarted driver could not get its interrupt back.** The first boots: `ahci64-test` - "the restarted ahci did not take the controller over" - and `usb64-test`'s stick, and `kill64-test`'s SATA PC. Each restart said `[ahci] the controller's registers or interrupt: InvalidArgument`, five times, and init gave up. A driver tries its kinds of interrupt in order: ahci MSI-X then MSI, usb MSI then MSI-X. The first start got the second kind; a restart asked for the first again, and the kernel - the device having an interrupt of another kind, for good - said InvalidArgument, which stops the trying. It says NotFound now: from then on the device has no interrupt of that kind, which is the truth, and the driver goes on to the one it had. Only a restart shows this, so only the tests that restart drivers found it.

**3. Mine: the AArch64 self-test drove a disk no driver speaks.** `aarch64-test`'s `modern` boot (a virtio disk offering only the newer interface) panicked in the disk self-test: blk "would not start: only the legacy register interface is spoken here". The kernel's `prepare` used to refuse that disk; the self-test, picking the first block device itself now, did not. It takes only one of the legacy interface; and the line saying that no driver speaks the modern one is init's now, which picks the drivers.

**4. Not a leak: a crash report still being written.** With main merged in (entries 162-165), `aarch64-keys-test` failed three runs in a row: "running programs left processes, threads or channels behind", `ps` saying 31 channels once and 32 (or 33) the other time. Plain main passed it; main at entry 164 with K75 passed it; so it took entry 165 (K138, crash reports) and K75 together. The events of a failing and a passing run were the same 387, in the same order - and the extra channel was in the *first* `ps`, not the last. The test crashes the keyboard driver first thing (F12); since K138, logd then writes a crash report to the disk, and a write under way holds a channel. On main it is done before the first `ps`; with K75 the disk's driver starts a little later - it asks for its BAR and interrupt itself now - and the write was still going. A race in the test, then, which K75's timing showed: logd now says `[logd] crash report: PATH` once a report is on the disk, and the typed session (`TEST_KEYS`: the BIOS, UEFI and AArch64 boots) waits for the crashed driver's (`see:-kbd-` on a PC, `-serial-` on AArch64) before its first `ps`. Plain main has the race too: one of four `aarch64-keys-test` runs there failed the same way (32 channels at the first `ps`); K75 made it likelier, not possible. My first try waited for any report: the kernel's self-tests crash a dozen programs at boot on purpose (cli, peek, memro, echod), logd writes their reports first, and the first of them was on the disk long before the driver's - the next run failed the same way, 33 channels.

**5. Not explained: blk stalled while bigmem started its threads.** In the same runs, `aarch64-bigmem-test` failed twice and once more in a bisect (main at entry 164 with K75): init's watchdog found blk not answering for 2 s while bigmem started 60 threads with 480 MiB of stacks, restarted it, and the new one took the disk over at once; bigmem passed its 23 checks every time. The bisect pointed at entry 164 (the crash record), but then bigmem alone passed four times out of four on K75 with main merged, and four out of four on main, alternating, and in the next `make test64`. Each failure was in a run where it shared the machine with other AArch64 boots. It looks like a wake-up or scheduling race under load - K192 and K?-slow-kill are of that family - and not this item's: K75 changed nothing in blk's waiting. Left open as K?-blk-stall in `sched`.

**6. Not explained: once, two channels more at the session's end.** After bug 4's fix, one `aarch64-keys-test` run of ten on K75 had 33 channels at its *last* `ps` against 31 at its first; the other nine, and seven on plain main, matched. Nothing was being written then, no driver restarted in the seconds before; `ps` cannot say whose the channels are. Left open as K?-ps-channels in `sched`, with bug 5: a count of channels per process in `ps` is the tool it needs.

**7. Another race since the system log: `disk check` on a disk being written.** The next `make test64`: `aarch64-keys-test` - "disk check: FAILED - sectors read together differ from sectors read one by one". The shell's check reads the disk's first 256 sectors at once, then one at a time, and compares. Those are TaajFS's superblock and the blocks after it, which every commit rewrites, and since K136 logd commits the system log in the background - here just after the test's serial stalls, which log a few lines. The check now reads the 256 sectors at once again after the single reads: if those changed, so did the disk, and it tries again (five times at most); if they did not, the single reads must match them - so a driver that really returns different bytes still fails it. Not K75's, but it is where it showed. Main fixed it the same way meanwhile, and at the next merge (entries 166-170) main's version was kept.

**8. Not reproduced: the AArch64 disk self-test's interrupts, once.** The `make test64` after bug 7's fix: `aarch64-test`'s disk boot panicked, "the disk interrupted 8 and 101 times: the driver is not waiting for it" (at least 20 each). The disk itself was fine - 348 checks, twice. An interrupt that comes while the line is off shares the next one, so a starved emulated CPU sees far fewer; `aarch64-test` alone, twice, gave 340 each time, as plain main does. A load outlier, left as it is.

## How I checked it

- **Host**: `kernel/abi` (a `Bar` through its bytes, `config`'s argument), the PCI library's tests (`kind()` gone).
- `make coverage` (`kernel/abi` 88.3%), `make unsafe-check`.
- The device tests, each driver started and, in most, killed or stalled and restarted: `boot64-test`, `ahci64-test`, `nvme64-test`, `usb64-test`, `e1000-64-test` (its four cards), `net64-test`, `aarch64-test`, `aarch64-net-test`, `kill64-test` - all passed, after bugs 2 and 3.
- `make test64`, before main moved: all passed (735 s). With entries 162-165 merged in, five runs, each failing one or two AArch64 parts, each time a different one: bugs 4 and 7 (races since the system log, fixed), 5, 6 and 8 (not reproduced alone: 4 of 4 bigmem, 6 of 6 keys, 2 of 2 `aarch64-test` passed; left open or as they are). Every part passed in at least one full run and when run alone.

## What is still missing

- **Configuration-space writes**: not offered. No driver needs them yet; when one does, the kernel must keep the BARs, command register and MSI structures to itself.
- **K76**: init's table is still init's code, one disk and one card at a time. The device manager makes it a table of its own, with several devices of a kind.
- **K77**: bus mastering stays on once turned on.

## Testing note

```
make boot64-test ahci64-test nvme64-test usb64-test e1000-64-test aarch64-test
```

| Look for | You should see |
|---|---|
| `[pci] N of N functions handed to init` | every function on the bus, each with a device handle |
| `[ahci] ... (BAR 5), 132 KiB DMA ..., MSI` | the driver's own choice of BAR and interrupt |
| `[nvme] ... +0x2000 (BAR 0) ... MSI-X` | the BAR short of the MSI-X table |
| `[virtio] 0xa003e00: interrupt 79 set up for its driver` | on AArch64, the line set up when blk asks |

Automated: `make test64`
