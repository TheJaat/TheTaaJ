# USB sticks: mass storage, and a stick as the system's disk

*Journal entry #89 · Era 7, step R6a. The `usb` driver now drives USB sticks too: SCSI commands carried by the Bulk-Only Transport, in a new module of the `usb` crate (`storage`). When the machine has no other disk, init gives `usb` the disk's request channels. fs then finds TaajFS on the stick, exactly as it does on a virtio, SATA or NVMe disk. The watchdog can stop and restart `usb` while it serves the disk, and `poweroff` stops it after fs. Booting *from* the stick is the next step (R6b).*

## Why

Era 7 ends with "it boots from a USB stick on my PC and I use it for a day". That has two halves:

1. once the kernel runs, the stick must be its disk: files read and written, programs loaded;
2. before that, the firmware and the loader must read the kernel from the stick.

This entry is the first half. It sits on top of entry 088's controller driver: a stick is just another device on the xHCI controller, with two bulk endpoints in place of a keyboard's interrupt endpoint.

## What it is

```
 shell / fs                  init                              usb (user/src/bin)                      the stick
 ----------                  ----                              ------------------                      ---------
 abi::block requests   ─►    no other disk: the disk's   ─►    block_service::serve_one,         ─►    CBW (31 bytes)
 on "disk" / "disk.look"     two channels go to usb,           from usb's own wait loop;               data (up to 128 KiB)
                             with the controller's             storage::Storage::transfer:             CSW (13 bytes)
                             description                       READ(10)/(16), WRITE(10)/(16)
```

- **`usb/src/storage.rs` (new):** `Storage`, a stick ready for READ and WRITE.
  - `start` asks the stick what it is: INQUIRY (names, removable), TEST UNIT READY until ready, READ CAPACITY (size), MODE SENSE (write-protected?).
  - `transfer` reads or writes up to 256 sectors.
  - `command` runs one SCSI command through the three steps of the Bulk-Only Transport, and recovers when one goes wrong.
- **`usb/src/xhci.rs`:**
  - `configure_bulk`: a bulk OUT and a bulk IN endpoint, in one Configure Endpoint command;
  - `bulk`: moves bytes over one of them and waits;
  - Stop Endpoint, for a transfer that never finishes;
  - 129 KiB more DMA memory for the stick's buffers.
- **`usb/src/descriptor.rs`:**
  - interfaces now record their bulk endpoints;
  - two new requests: CLEAR_FEATURE(ENDPOINT_HALT) and the class's Bulk-Only Mass Storage Reset.
- **`usb/src/device.rs`:** `attach(controller, port, storage_wanted)`. A device that is not a keyboard or mouse but is mass storage / SCSI / Bulk-Only gets its bulk endpoints configured and `Storage::start`ed. `Attached` now carries `storage` (the stick) or `storage_problem` (why not).
- **The simulator (`usb/src/tests/sim.rs`):**
  - `Disk`, a SCSI disk behind the Bulk-Only Transport. It checks every CBW, and can stall a data step, garble a status, go silent, have no medium, be write-protected, or have 4096-byte sectors.
  - The simulated controller learned bulk endpoints and Stop Endpoint.
  - 10 new tests in `usb/src/tests/storage.rs`; 24 in the crate.
- **`taaj::block_service`:**
  - `serve_one` answers a single request, for a driver that waits on more than the disk's channels;
  - `Device::present()`, a default method;
  - a new status, `abi::block::NO_MEDIUM`, "nothing in the drive".
- **`usb` (program):**
  - Its startup description can come with two handles, the disk's channels. It waits on them together with the controller's interrupt and init's pings, and answers each request with `serve_one`.
  - It implements `block_service::Device` for the stick.
- **init:**
  - `usb_disk`: no other disk, and a USB controller. The disk's channels are then kept open, not closed.
  - `disk_driver` is `"usb"`.
  - The disk watchdog (INFO on `disk.look`) watches `usb` in place of its pings.
  - At `poweroff`, `usb` is stopped after fs, not with the keyboards.
- **Kernel (TheKernel):**
  - `XHCI_DMA_PAGES`: 69 → 102 pages (408 KiB);
  - `abi::block::NO_MEDIUM`.
