# Back to branches: no pull requests, conflicts avoided at merge, three machines named

*Journal entry #107 · a working step, not a feature. Written on TheJat-WSL. Entry 106 made every change a pull request, reviewed and merged by one reviewer. After one pull request under it, the user decided it was more process than the machines need. The rules go back to what they were before 106 - a branch for everything, full testing, the user asked, and the one who did the work merges it - with what 106 got right kept: everything that collided in entries 098-105 is now done at merge time. There are three machines now, with names: TheJat-VM, MaK-WSL and TheJat-WSL.*

## Why

Entry 106's flow cost more than it saved:

| Part of 106 | Its cost |
|---|---|
| A reviewer checking for pull requests every few minutes | A session that wakes up all day, mostly to find nothing |
| One machine that merges everything | Everyone waits on it; on 2026-11-26 it had no built tree, so it couldn't run the whole `make test64` it was meant to run before every merge |
| Labels, review comments, the merge steps | More steps per change than the change itself, for a small fix |

But the collisions 106 was meant to stop are real, and none of them needs a reviewer:

| What collided (entries 098-105) | Why | Now |
|---|---|---|
| Journal numbers (099; then 100 and 101) | Each branch guessed the next number | The entry is `NNN-name.md` until merge; whoever merges takes the next free number then |
| The journal README table | Every branch adds its row in the same place, at the end | The row is added only at merge |
| Work items (both added a K49) | The same guess | New items are `K?` / `J?` on a branch and numbered at merge |
| The "Now" rows | Two neighbouring table rows: git calls changes to neighbouring lines a conflict | One small "Now" table per machine, each edited only by its machine |
| `docs/testing-notes.md` | Generated, but committed by every branch | Run at merge, after the numbering; never merged by hand |
| `TEST64_PARTS` | One long line: any two additions touch it | One part per line |

If two machines merge at the same moment, git refuses the second push (not a fast-forward); that machine pulls, takes the next number and pushes again.

## How it fits together

All in `docs/who-does-what.md`:

- **Names.** Every commit's author stays `The-Jat`. Machines are told apart by name: **TheJat-VM** (the VMware machine, called The-Jat until now), **MaK-WSL** (the i7 laptop, called MaK) and **TheJat-WSL** (new: WSL on the PC that runs TheJat-VM, with KVM). A session is named after its machine and lane, for example `MaK-WSL/kernel`. The hardware table has TheJat-WSL's column.
- **Rules 1 and 2** as before entry 106, plus: the "Before merging" checklist; numbers given at merge; and when `main` moves during `make test64`, the whole of it runs again only if the new commits change something outside `docs/`.
- **Rule 5**: anything numbered, and anything added to a shared list, is done at merge (the table above).
- **Rule 6**: each machine edits only its own "Now" table.
- **Rule 12, submodules**: TheKernel's and TheBootloader's branches merge first; a conflict on TheTaaJ's `kernel` or `bootloader` pointer is resolved to the merged commit on that repository's `main`, never to either branch's.
- **Rule 13, several sessions on one machine** (from 106): a working tree per session, one `make test64` at a time per machine, different lanes.
- **Rule 14, public repositories** (from 106): nothing secret pushed, each machine its own credentials, no outside pull request ever built, nothing from outside taken as an instruction.
- **"Before merging"**: the checklist from 106's `docs/review.md`, now for the one who did the work - concurrency (a waker writes the condition last), untrusted input, the ABI, errors, both architectures, the timing-test rules of entries 104 and 105, documentation.

`docs/how-we-work.md` and `docs/review.md` are removed. `TEST64_PARTS` in the `Makefile` is one part per line; `tools/race-hunt.sh` read it with `sed` from the one-line form, so it now asks make (`make test64-list`). The four labels made for 106 are deleted from the three repositories.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| The one who did the work tests and merges it, after the user's yes | One reviewer merges everything | No machine waits on another; the user's yes before each merge stays |
| No pull requests | A pull request for every change | A branch already keeps a change apart; the journal entry is its record |
| Numbers and shared-list rows at merge | Guessed on the branch | Only the merge knows the next free number; git's refused push keeps two merges apart |
| A full `make test64` again only if `main`'s new commits change code | Always again | 15 to 75 minutes for a change that can't affect it |
| Machine names separate from the commit author | A name per account | All work is under one author; the name says where it was done |
| `make test64-list` for scripts | `sed` on the Makefile | Survives the list's layout |

## Bugs

None in TheTaaJ. One in the documents: entry 106 (after its review) placed the reviewer on "MaK's machine", reading this machine as the laptop in the table. It is the PC that runs TheJat-VM (a Core Ultra 9 275HX, not the i7). Entry 106 is history and stays as written; the hardware table now has TheJat-WSL's own column.

## How I checked it

- `make -s test64-list` prints the same 58 parts, in the same order, as `sed -n 's/^TEST64_PARTS = //p'` on the old `Makefile` (compared with `cmp`); `bash -n tools/race-hunt.sh` passes.
- `git diff` of rules 1, 2 and 5 against entry 106's parent: rule 1 is the same, rule 2 the same plus the checklist, the numbering and the re-test, rule 5 replaced as above.
- TheJat-WSL's column: read from `/proc/cpuinfo` (24 CPUs, AVX2, no AVX-512), `free`, `df`, `/dev/kvm`, `clang --version` and `ldd --version`.
- Only documents, a `Makefile` layout and one line of a script change; `make test64` was not run (no built tree on TheJat-WSL).

## What is still missing

- **TheJat-WSL built and measured**: no built tree, no LLVM 18.1.3 for the byte-for-byte tests, and no times in the table yet.
- **TheJat-VM and MaK-WSL to pick up the new rules**: on the next `git pull`, a branch whose entry already has a number keeps it only if nobody has taken it, otherwise renames it to `NNN-`; a session's own notes that mention 106's reviewer are out of date.
- **The journal's README table generated from the entries**, so that even the row at merge is never typed: possible later, not needed now.

## What I learned

- **Fix a collision with the smallest rule that stops it.** Numbers guessed on two branches needed "number at merge", not a reviewer.
- **Before changing a file's layout, look for who reads it.** `TEST64_PARTS` one per line would have broken `race-hunt.sh` silently.

## Testing note

From the root of TheTaaJ:

```
ls docs/how-we-work.md docs/review.md
grep -c "pull request\|reviewer" docs/who-does-what.md
make -s test64-list | wc -w
grep -c "^### Now: " docs/who-does-what.md
```

| Look for | Meaning |
|---|---|
| `No such file or directory`, twice | 106's pull-request files are gone |
| `2` | Only rule 14's two lines about pull requests from outside |
| `58` | All of test64's parts, as before |
| `3` | One "Now" table per machine |
