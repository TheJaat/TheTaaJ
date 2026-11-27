# Miri over the host tests: undefined behaviour in three simulators

*Journal entry #NNN · a check, and what it found (K187, K112's second half). Written on TheJat-WSL/ci. Entry 121 made every `unsafe` say why it is sound; nothing checked that the reason is true. Miri does: it runs the host tests in an interpreter that checks every memory access against Rust's rules. Now `make miri` runs it over every crate with `unsafe` in it. It found undefined behaviour in the tests of three drivers - `usb`, `nvme`, `ahci` - and memory two test helpers never gave back.*

## Why

An `unsafe` block is a promise the compiler cannot check: this pointer is in bounds, aligned, not used by anyone else meanwhile. A wrong promise is undefined behaviour - the compiler may then do anything, and usually does something that works, until the day it doesn't. Tests pass either way. Miri is Rust's own interpreter for this: it tracks every allocation, every pointer's provenance and alignment, every borrow, and stops at the first access that breaks a rule - out of bounds, misaligned, use after free, a pointer made invalid by a later borrow (the aliasing rules), a data race, memory never freed.

## What it is

```text
make miri        (sh tools/miri-check.sh [CRATE...])
```

- **Which crates**: every one whose source has `unsafe`, found by looking - eight today: the kernel library, `heap`, `virtio`, `console`, `e1000`, `ahci`, `nvme`, `usb`. A crate without `unsafe` is skipped and said so: in safe Rust the compiler has already ruled out what Miri looks for, and its normal tests check the rest (`abi`, `taajfs`, `fileserver`, `net`, `registry`, `input`, `gfx`, `gui`, `acpi`).
- **The Rust**: a nightly of a fixed date, `nightly-2026-11-25` (Miri is only in nightlies), installed with Miri by the script the first time. The build's own Rust stays 1.100.0.
- **Not in `make test64`**: Miri interprets, and a test that takes a millisecond can take a minute. It is a job for the night runs.

**Tests that size themselves down.** Under Miri, `cfg!(miri)` is true:

| Test | Normally | Under Miri |
|---|---|---|
| `heap`'s randomized stress | 20000 rounds | 400 |
| `queue`'s producer and consumer (two threads, lock-free) | 1000000 bytes | 20000 |
| `sync`'s eight threads under contention | 20000 each | 500 |
| `ahci`'s and `nvme`'s simulated disks | 4096 sectors, 8192 blocks | 256 and 512 |

The interleavings a few thousand operations make are plenty for Miri's race detector; 2 MiB of simulated disk, copied a byte at a time, made one `ahci` test take four minutes. Eleven tests in modules with no `unsafe` - the scheduler's random runs, the device tree's and ELF's damaged-input runs, every day from 1970 to 2100 - are `#[cfg_attr(miri, ignore = "no unsafe in this module: ...")]`; two `ahci` tests that need sectors past the smaller disk likewise, with their reason. `nvme`'s two biggest transfers - the ones that make the driver build lists of pointers in DMA memory (PRP lists), the most worth checking - write at places that stay on the disk whatever its size, the same places as before with the normal disk. The normal tests run every one of them as before.

## What it found

**`usb`, `nvme`, `ahci`: a word written at any address.** Each driver reads and writes its DMA memory in 32- and 64-bit words. On a real machine that memory is a page, so the words are aligned. In the tests it was the simulator's `Vec<u8>` - aligned to one byte:

```text
error: Undefined Behavior: accessing memory based on pointer with alignment 1, but alignment 4 is required
   --> src/xhci.rs:287:18
287 |  unsafe { self.base.add(offset).cast::<u32>().write_volatile(value) }
```

`usb` and `nvme` failed so; `ahci` had the same code and passed only because its allocation happened to land aligned. The simulators' memory is now page-aligned (a `Memory` of `#[repr(align(4096))]` pages), and each driver's `Dma::new` says in its `# Safety` that the memory must be aligned to 8 bytes - and checks it.

**`usb`: a pointer made invalid by a later borrow.** With the alignment fixed, Miri went one step further:

