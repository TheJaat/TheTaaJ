# Fast tests: TaajFS stops reading the whole disk at every write, programs load in big reads, and the long boots run side by side

*Journal entry #80 · Era 6, after step E7. `make test64` took over an hour, and its longest part sometimes failed for running out of time. It now takes 14 to 16 minutes. The changes that made the difference were in TheTaaJ, not in the test scripts:
- the file system no longer rebuilds its free map, by walking every file on the disk, after each write;
- the file service reads a file's data many blocks to a request.

The rest is ordering: the long parts start first and more run at once, and the tcc test's three independent boots run side by side.*

## Why

The user asked for the tests to use the machine they run on: 16 CPUs and 16 GB. `make test64` already ran 7 parts at once, and the boot slots kept the emulated CPUs within the host's. Even so, the last full run took over an hour. Its slowest part, `aarch64-tcc-test`, ran past the 18 minutes its typed steps waited for, and failed. More parallelism alone would not have helped much: one boot of that test took 33 minutes, so the suite could never take less.

## How it fits together

What each boot spent its time on, found in turn:

```
 tcc-test: one boot, three jobs in a row       libc build 862 s, tcc building itself 332 s
   split into three boots, on three disks      tcc building itself, alone: 21 s  (!)
   the same, on the disk the libc build left   275 s              -> the disk's state
       TaajFS: every commit -> rebuild_free_map: read every directory,
               inode and index block on the volume
       now:    only what differs between the old root and the new
                                                                     12 s
 clang-test: clang --version                   12.6 s             -> loading an 80 MB file
       fs: one 4 KiB block a request to the disk driver (20,000 for clang)
       now: a run of neighbouring blocks, 32 to a request             0.8 s
```

**The free map, kept up to date** (`taajfs/src/volume.rs`, `update_free_map`):
- TaajFS keeps no free map on disk: it is whatever the committed tree does not reach. Until now `commit` rebuilt the map after every change, by walking the whole tree.
- Every write is a commit, and tcc writes an object file in pieces of 1 KB. So each piece read the inode and blocks of every file on the disk. A few hundred files made writing about twenty times slower.
- A block the committed tree reaches is never written over, so a block number both trees reach has the same contents in both, and so does everything below it.
- `changed_blocks` therefore walks only where the old root and the new one differ. It compares directories entry by entry, and goes down only into entries whose block changed. A directory moved elsewhere shows up as removed in one place and added in another, so it comes out right too.
- What only the old tree reached becomes free, and so do blocks handed out during the change that the new tree does not use. What only the new tree reaches becomes used.
- If the trees cannot be compared, the map is rebuilt, as before. Mount still rebuilds it, which also checks the tree.
- With tests built in, every commit compares the kept map with a rebuilt one and panics if they differ. So all 60 volume tests check it, the power-cut ones included.

**Files read in runs:**
- `BlockDevice::read_blocks` reads several neighbouring blocks. By default it reads one at a time.
- `Volume::read_at` first works out which block each piece of the buffer comes from. It then reads each run of neighbours, up to 64 blocks, with one call. A file written in one go mostly lies in one run, because blocks are handed out lowest first.
- The cache answers the blocks it holds from memory, and passes each stretch it lacks to the device as one read. It does not keep those: a run is file data read once, and keeping it would push out the blocks every request needs.
- fs reads a run through a window as big as the driver takes. `abi::block::SECTORS_MAX` goes from 32 sectors to 256, which is 128 KiB, or 32 blocks. A block device's DMA memory goes from 16 pages to 48 to hold that.

**The suite** (`Makefile`, `tcc-test.sh`):
- `TEST64_PARTS` lists the longest parts first, since make starts them in that order. `TEST_JOBS` goes from 7 to 12. The boot slots, not the job count, still decide how many emulated CPUs run at once.
- `tcc-test.sh` runs its three boots at once, each on its own copy of the disk:
  - the build;
  - TCC building itself, which needs nothing the build makes (it links with `/lib/libc.a`);
  - the editor.

  Each file is read back from the disk of the boot that wrote it.
- The long typed lines get forty `~~` waits instead of twelve. A spare one costs nothing, and running out of them was the failure.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Speeding up the slow parts | Measuring each boot, then fixing what it waited on | Only more parallelism: one boot took 33 minutes, so the suite could never take less |
| Freeing blocks after a commit | Comparing the old tree and the new one where they differ | Recording, at each place a change copies a block, which block it replaced: many places to get right, and one missed means a block lost or handed out twice |
| Checking the kept free map | A rebuild after every commit, in the tests | Trusting it: the tests run every kind of change, so comparing costs nothing there |
| Reading a file | Runs of neighbouring blocks, one disk request for each 32 | Reading ahead into the cache: more memory, and still one request per block to the driver |
| Runs and the cache | Blocks it holds come from memory; the others are not kept | Keeping them: an 80 MB program would push out the directories and inodes every request needs |
| A bigger transfer | 128 KiB, with 192 KiB of DMA memory per disk | Larger still: 32 blocks already makes a request's cost small beside the copying |
| Typing faster in the typed boot tests | Not done | Tried: keys 50 ms apart, through one kept connection to the monitor. Shift was still down for the next key (`ls \|` came out as `LS \|`), and the boot took as long as before. The guest is busy the whole time, not waiting for the keys |

