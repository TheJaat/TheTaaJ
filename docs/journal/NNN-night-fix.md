# Night tests wait for the fix: fix, test again, then the next branch

*Journal entry #NNN · a tool change. Written on TheJat-WSL/ci. The user's rule for the night: the session is not only the tester. When a branch fails, it fixes it, tests it again, and goes on to the next branch only after that; and so on for every branch, unless the user says otherwise. "Make use of time": don't spend the machine on tests that will have to be run again. The runner used to end a failed branch's stack and go on with the queue; a failure cost the night. Now, with `--wait`, it waits for the fix.*

## Why

The night queue is stacked in merge order (entry NNN): each branch is tested on the one before's result. When one fails, the ones after it can only be tested on a failed base, and every such test has to be run again after the fix: wasted. Waiting for the fix wastes nothing. Most failures are small - an expected text, a test's limit, a merge with `main` - and fixed in minutes. And while the runner waits, the test64 lock is free for the fix's own checks.

## What it is

```text
python3 tools/night-test.py --wait 60
```

1. The queue, one branch after another, as before.
2. **A branch fails**: the runner waits. Every minute it looks at the server for a new commit on the branch (TheTaaJ's or TheKernel's).
3. **The session, meanwhile**, reads the failure in the log (`~/.cache/taaj-night/logs/NIGHT/BRANCH-TRY.log`) and fixes it in its own tree, not the night tree. It checks the fix with `make test64-again` (only the failed parts, with the lock the runner left free), documents it (rule 10: in the item's entry, or its own), commits and pushes.
4. **The fix arrives**: the runner tests the branch again, the whole `make test64`, on the same base. Passed: its merge is pushed, and the next branch starts on its result. Failed again: back to 2.
5. **No fix for `--wait` minutes**: it stops there - the branches after it are `not run` - and the report says so. Without `--wait`, the first failure ends the stack, as before.

Every try is a row of the report: what failed, and the try that passed after the fix.

## How I checked it

- A queue of three stacked branches, a stand-in test failing `k187-miri`, `--wait 1`: `j92-clean-build` passed; `k187-miri` FAILED; `waiting for a fix (up to 1 min)`; `no fix pushed for 1 min`; `night-test` not run.
- The runner with the server stubbed, `k187-miri`'s fix arriving during the wait: the tests ran in exactly the user's order - `j92-clean-build` pass, `k187-miri` FAIL, `k187-miri` (try 2) pass, `night-test` pass. Nothing was tested on a failed base.
- A first try said `passed` for the branch that should fail: the stand-in command got the make flags as arguments, git refused them, and `!` turned the error into a pass. A stand-in must take its own arguments (`sh -c '...' -`).
- An earlier version of this change tested the branches after a failed one on its result (marked "a failed base") while waiting. The user's rule is better: those tests would all be run again after the fix.

## What is still missing

- The session has to be there to fix. Unattended, the runner waits `--wait` minutes and stops; the failure waits for the morning.
- A failure in a branch another lane owns (`--all`) is posted for that lane, not fixed here.

## Testing note

From a session's tree, with a queue of stacked branches:

| Do | You should see |
|---|---|
| `NIGHT_TEST="sh -c 'exit 1' -" python3 tools/night-test.py --wait 1` | The first branch FAILED, `waiting for a fix (up to 1 min)`, then `no fix pushed for 1 min`, and the branches after it `not run` |
| `python3 tools/night-test.py --wait 60`, a branch failing; push a fix to it | Within a minute `BRANCH on ... (try 2)`; when it passes, the next branch starts |

Automated: not part of `make test64` - it runs `make test64`.
