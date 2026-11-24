# The C library by make: TheTaaJ's C library and its programs built by make on TheTaaJ

*Journal entry #83 · Era 6, step E10. `/src/libc/c/Makefile` builds TheTaaJ's C library and its 12 C programs on TheTaaJ, with make and tcc:
- the C half's 21 files and libm's 39;
- libm.o, then libc.a;
- every program, `sh` among them, linked with that libc.a.

Run it again and it does nothing. Change one file and it compiles that file alone, then puts libc.a together again and links the programs again. Every file it writes is the same, byte for byte, as what tcc writes on the host. The shell it builds then runs a loop. The Rust half of the library still comes ready-made, as before.*

## Why

`source /src/build` (entry 069) builds the same things, but it is a list of commands: it compiles all 60 files every time, and it can only be run from the top. E9 gave sh the loops and patterns a Makefile's recipes use, so the library can now be built the way a library is built anywhere else. This is also the first time a program that make builds on TheTaaJ is the shell make itself runs its recipes through.

## How it fits together

```
 cd /src/libc/c; with programs make
   make (pdpmake): Makefile -> libc/c/Makefile.taaj
     ../libc.a  <- $(OBJECTS) libm.o           tcc -ar rcs ../libc.a /lib/libc-rust.o ... libm.o
     libm.o     <- libm/first.o libm/second.o   cd libm && tcc -r first.o second.o -o ../libm.o
     libm/first.o <- $(LIBM1:%=libm/%)          cd libm && tcc -r $(LIBM1) -o first.o
     X.o        <- X.c                          cd $(<D) && tcc -c $(<F)        (.c.o:)
     ../programs/P <- P.c, ../libc.a            cd $(<D) && tcc -L.. $(<F) -o $(@F)  (.c:)
   each recipe: sh -c 'set -e;cd libm && tcc -c sin.c'   (entries 081, 082)
```

- **The same commands as `build.taaj`, in the same directories.** A file is compiled in its own directory, so tcc sees `sin.c`, not `libm/sin.c`. The two halves of libm.o are joined in `libm` with the same lists, and libc.a gets the same members in the same order. That is why the files can be compared byte for byte with what the host makes from `build.taaj`.
- **POSIX 2024 make features that pdpmake has:** `$(LIBM1:%=libm/%)` (a pattern macro expansion), `$(<D)` and `$(<F)`, and inference rules for targets in other directories (`../programs/sh` from `../programs/sh.c`).
- **Short recipes.** A command, its arguments and its environment must fit in one Run request, 1024 bytes (entry 079). So each file is compiled by its own command, not by one `tcc -c` of them all. That is 60 commands, but it is also what lets make compile one file again alone.
- **The headers are prerequisites.** `format.h` is one for `format.o` and `stdio.o`, and libm's five headers are for every libm object.
- **The programs depend on `../libc.a`,** so a change to the library links them again.
- `tools/put-sources.sh` puts the Makefile at `/src/libc/c/Makefile`, so it is on every disk `make run-disk` makes.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where the Makefile is | `/src/libc/c`, beside the files it compiles, building `../libc.a` and `../programs/*` | At `/src/libc`: every compile would be `tcc -c c/x.c`, and the objects would not be the ones `build.taaj` makes |
| The programs | Built by the library's Makefile, from `../programs`, each depending on `../libc.a` | `cd ../programs && make`: `programs/Makefile` (entry 078) links with `/lib/libc.a`, and making it depend on `../libc.a` would break it on a disk without one |
| `build.taaj` | Kept, and the test compares make's output with it | Replaced by make: `tcc-test` would then depend on make, and `source /src/build` still works without make on the disk |
| Header dependencies | Written by hand, coarse | `tcc -MD`: there is no `include` in POSIX make to read them back |
| The test | A boot of its own, `libmake64-test` (`libc-make-test.sh`) | Inside `tcc-test`: it is already the longest boot, and a failure would not say which half failed |

## Bugs

- **`"${p##*/}"` matched nothing.** Inside double quotes, sh read the pattern in `${NAME##pattern}` as quoted, so `*` was a star and not a wildcard. POSIX says the pattern keeps its meaning there. It showed in this entry's testing note: `echo "made: ${p##*/}"` printed whole paths. The pattern is now read as if it were outside the quotes. A new host test covers `"${f##*/}"`, `"${f%.?}"` and a quoted part inside the pattern, and it fails with the old code. (Entry 082's tests and checks used `${f##*/}` only outside double quotes.)

