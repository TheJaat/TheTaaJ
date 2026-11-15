# The Kernel Goes Home: Three Repositories, Each With Its Own Work

*Journal entry #27 · moving the Rust kernel and its interface out of TheTaaJ and into TheKernel, with their history*

No new feature in this entry. It fixes where the code lives.

TheTaaJ has always been several repositories:

| Repository | What it is for | Where it appears in a checkout |
|---|---|---|
| **TheTaaJ** | The whole system: user space, libraries, tools, tests, this journal — and the build that ties everything together | the top level |
| **TheBootloader** | The BIOS and UEFI loaders | `bootloader/` (a submodule) |
| **TheKernel** | The kernel | `kernel/` (a submodule) |
| **TheToolchain** | The cross compiler for the 32-bit system | `toolchain/` (a submodule) |

When the 64-bit Rust kernel was started (entry #2), it was put in a new directory, `kernel64/`, at the top level of **TheTaaJ** — not in TheKernel. The bootloader work of the same period went into TheBootloader, as it should. The kernel did not, and for 27 commits the most important part of the project grew in the wrong repository, while TheKernel held only the old 32-bit C kernel.

That was a mistake of convenience: a new directory beside everything else needs no thought about submodules. It was noticed when the commits were pushed and the repositories on GitHub did not look the way the project is organised.

---

## What a submodule is, and why it makes this matter

A git **submodule** is a repository inside a repository. The outer one (TheTaaJ) does not store the inner one's files. It stores two things: the inner repository's address (in `.gitmodules`), and **one commit ID** — "at this point in my history, `kernel/` is TheKernel at commit `abc123`".

Two consequences worth knowing:

- **A change inside a submodule is two commits.** One in the inner repository (the change itself), and one in the outer (moving the pointer to the new commit). Until the second is made, the outer repository still means the old kernel.
- **The pointer can dangle.** If the outer repository is pushed while the inner commit it points at exists only on my machine, anyone cloning gets an error: the commit cannot be found. This happened, briefly — TheTaaJ was pushed pointing at bootloader and kernel commits that had not been. Pushing those two repositories fixed it. The rule: **push the inner repositories first, then the outer.**

## The layout now

**TheKernel** — both kernels, side by side:

```
TheKernel/
├── kernel64/     the 64-bit Rust kernel
├── abi/          its interface: system call numbers, errors, rights, message limits
├── arch/ driver/ ds/ libc/ system/ terminal/ headers/ …    the 32-bit C kernel, where it always was
└── Makefile, kernel.ld, kmain.cpp, …                        (its build, untouched)
```

**TheTaaJ**:

```
TheTaaJ/
├── bootloader/   → TheBootloader
├── kernel/       → TheKernel
├── toolchain/    → TheToolchain
├── user/         init, shell, fs, blk, display, kbd, and the programs
├── fileserver/ taajfs/ console/ input/ heap/ virtio/        libraries
├── tools/        tfstool
├── servers/ modules/ librt/ ramdisk-src/ shared/            the 32-bit user space
├── docs/journal/
└── Makefile
```

### Why `abi` went with the kernel

`abi` is the one crate both the kernel and every user program depend on: it *is* the boundary between them. It belongs to whichever side defines that boundary, and that is the kernel. User space now builds against `kernel/abi` — a program is compiled against the interface of the kernel it will run on, which is the honest direction for that dependency to point.

### Why the old kernel was not moved into a directory of its own

It would be tidier as `TheKernel/x86-32/`. But its build reaches out with relative paths (`../toolchain/i686`, `../../build`), and the 32-bit user space is built against it. Moving it means editing a build I am not otherwise touching, for neatness alone. It stays where it is; the two new directories sit beside it.

## Moving with history: `git subtree`

Copying the files across would have worked and thrown away their story — 27 commits of *why* each line of the kernel is the way it is. Instead:

```
# in TheTaaJ: make a branch holding only kernel64/'s history,
# as if that directory had always been a repository of its own
git subtree split --prefix=kernel64 -b split-kernel64
git subtree split --prefix=abi      -b split-abi

# in TheKernel: bring that history in, placed under a directory
git subtree add --prefix=kernel64 /path/to/TheTaaJ split-kernel64
git subtree add --prefix=abi      /path/to/TheTaaJ split-abi
```

`split` walks the outer repository's history and, for every commit that touched the directory, makes a new commit containing only that directory's part of the change — same message, same author, same date. `add` merges that line of history into TheKernel under the given prefix. `git log kernel64/` in TheKernel now shows the kernel's history back to its first commit.

One thing to understand about the result: those are **new commits with new IDs**. A commit in TheTaaJ that changed both the kernel and a user program appears in TheKernel with only its kernel half. The hashes in this journal's index still refer to TheTaaJ's history, which is unchanged and still contains the kernel's files up to this entry.

Then, in TheTaaJ, `kernel64/` and `abi/` were removed, and the paths that crossed the new boundary were changed — there were only eight:

| File | Change |
|---|---|
| `kernel/kernel64/Cargo.toml` | `heap`, `virtio`, `console`: `../X` → `../../X` |
| `kernel/kernel64/src/programs.rs` | embedded test programs: `../user/target` → `../../user/target` |
| `user/Cargo.toml`, `fileserver/Cargo.toml` | `abi`: `../abi` → `../kernel/abi` |
| `Makefile` | `KERNEL64_DIR = kernel/kernel64`; `cd kernel/abi && cargo test` |

That the list is so short is owed to an earlier habit: the Makefile named the kernel's directory once, in a variable, and every script finds its own location instead of assuming one.

## What is still not right

- **TheKernel does not build on its own.** The Rust kernel uses three small libraries that user programs use too — `heap`, `virtio`, `console` — and they are still in TheTaaJ, so the kernel reaches up into its parent for them. It also embeds a few test programs built from `user/`. Cloned alone, TheKernel's `kernel64` will not compile; inside a TheTaaJ checkout it does. The clean fixes are known (move the three libraries into TheKernel; load the test programs from disk instead of embedding them) and are for a later step.
- **Older journal entries say `kernel64/…`** where the path is now `kernel/kernel64/…`. They are left as written — they describe the tree as it was — and the journal's index says so.
- **Every kernel change is now two commits**, as explained above.

## How I checked it

Nothing about the kernel's code changed, so the check is that everything that passed before passes from the new places:

- `make test64` — host tests of every crate (the kernel's and `abi`'s now run from `kernel/`), the BIOS boot test, the UEFI boot test, and the seven fault boots.
- `make tfscheck` — the two file system tools against each other.
- `make all` — the 32-bit system: bootloader, the C kernel, ISO and disks, to confirm that two new directories in TheKernel did not disturb the old build.

All three passed - `make test64` on its second run. The first run failed in the UEFI boot test: the test's own log files in `build/` were gone partway through (`build/serial64-uefi.log: No such file or directory`), after the BIOS boot had passed. Something emptied `build/` while it ran; I do not know what. Run again by itself, the whole suite passed. I count that as a disturbed run, not as evidence about the move - but it is unexplained, and it is written here so that it is not forgotten if it happens again.

**Not checked:** a fresh `git clone --recurse-submodules` of TheTaaJ from GitHub followed by a build. The test above ran in my existing checkout.

## What I learned

- **Where code lives is part of the design.** "The kernel is its own repository" is a statement about what depends on what; putting the kernel in the parent quietly erased it.
- **A submodule is a pointer to a commit**, and a pointer can point at something only I have. Push inner, then outer.
- **History can move.** `git subtree split` and `add` carry a directory's past into another repository; the price is new commit IDs.
- **Names in one place pay off late.** A directory named once in a Makefile variable turned a move across repositories into a one-line edit.
- **Convenience decisions compound.** The first commit in the wrong place made the next twenty-six natural.

---

## Testing note

Nothing you type in the OS has changed. What to check is that everything builds and runs from the new layout.

| Do | You should see |
|---|---|
| `ls` in the TheTaaJ directory | No `kernel64` and no `abi` at the top level |
| `ls kernel` | `kernel64` and `abi`, beside the C kernel's `arch`, `driver`, `kmain.cpp`, … |
| `git -C kernel log --oneline -- kernel64 \| tail -1` | The kernel's first commit: `add the 64-bit Rust kernel, booted by the same bootloader` |
| `make test64` | Ends with `test64: all passed` |
| `make tfscheck` | Ends with `crosscheck passed` |
| `make all` | The 32-bit system builds as before, with no errors |
| `make run64` | Boots as before; `ls`, `mkdir docs`, `ps` behave as in entry #26 |

On GitHub: TheKernel should show `kernel64/` and `abi/` at its top level, and in TheTaaJ the `kernel` entry should link to TheKernel at its newest commit.

When you change the kernel from now on:

```
cd kernel
git add … && git commit      # 1: the change, in TheKernel
git push
cd ..
git add kernel && git commit # 2: TheTaaJ now means the new kernel
git push
```
