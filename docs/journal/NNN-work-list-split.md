# The work list split: the rules in one file, the items a file per era, rows merged one at a time

*Journal entry #NNN · a change to how we work. Written on TheJat-WSL/ci. `docs/who-does-what.md` was 954 lines: the rules, the machines, who does what now, and all 302 items of seventeen eras. Every session read all of it, and every session's branch changed some of it - the most common merge conflict, and a big part of each session's reading. Now it keeps the rules, Now, what waits for the user and an index (173 lines); the items are in `docs/items/`, a file per era; the machines in `docs/machines.md`. And git merges the tables' rows one item at a time, so two branches changing two rows of one table no longer conflict.*

## Why

The first of the three bottlenecks the user agreed to fix. Measured: the file was 954 lines; three days of merges brought conflicts in it to most sessions - mine too, the same afternoon: J92 and J93 marked done on one branch, J94 on another, three neighbouring rows of one table, and git called it one conflict. With fourteen sessions, the work list was where they met.

## What it is

| File | What | Lines |
|---|---|---|
| `docs/who-does-what.md` | The goals, the rules, Before merging, What waits for what, Now, Waiting for you, and **Where the items are** (an index) | 173 |
| `docs/machines.md` | The machines: hardware, measured times, the order decided for each | 90 |
| `docs/items/era-13.md` ... `era-29.md` | Each era's items (15 to 51 lines) | |
| `docs/items/thejat-vm.md`, `mak-wsl.md`, `either.md`, `later.md` | The machine-specific lists, Either machine, Later eras | |

Every section was moved as it was - only its heading went up a level, under a line pointing back to the rules. Checked: all 302 item rows are there, the same set; no other line was lost (17 "(above)" pointers in Later eras were dropped, as nothing is above now).

A session reads the hub and its own era's file: 173 lines and about 30, not 954.

### Rows merged one item at a time

`tools/merge-rows.py` is a git merge driver, for the work list's files (`.gitattributes`). Git merges first; where it leaves a conflict and every line of it - on all three sides - is a table row whose first cell names an item (`| K187 |`, `| J?-word |`), the rows are merged by that name: a row one side changed takes that side's; both the same, kept; added on one side, added where that side has it; both changed differently - a real conflict - stays, that one row, with git's markers. Anything else stays git's own conflict.

It is set up once per machine (the repository's config, which every tree shares) by `tools/new-session.sh`; in a tree whose branch has no `merge-rows.py` yet, the driver falls back to git's own merge, so a merge never ends half done.

### Branches made before the split

Merging `main` into a branch that changed `who-does-what.md` stops with a conflict: the branch changed lines that `main` moved. `tools/work-list-move.py`, run in the stopped merge, takes the branch's own changes (the merge base's file against the branch's, from git's index), puts `main`'s hub back, and makes each change again where its lines are now - found by the lines it replaced, or the lines around an addition - in exactly one file, or not at all: a change found nowhere or twice is printed for the session. The hub's first lines say so, where a session with that conflict will look.

### The tools that read the list

- `merge-item.py`: the next K and J numbers from every file of the list on `main`; new items (`K?-word`) added in any of them.
- `lanes-check.py`: the items of every file.
- `merge-item-test.sh`: puts its test item where K189 is; `MT_BASE` tries a branch as `main`.
- `start-here.md`, `lanes.md`: point at the new files.

## How I checked it

- **The real conflict**, J92/J93 against J94 (the files from both branches and their merge base): git alone, 1 conflict; `merge-rows.py`, none - each row from the side that changed it.
- **What it must not merge**: one row changed differently on both sides - left as a conflict, that row only, exit 1; a conflict in prose - left as git's; rows added on both sides - both kept, each after its neighbour.
- **The split**: 302 item rows before and after, the same; `lanes-check.py` - 256 open items, every one in one lane, as on `main`.
- **The merge script** on the split (`MT_BASE`, entry 125's harness): a clean merge numbers the test item K194 in its era's file; TheKernel's push refused and TheTaaJ's push refused leave everything as before; the run after - all as on `main`. With entry NNN's one-commit merge too: no second commit.
- **Every open branch of mine, `main`'s split merged in**: `j92-clean-build`, `k187-miri`, `j93-ci`, `j96-coverage`, `test64-no-rebuild` stopped with the conflict; `work-list-move.py` made every change (in `items/era-25.md` and `items/thejat-vm.md`, and `j96`'s line in rule 2 in the hub), nothing left by hand, no markers, `lanes-check` clean. `night-test` merged without one.

## What is still missing

- **Other lanes' open branches** meet the conflict at their next merge of `main` - the hub's first lines and start-here say what to run. Tell MaK-WSL's sessions.
- The driver needs its config on each machine: TheJat-WSL's is set; TheJat-VM and MaK-WSL get it from their next `tools/new-session.sh`, or the two `git config` lines in its header.
- `docs/testing-notes.md` (4,498 lines, rebuilt at every merge) is the next big file; a file per era would do there too.

## What I learned

- **Git merges lines, but a table is rows.** Teaching git what a row is removed the most common conflict without changing how anyone writes.
- **Move text verbatim when you split it**: the migration could find every change because the lines were the same lines.

## Testing note

In a session's tree:

| Do | You should see |
|---|---|
| `wc -l docs/who-does-what.md docs/items/*.md` | The hub about 173 lines; an era file 15 to 51 |
| `python3 tools/lanes-check.py` | `every open item is in one lane` |
| Two branches marking two neighbouring items done, merged | No conflict (with the driver set up: `git config --get merge.rows.driver`) |
| A branch from before the split: `git merge origin/main`, then `python3 tools/work-list-move.py` | `changed docs/items/...`, `every change made`; `git diff --cached` shows the rows in their era files |

Automated: not part of `make test64`; `merge-item-test.sh` covers the merge script on it.
