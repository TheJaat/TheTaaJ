# The second night: MaK-WSL's finished branches through TheJat-WSL's queue

*Journal entry #150 · a night. Written on TheJat-WSL/ci. The first night that ran another machine's work: the user's choice B - MaK-WSL's finished branches tested with the whole `make test64` and merged by TheJat-WSL's runner, crediting their sessions. Eight items merged, entries 141 to 148; two of MaK-WSL's wait for a test of their own. The two failures that were not MaK-WSL's - a stale build in the runner's tree, a flaky check - were found out the same night, and the branches passed on a second run.*

## Before

MaK-WSL named eight finished branches (two stacks and one alone) and kept `k193-cpu-locks`. A rehearsal in the sandbox found what would have stopped them, before any night: three faults of the runner and `merge-item.py` (entry 141 says which - `merge-item.py` pushed TheKernel's main before checking the lanes) and one of MaK-WSL's: every `boot` and `desktop` branch left its done items in its lane's table. MaK-WSL fixed its branches; the rehearsal then merged 9 of 9.

## What ran

| Branch | Session | Result | Entry |
|---|---|---|---|
| `queue-sessions` | TheJat-WSL/ci | passed, 9 min 11 s | 141 |
| `k129-stage2-tests` | MaK-WSL/boot | passed, 9 min 35 s | 142 |
| `k131-fast-bios-read` | MaK-WSL/boot | passed, 9 min 02 s | 143 |
| `k23-boot-time` | MaK-WSL/boot | passed, 8 min 59 s | 144 |
| `k130-any-size` | MaK-WSL/boot | **failed** after 41 s (the runner's stale build); passed on the second pass, 9 min 41 s | 146 |
| `k114-desktop-bench` | MaK-WSL/desktop | **failed** `power64-test` (flaky); passed on the second pass, 9 min 53 s | 147 |
| `k42-key-events` | MaK-WSL/desktop | **failed** `coverage` - MaK-WSL's to fix | - |
| `k46-wheel` | MaK-WSL/desktop | not run: stacked on `k42-key-events` | - |
| `e8-users-guide` | MaK-WSL/docs | passed, 10 min 02 s | 145 |
| `bootloader-fresh` | TheJat-WSL/ci | passed, 10 min 09 s (the runner's fix, below) | 148 |

Two passes: 67 min and 44 min; eleven runs of `make test64`, eight passed. After the merges, `main` points at TheBootloader's `d898aabc` (`k130-any-size`'s): TheBootloader's own `main` is MaK-WSL's to fast-forward to it (rule 7).

## The three failures

1. **`k130-any-size`: a stale build - the runner's.** `stage2.bin` did not link: `undefined reference to 'BootHeader'`. `k130-any-size` adds `global BootHeader` to an `.inc` file that `stage2`'s assembly includes; TheBootloader's Makefile does not track `.inc` files, and the night tree's `stage2.o`, assembled for the branch before, was kept. A fresh build of the same commit passes. The runner had called it MaK-WSL's to fix - wrongly. Fixed in `bootloader-fresh` (entry 148): the night tree builds TheBootloader from nothing whenever its commit changes. Passed then. For MaK-WSL: TheBootloader's `.o` rules could depend on the `.inc` files they include (`nasm -M`).
2. **`k114-desktop-bench`: a flaky check.** `power64-test`: `q35-off: ahci did not give up on a controller with no disk` - the check that the disk driver, restarted 5 times in 30 seconds, is given up on: a time window, the same family as the disk-driver kill that failed under load before (K186). `k114-desktop-bench` changes the display, the shell and one ABI message - nothing in `ahci` or `init`. Tested again: passed, `power64-test` included. A data point for J95.
3. **`k42-key-events`: `coverage` - MaK-WSL's.** `kernel/abi` at 86.6%, under its floor of 87 (entry 137): the branch adds `display::KEY`, `KEY_SIZE` and `key_message()` to the ABI crate, with no host test. The floor's first catch. A test of `key_message()` (three lines) brings it back; `k46-wheel` comes with it.

## For the morning: what could be faster

- **3.0 CPUs busy of 24** again, on every run - the suite waits on its longest parts (night 1's entry). The same five parts finished last.
- **A stale build costs a night's branch** - TheBootloader's missing `.inc` dependencies (above); the runner works round it, the Makefile should not need it.
- **Whose failure is it?** The runner's first answer - "the session's whose branch it is" - was wrong twice out of three. Before handing a failure to another session, the runner could look: a failure in `test64-shared` (the build) with the same commit building fresh elsewhere, or a part that failed on a branch that does not touch what it tests, is likely the runner's or a flaky one - try once more before saying so.

## Testing note

| Do | You should see |
|---|---|
| `git log --oneline origin/main` | entries 141 to 148, one commit each |
| The night's reports, `~/.cache/taaj-night/reports/` | two: the first pass and the second |

Automated: not part of `make test64`.
