# Bus mastering tied to a handle

*Journal entry #NNN · a step, MaK-WSL/devices' K77, Era 15. Written on MaK-WSL/devices. A PCI device's DMA - its permission to read and write memory on its own - was switched on when its driver first asked for a BAR, and stayed on until the machine was switched off. A driver that died left its device free to go on writing wherever it had been told. Now DMA is on only while the driver holds a **bus-master handle** for it, and when the last one closes - the driver ended, whatever the way - the kernel switches it off, until a new driver asks again.*

## Why

Era 15's "Not secure on the bus": "Bus mastering is turned on at hand-out and never off." K77 is the part of that the kernel can do without an IOMMU (K27): the device of a driver that is not there does no DMA. A driver that crashed in the middle of setting up a transfer, or one killed by the watchdog, leaves the device pointed at memory it may still write; init keeps that memory (it gives the next driver the same), so it was never memory of anyone else's - but nothing said the device stops. Now the bus does.

## How it works

One more operation on a device handle (K75's): `abi::device::BUS_MASTER`, which returns a handle of a new kind, `Object::BusMaster` (`kernel/kernel64/src/device.rs`).

- All the handles to it - the driver's, any copies - share one `BusMaster` object. The first one made switches the function's bus-master bit on in its command register; when the last is dropped, `Drop` switches it off.
- A driver never closes its handle, so it closes as the driver ends: by exit, by a fault, by init's watchdog. That is the "last handle closes" of the item: init keeps the *device* handle across restarts, so it could not be that one.
- One race is closed by the device's lock: a new driver asking just as the old one's last handle is dropped. Making a new object and switching on happen under it; the drop looks under it whether a newer object is alive, and switches off only if none is. Either order leaves the bit as the handles say.
- BAR and INTERRUPT turn on decoding only, no more DMA. `taaj::device::claim_pci` asks for the bus-master handle after the BAR, so no driver's code changed.
- On AArch64 a virtio place in memory has no such switch; the handle is given and changes nothing.

The kernel says each switch:

```
[pci] 00:04.0: bus mastering on, for its driver
[pci] 00:04.0: bus mastering off - its driver's last handle for it closed
```

A new driver resets its device first thing, as every driver here does, before the device could act on anything the old one left in its queues.

## Bugs

**1. Not K77's, seen in its test: K82's kill points are not fixed any more.** `kill64-test` said "36 writes acked, 2 tried again" where it always said 3. Its log: the first kill (`disk crash 3`) landed in a write at sector 5512, not at sector 0 as entry 160 found every time - in none of killcheck's writes, so killcheck had nothing to retry. Since entry 163 logd writes the system log to the disk in the background, and `disk crash N` counts every write, logd's among them: where each death lands now depends on when logd writes. The test still passes - every acked write is on the disk - but the torn superblock entry 160 counted on is no longer certain in every run. Left as K?-kill-points in `devices`: count only the writes of a chosen client, or quiet logd for the test.

## How I checked it

- `kill64-test` (K82) checks it: each disk driver killed three times mid-write, the device's DMA switched off after each kill and on for each new driver; the network card's switched off when nic was killed. Passed, with `boot64-test`, `ahci64-test`, `nvme64-test`, `usb64-test`, `e1000-64-test` (its four cards), `net64-test` and `aarch64-test`: every driver started with DMA on, and each kill switched it off.
- `make test64`: RESULT.

## What is still missing

- **K27, the IOMMU**: while the driver runs, its device can still DMA anywhere. K77 only stops a device whose driver is gone.
- A driver cannot give DMA up while it runs (to quiesce a device, say): it would close its handle, and asking again switches it back on - enough for now.

## Testing note

```
make kill64-test
```

| Look for | You should see |
|---|---|
| `build/kill64.log.blk` | `bus mastering off - its driver's last handle for it closed` after each `[init] blk ended with code -1000`, and `bus mastering on, for its driver` before each new `[blk] ... virtio disk at` |

Automated: `make kill64-test`, `make test64`
