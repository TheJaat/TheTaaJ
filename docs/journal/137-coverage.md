# Coverage of the host tests, and a floor that only rises; make test64 reports every failure, make test64-again runs only those

*Journal entry #137 · a test (J96). Written on TheJat-WSL/ci. The kernel's crates have hundreds of host tests, but nobody knew how much of the code they run. Now `make coverage` measures it per crate - the kernel library, `abi`, and the fifteen crates of drivers, services and libraries - and fails if a crate falls below its floor in `tools/coverage-floor`. It is a part of `make test64`, about 20 seconds.*

## Why

A test suite that grows with the code says nothing about the code that has no tests. J96 asks for the host tests' coverage, "kept in the journal", and "a floor that may only rise": a change that adds code without tests, or loses tests, should show.

## What it is

```text
make coverage          the table; fails if a crate fell below its floor
make coverage-raise    the same, then each floor raised to today's number where it is higher
python3 tools/coverage.py [--raise] [CRATE...]
```

1. **The crates `make kernel64-test` tests**, the kernel library for the host as that does, each built with rustc's own instrumentation (`-C instrument-coverage`) and its `cargo test --tests` run. The build is in `build/coverage`, so the normal builds are not built again after it.
2. **The Rust the kernel is pinned to** (`kernel/kernel64/rust-toolchain.toml`, 1.100.0) for every crate, and its own `llvm-profdata` and `llvm-cov` - rustup's `llvm-tools`, which the script installs the first time. A profile from one LLVM read by another can be wrong or refused.
3. **Only the crate's own code.** Each crate's report has only its `src/`; the crates it uses are counted as themselves. Its tests are left out - `tests.rs` and `tests/` files, and every `#[cfg(test)]` item in the other files, found by its attribute and closed by counting braces (strings, characters and comments left out).
4. **The floor**: lines covered, in whole percent rounded down, per crate. Below it, `make coverage` fails and says `BELOW ITS FLOOR`. `--raise` only raises; lowering is by hand, in a commit that says why.

## What it found

The first numbers - lines llvm-cov can count (lines with code), and functions:

| Crate | Lines | Functions | Lines counted |
|---|---|---|---|
| kernel/kernel64 (the library) | 91.5% | 90.7% | 2628 |
| kernel/abi | 87.1% | 93.0% | 636 |
| input | 92.5% | 95.7% | 400 |
| console | **78.5%** | 93.3% | 107 |
| heap | 99.0% | 100.0% | 101 |
| fileserver | 89.0% | 92.0% | 970 |
| registry | 100.0% | 100.0% | 147 |
| taajfs | 95.6% | 86.9% | 1165 |
| virtio | 92.6% | 92.3% | 163 |
| ahci | 97.8% | 100.0% | 323 |
| nvme | 95.2% | 100.0% | 315 |
| acpi | 100.0% | 100.0% | 97 |
| e1000 | 99.3% | 100.0% | 148 |
| usb | 92.0% | 97.5% | 965 |
| net | 97.1% | 98.9% | 1469 |
| gfx | 100.0% | 100.0% | 183 |
| gui | 95.6% | 95.5% | 710 |

- **The tests' own lines, counted, made every crate look better.** Most crates keep their tests in a `mod tests` at the end of the file; counted, they were half of the lines (the kernel library 6048 lines, not 2628), all of them run, and every crate looked 3 to 12 points better - console 90.3%, not 78.5%.
- **console** is the lowest: the parts of its terminal the tests don't drive. **kernel/abi** and **fileserver** are next.
- **The kernel library is not the kernel.** The host tests run the modules `lib.rs` names - the pure logic (memory, paging, scheduling, parsers) - not the rest of the kernel, which the boot tests check in QEMU. Coverage of those is a different question (Era 25, later).
- Two runs, cold and warm, gave the same numbers to the tenth: the tests are deterministic enough for a whole-percent floor.

## Design decisions

- **No `cargo llvm-cov`**: one more tool to install on every machine, for what three commands do. The script calls rustc's instrumentation and LLVM's tools itself.
- **In `make test64`**: 20 seconds (65 of CPU), cold. Measured only when someone remembers, a floor would only be checked when someone remembers.
- **Lines, not regions or branches**: a floor that people can read and raise by hand.
- **Whole percent, rounded down**: a test whose path depends on thread timing can move a line or two without failing the floor.

## How I checked it

- `make coverage`: the table above, `every crate at or above its floor`. Again from an empty `build/coverage`: the same, 19 s.
- console's floor set to 90 by hand: `BELOW ITS FLOOR`, exit 1.
- The `#[cfg(test)]` ranges found in every file of `net`, `fileserver` and the kernel library: each ends on a `}` at the file's end or around a single item, none runs past it.

## What is still missing

- **The kernel outside its library**, and the user programs: run only in QEMU. Coverage there needs an instrumented kernel that writes its counts out - Era 25, later.
- **libc's host tests** (`make libc-test`, C): not measured here.
- Raising the low ones - console, abi, fileserver - is each crate's lane's work; the floor only keeps them from falling.

## What I learned

- **Leave the tests out before trusting a coverage number.** The first run said 90% to 100% everywhere; half of it was the tests covering themselves.

## Part two: make test64 reports every failure; make test64-again runs only those

