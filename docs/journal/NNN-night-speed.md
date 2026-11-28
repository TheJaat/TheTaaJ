# The night, faster and fairer: parts ordered by their times, a retry before a failure, --from, and the rehearsal's toolchain link

*Journal entry #NNN · four fixes from the first two nights' notes (entry 139, and the second night's entry, "The second night"). Written on TheJat-WSL/ci, by day, with quick checks only; the next night is the first real run.*

## 1. The parts ordered by their own times

Each branch's `make test64` took about 10 minutes, with only about 3 of the machine's 24 CPUs busy while the parts ran. The parts run `TEST_JOBS` (12) at a time, started in `TEST64_PARTS`' order. A long part given late is still running when the rest are done. On night 1, the last to end were `aarch64-keys-test` (10:08), `times64-test` (9:56), `schedbench64-test` (9:42), `aarch64-times-test` (9:31) and `e1000-64-test` (9:11). The other parts had finished by about 9:00.

The logs could not say how long a part ran, only when it ended (`--output-sync` prints a part's output when it is done). So:

- **`tools/part-time.sh`** is the shell make runs each part's recipe lines with (`--eval "PARTS: SHELL = tools/part-time.sh $@"`, given by `tools/test64.sh`). It runs each line with `/bin/sh` as before, and writes `TARGET START END` to a file. A part's time is from its first line's start to its last line's end.
- **`tools/test64.sh`** keeps each part's seconds from the last run that ran it in `build/test64-times`. Next time it gives make the parts longest first by those times. A part with no time yet (a new one) goes before them all. At the end it prints the five longest. The Makefile's order is only the first run's.

The night tree keeps its `build/`, so from the night's second branch on, every run is ordered by the run before it.

Checked: a stand-in Makefile (a 2 s part with a 1 s prerequisite, a 1 s part through a sub-make, a 0 s part, then a new failing one). Times `slow 2, mid 1, fast 0`; the next run's order was `bad slow mid fast`; `build/test64-failed` was still right. A dry run of the real Makefile with the `--eval` parses and prints `aarch64-keys-test`'s recipe unchanged. No recipe uses `$(SHELL)` or `.ONESHELL`.

## 2. A failed part runs once more before the branch is called failed

Night 2's `k114-desktop-bench` failed `power64-test` only, in its q35-off case ("ahci did not give up"). The branch touches nothing there, and it passed on its second full run, 10 minutes later. That was the same timing family as K186's flaky disk-driver kill.

Now `night-test.py` runs `make test64-again` (only the parts that failed) once before it calls the branch failed:

- passed on the retry: the report says `passed on a retry of power64-test (failed once: flaky?)`. That is a data point for J95. Every part has then passed on exactly this tree, so the standing yes applies as for any pass.
- failed again: `FAILED (failed on the retry too)`, and only then is the branch's session told. A real failure fails twice. Night 2's coverage failure in `k42-key-events` would have, and so would `k130-any-size`'s link failure.
- a failed shared build (`test64-shared`) is not retried: it fails the same way.

`--all` (the CI) retries the same way. `NIGHT_TEST_AGAIN` stands in for `make test64-again` when trying the runner with `NIGHT_TEST`.

## 3. `--from BRANCH`

On night 1, the runner stopped on its own fault after the first branch. The 8 after it were "not run", and the rest of the queue had to be copied into a second file by hand. `--from BRANCH` starts the queue there. A branch passed over that is on main already counts as merged for the branches stacked on it. merge-item.py deletes a merged branch from the server, so "on main" is found by its merge commit (`BRANCH, entry N:`), or else by its commit being in main. A name not in the queue is refused before anything runs.

## 4. The rehearsal sandbox has the cross compiler's link

Every real tree has `toolchain/i686`, a link to the cross compiler, and a merge in the night tree must not see it as a change. Night 1's first merge did (" M toolchain", entry 139). The rehearsal sandbox had no link, so it could not see that. `merge-rehearsal.sh` now copies TheToolchain too and makes the link (it builds nothing with it).

Checked: the queue (`night-2`) rehearsed, 1 of 1 merged; with the night tree's exclude taken out by hand, the sandbox shows ` ? toolchain` - the fault the sandbox now finds by day.

## For the morning

- The first real run gives `build/test64-times`. Look at its top lines: the real long tail, and whether a part's own prerequisites (not counted in its time) should be.
- `TEST_JOBS` above 12 is still to try, under load, one night (rule 13: the timing tests first).

## Testing note

| Do | You should see |
|---|---|
| `make test64` twice | The second run's parts given longest first; `test64: the longest parts (s): ...` at the end; `build/test64-times` |
| A night where a part fails once and passes again | `passed on a retry of PART (failed once: flaky?)` in the report |
| `night-test.py --from BRANCH` | The branches before it `passed over`; those on main `on main already` |
| `sh tools/merge-rehearsal.sh` | `toolchain/i686` in the sandbox's trees |

Automated: not part of `make test64`.