```text
error: Undefined Behavior: attempting a write access using <185314> at alloc49991[0x1000],
       but that tag does not exist in the borrow stack for this location
```

The driver's `Dma` was given its pointer through a `&mut [u8]`; the simulator - playing the controller - then took new `&mut` slices of the same memory to read what the driver wrote. Each new exclusive borrow made every pointer taken before it invalid (Rust's aliasing rules, which Miri checks as "Stacked Borrows"), and the driver's next write used one. On a real machine the controller is outside Rust; in the test it was not. Now `Memory` keeps one raw base pointer, made from the `Vec`'s buffer with no reference in between, and both the driver and the simulator start from it.

**Memory never given back**, in two test helpers: `heap`'s tests leaked the memory each test's heap managed, and the frame allocator's tests leaked their bitmaps (`FrameAllocator::new` takes `&'static mut`, as the kernel's bitmap lives for ever, and the tests met that with `.leak()`). Each helper now returns an owner that frees it when the test ends - bound before the allocator (`let (_storage, mut a) = ...`), so that, dropped in reverse order, it goes after it.

Everything else - the kernel library's 234 tests, `heap`, `virtio`, `console`, `e1000` - passed, with nothing to report.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Crates with `unsafe`, found by looking | A list | A crate that gains `unsafe` is checked from then on |
| Fewer rounds under Miri | Ignoring slow tests | The slow tests with `unsafe` under them - the lock-free queue, the spin lock - are the ones Miri is for |
| Ignore, with a reason, in modules with no `unsafe` | Fewer rounds there too | Miri adds nothing there; the reason says so where the test is |
| Fix the simulators, not the drivers | `read_unaligned` in the drivers | Real DMA memory is aligned; the drivers are right to need it. The test was the one breaking the rule - and now `Dma::new` says so |
| A dated nightly | `nightly` | The same Miri on every machine, and the same answers |
| Out of `make test64` | In it | Half an hour; a night job |

## How I checked it

- `make miri` (1195 s): the kernel library 234 passed (11 ignored), `heap` 9, `virtio` 12, `console` 10, `ahci` 16 (2 ignored), `usb` 25, `e1000` 10; `nvme` 12 of 14 - its two biggest transfers wrote past the smaller disk. With their places kept on the disk, `nvme` alone under Miri: 14 passed (589 s).
- The normal tests after each change: the kernel library 245, `heap` 9, `usb` 25, `ahci` 18, `nvme` 14 - all pass, none ignored.
- Before each fix, the failure above; after it, none.

## What is still missing

- **`libc` and `user/`** are not run under Miri: they are built for TheTaaJ, and their host tests are compiled differently (`make kernel64-test`'s `rustc --test` files). Their `unsafe` - 37 functions of `threads.rs` alone - is checked only by its written reasons (entry 121).
- Miri's other model, Tree Borrows, and its `-Zmiri-strict-provenance`: the drivers turn integers into pointers (MMIO addresses), which strict provenance refuses; worth a look later.
- Coverage: which `unsafe` blocks the tests reach at all (J96).

## What I learned

- **A test can have undefined behaviour the code it tests does not.** All three findings were in simulators: memory the real machine provides, made by the test in a way the real machine never would.
- **One undefined behaviour hides the next.** Miri stops at the first; the aliasing violation in `usb` was under the misaligned write, and showed only once that was fixed.
- **"Passed" can be luck.** `ahci` had `nvme`'s bug; its allocation landed aligned.

## Testing note

From the root of TheTaaJ (the first run installs the nightly; then 20 to 30 minutes):

| Do | You should see |
|---|---|
| `make miri` | A table: the eight crates with `unsafe` and `ok. N passed; 0 failed; M ignored`, the others `skipped: no unsafe` |
| In `usb/src/tests/mod.rs`, `start`, hand the driver a misaligned base: `(s.memory.base().wrapping_add(1), s.memory.len() - 1)`; then `cd usb && cargo test` | The tests that start the controller (23 of 25) panic: `DMA memory must be aligned to 8 bytes` - the check `Dma::new` now makes. Put it back |

Automated: not part of `make test64`; `make miri` is a night job.
