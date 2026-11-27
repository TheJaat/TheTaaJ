# One commit per merge: the README row's commits written in the merge itself

*Journal entry #131 · a tool change. Written on TheJat-WSL/ci. Every merge was two commits on `main`: the item, numbered, and then `journal: entry N's commits`, a second commit only to fill in the README row's commit - because the row named the merge's own commit, which has no name until it is made. In three days, 46 of `main`'s 196 commits were those. Now the row names the item's own last commit, which is known before the merge, and is written in the same commit.*

## Why

One of the three bottlenecks the user agreed to fix. The second commit doubled each merge's pushes to `main` - and each push is a moment in which another machine's merge collides and is refused. And it filled `main`'s history with bookkeeping: a quarter of its commits.

## What changed

- `tools/merge-item.py`, step 4: the README row is written with the item branch's last commit (and TheKernel's `main` after its merge, when the item changed TheKernel), e.g. `` `b291d784` (TheKernel `28e3efd`) ``. That commit is in `main`'s history once the merge is pushed - the numbering commit is made on top of it - so the row points at the item's work, as before.
- Step 5, the follow-up commit with up to three retries, is gone: one push to `main` per merge.
- `docs/start-here.md` section 6 says the same for a merge by hand: the row with the item's last commit; step 8 (the follow-up commit) removed.

The commit the row names is the last of the item's own commits, not the numbering commit after it: `git show` of it shows the item's last change, `git log` up to it the whole item.

## A bug found on the way

Replaying the morning's merges in a sandbox (local copies of both repositories, every branch of the queue merged in turn with this script) stopped at the second: the first had left `tools/merge-item.py` changed and not committed. `renumber()` numbers `NNN` on the branch's added lines in every file - a script's comment saying "journal entry 131" too - but only `docs/` was added to the commit. So the comments were numbered in the tree, left out of `main`, and the next merge found the tree dirty. No merge before this one had added such a comment in TheTaaJ (TheKernel's were added: its own `git add` used the list). Now every file `renumber()` changed is added, and the harness checks it: a comment with `NNN` in a script, numbered on `main`, the tree clean after.

## How I checked it

`sh tools/merge-item-test.sh SCRATCH` (entry 125's harness: local copies of both repositories, four runs), with main's script and then with this one:

| | Main's script | This one |
|---|---|---|
| A: a clean merge | row `` `0488b8d9` (TheKernel `b612faa`) ``; last commit on main `journal: entry 131's commits` | row `` `074a3915` (TheKernel `0b607cb`) ``; last commit on main the merge itself |
| B: TheKernel's push refused | stopped, nothing changed | the same |
| C: TheTaaJ's push refused | stopped, numbering undone | the same |
| A, a script's comment with `NNN` | numbered in the tree, left out of `main` (found in the sandbox) | numbered on `main`; the tree clean after |
| D: C's branch, main merged in, run again | the row's commit in main; one second commit | the row's commit in main; **no second commit** |

The harness's last check named entry 124, which main has long passed; it now reads the last row, checks its commit is in `main`'s history, and counts `journal: entry` commits.

## What is still missing

- The rows of entries before this one keep their old form; nothing to change.

## Testing note

From a session's tree:

| Do | You should see |
|---|---|
| `sh tools/merge-item-test.sh /tmp/mit` | A and D `exit 0`; `second commits: 0`; `the row's commit ... is in main: yes`; B and C stopped as before |

Automated: not part of `make test64` - it is the merge script's own test.
