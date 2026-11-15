# A Faster Disk: Shared Memory and a Block Cache

*Journal entry #18 · many sectors per request, and reading each block once*

Entry #17 ended with a test that failed for a reason that had nothing to do with what it tested: loading a 33 KB program took a quarter of a second, long enough for nine keystrokes to pile up. `ls` — now a program on the disk — took more than half a second to print six lines.

Nothing was broken. Every layer did exactly what it was written to do. This entry is about finding where the time went, and the two changes that took `ls` from 0.57 s to 0.07 s.

---

## Measure first

I did not want to guess, so the first thing I added was a way to measure. The shell got a `time` command:

```rust
"time" if !rest.is_empty() => {
    // Ticks are 10 ms apart: good for "is it slow", no finer.
    let start = taaj::ticks();
    self.run(rest);
    let hundredths = taaj::ticks() - start;
    say!(self, "took {}.{:02} s\n", hundredths / 100, hundredths % 100);
}
```

The resolution is one timer tick, 10 ms. That is coarse, but the problem was not subtle. In QEMU, before any change:

| Command | Time |
|---|---|
| `ls` (load a 26 KB program, list 8 files) | 0.57 – 0.79 s |
| `primes` (load a 28 KB program, compute) | 0.49 s |
| `write a b` (one line into a new file) | 0.78 s |

## Where the time went

Follow one request. The shell reads a program file by asking the file service for it, about 1 KB at a time. For **each** of those requests, the file service asked TaajFS, and TaajFS:

1. read the directory block, to find the file,
2. read the file's inode block, to find where its data is,
3. read the data block.

Three 4 KiB blocks for every kilobyte delivered. The directory and the inode were the same two blocks every single time, and each data block was read four times over (four 1 KB requests land in the same 4 KiB block) — because nothing remembered any of them.

And each 4 KiB block was read as **eight separate requests** to the disk driver, one per 512-byte sector, because a sector is what fits in a message. Each of those was: create a reply channel, send, the driver wakes, builds a virtio request, notifies the device, waits for the interrupt, copies the sector into a reply message, sends it back.

So a 33 KB program cost roughly 33 requests × 3 blocks × 8 sectors ≈ **800 round trips to the device**, to read data that fits in 9 blocks plus two of metadata. Two separate wastes, multiplied together:

- **Too many trips per block** (8 instead of 1).
- **The same blocks fetched again and again** (about a hundred reads where eleven would do).

They need different fixes.

## Fix 1: many sectors in one request, through shared memory

