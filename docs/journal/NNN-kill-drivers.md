# Drivers killed mid-I/O: the system recovers, and nothing acknowledged is lost

*Journal entry #NNN · a step, MaK-WSL/devices' K82, Era 15's measure. Written on MaK-WSL/devices. Every driver here is a program that init restarts when it dies, and each test already made one hang while idle and checked that the next one works. None killed a driver while it was in the middle of writing - the moment a crash can lose data. Now `kill64-test` does, on five PCs at once: each disk driver (virtio, SATA, NVMe, USB) dies under three different writes, and the network card's driver dies in the middle of a download. Every write the system acknowledged is read back whole, in the running system and from the disk image afterwards; the download arrives byte for byte.*

## Why

Era 15 is done when, among other things, "a test that kills each driver in the middle of its I/O finds the system recovered". K82 is that test, early, so that the era's other items are measured by it. The question it asks is the one a user asks after a crash: was anything lost that the system said was saved?

## How it works

### Dying on purpose, mid-I/O

Two test requests, beside the watchdog's STALL, in the code every driver of a kind shares:

| Request | Where | What the driver does |
|---|---|---|
| `abi::block::CRASH` N (`disk crash N` in the shell) | `user/src/block_service.rs`: blk, ahci, nvme, usb's stick | Dies in its Nth WRITE or WRITE_MANY from now: half of a WRITE_MANY's sectors written, then a fault - no answer |
| `abi::nic::CRASH` N (`niccheck crash N`) | `user/src/nic_service.rs`: nic, e1000 | Dies when the Nth frame to send is whole: that frame never reaches the card, frames before it may still be in its ring |

A fault, not an exit: the driver is killed by the kernel as a buggy one would be (code -1000), and init restarts it. A request in flight gets no answer; its reply channel closes with the driver, and the client is told the driver stopped. The disk's CRASH is taken on either of its channels, as STALL is: the shell, which may only read the disk, asks, and the writes counted are fs's.

### The disks: `killcheck`

`killcheck write SET 12` writes twelve 8 KiB files, each made from its name and number (so a file holding another's bytes, or half old and half new, is told apart), and says each write the file service answered: "killcheck: acked a3". An answer means committed - TaajFS commits every change, copy-on-write, the disk's cache flushed, before it replies. A write that fails is said and tried again a second later.

`kill-test.sh` types, on each PC: `disk crash 3`, `killcheck write a 12`; `disk crash 17`, `... b 12`; `disk crash 40`, `... c 12`; then `killcheck check` for each set. After the machine is off, on the host: `tfstool dump` (the volume healthy) and `tfstool get` of every acked file, compared byte for byte (`killcheck-file.py`, the same pattern) - on the medium, not only in fs's cache.

Where the three deaths land is the same on every driver - the test is deterministic:

| `disk crash` | The write it dies in | What that is |
|---|---|---|
| 3 | 4 of 8 sectors from sector 0 written | a torn superblock |
| 17 | 4 of 8 from sector 4608 | a file's data |
| 40 | 4 of 8 from sector 8 | the block after the superblock |

Each time fs reports "the disk failed, or the file system on it is damaged" for that one write, killcheck tries it again on the new driver, and it is acked. The torn superblock is the case that matters most: TaajFS's copy-on-write commit keeps the volume whole through it.

### The network card: a download through it

A fifth PC, on QEMU's own network with port 80 passed to this host: `niccheck crash 200`, then `httpd 80 /www 2`; the host fetches the 1 MiB `big.bin` twice with curl. The driver dies under the first download, after about 200 frames; `net` sees the card's channel answered by a new driver ("the card's driver was replaced: listening to the new one"); TCP sends again what was lost. Both downloads are the file, byte for byte.

### Input

Already covered: `boot64-test`, `boot64-uefi-test` and `aarch64-test` press F12 as their first key, which crashes the keyboard driver in the middle of a key press, wait for the new driver, and type the rest of a long session through it. The USB keyboard is served by `usb`, which this test kills as a disk driver.

## Bugs

**1. The shell could not ask.** The first run: `disk: the disk service did not understand`. CRASH was taken only on the channel that may write - but that one is fs's alone; the shell has the read-only one (`disk.look`), by design. STALL is taken on either, as a test request that changes no data; CRASH is the same kind of request, and now is too.

**2. A test of mine: three boots with no disk.** The SATA, NVMe and USB boots failed at once, QEMU saying `Could not open '.ahci'`. In `DISK="$disk" QEMU_ARGS="$(options $driver)"` the shell sets DISK first, then runs `options`, which reads `$DISK` - now empty for those three. The options are made first now (usb-test.sh has a note on the same trap).

**3. Mine, in passing: an edit that did not apply.** My script for bug 1's fix stopped at its first replacement (the comment's lines were broken differently than I wrote them), changed nothing, and I ran the test again on the old code: the same failure, 31 s lost. A script of edits now prints that it applied them all.

## How I checked it

- **Host**: `kernel/abi` 40 tests (both CRASH requests: their counts, the limits, a STALL or a short request not taken for one). `make coverage` (`kernel/abi` 88.0%), `make unsafe-check`.
- `kill64-test`: passed - five PCs at once, 101 s:

```
net: nic killed mid-send and restarted while a download was under way; it and the next arrived whole (1048576 bytes each)
blk: killed mid-write 3 times and restarted; 36 writes acked, 3 tried again; all on the disk as written
ahci: ... nvme: ... usb: ... (the same)
```

- `make test64`: RESULT.
- **Not tested**: the e1000 driver killed (the same `nic_service`, its own card code); a driver killed while *reading* (nothing is acknowledged by a read; the program reading is told, and reads again); real hardware (K1-K3).

## What is still missing

- **Killed at random moments**, not at a chosen write: a race hunt over `disk crash N` for N from 1 to 100 would cover every write of a commit (J95's race-hunt could run it).
- **Two drivers dying at once**, or the file service dying with a driver: init restarts each, but nothing tests them together.
- K82's other uses: every later Era 15 item (the device manager K76, devices handed out the same way K75) is measured against this test.

## What I learned

- **The interesting failure is the one in the middle.** Idle restarts had passed for months; a torn superblock is what a kill mid-write can do, and only a test that kills there can show the file system survives it.
- **Test requests follow the reader's rights, not the writer's.** The request that makes a driver die changes no data itself, so whoever may look may ask - as for the stall.

## Testing note

```
make kill64-test
```

| Look for | You should see |
|---|---|
| `blk:`, `ahci:`, `nvme:`, `usb:` | `killed mid-write 3 times and restarted; 36 writes acked, 3 tried again; all on the disk as written` |
| `net:` | `nic killed mid-send and restarted while a download was under way; it and the next arrived whole (1048576 bytes each)` |
| `kill64-test passed` | |

By hand, in `make run64` with a disk: `disk crash 5`, then `with files killcheck write x 10` - one write fails, is tried again, and all ten are acked; `with files killcheck check x 10` says all as written.

Automated: `make kill64-test`, `make test64`