- **Makefile:**
  - `make run64 RUN_DISK_BUS=usb`: the run disk as a USB stick;
  - `make usb64-test` now boots five machines (below).

## How a USB stick is spoken to

A USB stick is, inside, a SCSI disk. SCSI is the command set of 1980s server disks: INQUIRY, READ(10), WRITE(10) and so on. Its commands still run nearly every storage device that isn't NVMe. USB carries them in the simplest way it can, the **Bulk-Only Transport** (BOT). Every command is three steps on the stick's two bulk endpoints:

1. **CBW**, the Command Block Wrapper: 31 bytes *out*. It holds "USBC", a tag, how many bytes the data step moves and which way, the LUN, and the SCSI command (6 to 16 bytes).
2. **Data**, if the command has any: *in* for READ and INQUIRY, *out* for WRITE.
3. **CSW**, the Command Status Wrapper: 13 bytes *in*. It holds "USBS", the same tag, the residue (bytes the data step did *not* move), and a status: 0 passed, 1 failed, 2 phase error.

"Failed" does not say why. The next command, REQUEST SENSE, does: a *sense key* (2 not ready, 3 medium error, 5 illegal request, 6 unit attention, 7 write-protected) and a more precise code. The first TEST UNIT READY of nearly every stick fails with "unit attention: the medium may have changed". That is the stick saying hello, so it is simply asked again.

When the stick does not want to go on with a data step (a read error, say), it **stalls** the endpoint: every packet is answered "halt". The host then:

1. resets the endpoint on the controller (Reset Endpoint, which the controller needs after any error);
2. clears the stall on the stick: CLEAR_FEATURE(ENDPOINT_HALT) to that endpoint. This also resets the DATA0/DATA1 sequence both sides keep;
3. reads the CSW, which says "failed", and REQUEST SENSE says why.

Anything worse is the **reset recovery**: a CSW that isn't one, a phase error, or a transfer that never finishes. The host sends the class's Bulk-Only Mass Storage Reset, then clears both endpoints' stalls. The stick is then back where a new command can start.

**The 64 KiB rule.** One transfer TRB may move at most 64 KiB and must not cross a 64 KiB boundary in physical memory. The 128 KiB data buffer starts 0x6000 short of such a boundary, so a 128 KiB read is three TRBs: 24 KiB, 64 KiB, 40 KiB. `bulk` gives the controller one TRB at a time and waits for it. That costs a little speed but keeps one thing simple: after a short packet, nothing more is queued, so the CSW that follows can never land in a data buffer.

**Which commands.** READ(10) and WRITE(10) carry a 32-bit sector number. A stick bigger than 2 TiB (or a request that straddles sector 2^32) needs READ(16) and WRITE(16). READ CAPACITY(10) says "0xFFFFFFFF" for such a stick, and READ CAPACITY(16) is asked instead. Only 512-byte sectors are served, because `abi::block` is 512-byte sectors; a stick with 4096-byte sectors is named and left alone.

## The stick as the system's disk

Every disk driver in TheTaaJ serves the same protocol (`abi::block`) on the same two channels, one that may write and one that may only read. fs asks the registry for "disk" and never learns who answers. So "the stick is the disk" is almost entirely an init decision:

- With another disk (virtio, SATA, NVMe), nothing changes. `usb` still starts the first stick it sees and logs it, but nobody sends it requests.
- With no other disk and a USB controller, init keeps the disk's channels open and gives them to `usb`. `usb` then serves them from the first stick. With no stick in, every request gets `NO_MEDIUM`, and fs says `no disk (nothing in the drive): files in memory only`.

The channels travel with the controller's description, not as startup handles. A process starts with at most eight handles (one message's worth). `usb` already had seven, and the first try, with nine, made init panic on an index out of bounds (see Bugs). The power driver already gets its handles the same way, so this was the existing pattern, not a new one.

The watchdog that tests a disk driver (INFO on the read-only channel, answered within 2 s) now watches `usb` when it is the disk driver. The test stalls `usb` for 3 s, init stops it and starts a new one. The new `usb` resets the controller, enumerates the stick again, and serves the same channels: fs never notices.

## The Rust in it

