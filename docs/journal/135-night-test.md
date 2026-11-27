# Night tests: develop by day, run the full test at night

*Journal entry #135 · a tool. Written on TheJat-WSL/ci. A session's day went mostly to waiting for `make test64` (about 7 minutes on TheJat-WSL, more on the others), once per item and again whenever `main` moved. The user's plan, tried for a few days: by day, items are developed with quick checks only and pushed to their branches; at night, when the user says so, `python3 tools/night-test.py` runs the full test on each, one after another, and leaves a report for the morning's merges.*

## Why

The full test is a must before a merge (rule 2), and it stays one. What changes is when it runs: not while a session waits, but in a row, with nobody waiting. The day's checks are an item's own boot test, the host tests and `make unsafe-check` - minutes, not the whole suite.

## What it is

```text
python3 tools/night-test.py [QUEUE]
```

- **The queue** (`~/.cache/taaj-night/queue`, machine-local): one branch a line, in the order to test and merge. `after BRANCH` stacks an item on another that is not merged yet: it is tested on that one's tested result, and not at all if that one failed.
- **Its own tree** (`../TheTaaJ-night`), made the first time as `tools/new-session.sh` makes one, so no session's tree is touched.
- **For each branch**: the branch with `main` merged in (or its base's result), TheKernel's branch of the same name the same way and `kernel` pointed at it; then `make test64` under the machine's test64 lock (rule 13), with this machine's LLVM settings if it has them.
- **Passed**: the merges are pushed to the item's branches (TheKernel's first), so in the morning `tools/merge-item.py` merges them as they are - rule 15 has nothing to re-test unless `main` moved again.
- **The report** (`~/.cache/taaj-night/reports/`): a line per branch - passed or failed, the time, the parts that failed, the log; a branch that does not merge, is not on the server, or whose base failed says so.

`NIGHT_TEST="make unsafe-check"` (any command) stands in for `make test64` to try the script itself in minutes; then nothing is pushed.

## What it found

Trying it with stand-in tests found three faults in the script itself, fixed before its first real night:

1. **A stacked branch built against an older kernel.** With no TheKernel branch of its own, it kept the kernel its branch had, while the merge had moved the pointer to the base's newer one - the programs were built against an older ABI. The submodule is now checked out as the merged result says.
2. **A stand-in test pushed its merges.** Only a real `make test64` makes a merge fit to push; under `NIGHT_TEST` nothing is.
3. **A branch not pushed** gave git's error; it now says "not pushed".

And `make unsafe-check` failed in a fresh tree: the kernel embeds the user programs, so linting it needs them built. It now depends on `user64` and `user-aarch64`.

## How I checked it

- A queue of three branches, one stacked (`after`), with `NIGHT_TEST="make unsafe-check"`: each tested on the right base, the stacked one on its base's result; a branch not on the server reported; nothing pushed.
- Its first real night is its own: `night-test` is in the queue.

## What is still missing

- **J93**: every branch found by itself, not queued, and the result posted beside it (`--all`).
- Whether it makes more items merged a day is the trial's question: count them, the night failures, and the rework of stacked branches, over a few days.

## What I learned

- **Test the tool that tests.** All three faults were in the script, not in the branches; a stand-in test found them in minutes, not nights.

## Testing note

From a session's tree (`~/Projects/TheTaaJ-ci`, say), with a queue in `~/.cache/taaj-night/queue`:

| Do | You should see |
|---|---|
| `NIGHT_TEST=true python3 tools/night-test.py` | `night-test: BRANCH on main` for each, and a report table, every branch **passed** (nothing is pushed) |
| `python3 tools/night-test.py` | The same with `make test64`, about 7 minutes a branch on TheJat-WSL; the report in `~/.cache/taaj-night/reports/` |

Automated: not part of `make test64` - it runs `make test64` itself.

## Files

- `tools/night-test.py`: the runner.
- `Makefile`: `unsafe-check: user64 user-aarch64`.
