# Coverage of the host tests, and a floor that only rises

*Journal entry #NNN · a test (J96). Written on TheJat-WSL/ci. The kernel's crates have hundreds of host tests, but nobody knew how much of the code they run. Now `make coverage` measures it per crate - the kernel library, `abi`, and the fifteen crates of drivers, services and libraries - and fails if a crate falls below its floor in `tools/coverage-floor`. It is a part of `make test64`, about 20 seconds.*

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

## Testing note

From a session's tree, with rustup (the first run installs `llvm-tools` for Rust 1.100.0):

| Do | You should see |
|---|---|
| `make coverage` | A table of 17 crates - lines, functions, lines counted, floor - and `coverage: every crate at or above its floor`, in about 20 seconds |
| Set a crate's floor in `tools/coverage-floor` above its number, `python3 tools/coverage.py CRATE` | `BELOW ITS FLOOR` and `coverage: FAILED`, exit 1; put the floor back |

Automated: a part of `make test64` (`coverage`).
