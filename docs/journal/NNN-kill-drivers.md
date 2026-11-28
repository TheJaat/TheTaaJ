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

**4. A test of mine: the downloads tried before the PC was up.** In the first `make test64`, and again in `make test64-again`, the network PC failed: "the steps did not all run", httpd waiting for connections that never came. curl's `-s` said only `000 0`; with `-S` it said `Failed to connect to 127.0.0.1 port 33037 after 0 ms` - nothing listening yet. The host's side waits for "httpd: serving" in `kill64.log.net`, and `kill-test.sh` did not delete the last run's log: the old line was already there, and the downloads ran before QEMU had started. The first run ever (no old log) passed, every later one would fail. The logs are deleted first now (by name - the test disks are `kill64.log.disk` and `kill64.log.net.disk`), and curl's errors and wait are in the log. `kill64-test` then passed twice in a row - and failed again in `make test64`, the same way, the log fresh. The second cause: `qemu-boot.sh` first waits for free CPUs (`boot-slots.sh`), and under test64's load the net PC waited more than the host's fixed 120 s - the downloads ran with no QEMU yet. Alone there is always a slot, so it passed. The host's side has no time limit of its own now: it waits for httpd's line, or for that boot to end. Proof: `make test64` (below).

**5. Mine: the file count.** `killcheck` is one more program on the test disk, so `ls` there says 40 files, not 39: `boot64-test`, `boot64-uefi-test` and `aarch64-keys-test` failed on `EXPECT_FILES`. `ROOT_FILES` is 40 now (28 with no C compiler).

**6. Not mine, and not explained: a kill too slow on AArch64.** The same `make test64` (bug 4's second run): `aarch64-thread-test` panicked in the kernel's self-test, "forever sleep took 12 ticks to end, three times over" (src/selftest/user.rs:183). K82 changes nothing in the kernel or on AArch64. It is entry 153's bug 10 come back - there on x86, one CPU, during a race hunt; here on emulated AArch64 while test64 ran twelve parts at once (`e1000-64-test` took 646 s this time, 370 s the run before). That entry left it open unless it came back; it has, so it has a row now: K?-slow-kill in the `sched` lane.

## How I checked it

- **Host**: `kernel/abi` 40 tests (both CRASH requests: their counts, the limits, a STALL or a short request not taken for one). `make coverage` (`kernel/abi` 88.0%), `make unsafe-check`.
- `kill64-test`: passed - five PCs at once, 101 s:

```
net: nic killed mid-send and restarted while a download was under way; it and the next arrived whole (1048576 bytes each)
blk: killed mid-write 3 times and restarted; 36 writes acked, 3 tried again; all on the disk as written
ahci: ... nvme: ... usb: ... (the same)
```

- `make test64`: all passed, 811 s (after bugs 4 and 5; the run before failed only on bug 4's second cause and bug 6). `kill64-test` in it, with test64's load: every disk driver as above, and the downloads `200 1048576` both. Again with main merged in (entries 158, 159 - the Makefile on both sides, so rule 15): all passed, 812 s.
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
