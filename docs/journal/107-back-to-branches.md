# Back to branches: no pull requests, no reviewer

*Journal entry #107 · a working step, not a feature. Entry 106 made every change a pull request, reviewed and merged by one reviewer. After one pull request under it, the user decided it was more process than two machines need. The rules go back to what they were before 106: a branch for everything, full testing, the user asked, and the one who did the work merges it.*

## Why

Entry 106's flow cost more than what it saved:

| Part of 106 | Its cost |
|---|---|
| A reviewer checking for pull requests every few minutes | A session that wakes up all day, mostly to find nothing |
| One machine that merges everything | Everyone waits on it, and on 2026-11-26 it had no built tree, so it couldn't run the whole `make test64` it was meant to run before every merge |
| Labels, review comments, the merge steps | More steps per change than the change itself, for a small fix |

What 106 was meant to fix - two machines taking the same journal number, and `main` moving during a long `make test64` - is handled well enough by the rules from before: whoever merges first takes the number, and the other renumbers.

## How it fits together

`docs/who-does-what.md`, "Rules for both machines", rules 1, 2 and 5 are back to their wording before entry 106:

1. A separate branch for everything, with the same name in every repository it touches.
2. Nothing merged into `main` without full testing (`main` merged in, `make test64`, the step's own tests and its testing note) and the user's yes; then TheKernel and TheBootloader first, TheTaaJ last.
5. Journal numbers taken at merge time, by whoever merges first.

`docs/how-we-work.md` and `docs/review.md` are removed.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| The one who did the work tests and merges it | One reviewer merges everything | Two machines don't need a gatekeeper; the user's yes before each merge stays |
| No pull requests | A pull request for every change | A branch already keeps a change apart; the journal entry is its record |
| Rule 3 keeps 106's wording: nothing that names the tools used | 105's "no Claude" | The same rule, said more generally |

## Bugs

None: this changes how changes are made, not TheTaaJ.

## How I checked it

- `git diff` of `docs/who-does-what.md` against entry 106's parent (8117892^): only rule 3's wording and the "Now" rows differ.
- Nothing in `docs/`, `README.md` or `tools/` points to `how-we-work.md` or `review.md` any more, except entry 106 itself, which is history.
- Only documents change, so `make test64` was not run.

## What is still missing

- The labels made for entry 106 (`review:pass`, `review:changes`, `merge-ok`, `hold`) are still in the three repositories, unused. They can be deleted.
- The testing note of entry 106 is still in `docs/testing-notes.md`, since the file is collected from every entry; it no longer applies.

## What I learned

- **Fix the problem with the smallest rule that fixes it.** A collision of journal numbers needed "whoever merges first takes it", not a reviewer.

## Testing note

From the root of TheTaaJ:

```
ls docs/how-we-work.md docs/review.md
grep -c "pull request\|reviewer" docs/who-does-what.md
```

| Look for | Meaning |
|---|---|
| `No such file or directory`, twice | The pull-request flow's files are gone |
| `0` | `who-does-what.md`'s rules no longer mention pull requests or a reviewer |
