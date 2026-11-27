# make test64 no longer rebuilds what has not changed: 51 s to 17 s

*Journal entry #NNN · a build fix (J94's last open point). Written on TheJat-WSL/ci. Entry 117 found LLVM's tools linked again on every `make test64`, and left it. The cause was not LLVM: the C library was built again, from scratch, on every run - so everything linked with it was older than it, and was linked again too. Now a `test64-shared` with nothing to do takes 17 seconds instead of 51, on every run of `make test64` and `make test64-again`, on every machine.*

## Why

A part of the bottleneck list the user agreed to fix (the three: the docs split, one commit per merge, and this). Every `make test64` starts with the shared builds; when nothing changed - a second run, `make test64-again` after a fix to a test script, the night's next branch - they should take seconds.

## What it found

`tools/build-llvm.py` already links a tool only when something it is made of is newer. Two runs in a row showed why it did anyway:

```text
  CC      libc/c/format.c        <- every run, all 50-odd files, for each CPU
  ...
thin-archive: build/c/libc.a.new: 95 of 482 members ...
  LD      llvm-tblgen (x86_64)   <- libc.a is newer than the tool, so...
  LD      clang (x86_64) ...
```

`libc64` and `cprogs-aarch64` compiled every C file of the library, archived it again and ran `thin-archive.py` (6 to 10 s alone) every time: make has no rule saying when not to. The library came out **byte for byte the same** (checked with `cmp`), but new - and so newer than LLVM's tools, TCC and every C program.

Measured, ranked (a `test64-shared` with nothing to do, each line's time since the one before):

| Step | Seconds |
|---|---|
| libc for AArch64 (compile, archive, thin-archive) | ~16 |
| libc for x86_64 | ~8 |
| TCC, both CPUs (`tools/build-tcc.sh` builds everything, always) | ~11 |
| LLVM's ten links | a few: lld is fast; not the cost entry 117 thought |

## What changed

1. **libc.a is built again only when an input is newer than its last build** (`build/c/libc.a.stamp`): the Rust half (cargo decides that), `libc/c`, `libc/include`, the `Makefile` (the flags), `thin-archive.py`. Otherwise `libc.a is up to date`.
2. **And then it replaces the old one only if it differs** (`replace-if-changed`: built as `libc.a.new`, `cmp`): a library that comes out the same keeps its old time, so nothing linked with it is linked again.
3. **TCC likewise**: `build-tcc.sh` keeps `OUT/.built` with the arguments it was built with, and stops at once - `tcc (x86_64) is up to date` - when nothing it is made of (TCC's sources, libc.a, the headers, the script) is newer.

| `make test64-shared`, nothing to do | Wall | CPU (user + system) |
|---|---|---|
| Before (`main`'s Makefile) | 51.5 s | 53 s |
| After | **16.8 s** | 17 s |

## How I checked it

- Two runs in a row: the second links nothing and says `libc.a is up to date`, `libc.a (AArch64) is up to date`, `tcc (x86_64) is up to date`, `tcc (aarch64) is up to date`.
- `touch libc/c/stdio.c`: the library is built again, comes out the same, keeps its time - no tool linked again.
- The old and the new Makefile on the same tree, `make -f`: 51.5 s and 16.8 s.
- The tests that use what is no longer rebuilt: `tcc64-test`, `aarch64-tcc-test`, `make64-test`, `llvm64-test` - all passed.
- The whole `make test64`: the night's (rule 2).

## Design decisions

- **A stamp, not make's own rules**: the library's inputs are 60 C files, their headers, a cargo build and the Makefile; a `find -newer` over the directories is one line and cannot miss a header a C file includes, where a hand-written prerequisite list would.
- **The Makefile itself is an input**: a changed flag must rebuild. Any Makefile change rebuilds the library - which then usually comes out the same and keeps its time (2).

## What is still missing

- About 9 seconds still go somewhere between the C library and pdpmake in a run with nothing to do, and `cxxlib.cpp` is compiled again (1 s per CPU) - the next to look at, with the night logs' stamps.
- pdpmake and the C programs are compiled again every run (fast: a second for all).

## Testing note

In a session's tree:

| Do | You should see |
|---|---|
| `make test64-shared` twice | The second: `libc.a is up to date`, `libc.a (AArch64) is up to date`, `tcc (x86_64) is up to date`, `tcc (aarch64) is up to date`, no `LD` lines, about 17 s on TheJat-WSL |
| `touch libc/c/stdio.c`, `make libc64` | `CC libc/c/...` and thin-archive again; `stat build/c/libc.a` shows its old time |

Automated: `make test64` runs it first.