A message in TheTaaJ carries at most 1,024 bytes. That limit is deliberate — messages are for *control*, and small fixed-size messages keep the kernel's IPC path simple and bounded. Bulk data is supposed to move another way: **shared memory**. The 32-bit system's disk path already worked like this, and it is in the vision (entry #1) under "Efficient".

The idea: the client owns a piece of memory — a *memory object* — and lends the driver a handle to it. The request then only says *"sectors 800 to 807"*. The bytes are already somewhere both processes can reach.

Two new requests in the block protocol:

```rust
/// Payload: the first sector (u64 LE) and how many (u32 LE, 1 to
/// SECTORS_MAX). The data does not travel in messages: the request
/// carries a second handle, after the reply channel - a memory
/// object ...
pub const READ_MANY: u8 = 4;
pub const WRITE_MANY: u8 = 5;
/// Most sectors one READ_MANY or WRITE_MANY moves: 16 KiB.
pub const SECTORS_MAX: u32 = 32;
```

The old one-sector `READ` and `WRITE` stay; the shell's `disk read N` uses them.

### Design decision: the driver remembers nothing

The usual way to do this is to *register* a shared buffer once ("here is my buffer, use it from now on") and then send small requests forever after. That is one fewer handle transfer per request. I did not do it, for two reasons that are specific to this system:

- **The driver can be restarted.** `init` restarts a crashed driver, and the whole point is that clients do not notice. A registered buffer is state in the driver; after a restart it is gone, and every client would need a way to find out and register again.
- **The driver cannot tell clients apart.** All clients send on the same request channel. There is no "connection" to attach a buffer to.

So every request **carries** a handle to the memory object. The driver maps it, does the transfer, unmaps it, closes the handle, and replies. Nothing survives the request. Mapping eight pages costs a few microseconds; a disk request costs a hundred times that. On the client side it is one extra call per request:

```rust
let lent = crate::duplicate(window.memory, rights | crate::rights::TRANSFER)?;
self.call_with(&request, Some(lent), &mut reply).map(|_| ())
```

The rights are part of the request, too. For a read the driver's copy of the handle has `READ | WRITE` (it must fill the memory); for a write only `READ`. The driver cannot be tricked into writing to memory the client only meant it to read — the kernel refuses the mapping.

### The driver must not trust the memory

This is a new kind of input for the driver: an object of the client's choosing. It might be smaller than the request claims. It might not be a memory object at all. If the driver mapped a one-page object and copied 16 KiB into it, the copy would run off the end of the mapping and fault — **in the driver**. Any client could crash the disk driver with one request.

The driver needs to know the size before it touches anything, and there was no way to ask. So the kernel got one small system call, `memory_size(handle)`:

```rust
// Is it a memory object, and big enough? Asked before mapping:
// copying past the end of a smaller one would be a fault - in
// this process, the driver.
if !taaj::memory_size(lent).is_ok_and(|size| size >= length as u64) {
    return block::BAD_REQUEST;
}
// Writable only to fill it; refused if the handle lacks the right.
let Ok(theirs) = taaj::memory_map(lent, !write) else {
    return block::BAD_REQUEST;
};
```

`memory_size` fails with `WrongType` for a handle that is not a memory object, so one check covers both cases. A bad request costs the client its request and the driver nothing.

### Why still one copy?

The data is copied once: between the client's memory and the driver's own DMA buffer. Truly zero-copy would let the disk write straight into the client's pages. It cannot, yet: a device addresses memory by *physical* address, and the driver only knows physical addresses for its DMA memory — the kernel tells it those and no others. Letting a driver learn the physical address of arbitrary client pages would hand it a way to point the device at any memory in the machine. Doing this safely needs an IOMMU, which is a later step. One copy of 16 KiB is not what was slow.

The driver's DMA data area grew from one sector to 32, and a whole run of sectors is now **one** virtio request:

```rust
Buffer { physical: at(DATA), length: count * block::SECTOR as u32, device_writes: !write },
```

In the file service, reading a block became a single call:

```rust
fn read_block(&mut self, number: u32, buffer: &mut Block) -> Result<(), DeviceError> {
    let first = self.first_sector(number)?;
    self.disk.read_many(&mut self.window, first, SECTORS_PER_BLOCK as u32).map_err(device_error("disk read"))?;
    buffer.copy_from_slice(&self.window.bytes()[..BLOCK_SIZE]);
    Ok(())
}
```

## Fix 2: a block cache

The second waste — reading the same few blocks dozens of times — is fixed by remembering them. `taajfs::Cache` wraps any block device and is itself a block device, so TaajFS does not know it is there:

```rust
pub struct Cache<D: BlockDevice> {
    device: D,
    blocks: BTreeMap<u32, Entry>,   // block number -> its bytes, and when last used
    capacity: usize,
    clock: u64,
    ...
}
```

A read looks in the map first. On a miss it asks the device and remembers the answer. When the cache is full (256 blocks, 1 MiB, in the file service), the block used **longest ago** is dropped — LRU, least recently used. Finding it is a scan of every entry; for 256 entries that is far cheaper than the disk read it saves, and a cleverer structure can come when the numbers say so.

### The decision that matters: writes are not delayed

A cache can do two jobs. It can save *reads* (remember what was read). It can also save *writes*: hold changes in memory and write them later, merged and reordered — "write-behind". That is where most of a file system's speed comes from on a real system, and it is where most of its data-loss bugs come from too.

TaajFS is copy-on-write: a change writes new blocks to free space, and only then writes one superblock that points to them. The promise — *after a power cut the disk holds either the old state or the new, never half* — depends entirely on that **order**. A cache that delays or reorders writes can break it without a single line of TaajFS changing.

So this cache is **write-through**: a write goes to the device immediately, and is remembered only if the device accepted it.

```rust
fn write_block(&mut self, number: u32, buffer: &Block) -> Result<(), DeviceError> {
    match self.device.write_block(number, buffer) {
        Ok(()) => {
            self.remember(number, buffer);
            Ok(())
        }
        Err(error) => {
            // What the device holds now is unknown - the old block,
            // the new, or part of each. Forget it: the next read
            // asks the device.
            self.blocks.remove(&number);
            Err(error)
        }
    }
}
```

The cache never changes *when* a write happens. It only saves reads. That gives it one invariant simple enough to test: **a block is in the cache only if the device holds exactly those bytes.**

It is also only correct while nothing else writes to the disk. In TheTaaJ the file service is the single holder of the disk's read-write channel (the shell's channel is read-only, enforced by the driver), so that holds — by capability, not by convention.

