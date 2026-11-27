# A merge script, after a merge that went half wrong

*Journal entry #NNN · a tool and a fix to how we work. Written on TheJat-WSL/ci. Merging an item is a dozen steps across two repositories, and with fourteen sessions merging, two can reach the same step in the same minute. One of mine (entry 123) went on after TheKernel's push was refused, and left TheTaaJ's `main` pointing at a TheKernel commit that was only on a branch. Now `tools/merge-item.py` does the merge, and stops - undoing what it did - when a push is refused.*

## What happened

Entry 123 (the fuzzer) and MaK-WSL/sched's K20 were merged in the same minute:

```text
 TheJat-WSL/ci                               MaK-WSL/sched
 numbers its TheKernel commits "123"
                                             numbers its TheKernel commits "123"
                                             pushes TheKernel's main      - accepted
 pushes TheKernel's main  - refused
 ... and goes on, in the same command line:
 numbers TheTaaJ's entry 123, points kernel
 at its own commit, pushes TheTaaJ's main     - accepted
                                             pushes TheTaaJ's main        - refused
                                             (renumbers to 124, as rule 5 says)
```

The two halves of my merge were joined with `;`, not with `&&`, so the TheTaaJ half ran after the TheKernel half had failed. TheTaaJ's `main` pointed at `4f090e0`, a TheKernel commit on the item's branch only, and its README row said `(TheKernel ``)`. I repaired it by hand: merged TheKernel's main (K20) into mine and pushed, then pointed TheTaaJ at my TheKernel commit - reachable from TheKernel's main now, and the right pair for TheTaaJ's main, which did not have K20's half yet. Then, testing a first version of the script, the same mistake again, on my own tree this time: a step after a failed one ran in the wrong directory. Nothing was pushed; a local commit was undone. Two in a day: a person can't type this reliably.

## What it is

```text
python3 tools/merge-item.py "README ROW"
```

run from the session's tree, on the item's branch, once the item is tested with main merged in and the user has said yes. It does `start-here.md` section 6, steps 6 to 8:

1. **Checks**: both trees clean; one entry named `NNN-*.md`; each branch has its main merged in (if not: merge, test again, run again).
2. **Numbers**: the next free entry number (from main's README), and the next free K and J for new items - rows the branch added to `who-does-what.md` that begin `| K?-word |`.
3. **TheKernel first**: `NNN` numbered on the lines this branch added; committed, merged into TheKernel's main, pushed. **Refused**: the commit undone, TheTaaJ untouched, and it stops.
4. **TheTaaJ**: the entry renamed and numbered, the new items numbered, the README row (the argument's text), the testing notes, the lanes check; `kernel` pointed at what was just pushed; committed and pushed. **Refused**: the commit undone; if TheKernel's half is already on its main, the branch records that, and the next run points at it.
5. **The row's commits**, in a commit of their own - rebased and pushed again if main moved meanwhile: it is one line.
6. **The branches** deleted, there and here.

Two rules make it safe to run on any branch:

- **Only lines the branch added are changed.** `who-does-what.md`'s rules say "Done (entry `NNN`)", and its examples name `K?-hubs`; they stay. (A first version numbered `K?-hubs`.)
- **`NNN` in backticks, or followed by a hyphen, is the convention's name**, not a number to give: `NNN-name.md` stays as written. The script's own pattern is spelled in two pieces, so that merging the script with itself leaves it whole - this entry was merged that way.

## How I checked it

A copy of both repositories, bare, made locally from today's mains, a tree cloned from them, and an item branch in both - a new entry, an `NNN` comment in TheKernel, a new `K?` item. Four runs, each on a fresh copy (`tools/merge-item-test.sh SCRATCH`, which anyone can run again):

| Run | What it must do | Result |
|---|---|---|
| Clean | Merge: entry 125, the item K191, TheKernel numbered and pushed, TheTaaJ pointed at exactly that, the row's commits, branches gone | Done |
| TheKernel's push refused (a hook pushes a rival commit first) | Stop; TheTaaJ's main unchanged; TheKernel's branch as it was | Done |
| TheTaaJ's push refused | Stop; its commit undone; say TheKernel's main has the item; the branch records the new pointer | Done |
| The same tree, TheTaaJ's main merged in, run again | Merge, pointing at the TheKernel commit the refused run had pushed | Done |

The first version failed three of these: it numbered the rules' `K?-hubs`; after a refused TheTaaJ push it left TheKernel checked out on main, and the tree dirty; and the run after that pointed at TheTaaJ main's old TheKernel commit. Then this item: merged by the script.

`make test64` as for any item.

## What is still missing

- `abi` numbers (rule 16) and TheBootloader (rule 7): by hand.
- A refused TheTaaJ push after TheKernel's was accepted leaves TheKernel's comments saying a number the other session may take; the script says so, and the next merge corrects them. Merging both repositories at once is not possible.

## What I learned

- **`&&`, never `;`, between steps that depend on each other** - and a script, not a command line, for anything done by many people many times.
- **A tool that changes text must know which text is its to change.** "Lines this branch added" is what makes renumbering safe in files full of examples.

## Testing note

From a session's tree, on an item's branch that is ready (tested, the user's yes):

| Do | You should see |
|---|---|
| `python3 tools/merge-item.py` | `merge-item: usage: merge-item.py "README ROW" (the row's text)` |
| `python3 tools/merge-item.py "What the item did"` | `merge-item: entry N`, `TheKernel's main is ...` (if it changed TheKernel), `TheTaaJ's main is ...`, `entry N merged: ...` |
| The same, after another session merged first | `merge-item: TheKernel's push was refused ...` or `TheTaaJ's push was refused ...`, and the trees as they were |

Automated: `make test64` does not run it; `sh tools/merge-item-test.sh SCRATCH` tests it, without touching the server.