*Two questions from the user: when the 19th part fails, are the other parts still run? And after the fix, why test again the ones that already passed? The answers were "not always" and "sometimes rightly, sometimes not". Now one `make test64` always runs every part it can and writes down what failed, and `make test64-again` runs only that.*

### Why

`make test64` had three steps - the shared builds, `smp64-test` alone, every other part in parallel - as three lines of one recipe. A line that failed ended the recipe: if `smp64-test` failed, none of the other parts ran, even under `make -k` (which keeps going inside a step, not to the next line). A night's test of a branch would report one failure and test nothing else - and the fix's check the next night would find the second.

And after a fix, the whole suite ran again (7 minutes on TheJat-WSL), also when the fix was to one part's own test script. Whether the passed parts need running again depends on what the fix touched - the same question as rule 15's for a moved `main`:

| The fix touched | Run again |
|---|---|
| Only the failed part's test - its script, its expected text, a program only it runs | That part: `make test64-again` |
| One crate's host tests, or code not in the kernel image | That part, `coverage` and `unsafe-check` |
| The kernel, libc, `kernel/abi`, the bootloader, the build | All: `make test64` - every part boots the new image, so the old passes say nothing about it |

### What it is

The steps moved into `tools/test64.sh`, which the Makefile runs with its lists:

1. **The shared builds** (`test64-shared`). If they fail, nothing can be tested: it stops, and writes `test64-shared`.
2. **`smp64-test` alone**, as before (entry 117). If it fails, it is written down and the parts still run.
3. **Every other part**, in parallel, now always with `-k`: a failing part never stops the others. The output is kept in `build/test64-parts.log`, and from make's own lines at the parts' level it takes the parts that failed (`*** [Makefile:N: PART] Error`) and those a failed build stopped (`Target 'PART' not remade because of errors` - make names the build, not the part, when a part never ran).
4. **`build/test64-failed`**: those parts, one a line. `test64: FAILED: PART PART - after fixing, make test64-again runs only these`.

`make test64-again` reads the list and runs the shared builds (up to date in seconds after a test-only fix) and then only those parts - `smp64-test` alone first if it is one - and writes the list again: empty when all passed. With nothing listed, it says so and passes. If make's output could not be read, the list says `test64-parts`, and all of them run again: never fewer than failed.

### The rule: the shortcuts never qualify a merge

Rule 2 now says it outright: a branch is merged only after one whole `make test64`, in one run, on exactly what is merged - `main` merged in, and for a stacked branch the ones before it. `make test64-again`, an item's own boot test and the host tests are for the work before it. So every shortcut saves time while an item is developed and fixed, and none takes a test away from what reaches `main`:

| Stage | What runs |
|---|---|
| By day, developing | The item's own test, the host tests, `make coverage`, `make unsafe-check` |
| After a fix | `make test64-again` - or all of `make test64` if the fix touched the kernel, libc, `kernel/abi` or the build |
| Before the merge | All of `make test64` on the merge result (the night test, stacked in merge order) |
| After merges | `night-test.py --all` tests `main` itself (J93): what two changes tested apart break together, and what rule 15's shortcut for a moved `main` let through |

The night queue on TheJat-WSL is stacked in merge order for the same reason: each branch tested on the one before's result is what each morning merge makes, not a combination no test has seen.

### How I checked it

- A scratch Makefile with stand-in parts: `smp64-test` failing, a part failing, a part whose build fails. `make test64` ran all of them and listed `p2 p3 smp64-test`; with two fixed, `test64-again` ran just those three and listed `p2`; with that fixed, ran only `p2`, `test64: all passed`; then "nothing failed". `test64-parts` in the list ran every part, without `smp64-test`.
- In this tree, `coverage` in the list: `make test64-again` built the shared builds - 13 minutes, because main had moved and LLVM's port was rebuilt - then ran `coverage` alone, `test64: all passed`, and emptied the list.

### What is still missing

- Knowing **what a fix touched** is still a person's (or session's) question; the table above is the rule. A record of each part's inputs - the image, the script - would let make skip a part whose inputs did not change, as Bazel does; for us it would mostly help the host-test parts, since a kernel fix changes every boot test's image.
- `test64-shared` still relinks LLVM's tools on every run (entry 117), so even `test64-again` takes a minute or two.

### What I learned

- **`make -k` keeps going within a make, not across a recipe's lines.** Three steps in one recipe are three chances to stop.

## Testing note

From a session's tree, with rustup (the first run installs `llvm-tools` for Rust 1.100.0):

| Do | You should see |
|---|---|
| `make coverage` | A table of 17 crates - lines, functions, lines counted, floor - and `coverage: every crate at or above its floor`, in about 20 seconds |
| Set a crate's floor in `tools/coverage-floor` above its number, `python3 tools/coverage.py CRATE` | `BELOW ITS FLOOR` and `coverage: FAILED`, exit 1; put the floor back |

And for part two, `make test64-again`:

In a session's tree, after a `make test64`:

| Do | You should see |
|---|---|
| `make test64` with a part failing | Every other part still run, then `test64: FAILED: PART - after fixing, make test64-again runs only these`; `cat build/test64-failed` lists it |
| `make test64-again` | `test64-again: PART`, the shared builds, only that part, and `test64: all passed` once it does |
| `make test64-again` again | `test64-again: nothing failed in the last make test64` |

Automated: a part of `make test64` (`coverage`); part two is `make test64` itself.