- **Default methods in a trait.** `block_service::Device` gained `fn present(&self) -> bool { true }`. A trait method with a body is a *default*: `blk`, `ahci` and `nvme` don't mention it and get `true`; `usb` overrides it. Nothing else had to change.
- **Errors that carry data.** `storage::Error` is `Failed { key, code, qualifier }` or `Transport(&'static str)`. A `match` can pick out exactly one case, as in `Err(Error::Failed { key: 0x2, code: 0x3A, .. }) => "no medium"`, where `..` means "the other fields, whatever they are". The block service turns key 7 into `READ_ONLY` and the rest into `IO_ERROR`.
- **Disjoint borrows of fields.** In `usb`, `transfer` borrows `self.devices[..].attached.storage` mutably *and* `self.controller` mutably at the same time. Rust allows that inside one function because they are different fields. It would refuse the same thing through two method calls on `self`, because a method borrows the whole of `self`.
- **`Option::is_none_or`.** `self.storage().is_none_or(|s| s.read_only)`: true when there is no stick, or when the stick is read-only. It is the mirror of `is_some_and`.
- **How long a temporary lives (a test bug).** `(disk(&sim).bulk_resets, disk(&sim).clear_halts)` panicked with "RefCell already borrowed". `disk()` returns a `RefMut`, a guard that releases the borrow when it is dropped. Temporaries live until the end of the whole *statement*, so the first guard was still alive when the second one was made. Two separate `let` statements fixed it.

## Design decisions