## Bugs

- **The test that timed out.** `aarch64-tcc-test` gave its build twelve `~~`, at most 18 minutes, and the build took 23. The earlier "flake" in entry 079's run was this.
- **TaajFS's cost grew with the disk.** Every write walked every file. Writes on a fresh disk were fast, so the tests that write a few files never showed it. A build, which writes hundreds, did.
- **A program loaded 4 KiB at a time**: 20,000 requests to the disk driver for clang.
- **blkcheck's sectors overlapped** once the limit was 256: the 257 sectors written from 101 would have run into those it checks were never written, from 200. They are written from 1400 now, past the read-many check at 1000, and `disk-pattern.py` knows that.

## How I checked it

Measured, with a timed `time source ...` on TheTaaJ, the same work before and after:

| What | Before | After |
|---|---|---|
| tcc building itself, on a disk where the libc build ran | 275.7 s | 12.4 s |
| the same, in tcc-test after the build, PC / AArch64 | 332 s / 448 s | 21 s / 36 s (its own boot) |
| the libc build, PC / AArch64 | 862 s / 1404 s | 50 s / 83 s |
| `clang --version` (loading 82 MB) | 12.6 s | 0.78 s |
| an lld link | 7.6 s | 1.2 s |
| `tcc64-test`, `aarch64-tcc-test` | 1298 s, about 1945 s | 145 s, 171 s |
| `make test64` | over an hour (and failing) | 851 s, 973 s (two runs) |
| the testing note's `time source /src/build`, PC / AArch64 | (862 s / 1404 s in the tests) | 42.8 s / 76.0 s |

On the host:
- taajfs: 62 tests, every commit compared with a rebuild of the free map. Two new tests:
  - a run of blocks is one device request for each stretch the cache lacks, and is not kept;
  - a 100-block file is read in at most 4 requests.
- `make tfscheck`: crosscheck passed. The C implementation still makes the same images, since what is allocated, and where, did not change.
- fileserver: 37 tests. abi: 25 tests.

Mutation experiments, against taajfs's tests:
- M1: entries only in the old directory not freed. 10 tests fail. Caught.
- M2: blocks handed out during a change and not used kept as used. 6 tests fail. Caught.

`make test64` passed, all parts. `blkcheck` now moves 256 sectors at once, both ways. `aarch64-test` checks the disk image afterwards: sectors 1400 to 1656 and the last written, the rest as they were.

The testing note below was run by typing its steps.

Not tested:
- A disk whose blocks are scattered: runs only make reads faster, so such a file reads as before, block by block.
- The typed boot tests' speed. They take 6 minutes, now the longest parts, and are not changed.

## What is still missing

- **The typed boot tests** (`boot64-test`, `boot64-uefi-test`, `aarch64-keys-test`) are now the critical path, at 4 to 6 minutes. The guest is busy all the way through them. Making them faster means looking inside them, not at the typing.
- **The boot slots serve waiters in no particular order.** Early in the suite the parts ask for far more than the 15 slots. Short boots often get in first, and `aarch64-keys-test`, third in the list, started at minute 11 in one run. Most of the suite's time is spent with every slot busy, so this is bounded by the host's CPUs: ordering decides only what finishes last.
- **Writes are still one block a request**, and one commit per request. A program writing in 1 KB pieces commits 1 KB at a time.
- **The C TaajFS** (`servers/tfs`) still rebuilds its free map at every commit. It is only the reference the Rust one is checked against.

## What I learned

- A suite's time is its slowest part's. Before adding parallelism, find what that part is waiting on.
- The free map was correct and simple, and quadratic: each write cost as much as the disk held. Copy-on-write gives the fix for free, since an unchanged block number means an unchanged subtree.
- A test that waits a fixed time for something slow fails when the machine is busy, and looks like a flake. Here the fix was to make the slow thing fast, and to give the wait room.

## Testing note

**1. The PC: a build and a big program**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `time source /src/build` | the C library and 11 programs built; `took` well under two minutes (it was over ten) |
| `time with readonly ls` | `took` a fraction of a second |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `test64: all passed`, in 14 to 16 minutes | every part |

Automated: make test64
