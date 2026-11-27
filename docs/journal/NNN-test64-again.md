# make test64 reports every failure; make test64-again runs only those

*Journal entry #NNN · a test change. Written on TheJat-WSL/ci. Two questions from the user: when the 19th part fails, are the other parts still run? And after the fix, why test again the ones that already passed? The answers were "not always" and "sometimes rightly, sometimes not". Now one `make test64` always runs every part it can and writes down what failed, and `make test64-again` runs only that.*

## Why

`make test64` had three steps - the shared builds, `smp64-test` alone, every other part in parallel - as three lines of one recipe. A line that failed ended the recipe: if `smp64-test` failed, none of the other parts ran, even under `make -k` (which keeps going inside a step, not to the next line). A night's test of a branch would report one failure and test nothing else - and the fix's check the next night would find the second.

And after a fix, the whole suite ran again (7 minutes on TheJat-WSL), also when the fix was to one part's own test script. Whether the passed parts need running again depends on what the fix touched - the same question as rule 15's for a moved `main`:

| The fix touched | Run again |
|---|---|
| Only the failed part's test - its script, its expected text, a program only it runs | That part: `make test64-again` |
| One crate's host tests, or code not in the kernel image | That part, `coverage` and `unsafe-check` |
| The kernel, libc, `kernel/abi`, the bootloader, the build | All: `make test64` - every part boots the new image, so the old passes say nothing about it |

## What it is

The steps moved into `tools/test64.sh`, which the Makefile runs with its lists:

1. **The shared builds** (`test64-shared`). If they fail, nothing can be tested: it stops, and writes `test64-shared`.
2. **`smp64-test` alone**, as before (entry 117). If it fails, it is written down and the parts still run.
3. **Every other part**, in parallel, now always with `-k`: a failing part never stops the others. The output is kept in `build/test64-parts.log`, and from make's own lines at the parts' level it takes the parts that failed (`*** [Makefile:N: PART] Error`) and those a failed build stopped (`Target 'PART' not remade because of errors` - make names the build, not the part, when a part never ran).
4. **`build/test64-failed`**: those parts, one a line. `test64: FAILED: PART PART - after fixing, make test64-again runs only these`.

`make test64-again` reads the list and runs the shared builds (up to date in seconds after a test-only fix) and then only those parts - `smp64-test` alone first if it is one - and writes the list again: empty when all passed. With nothing listed, it says so and passes. If make's output could not be read, the list says `test64-parts`, and all of them run again: never fewer than failed.

## The rule: the shortcuts never qualify a merge

Rule 2 now says it outright: a branch is merged only after one whole `make test64`, in one run, on exactly what is merged - `main` merged in, and for a stacked branch the ones before it. `make test64-again`, an item's own boot test and the host tests are for the work before it. So every shortcut saves time while an item is developed and fixed, and none takes a test away from what reaches `main`:

| Stage | What runs |
|---|---|
| By day, developing | The item's own test, the host tests, `make coverage`, `make unsafe-check` |
| After a fix | `make test64-again` - or all of `make test64` if the fix touched the kernel, libc, `kernel/abi` or the build |
| Before the merge | All of `make test64` on the merge result (the night test, stacked in merge order) |
| After merges | `night-test.py --all` tests `main` itself (J93): what two changes tested apart break together, and what rule 15's shortcut for a moved `main` let through |

The night queue on TheJat-WSL is stacked in merge order for the same reason: each branch tested on the one before's result is what each morning merge makes, not a combination no test has seen.

## How I checked it

- A scratch Makefile with stand-in parts: `smp64-test` failing, a part failing, a part whose build fails. `make test64` ran all of them and listed `p2 p3 smp64-test`; with two fixed, `test64-again` ran just those three and listed `p2`; with that fixed, ran only `p2`, `test64: all passed`; then "nothing failed". `test64-parts` in the list ran every part, without `smp64-test`.
- In this tree, `coverage` in the list: `make test64-again` built the shared builds - 13 minutes, because main had moved and LLVM's port was rebuilt - then ran `coverage` alone, `test64: all passed`, and emptied the list.

## What is still missing

- Knowing **what a fix touched** is still a person's (or session's) question; the table above is the rule. A record of each part's inputs - the image, the script - would let make skip a part whose inputs did not change, as Bazel does; for us it would mostly help the host-test parts, since a kernel fix changes every boot test's image.
- `test64-shared` still relinks LLVM's tools on every run (entry 117), so even `test64-again` takes a minute or two.

## What I learned

- **`make -k` keeps going within a make, not across a recipe's lines.** Three steps in one recipe are three chances to stop.

## Testing note

In a session's tree, after a `make test64`:

| Do | You should see |
|---|---|
| `make test64` with a part failing | Every other part still run, then `test64: FAILED: PART - after fixing, make test64-again runs only these`; `cat build/test64-failed` lists it |
| `make test64-again` | `test64-again: PART`, the shared builds, only that part, and `test64: all passed` once it does |
| `make test64-again` again | `test64-again: nothing failed in the last make test64` |

Automated: this is `make test64` itself.