## Results

Same commands, same QEMU, after both changes:

| Command | Before | After |
|---|---|---|
| `ls` | 0.57 – 0.79 s | 0.07 s |
| `primes` | 0.49 s | 0.03 s |
| `write a b` | 0.78 s | 0.11 s |

Roughly 8× to 16×. `write` improves least, as it should: its writes all still go to the disk, in order; only its reads got cheaper.

One thing did **not** get faster: `cat` of a 270 KB text file was still printing after ten seconds. That is not the disk — the data arrives quickly — it is the display service drawing a quarter of a megabyte of text one glyph at a time. A different bottleneck, for a different day, and exactly why measuring comes first.

## How I checked it

**The cache, on the host** (six tests in `taajfs`):

- a second read does not reach the device;
- a write is on the device before `write_block` returns, and is then read back from memory;
- the least recently used block is the one evicted;
- a failed read is not remembered, and a failed write is forgotten;
- a TaajFS volume gives byte-identical results with and without the cache, with fewer than a fifth of the device reads;
- **the power-cut test:** a sequence of file operations is run with the power "cut" after 0 writes, 1 write, 2 writes… up to completion. After every one, each block the cache still holds must equal the block on the device, and the volume must mount.

**The driver, in the running system.** The shell got a diagnostic, `disk check`, which runs nine checks over its own read-only channel. One confirms that 32 sectors read together are the same bytes as 32 read one at a time. The other eight are requests the driver must refuse — and survive:

| Request | Must answer |
|---|---|
| A memory object smaller than the request needs | bad request |
| A channel handle instead of memory | bad request |
| Memory the driver may not write, for a read | bad request |
| Zero sectors | bad request |
| More sectors than the limit | bad request |
| A range that runs off the end of the disk | no such sector |
| A sector number so large that adding the count wraps around | no such sector |
| A write, on the read-only channel | read-only |

The boot tests (BIOS and UEFI) run `disk check` and `time ls`, and everything from the earlier entries still passes — including the host tool reading back what the OS wrote, which now travelled through the new path.

## What is still slow, or missing

- **One request at a time.** The driver finishes a request before it looks at the next. Real disks get much of their speed from many requests in flight.
- **The client still reads files 1 KB per message** from the file service. Now that blocks are cached this is cheap, but a large file would be better read into shared memory the same way.
- **No write-behind**, by choice, as explained above.
- **The cache is not shared or sized by anything**: 256 blocks, fixed.
- **Text output** is now the slowest thing a user will notice.

---

## Testing note

```
make run64
```

Type in the QEMU window.

| Type | You should see |
|---|---|
| (at boot, in the terminal log) | `[fs] ready: TaajFS on the disk, N of 4096 blocks free; a cache of 256 blocks` |
| `time ls` | The file list, then `took 0.0N s` — under a tenth of a second, where it used to be over half |
| `time ls` again | About the same or less (the program's blocks are now cached) |
| `time primes` | The primes, then `took 0.0N s` |
| `disk check` | `disk check: all 9 checks passed` |
| `disk` | The disk's size — the driver is still alive after the checks |
| `write big hello`, `cat big` | `big: 6 bytes`, then `hello` — writes and reads through the new path |
| `ps` | Six services; `blk` and `fs` still running |

Persistence (proves writes really reach the disk rather than staying in the cache): `write keep me`, close QEMU, `make run64`, `cat keep` → `me`.

If `disk check` ever prints `FAILED - …`, the text after it says which of the nine checks it was.

Automated: `make test64` runs the host tests (including the cache's), then boots BIOS and UEFI and checks all of the above.
