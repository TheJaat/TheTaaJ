# Our own CI: every branch of ours tested, the result beside it on GitHub

*Journal entry #NNN · a tool (J93). Written on TheJat-WSL/ci. Fourteen sessions push branches, and each tests its own on its own machine - or, by day, only quickly, leaving the full test for the night. Nobody tested `main` after a merge, or told a session its branch no longer merged. Now `python3 tools/night-test.py --all` tests `main` and every branch of ours not merged yet, and posts each result as a commit status, where the branch's session sees it.*

## Why

Two changes tested apart can still break together (rule 15): only a test of `main` after the merges finds it. And a branch left a few days behind `main` can stop merging without its session knowing. J93 asks for "a machine of ours that builds every pushed branch, runs the host tests and `make test64`, and posts the result where the branches are; no outside service gets the code to run", and never anyone else's pull request.

## What it is

The night runner (entry NNN, `tools/night-test.py`), with its branches found rather than queued:

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

## Files

- `tools/night-test.py`: `--all` (`everything`, `heads`), the record of what was tested, `post` (commit statuses), `main` tested as it is.