## How I checked it

On the host:
- The Makefile, run with pdpmake built for the host and the cross tcc, did 76 commands. A second run said "nothing to be done for all". The 75 files it wrote are, byte for byte, what `build.taaj` wrote in `tcc64-test`.
- `sh.rs`: 30 tests, one stronger (above).

By booting, both machines (`libmake64-test`, `aarch64-libmake-test`, new), in `/src/libc/c`:
- `with programs make`: 60 files compiled (21 and libm's 39), libc.a put together, 12 programs linked, exit code 0. This took 77.48 s on x86_64 and 129.86 s on AArch64.
- `with programs make` again: `make: nothing to be done for all`, and no tcc started.
- `with files append ctype.c /* changed */`, then `make`: `cd . && tcc -c ctype.c` alone, libc.a, the 12 links, and libm.o not joined again.
- `../programs/sh -c 'for i in 1 2; do echo "built here: $i $((i * 21))"; done'`: the sh just built prints `built here: 1 21` and `built here: 2 42`.
- On the host, `build.taaj` with the cross tcc, the ctype.c from the disk (a comment more), and `sh.c` linked the same way. All 76 files compared:
  - libc.a;
  - 22 objects, libm.o one of them;
  - libm's 39 objects and its two halves;
  - the 12 programs.
  
  Each is byte for byte the same.

Mutation experiments:

| Mutation | Result |
|---|---|
| M1: the programs not depending on `../libc.a` | "after ctype.c changed, make did not link the 12 programs again" |
| M2: each file compiled from `c/` (`tcc -c libm/sin.c -o libm/sin.o`) | "the first make compiled 0 files, not 60" |

`make64-test`, `aarch64-make-test`, `pipe64-test` and `aarch64-pipe-test` passed as before, with the fixed sh.

clippy: libc on x86_64 and AArch64, clean.

`make test64`: every part passed, the first time, in 1141 s, with the two new parts (the library build took 108.96 s on x86_64 and 164.01 s on AArch64 beside the others).

The testing note below was run by typing its steps on both machines, each on a disk of its own.

Not tested:
- A change to a header (`format.h`, `libm.h`). make-test (entry 078) tests header prerequisites; these are written the same way.
- `make -j`: pdpmake runs one recipe at a time.

## What is still missing

- **The Rust half of the library is not built on TheTaaJ.** There is no Rust compiler there. `/lib/libc-rust.o` comes from the host.
- **make itself (pdpmake) and tcc are not in this Makefile.** tcc builds itself with `source /src/tcc/build`. pdpmake's sources are not on the disk.
- **Every recipe starts sh and then tcc:** 76 commands are 152 programs started. sh does not yet start a lone simple command in its own place, as `exec` would.
- **The header dependencies are by hand.**

## What I learned

- A Makefile that is a faithful copy of a build script is checkable: same commands, same directories, same bytes. That made one comparison prove it right, instead of a judgement about whether the outputs were "equivalent".
- The 1024-byte Run request decided the Makefile's shape: one file per command. Here, the shape it forced is also the right one for make.
- A testing note typed by hand found a bug that 30 host tests and 82 checks did not: they had all put `${f##*/}` outside quotes.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `cd /src/libc/c` | |
| `time with programs make` | each command (`cd . && tcc -c format.c`, ... `cd ../programs && tcc -L.. sh.c -o sh`), then `took ... s` (about 80 s) |
| `with programs make` | `make: nothing to be done for all` |
| `with programs ../programs/sh -c 'for p in ../programs/c*o; do echo "made: ${p##*/}"; done; echo "$((6 * 7))"'` | `made: chello`, `made: cstdio`, `42` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1 (the make takes about 130 s).

**3. The tests by themselves**

```
make libmake64-test aarch64-libmake-test
make test64
```

| Look for | Meaning |
|---|---|
| `make on TheTaaJ (x86_64) built the C library and its 12 programs ... all 76 files byte for byte the host's tcc's` | the library and programs, by make, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make libmake64-test aarch64-libmake-test
