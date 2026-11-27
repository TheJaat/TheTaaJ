# Other sessions' finished branches in the night queue

*Journal entry #NNN · a tool change and a change to how we work. Written on TheJat-WSL/ci. The user's choice for MaK-WSL's work (2026-11-27): branches its sessions are still working on stay with them; finished ones may be handed to TheJat-WSL's night queue, tested with the whole `make test64` and merged there under the standing yes (rule 2). For that, the night runner learned whose item each queued branch is.*

## What it is

A queue line may end in `session=MACHINE/LANE`:

```text
k130-any-size session=MaK-WSL/boot
k23-boot-time after k130-any-size session=MaK-WSL/boot
```

- **The README row names that session**: `TITLE (MaK-WSL/boot, merged after its night test)`, not the runner's own (`TAAJ_SESSION`).
- **A failure is that session's to fix.** For the runner's own items, a failed test waits for a fix (`--wait`); for another session's, the report says `failed - for MaK-WSL/boot to fix; not waited for`, the branches stacked on it are not run, and the queue goes on with the next one that is not. The fixer of a lane's code is that lane (rule 15's last line); the night's runner fixes only its own, and bookkeeping (`merge-main.py` settles that before the test anyway).

`docs/start-here.md` says how a session hands a finished branch over: `main` merged in with `merge-main.py`, its own checks, then the branch's name to the user.

Before this, all of MaK-WSL's open branches were tried against today's `main` (entries 131 to 140) in a scratch tree: all merge in cleanly, or are settled by `merge-main.py`; the TheKernel side of every one with a TheKernel branch merges cleanly too.

## How I checked it

- The queue's lines parsed: `session=` read on a plain line and on a stacked one, with a comment after it; `theirs()` true for MaK-WSL's, false for a line without one or with the runner's own.
- A sandbox rehearsal with two of MaK-WSL's real branches, `k23-boot-time` given a stand-in test that fails and `e8-users-guide` one that passes: `k23-boot-time` - `FAILED`, then `failed - for MaK-WSL/boot to fix; not waited for`, at once; `e8-users-guide` - `passed`, `merged`, its README row `Using TheTaaJ: a page for the people who run it (MaK-WSL/docs, merged after its night test)`.

## Testing note

| Do | You should see |
|---|---|
| A queue line `BRANCH session=MaK-WSL/LANE`, `sh tools/merge-rehearsal.sh` | `BRANCH ... merged`; in the sandbox's main, the README row ends `(MaK-WSL/LANE, merged after its night test)` |

Automated: not part of `make test64`.