- **One stick at a time.** The DMA memory has one 128 KiB buffer, so `attach` drives a stick only when no other is driven. A second one is named with "a disk is driven already". One disk is what fs uses anyway.
- **The stick is the disk only when there is no other.** Putting a stick in must never quietly change which disk the system runs on. When a virtio or NVMe disk is there, it wins.
- **One TRB in flight per bulk transfer.** This is slower than queuing all three TRBs of a 128 KiB transfer, but there is no way to read a status into a data buffer by mistake. Speed can come later, with numbers to show it is worth it.
- **Stop Endpoint for a transfer that never ends.** An endpoint that timed out has not halted. Reset Endpoint is only for halted ones (the simulator panics otherwise, as the spec says it's an error). So `recover_as(.., halted: false)` stops it, then points it past the TRB with Set TR Dequeue, then the stick gets the reset recovery.
- **The disk's channels in the description message, not startup handles.** That keeps the 8-handle limit and needs no "nobody" channel for the case where `usb` isn't the disk driver.
- **fs decides at boot.** A stick plugged in after boot is started and served, but fs has already chosen files in memory, and doesn't mount it later. Doing that would mean telling fs "there is a disk now", a new message. That waits until it's needed.

## Bugs

1. **init panicked when starting `usb`:** `index out of bounds: the len is 8 but the index is 8`. My first version gave `usb` the disk's channels as startup handles 6 and 7 and moved the interrupt to 8. That is nine handles; `start` copies grants into an `[_; 8]`, because one message carries at most eight. Every machine with a USB controller stopped booting. The fix was to send the two channels with the description, as `power` gets its handles.
2. **The stick machine had no stick:** `Could not open '.sole'`. In the test script, `DISK= ... QEMU_ARGS="...file=$DISK.sole"` expands `$DISK` *after* the prefix assignment `DISK=` has emptied it. The QEMU options are now built first, in a variable of their own.
3. **The test helper's double borrow** (above, in the Rust section).

## How I checked it

**Host tests:** `cd usb && cargo test`, 24 passed. The 10 new ones:

| Test | What it shows |
|---|---|
| `a_stick_is_configured_named_and_sized` | the INQUIRY names, size, removable; commands in order: INQUIRY, TUR (unit attention), REQUEST SENSE, TUR, READ CAPACITY(10), MODE SENSE |
| `reads_and_writes_up_to_128_kib_each_across_64_kib_boundaries` | 256 sectors written and read back; the sim refuses a TRB that crosses 64 KiB |
| `past_the_end_fails_with_the_devices_reason` | "illegal request", and the stick still works after |
| `a_write_protected_stick_says_so_and_refuses_writes` | `read_only`, "write-protected", nothing written |
| `a_stalled_read_is_cleared_and_its_reason_asked` | Reset Endpoint once, CLEAR_FEATURE once, no BOT reset; "medium error" |
| `a_status_that_is_not_one_resets_the_device` | a garbled CSW gives one BOT reset and both endpoints cleared |
| `a_device_gone_quiet_is_stopped_and_reset` | after the 10 s bulk wait: Stop Endpoint, BOT reset, and the next read works |
| `a_disk_past_2_to_the_32_sectors_uses_the_16_byte_commands` | READ CAPACITY(16), READ/WRITE(16) at and across 2^32, READ(10) below |
| `sticks_that_cannot_be_driven_say_why` | no medium; 4096-byte sectors; a second stick; too little DMA memory |
| `a_keyboard_and_a_stick_side_by_side` | a key pressed during a read waits as an event; the read is not confused |

**By booting (`make usb64-test`, in `make test64`): five machines at once.**

| Machine | What it checks |
|---|---|
| pc (BIOS, qemu-xhci, MSI-X) | keyboard, held key, mouse clicks (as in 088); the stick started: `disk "QEMU QEMU HARDDISK" (46f4:0001, SuperSpeed), 8192 sectors (4 MiB) ready`, not used because there's a virtio disk |
| q35 (UEFI, nec-usb-xhci, MSI) | keyboard typed; a stick that OVMF drove first, taken over and started |
| polled (no MSI/MSI-X, **no disk**) | keyboard typed; `[fs] no disk (nothing in the drive): files in memory only` |
| hot | a keyboard plugged in, typed on, pulled out (as in 088) |
| **stick (no disk but the stick)** | `[fs] ready: TaajFS on the disk`; `disk` = 8192 sectors; `disk check: all 9 checks passed`; `cat`, `wc long` (many sectors at once); `write`, `cp`; programs loaded from the stick; `disk stall 3` → `[init] usb has not answered for 2 s - stopping it` → a second `[usb] xHCI ...` → `disk check` passes again and `cat saved` shows `kept`; at `poweroff` the stop order is `shell kbd fs usb`; then on the host, `tfstool get` finds `saved` = `kept` and `copy` = `long` on the stick's image |

`make test64`: all passed, in 14 minutes 20 seconds.

**Testing note, as a scripted boot:** `make run-disk RUN_DISK=build/usb-note.img`, booted with the stick as the only disk and a USB keyboard and mouse. The log showed `disk "QEMU QEMU HARDDISK" ... 32768 sectors (16 MiB) ready` and `[fs] ready: TaajFS on the disk, 2052 of 4096 blocks free`. `disk` printed `32768 sectors of 512 bytes (16384 KiB)`. Typed on the USB keyboard, `write hello on the stick` then `cat hello` printed `on the stick`. `poweroff` stopped `fs`, then `usb`. On the host, `tfstool get build/usb-note.img hello` printed `on the stick`.

**Mutation experiments:** each change made on purpose, the tests run, the change undone.

| Mutation | Result |
|---|---|
| M1: after a stall, no Reset Endpoint on the controller | caught: `a_stalled_read_is_cleared_and_its_reason_asked` |
| M2: TRBs not split at 64 KiB boundaries | caught: `reads_and_writes_up_to_128_kib_...` (the sim refuses the TRB) |
| M3: READ(10) used for a request straddling 2^32 | caught: `a_disk_past_2_to_the_32_sectors_...` |
| M4: a timed-out TRB left on the ring (no Stop Endpoint) | caught: `a_device_gone_quiet_is_stopped_and_reset` |
| M5: init stops `usb` with the keyboards even when it is the disk driver | caught by `usb64-test`: `the services stopped in the order: shell usb kbd fs` |
| M6: no `NO_MEDIUM` answer without a stick | caught by `usb64-test`: `fs was not told that no stick is in` |

Clippy is clean for `usb` (with its tests), `user` (x86_64 and the AArch64 programs) and the kernel. The kernel image now ends at 0x258000, leaving 672 KiB under the BIOS loader's 3 MiB limit.

## What is still missing

- **Booting from the stick (R6b).** The loader reads the kernel from the CD today. It needs to read it from the stick through the BIOS (int 13h) and from a UEFI system partition.
- **Telling the stick to write its cache.** Nothing sends SYNCHRONIZE CACHE before `poweroff`. QEMU writes through, but a real stick may hold writes in RAM for a moment. This joins the open "FLUSH before power-off" item for virtio, AHCI and NVMe.
- **A stick plugged in after boot** is served, but fs doesn't mount it (fs decides at start). A stick pulled out while fs uses it makes fs's requests fail with "nothing in the drive".
- **One stick, LUN 0 only.** A card reader with several slots (LUNs) shows its first. No hubs (from 088), so a stick behind a hub isn't seen.
- **UAS (USB Attached SCSI),** the faster protocol of newer USB 3 disks, isn't spoken. Those disks also offer BOT on their first interface setting, which is what is used.
- **Only 512-byte sectors.**
- **Not tried on real hardware yet (R7).** QEMU's `usb-storage` is a well-behaved stick. Real ones are known for odd answers (wrong residues, refusing MODE SENSE); MODE SENSE failing is already tolerated.

## What I learned

- A USB stick is a small SCSI disk. Bulk-Only adds only the envelope (CBW, CSW); everything a disk says comes back as sense data.
- "Stall" is not an error, it's the protocol. The stick stalls to say "I'm not sending what you asked for", and the CSW after the stall says why.
- Two sides keep state that must be reset together: the controller's endpoint (Reset Endpoint) and the device's packet sequence (CLEAR_FEATURE). Resetting only one leaves them disagreeing about DATA0 and DATA1.
- Having every disk speak one protocol paid off again. fs, the shell's `disk check`, the watchdog and `poweroff` all worked on the stick with nothing changed but who answers.

## Testing note

**1. A USB stick as the system's disk**

```
make run64 RUN_DISK_BUS=usb RUN_USB=yes RUN_DISK=build/usb-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched); here it is plugged in as a USB stick, with a USB keyboard and mouse beside it. In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:04.0 USB controller: registers 0xfebb0000+0x3000, 408 KiB DMA at 0x..., MSI-X - handed to init` | the controller, with room for the stick's buffers (408 KiB) |
| `[usb] port 1: disk "QEMU QEMU HARDDISK" (46f4:0001, SuperSpeed), 32768 sectors (16 MiB) ready` | the stick answered INQUIRY, TEST UNIT READY, READ CAPACITY |
| `[fs] ready: TaajFS on the disk, ... blocks free` | fs found the volume on the stick, through `usb` |
| no `[blk]`, `[ahci]` or `[nvme]` lines | there is no other disk |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` |
| `write hello on the stick`, then `cat hello` | `on the stick` |
| `disk stall 3` | `[init] usb has not answered for 2 s - stopping it`, a second `[usb] xHCI ...` line and the stick `ready` again; then `cat hello` still works |
| `poweroff` | `[init] stopped shell`, `kbd`, ..., `[init] stopped fs`, then `[init] stopped usb` (after fs: it is the disk's driver) |

After QEMU ends, on the host:

```
tools/tfstool-rs/target/release/tfstool get build/usb-disk.img hello
```

prints `on the stick`.

**2. No stick, no disk**

```
make run64 RUN_DISK_BUS=usb RUN_USB=yes RUN_DISK=build/usb-disk.img RUN_DISK_ARGS="-device qemu-xhci,id=xhci"
```

(`RUN_DISK` still names the disk the programs are copied to; `RUN_DISK_ARGS` leaves it out of the machine.) The log shows `[fs] no disk (nothing in the drive): files in memory only`.

**3. A stick beside another disk**

```
make run64 RUN_USB=yes RUN_USB_ARGS="-device qemu-xhci,id=xhci -drive if=none,id=s,format=raw,file=build/usb-disk.img -device usb-storage,bus=xhci.0,drive=s" RUN_DISK=build/other-disk.img
```

(`build/usb-disk.img` from 1 is the stick; a second new disk, `build/other-disk.img`, is the system's.) The stick is started (`[usb] port ...: disk "QEMU QEMU HARDDISK" ... ready`), but `[blk]` serves the system's disk.

2 and 3 are the `polled` and `pc` machines of `usb64-test`.

**4. The tests by themselves**

```
cd usb && cargo test
make usb64-test
make test64
```

| Look for | Meaning |
|---|---|
| `test result: ok. 24 passed` | the controller, enumeration and the stick against the simulator |
| `USB worked: ... and a stick was the system's disk - across a restart of its driver` | the five machines of `usb64-test` |
| `test64: all passed` | every part |
