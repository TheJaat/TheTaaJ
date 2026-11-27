# Programs in /bin

*Journal entry #NNN · J55 of Era 14: the programs on the run disks and the test disk moved from the root into `/bin`. Written on TheJat-VM/files, on the branch `j55-bin` in TheTaaJ and TheKernel.*

## Why

Since entry 100 libc's `exec` has looked for a command in `/` and then in `/bin`, and since entry 101 the shell does too. But every disk still had its programs in the root. `ls /` on the run disk showed about forty programs mixed with the user's own files, and `rm notes` sat one typo away from `rm` itself.

Every Unix-like system keeps programs apart from the user's files: `/bin` and `/usr/bin` for the system's, the home directory for the user's. Ported programs and scripts expect it too (`#!/bin/sh`, `/bin/ls`). Entry 101 noted the other half of J55 as missing: the disk recipes and the tests still put programs in the root.

## What it is

- **`make run-disk` and `make run-aarch64-disk`** put every program in `/bin`: the Rust ones (`ls`, `cat`, `edit` and the rest), the C ones (`chello`, `sh`, `clib` ...), C++, `make`, the test programs (`handles`, `devtest` ...). So does `make cput C=name`.
- **`tools/put-tcc.sh`** puts the compiler at `/bin/tcc`. Its headers and libraries stay in `/include` and `/lib`, where tcc looks for them.
- **The boot tests' disk** (`make-test-disk`, used by `boot64-test` and 22 other tests) has its programs in `/bin`. Its root now holds only the test's own files: `welcome`, `long`, `broken` (a damaged program, kept in the root on purpose), and the `docs`, `tools`, `dev`, `tmp` and `bin` directories.
- **An older disk is fixed as it is filled.** The run disk is kept from one run to the next, so it has the old copies in the root. libc and the shell look in `/` first, so a stale `/ls` would be found before a fresh `/bin/ls`. The Makefile's new `put-program` removes the root's copy as it puts the new one in `/bin`. After one `make run64` an old disk has the new layout, and the user's own files are untouched.

## How it works

```
define put-program        (DISK NAME FILE)
    tfstool mkdir DISK bin          (there already: fine)
    tfstool rm DISK NAME            (an older disk's copy, in the root)
    tfstool rm DISK bin/NAME
    tfstool put DISK bin/NAME FILE
```

All 33 `rm` and `put` pairs of the two run disks (x86_64 and AArch64) became one `$(call put-program,...)` each, the loops over `DISK_PROGRAMS`, `C_PROGRAMS` and `CXX_PROGRAMS` included.

Nothing in TheTaaJ itself changed. `ls` typed at the prompt is found in `/bin` by the shell's `program_path` (entry 101). `posix_spawnp("ls")` finds it by libc's search (entry 100). A script's `tcc -c x.c` finds `/bin/tcc` the same way. The search order stays root first, then `/bin`, so a program put in the root by hand (`tfstool put disk myprog`) still runs by its name.

## Design decisions

- **Root first, then `/bin`, as before.** Unix's default `PATH` would put `/bin` first. Keeping the root first means nothing that worked stops working: programs a user puts in the root, and the special-purpose test disks (`handles-test.sh`, `times-test.sh` and the rest), which still put their few programs in the root. It costs one failed lookup per command, a request to the file service. Putting `/bin` first can come with a real `PATH` in the shell's environment.
- **The special test disks stay as they are.** Each one is built by its own script, with only the programs that test needs, and checks them by name. The search finds them either way, so moving them would only be churn. The disks people use (the run disks) and the one most tests share are what change.
- **Fix the disk, don't ask the user to.** A `make clean-disk` or "delete `build/taaj-disk.img`" step would lose the user's own files. `put-program` cleans up only the names it is about to put.

## Bugs met

Three places in `boot64-test` used a program as test data, by its root name:

- `cwords -n 25 cwords long > cw1` counts the words in the `cwords` program itself. It is now `/bin/cwords`.
- `cat ls ls > big`, `cat ls ls | cat | cat > big3` and the background job `cat ls ls | cat > big4 &` push the `ls` program through pipes as 50 KB of binary data. They now name `bin/ls`. The host-side check that makes the file to compare (`get ls; get ls`) also reads `bin/ls`.
- The root listing's expected count: 39 files and 4 directories before, now 4 files and 5 directories.

## How I checked it

- `boot64-test`: every command typed in it still finds its program. The root's listing has `bin/` and 4 files. The 50 KB pipelines and `cwords` read `/bin/ls` and `/bin/cwords`, and the host compares the results with the same files taken off the disk.
- `make test64`, with all 23 tests that use the shared test disk, the tcc tests (`tcc` now in `/bin`) and the AArch64 ones: see the testing note's last line.
- **Not tested automatically**: an existing run disk being fixed up. I checked it by hand: `put-program` on a disk with `/ls` in the root leaves only `/bin/ls`.

## What is still missing

- **No `PATH` in the environment.** The search order is libc's built-in default (`/`, then `/bin`). A shell `PATH` variable, with `/bin` first, would let a user choose.
- **The special test disks** still put their programs in the root (see Design decisions).
- **No `/usr`, no `/sbin`.** Services (`fs`, `dev`, the drivers) are built into the kernel image, not on the disk, so there is nothing to put in `/sbin` yet.

## What I learned

- **A layout change is mostly the tests.** The OS needed no change at all: entries 100 and 101 had already made both searches look in `/bin`. What changed were the recipes, and three tests that had used a program as a convenient big file. A program used as data is a hidden dependency on where it lives.
- **A disk that persists needs a migration.** A fresh disk is easy. A kept one has the old layout, and the search order made the old copy win. The fix had to be part of filling the disk, not a separate step someone might skip.
- **Make's `define` and `$(call ...)` are Make's functions.** `put-program` takes three arguments, `$(1)` to `$(3)`, and expands into a shell line. One definition replaced 33 copies of the same two commands. That is where the next change of layout will happen, in one place.

## Testing note

Start TheTaaJ:

```
make run64
```

| Type | You should see |
|---|---|
| `ls /` | `bin/` among the directories, and no programs |
| `ls /bin` | `cat`, `ls`, `edit`, `tcc`, `chello`, `sh` and the rest |
| `ls` and `chello` | they run as before: found in `/bin` |

On a disk from before this change, the first `make run64` moves the programs. Your own files in the root stay.

The same on AArch64 with `make run-aarch64`.

Automated: `make boot64-test`, and `make test64`.
