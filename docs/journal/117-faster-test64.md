# A faster make test64, part 3: nothing built twice, and two failures under load gone

*Journal entry #117 · a test change (J94). Written on TheJat-WSL/ci. After entries 108 and 109 the waits were gone from `make test64`; this looked at what was left. The test phase no longer builds again what `test64-shared` has just built - LLVM's tools were linked twice on every run, now once - and the suite uses 22% less CPU. Two failures that came with a busy machine are gone: the disk driver stopped as hung in the middle of a test, and a Ctrl+C that reached the shell before the program it was meant for. On a machine now shared with other sessions, the wall-clock time moved within the noise.*

## Why

Three things showed up when every part was timed again, after entry 109:

| Seen | Where it came from |
|---|---|
| `e1000-64-test` took 216 s in the suite, one boot 98 s where the same check on the virtio card took 17 | The waits entry 109 fixed: alone it is now 113 s, that boot 18 s. Nothing left to do there |
| The long AArch64 parts (`aarch64-clang`, `-llvm`, `-libmake`, `-keys`, 115 to 136 s each) started at 300 s | Their prerequisites - `llvm-aarch64`, `kernel-aarch64`, ... - are `.PHONY`, so make ran them again in the test phase, though `test64-shared` had built them a minute before. While a part waited for that, make filled its 12 jobs with parts that were ready: `llvm-aarch64` found a free job only at 298 s. And `llvm64` relinked clang, lld and the rest every run |
| Under a heavier start, two failures: `times64-test` and `boot64-test` | See Bugs |

## How it fits together

`Makefile`:

```make
TEST64_SHARED = build-bootloader user64 iso64 ... llvm64 llvm-aarch64 pdpmake64 pdpmake-aarch64

test64-shared: $(TEST64_SHARED)

test64:
	@$(MAKE) --no-print-directory test64-shared
	@$(MAKE) --no-print-directory smp64-test
	@$(MAKE) --no-print-directory -j$(TEST_JOBS) --output-sync=target $(addprefix -o ,$(TEST64_SHARED) smp64-test) test64-parts
```

- `make -o TARGET` ("old file") tells make a target is up to date and not to run its recipe, `.PHONY` or not. The test phase gets every target `test64-shared` built, so no part waits for a rebuild and nothing is built twice. A part run alone (`make clang64-test`) still builds what it needs.
- `smp64-test` runs alone first, about 17 s. With every part ready at once, its 20-CPU boot - 20 boot slots at once - found them taken, and the typed boots, which wait for it (entry 108), started three minutes late.

`kernel/kernel64/scripts/` (TheKernel):

- `qemu-boot.sh` and `aarch64-boot.sh` give every test disk that has a file `cache=unsafe`: QEMU still offers the machine a disk with a write cache and takes its flushes, but no longer makes each one an `fsync` of the image on the host. An empty CD drive is left alone - QEMU refuses a cache mode on a drive without a medium.
- `qemu-boot.sh`: before Ctrl+C, wait for a program started since the last Enter (`... for the shell` from init), then 0.3 s - as `serial-type.py` does since entry 109.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| `make -o` for what `test64-shared` built | Making the build targets real files with prerequisites | One line; the targets stay `.PHONY` for everyone who builds by hand |
| `smp64-test` alone before the rest | First in `TEST64_PARTS` | Tried: with every part ready at once, the other boots reached the slots first anyway |
| `cache=unsafe` on test disks | A longer watchdog than init's 2 s | The watchdog is right for a real disk; on a test machine a slow host `fsync` is not the driver's fault. Tests need the image only while the machine runs and when the host reads it after: both see the data without an `fsync` |
| Added in the two boot scripts | In each of the 37 places a test names a disk | One place; a new test gets it too |
| The `e1000` boots left in a row | Running them side by side | Alone the part is 113 s, and it ends long before the typed boots |

## Bugs

- **The disk driver stopped as hung, mid-test** (`times64-test`, and `aarch64-times-test` on `main`). Evidence: `[init] blk has not answered for 2 s - stopping it` while `times` wrote its 301 MiB file, with no stall asked for; then "its time is a time of day" and four more file-time checks failed, and a file was missing. Cause: each flush became an `fsync` on the host, and with a dozen boots writing large images at once one took longer than init's 2 s. The `clib` failure of entry 109 in `tcc64-test` was most likely the same, and so were entry 115's on TheJat-VM (`blk` stopped during `llvm64-test` and `boot64-uefi-test`, worked around there with `TAAJ_BOOT_SLOTS=10`). On TheJat-VM, with no KVM, a machine starved of CPU could also miss the 2 s; that I could not test from here. Fix: `cache=unsafe`. Proof: no unexpected `blk` stop in five full runs; `main`, run between them, failed this way once. What happens to files after such a restart is a bug of its own, added as `K186`.
- **Ctrl+C before `cat` had started**, on x86 (`boot64-test`: no `echo clean`), as on AArch64 in entry 109. Fix: as above. Proof: five full runs.
- **My first `cache=unsafe` broke `ahci64-test`** in two runs: added to every `if=none` drive, it reached the AHCI test's empty CD drive, QEMU refused to start, and the boot was retried until it gave up - minutes each time. Fix: only drives with a file.

## How I checked it

All on TheJat-WSL (KVM, 24 CPUs), now shared with other sessions (load 2 to 4 when each run started), every run under the machine's lock:

| Run | Wall | CPU (user + system) | Result |
|---|---|---|---|
| `main` | 412 s | - | passed |
| this branch | 437 s | - | passed |
| this branch | 494 s | 1871 s | passed |
| `main` | 515 s | 2398 s | **failed**: `aarch64-times-test`, the disk driver stopped |
| this branch, also | 447, 464 s | - | passed |

So: **22% less CPU** for the same tests, no failure under load in five runs where `main` failed once; and the wall-clock time within the noise - the other sessions' work decides as much of it as this. `e1000-64-test` alone: 113 s.

## What is still missing

- **LLVM's tools are still linked once on every run**, in `test64-shared`, even when nothing they are built from changed (`llvm64` and `llvm-aarch64` are `.PHONY`, and `build-llvm.py` links each time).
- **The suite is now bound by the machine's CPUs, not by waits.** With every part started at once, the boot slots are full and the parts queue; a faster suite needs less work: fewer or shorter emulated AArch64 boots (they cost the most CPU: four host threads each, emulating), or test programs that don't need a boot.
- `TEST_JOBS` (12) and the boot slots (CPUs less one) are worth measuring again on a quiet machine.
- `K186`, for the `services` lane.

## What I learned

- **Wall-clock time lies on a shared machine.** Two runs of the same code differed by 80 s; CPU time showed the change.
- **Rebuilding nothing is the cheapest speed-up**, and `.PHONY` targets rebuild by design.
- **A flaky test is often a slow host.** The kernel's watchdog was right; what it watched was QEMU waiting on the host's disk.

## Testing note

From the root of TheTaaJ:

```
make test64 2>&1 | tee build/test64.out
awk '/smp64-test passed/ { t = 1 } t && /LD +(clang|lld)/' build/test64.out
grep -l "blk has not answered" build/times-64.log build/aarch64/times.log
```

| Look for | Meaning |
|---|---|
| `test64: all passed` | Every part passed |
| Nothing from the `awk` | No LLVM tool linked in the test phase (after `smp64-test`): only in `test64-shared`, before it |
| Nothing from the `grep` | The disk driver was not stopped as hung during the file-time tests |

Automated: `make test64`.
