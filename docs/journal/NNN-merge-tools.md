# Stacks kept stacked, merges rehearsed by day, and the night runner merges what passed

*Journal entry #NNN · tools and a change to how we work. Written on TheJat-WSL/ci. Three things that were done by hand today, and went wrong by hand, are tools now: carrying a fix up a stack of branches (`tools/restack.py`), trying the night's merges before the night (`tools/merge-rehearsal.sh`), and the merges themselves (`tools/night-test.py --merge`, under the user's standing yes).*

## Why

The user agreed to two of the next bottlenecks: the stack work done by hand, and the merges. Today, eight branches were stacked in the night queue, and every fix to one meant merging it into each branch above, in order - by hand three times, and once a careless commit put a submodule checked out for another branch into a pointer. And a replay of the morning's merges, done by hand in a sandbox, found four faults that would have broken real merges (entries NNN of the split and the one-commit merge say which). Then the merges themselves: after a night pass, `merge-item.py` runs mechanically; the user said the runner may do it.

## What it is

**`tools/restack.py [--check] [QUEUE]`** - for each `b after a` of the queue, in order, in a tree of its own (`../TheTaaJ-restack`): b with a merged in if it lacks it (`merge-main.py` settles the bookkeeping conflicts), TheKernel's b with TheKernel's a when both have one, and `kernel` set to b's own TheKernel branch or else to a's pointer; pushed, TheKernel first. A real conflict stops it, named. `--check` says what it would do, the whole chain: `lacks a`, `will take in what a takes in`, a wrong pointer.

**`sh tools/merge-rehearsal.sh [QUEUE] [SCRATCH]`** - copies `main` and every queued branch of TheTaaJ, TheKernel and TheBootloader into bare repositories in SCRATCH (nothing on the server is written), and runs the night runner there with a stand-in test and `--merge`: the whole night, merges and all, in about 13 seconds. Then it checks `main` there: a row for each branch, no unnumbered entry, no second commit, `lanes-check`. `REHEARSAL_SETUP_ONLY=1` makes the sandbox only, to try a tool in.

**`night-test.py --merge`** - after a branch passes, the runner merges it, as its session would:

1. If `main` moved since the test: main's new commits touch a file this branch changes, or `kernel/abi` - tested again (rule 15); else `merge-main.py` merges it in.
2. `merge-item.py`, with the entry's title and `TAAJ_SESSION` as the README row.
3. The next branch is then tested on `main`, which has this one.

A branch with more than one unnumbered entry, or one `merge-item.py` refuses, is left for its session, and the ones stacked on it are not run. A stand-in test may merge only into a sandbox: the runner checks that `origin` is a directory on this machine.

**Rule 2** says it: an item in TheJat-WSL's night queue whose whole `make test64` passed in the night run, on exactly what is merged, is merged by the runner without asking again - the user's standing yes of 2026-11-27. Nothing else is.

`prepare()` in the runner uses `merge-main.py` too, when a branch does not take its base in by itself: a stacked branch, once the one below is merged, meets its lines numbered on `main`.

## How I checked it

- **The rehearsal**, the night queue's eight branches: `8 of 8 merged` - entries 131 to 138, each row with its commit, k187's with TheKernel's; no second commit; no unnumbered entry; `lanes-check` clean. 13 seconds.
- **The stand-in guard**: against a copy whose `origin` is GitHub, `--merge` with a stand-in merges nothing (the check is `origin` a path).
- **restack, in the sandbox**: a fix pushed to the bottom branch (a new file) and one to k187's TheKernel side; `--check` named all seven branches above and k187's pointer; `restack.py` merged each into the next, in order; after it, `--check` - every branch stacked; every branch has the fix; k187 and the four above it point at k187's new TheKernel commit, the two below at `main`'s.
- **restack and a real conflict**: a first try's fix changed lines `docs-split` also changes - it stopped there, `settle it by hand`, nothing pushed.
- **On the server**: `restack.py --check` on the real queue - every branch stacked.

## What is still missing

- `--merge` has run in the sandbox only. Its first real use is the night the user starts.
- A branch with two unnumbered entries is still left to its session: `merge-item.py` numbers one a merge.

## Testing note

From a session's tree:

| Do | You should see |
|---|---|
| `sh tools/merge-rehearsal.sh` | A row per queued branch, `passed`, then `merged: merge-item: entry N merged`; at the end `N of N merged; second commits 0; unnumbered entries on main 0` |
| `python3 tools/restack.py --check` | `stacked` for every branch, or what each would take in |
| `TAAJ_SESSION=TheJat-WSL/ci python3 tools/night-test.py --wait 60 --merge` | The night: each branch tested, `merged`, the next on `main` |

Automated: not part of `make test64`.
