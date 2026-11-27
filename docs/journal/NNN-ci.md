# Our own CI: every branch of ours tested, the result beside it on GitHub; night tests that wait for the fix

*Journal entry #NNN · a tool (J93). Written on TheJat-WSL/ci. Fourteen sessions push branches, and each tests its own on its own machine - or, by day, only quickly, leaving the full test for the night. Nobody tested `main` after a merge, or told a session its branch no longer merged. Now `python3 tools/night-test.py --all` tests `main` and every branch of ours not merged yet, and posts each result as a commit status, where the branch's session sees it.*

## Why

Two changes tested apart can still break together (rule 15): only a test of `main` after the merges finds it. And a branch left a few days behind `main` can stop merging without its session knowing. J93 asks for "a machine of ours that builds every pushed branch, runs the host tests and `make test64`, and posts the result where the branches are; no outside service gets the code to run", and never anyone else's pull request.

## What it is

The night runner (the night-test entry, `tools/night-test.py`), with its branches found rather than queued:

```text
TAAJ_MACHINE=TheJat-WSL python3 tools/night-test.py --all
```

1. **What it tests**: `main`, as it is, first; then every branch on TheTaaJ's own server that `main` does not already contain - `llvm-23` left out - with `main` merged in, and TheKernel's branch of the same name, if there is one, the same way. Only `git ls-remote --heads origin`: branches the project's sessions pushed, never a pull request or a fork (rule 14).
2. **Only what is new**: what was tested - the branch's newest commit, TheTaaJ's and TheKernel's - is kept in `~/.cache/taaj-night/tested`. A branch is tested again only when one of them moves; `main` after every merge.
3. **The test**: `make test64`, which runs the host tests too (`kernel64-test`, every crate's `cargo test`, `libc-test`, `unsafe-check`), under the machine's test64 lock.
4. **The result, beside the branch**: a commit status on the branch's newest commit (TheTaaJ's, and TheKernel's when it has a branch of its own) - `pending` while it runs, then `success` or `failure`, context `TheJat-WSL test64`, and the parts that failed. A branch that does not merge with `main` gets a `failure` saying so. `gh api repos/OWNER/NAME/statuses/COMMIT`, with the account the machine is logged in with.
5. **Nothing pushed**: the branches are other sessions'. (A queued night test still pushes `main` merged in to its branch, as before.) The report is written as the night's is, headed CI.

`NIGHT_TEST=true` (any stand-in for test64) tries the run in seconds and then neither posts nor records anything.

## What it found

The first run, with a stand-in test, found the six it should - `main`, `j55-bin`, `j66-proc`, `j92-clean-build`, `k187-miri`, `night-test` - and left out the 23 branches `main` already contains:

```text
| main            | main | passed           |
| j55-bin         | main | does not merge   | TheKernel's j55-bin does not merge with origin/main |
| j66-proc        | main | does not merge   | TheKernel's j66-proc does not merge with origin/main |
| j92-clean-build | main | passed           |
| k187-miri       | main | passed           |
| night-test      | main | passed           |
```

Two old branches no longer merge - the kind of thing nobody was told before.

## How it is run

When the user says so (no cron, by the user's choice): after the queued night tests, or alone. It takes `make test64`'s time - about 7 minutes on TheJat-WSL - for each branch that moved. J95 (every part 50 times) is built on it next.

## How I checked it

- `NIGHT_TEST=true python3 tools/night-test.py --all`: the six branches above, the merged ones left out, nothing posted or recorded.
- One status posted by hand with the same call, on this branch's commit: `pending | TheJat-WSL test64` read back from GitHub.
- Its first real run is the night's: `j93-ci` is queued after `night-test`.

## What is still missing

- **Statuses on old commits**: a branch tested before its new commit only shows the old one's result until the next run.
- **J95**: every part 50 times - on this runner, when the user says.
- Only TheJat-WSL runs it; another machine would post under its own name.

## What I learned

- **Branches nobody watches drift.** The first run found two that no longer merge with `main`.

## Part two: Night tests wait for the fix: fix, test again, then the next branch

*The user's rule for the night: the session is not only the tester. When a branch fails, it fixes it, tests it again, and goes on to the next branch only after that; and so on for every branch, unless the user says otherwise. "Make use of time": don't spend the machine on tests that will have to be run again. The runner used to end a failed branch's stack and go on with the queue; a failure cost the night. Now, with `--wait`, it waits for the fix.*

### Why

The night queue is stacked in merge order (rule 2): each branch is tested on the one before's result. When one fails, the ones after it can only be tested on a failed base, and every such test has to be run again after the fix: wasted. Waiting for the fix wastes nothing. Most failures are small - an expected text, a test's limit, a merge with `main` - and fixed in minutes. And while the runner waits, the test64 lock is free for the fix's own checks.

### What it is

```text
python3 tools/night-test.py --wait 60
```

1. The queue, one branch after another, as before.
2. **A branch fails**: the runner waits. Every minute it looks at the server for a new commit on the branch (TheTaaJ's or TheKernel's).
3. **The session, meanwhile**, reads the failure in the log (`~/.cache/taaj-night/logs/NIGHT/BRANCH-TRY.log`) and fixes it in its own tree, not the night tree. It checks the fix with `make test64-again` (only the failed parts, with the lock the runner left free), documents it (rule 10: in the item's entry, or its own), commits and pushes.
4. **The fix arrives**: the runner tests the branch again, the whole `make test64`, on the same base. Passed: its merge is pushed, and the next branch starts on its result. Failed again: back to 2.
5. **No fix for `--wait` minutes**: it stops there - the branches after it are `not run` - and the report says so. Without `--wait`, the first failure ends the stack, as before.

Every try is a row of the report: what failed, and the try that passed after the fix.

### Everything written down, for the morning

The user's second rule: document everything, so that in the morning we can look for what to make faster - there is always something. So a night leaves three records:

1. **The logs**, one per try (`~/.cache/taaj-night/logs/NIGHT/BRANCH-TRY.log`). Every line is stamped with the minutes and seconds since the test started; `make test64` prints a part's output as one block when the part ends (`--output-sync`), so the stamps say when each part finished, and the long ones stand out. `night-test: the test64 lock taken` shows how long it waited for the lock. At the end, from `/usr/bin/time`: the CPU time (user and system), the most memory, the wall time.
2. **The report** (`~/.cache/taaj-night/reports/NIGHT.md`): a row per try - the time, its CPU time, the parts that failed (from `build/test64-failed`, which `make test64` writes) - and a row per fix: how long it took, and the fix's own commits, from git (`fixed in 12 min by: abc1234 ...`). At the end, **For the morning**: the night's wall time split into testing, waiting for fixes and the rest (fetching, merging), the runs passed and failed, the CPU time, and how many CPUs were busy on average.
3. **A journal entry per night**, written by the session as it goes (`docs/journal/NNN-night-YYYY-MM-DD.md`, on its own branch, merged in the morning): what ran, each failure - what it was, why, how it was fixed - the numbers from the report, and **ideas for what could be faster**, each with the number that suggests it. The morning starts from that list.

### The first ideas, from the trials

- **The time between tests.** Two stand-in tests of 2 seconds each took 64 seconds of wall time: fetching both repositories, checking out, merging and updating the submodules, for each branch and each try. Eight branches: 4 minutes. One fetch per round, and `git worktree` checkouts without `submodule update` when the pointer did not move, would cut most of it.
- **`test64-shared` relinks LLVM's tools every run** (entry 117): each branch pays it again. A stamp file per tool, rebuilt only when its inputs change.
- **A stacked branch builds everything again** although it differs from the one before by a few files: incremental builds in the night tree help already; a measure of how long each branch's build part takes (the stamps) will say how much is left.

### How I checked it

- The report and the stamped log with a stand-in test that prints, sleeps 2 s and fails `k187-miri`: stamps `[00:00]`, `[00:02]`, the CPU line, and the **For the morning** summary; and, with the server stubbed, the row `fixed in ... by: COMMIT MESSAGE` before the passing try.
- A queue of three stacked branches, a stand-in test failing `k187-miri`, `--wait 1`: `j92-clean-build` passed; `k187-miri` FAILED; `waiting for a fix (up to 1 min)`; `no fix pushed for 1 min`; `night-test` not run.
- The runner with the server stubbed, `k187-miri`'s fix arriving during the wait: the tests ran in exactly the user's order - `j92-clean-build` pass, `k187-miri` FAIL, `k187-miri` (try 2) pass, `night-test` pass. Nothing was tested on a failed base.
- A first try said `passed` for the branch that should fail: the stand-in command got the make flags as arguments, git refused them, and `!` turned the error into a pass. A stand-in must take its own arguments (`sh -c '...' -`).
- An earlier version of this change tested the branches after a failed one on its result (marked "a failed base") while waiting. The user's rule is better: those tests would all be run again after the fix.

### What is still missing

- The session has to be there to fix. Unattended, the runner waits `--wait` minutes and stops; the failure waits for the morning.
- A failure in a branch another lane owns (`--all`) is posted for that lane, not fixed here.

## Files

- `tools/night-test.py`: `--all` (`everything`, `heads`), the record of what was tested, `post` (commit statuses), `main` tested as it is; the queue in rounds with `--wait` (`night_queue`, `wait_for_fix`, `push`), the logs stamped, CPU and memory measured (`test`), the fix's commits (`fixes`), the night's summary.

## Testing note

From a session's tree (`~/Projects/TheTaaJ-ci`, say), logged in with `gh`:

| Do | You should see |
|---|---|
| `NIGHT_TEST=true python3 tools/night-test.py --all` | A report headed `CI`: `main`, then each branch `main` does not contain - passed, or `does not merge`; nothing posted |
| `TAAJ_MACHINE=TheJat-WSL python3 tools/night-test.py --all` | `make test64` on each; on GitHub, each branch's newest commit shows `TheJat-WSL test64` - success or failure; a second run right after tests nothing |

And for part two, the wait for a fix:

From a session's tree, with a queue of stacked branches:

| Do | You should see |
|---|---|
| `NIGHT_TEST="sh -c 'exit 1' -" python3 tools/night-test.py --wait 1` | The first branch FAILED, `waiting for a fix (up to 1 min)`, then `no fix pushed for 1 min`, and the branches after it `not run` |
| `python3 tools/night-test.py --wait 60`, a branch failing; push a fix to it | Within a minute `BRANCH on ... (try 2)`; when it passes, the next branch starts |

Automated: not part of `make test64` - it runs `make test64` itself.
