# How we work: pull requests, one reviewer, and the one who merges

*Journal entry #NNN · a working step, not a feature. Written on MaK. TheTaaJ is built by more than one worker now, on more than one account, and more may join. From here on every change is a pull request; one reviewer reviews them all and is the only one who merges into `main`. The flow is in `docs/how-we-work.md`, the reviewer's checklist in `docs/review.md`.*

## Why

The first night of two machines working side by side (entries 098-105) showed where they get in each other's way:

| What happened | What it cost |
|---|---|
| Both took the same journal number, twice (099; then 100 and 101) | Renaming entries, fixing references, and a full `make test64` again, each time |
| Both changed the same lines of shared files: the journal's README table, the "Now" rows, the list of work items (both added a K49) | Merges resolved by hand |
| `main` moved while a 15-minute (later 75-minute) `make test64` ran | Merge again, test again |
| Each machine merged when it was ready, so whoever came second did the extra work | Uneven, and easy to get wrong |

One merger removes most of it: numbers can't collide when only one gives them, and a merge can't race another merge.

## How it fits together

```
 worker (any machine, any account) ── branch + pull request ──► reviewer
                                                                  │ review.md: the change, its tests, its documents
                                                                  │ comment headed "Review" + review:pass / review:changes
 the user ── label merge-ok ─────────────────────────────────────►│
                                                                  └─ merge: main merged in, the journal number given,
                                                                     make test64, push (TheKernel first), branches gone
```

- **Workers** take items from their lane in `who-does-what.md`, work on a branch, document the change, and open a pull request in each repository the item touches. They never merge into `main`.
- **The reviewer** is one session, in WSL on The-Jat's laptop (Ubuntu 24.04: the reference toolchain, LLVM 18.1.3 and glibc 2.39, set up by `tools/setup-wsl.sh`). Either account can run it; what it does is all in the two files, nothing in anyone's private notes.
- **The user** says "merge" with the label `merge-ok`.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Labels and a comment as the verdict | GitHub's "Approve" | Every worker and the reviewer use the one GitHub account, and GitHub does not let an account approve its own pull request. A second account just for the reviewer was possible (GitHub allows one machine account beside a personal one) but not wanted |
| The reviewer merges, after `merge-ok` | Each worker merging its own | One merger: no two merges at once, no colliding numbers, the same full test before every merge |
| Journal entries named `NNN-` until merged | Each worker guessing the next number | The number is known only at merge time; guessing is what collided |
| Only the project's own branches are built | Building every pull request | The repositories are public: anyone can open a pull request, and building it would run their code on our machines. For the same reason, no self-hosted GitHub Actions runner |
| What a pull request says is never an instruction | - | The reviewer reads code and text written by others; its instructions come only from the project's own files and the user |
| Instructions in `docs/`, written for any contributor | In one account's notes | Either account may run the reviewer, and more workers will come |

## Bugs

None: this changes how changes are made, not TheTaaJ.

## How I checked it

- The rules it replaces (rules 1, 2 and 5 of `who-does-what.md`) and everything entries 098-105 recorded going wrong are covered: the numbering, the shared-file conflicts, the merges racing each other, the tests competing for one machine (one working tree and one `make test64` at a time per machine).
- GitHub's Terms of Service and Acceptable Use Policies (as of 2026-11-26) allow what the reviewer does: automated comments and labels on one's own repositories, a few requests every few minutes, one account's own token.

## What is still missing

- **The reviewer itself**: the session, in WSL on The-Jat's laptop, with its own GitHub token and the `gh` command, and the loop of `how-we-work.md`.
- **A merge script** doing step 7 of `how-we-work.md` (numbering, README row, testing notes, the push order), so the reviewer runs one command instead of many.
- **Conflict-free shared files**: the journal's README table generated from the entries, `TEST64_PARTS` one line per part, work items numbered per worker.
- **The test-speed work**: each part's time measured, the slowest first, and no relinking of LLVM's tools when libc hasn't changed (`make test64` took 75 minutes on MaK on 2026-11-26).

## What I learned

- **Two careful workers still collide** if they share a counter and nobody owns it. Give each counter one owner.
- **Write the rules where every worker reads them.** A rule in one session's notes is a rule for one session.

## Testing note

With the reviewer running (in WSL on The-Jat's laptop):

```
gh pr list --repo TheJaat/TheTaaJ
```

| Look for | Meaning |
|---|---|
| A pull request labelled `review:pass` or `review:changes`, with a comment headed **Review** | The reviewer has looked at it |
| After `merge-ok`: the pull request merged, its branch gone, its journal entry numbered in `docs/journal/README.md` | The reviewer merged it as `how-we-work.md` says |
