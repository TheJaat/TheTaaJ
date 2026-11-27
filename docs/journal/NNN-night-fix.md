# Night tests wait for the fix: a failure is fixed the same night

*Journal entry #NNN · a tool change. Written on TheJat-WSL/ci. The user's rule for the night: the session is not only the tester - when a branch fails, it fixes it, commits, pushes, and goes on with the rest. The runner used to stop a failed branch's whole stack and end with the queue; a failure cost the night. Now it keeps testing, and with `--wait` it waits for the fix and tests again.*

## Why

The night queue is stacked in merge order (entry NNN), so one failed branch held back every branch after it: they were "not run", and a fix waited for the next night. Most failures are small - an expected text, a test's limit, a merge with `main` - and fixed in minutes.

## What it is

```text
python3 tools/night-test.py --wait 60
```

1. **A failed branch does not stop its stack.** The branches after it are tested on its result all the same, marked `a failed base`: their own failures show the same night. Nothing on a failed base is pushed - its merge is not fit to be merged.
2. **Rounds.** After the queue, it looks at the server every two minutes. A failed branch with a new commit (TheTaaJ's or TheKernel's) is tested again, and every branch stacked after it, on the new result. It ends when every branch passed on a passed base, or when no new commit came for `--wait` minutes (none: it ends after the queue, as before).
3. **The session, meanwhile**, reads the failure in the log (`~/.cache/taaj-night/logs/NIGHT/BRANCH-ROUND.log`), fixes it in its own tree - not the night tree - and runs its quick checks and `make test64-again`, which wait for the test64 lock between two of the runner's branches. Then it commits and pushes the fix to the branch, and the runner takes it from there. The fix is documented like any other (rule 10): in the item's entry, or its own.

Every round is a row of the report, so the morning sees what failed, what fixed it, and the test that passed after.

## How I checked it

- A queue of three stacked branches with a stand-in test failing `k187-miri` and everything containing it: `k187-miri` FAILED, `night-test` tested on it and FAILED, marked `a failed base`; `--wait 1`: `no fix pushed for 1 min: stopped`.
- The runner with the server stubbed - `k187-miri` broken in the first two looks, fixed in the third: it waited (two sleeps), then tested `k187-miri` again (passed) and `night-test` on its new result (passed), in round 3; `j92-clean-build`, which had passed, was not tested again.
- A first try said `passed` for the branch that should fail: the stand-in command got the make flags as arguments, git refused them, and `!` turned the error into a pass. A stand-in must take its own arguments (`sh -c '...' -`); `make test64` itself ignores none of them.

## What is still missing

- The session has to be there, watching. Unattended, a failure still waits for the morning.
- A failure in a branch another lane owns (`--all`) is that lane's: posted, not fixed here.

## Testing note

From a session's tree, with a queue of stacked branches:

| Do | You should see |
|---|---|
| `NIGHT_TEST="sh -c 'exit 1' -" python3 tools/night-test.py` | The first branch FAILED, the ones after it tested all the same, `a failed base`, nothing pushed |
| `python3 tools/night-test.py --wait 60`, a branch failing; push a fix to it | `waiting for a fix to BRANCH`, then within two minutes `BRANCH on ... (round N)`, and the branches after it again |

Automated: not part of `make test64` - it runs `make test64`.
