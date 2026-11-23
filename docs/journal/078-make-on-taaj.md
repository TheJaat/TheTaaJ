# make on TheTaaJ: files that know when they changed

*Journal entry #78 · Era 6, step E6. `make` runs on TheTaaJ: pdpmake, a public-domain POSIX make, ported. It starts each command with `posix_spawn`, and it rebuilds only what a change put out of date. That needs to know which file changed last, so TaajFS gains change stamps (version 4). `cd /src/libc/programs` and `with programs make` builds the eleven C programs with tcc; change one `.c` file and make builds that one again.*

## Why

E5 let a program start another. The first program that needs this is `make`: building is mostly "run this command if that file is newer than this one". Make needs three things TheTaaJ did not have:
- a time for each file, to compare;
- `system`, to run a command;
- the small Unix functions make itself is written with (`getopt`, an environment, `glob` and the others).

## How it fits together

```
 shell:  with programs make          (files, and init: the right to start programs)
           v
 make    reads Makefile; stat(main.c), stat(main.o)   -> st_mtime = change stamp
           |   main.o older than main.c (or missing)?
           |   system("tcc -c main.c")                 libc/src/spawn.rs
           |     words: ["tcc", "-c", "main.c"]          libc/src/words.rs
           |     posix_spawnp, waitpid -> status
           v
 tcc     writes main.o: the file server stamps its inode with the
         generation of the commit it goes into          taajfs/src/volume.rs
```

What it took:

**Change stamps in TaajFS** (`taajfs/src/format.rs`, `volume.rs`, version 4):
- A stamped inode has its own magic, `JAFT` (the old one is `JAFS`). Its header is 24 bytes, with a 64-bit stamp, so it has 1018 block slots where the old inode has 1020. Both kinds can be read; a version-4 volume writes only the new one.
- The stamp is the generation of the commit the inode goes into. Every commit raises the generation, so a file written later has a higher stamp, and it stays right across reboots. No clock is needed, and TheTaaJ has none that lasts past a power cut.
- A volume becomes version 4 only when it is asked to (`Volume::allow_stamps`), as version 3 did for big files. TheTaaJ's file server asks when it mounts a disk. The host's `tfstool` does not ask, so a disk it makes stays version 1, which the old C tool can still read (`make tfscheck`).
- `Volume::touch` gives a file a new stamp without changing it, and makes it if it is missing. `Volume::stamp` reads a file's stamp.

**Through the file server and the user library**:
- The `STAT` reply now has the stamp: size, kind, stamp, 17 bytes.
- `TOUCH` (12) is new. It is refused on a read-only channel, as every change is.
- The memory store (`ram.rs`) keeps a counter of its own for its stamps.
- `user::files` has `stat_stamped` and `touch`.
- `fscheck` checks stamps on the disk (section 13).
- `tfstool dump` prints each stamp, and `tfstool touch` touches a file on a disk image.

**Times in the C library** (`libc/src/status.rs`):
- `st_mtime` (and `st_atime`, `st_ctime`) is the stamp, at least 1, because pdpmake takes 0 to mean "no such file".
- `utimensat`, `futimens`, `utime` and `utimes` touch the file when given no times or `UTIME_NOW`. A real time of day fails with `EPERM`: a stamp cannot be set, only moved forward.

**`system`, without a shell** (`libc/src/spawn.rs`, `words.rs`). There is no shell a program can start, so `system` does the part of a shell's job that simple commands need. It splits the command into words as `sh` does, with single and double quotes and backslashes, then starts the first word with `posix_spawnp` and waits for it. A command that needs a real shell is refused, with a line on standard error and -1 (`ENOSYS`):
- a pipe, `&&` or `;`;
- a redirection;
- `$` or backquotes;
- a glob pattern;
- a word starting `#` or `~`;
- `NAME=value`.

A program it cannot find gives exit status 127, as with a shell.

**The rest of what pdpmake uses** (`libc/c/unix.c`, new):
- `getopt` (with `optind = 0` to start again, as pdpmake does);
- `environ`, `getenv`, `setenv`, `unsetenv`, `putenv`;
- `basename`, `dirname`;
- `fnmatch`, `glob`, `globfree`;
- `popen` and `pclose`, which fail: there are no pipes;
- in other files, `strcasecmp`, `strncasecmp`, `strndup` and `stpcpy`;
- the new headers `strings.h`, `libgen.h`, `fnmatch.h`, `glob.h`, `utime.h` and `ar.h`.

**pdpmake** (`ports/pdpmake`, 2.0.4). It comes in as it is upstream, then one change. In POSIX mode it hands each command to the shell as `set -e;COMMAND`. Here there is no shell, and `system` would refuse the `;`. It is compiled with the build's flags and linked with `libc.a`: about 116 KB on the PC, 120 KB on AArch64. `make pdpmake64` and `make pdpmake-aarch64` build it, and the run disks get it as `/make`.

**A Makefile for the C programs** (`libc/programs/Makefile.taaj`, put on the run disks as `/src/libc/programs/Makefile`). It is one suffix rule, `.c:` → `tcc $< -o $@`, for the eleven programs.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What a file's time is | The commit generation it was written in | A clock: none lasts across a power cut here, and make only needs "later than", which the generation gives for certain |
| Where the stamp lives | In the inode, a new inode kind (`JAFT`) | In the directory entry: a file renamed or in two places would need it twice |
| Turning stamps on | Asked for per volume (`allow_stamps`), by TheTaaJ's file server | Always: the host's C tool reads only version 1, and its crosscheck would stop working |
| Setting a time | Only "now" (`UTIME_NOW`, or no times); a given time is `EPERM` | Storing any number given: a stamp from the past or the future would break "later than" for every file |
| `system` | Words split as `sh` splits them, one program, no shell; the rest refused out loud | A shell for programs: a large job of its own. A command with a metacharacter run as plain words would do something else without a word |
| make | pdpmake: one public-domain program, POSIX, small | GNU make: GPL, and many times the size, with more of the system under it |
| pdpmake's `set -e;` | Left out on TheTaaJ | Teaching `system` to skip it: a special case for one program, hidden in the library |

## Bugs

- **`system: tcc: not found`**, from make in `/proj`. `posix_spawn` resolved a relative program path from the current directory, giving a path from the root without its leading `/`. It then asked the file server for the file with that path, which the server took as relative to the current directory again. So `/proj/tcc` was looked for, not `/tcc`. E5's clang test runs from `/`, where the two are the same. The path now goes to the server with its `/`. Mutation M5 below puts the bug back and is caught.
- **`unknown command: prog`**: the test's mistake, not make's. A bare name is looked up in the root, so a program in the current directory is `./prog`.
- **`unknown command: append`**: the test disk had only tcc and make. It gets `append` now, to change the sources between the steps.
- **`make -n` said one command, not two.** That is what make should do: nothing is run, so `prog` is still newer than `main.o`. pdpmake on the host says the same. The test expected the wrong thing.
- **pdpmake's messages name the Makefile's line** (`make: (Makefile:16): failed to build 'bad.o' exit 1`), and make exits with 2 for an error, as POSIX says. The test now looks for both.
- Before the port: in `unix_test.c`, `dirname("//")` may be `/` or `//` (POSIX allows both; glibc says `//`), and a trailing backslash in an `fnmatch` pattern matches nothing (as in glibc). LeakSanitizer flags `setenv`'s replaced strings, which are left on purpose because a program may still point at one. That test runs with leak detection off.

## How I checked it

Automated, by booting: `make64-test` and `aarch64-make-test` (`kernel/kernel64/scripts/make-test.sh`, in `make test64`). A disk with tcc, make, `append` and `/proj` holds a Makefile, `main.c`, `util.c`, `util.h` and a broken `bad.c`. Each step is marked in the log by a line of its own, and the test checks each part separately:

| Step | Shows |
|---|---|
| `with programs make` | `tcc -c main.c`, `tcc -c util.c`, the link, once each; `./prog` says `prog: 42` |
| `with programs make` again | `make: 'prog' is up to date`, nothing compiled |
| `append util.c ...`, `make` | only `util.c` compiled, then the link |
| `append util.h ...`, `make` | both compiled (each includes it), then the link |
| `append main.c ...`, `make -n` | `tcc -c main.c` said and not started; then `make` runs it, and the link |
| `append main.c ...`, `make -t` | `touch main.o`, `touch prog`, no tcc; then `make`: up to date |
| `make fails` | tcc's error, `make: (Makefile:16): failed to build 'bad.o' exit 1`, make exits with 2 |
| `make shell` (`echo one && echo two`) | `system: cannot run "echo one && echo two": '&' needs a shell, and there is none here`; `one` never printed |
| `with files make` | `couldn't execute 'tcc -c main.c'`: no right to start programs, no tcc started |

Afterwards `main.o`, `util.o` and `prog` are read off the disk and compared byte for byte with what the host's cross tcc makes from the sources as they ended up. They are identical on both machines. The disk's inodes carry stamps (`tfstool dump`).

On the host:
- `taajfs`: 60 tests, among them stamps raised by each write, kept by a rename, the 1018-slot limit, and the power-cut test run again with stamps on.
- `fileserver`: 34 tests, with `STAT`'s stamp and `TOUCH` against both stores.
- `libc-test`: `unix: 303 checks`, against glibc or POSIX; `system`'s word splitting (`words.rs`); `stpcpy`.
- `clib` on TheTaaJ checks `utimensat`, `utime`, `futimens`, `EPERM` and `ENOENT`.

Mutation experiments, 5, each run against `make64-test`:
- M1, every stamp the same (`next_stamp` gives 1): `a second make did not say prog is up to date`. Caught.
- M2, `st_mtime` always 1: caught the same way.
- M3, `utimensat` does nothing: `make -t did not touch prog`. Caught.
- M4, `&` not refused by `system`: `the command that needs a shell was not refused`. Caught.
- M5, the relative-path bug above put back: `the first make did not compile both files and link them: 1 0 0`. Caught.

The testing note below was run on both machines by typing its steps: 11 programs in 25 s on the PC and 43 s on AArch64, then `make: nothing to be done for all`, then `cprimes` alone after a change.

Not tested:
- A person typing.
- Stamps on a disk across a real power cut (the host's power-cut test covers it).
- make's `include`, `$(shell ...)` and `!=`: they need `popen`, which fails.

## What is still missing

- **A shell for programs.** `system` runs one command, so a Makefile with `&&`, `;`, redirections or `$VAR` fails, and says so. A recipe of several lines is several commands, which works.
- **`popen`, pipes and redirections** (`posix_spawn_file_actions`): still `ENOSYS`.
- **Times of day.** A stamp orders changes; it does not say when. `ls` shows none.
- **The child's environment.** `setenv` works within a program, but `posix_spawn` does not pass it on, so make's exported macros do not reach the commands.
- **make with clang**: clang needs `@cflags @lflags` and is slow to start, but nothing else is in the way.

## What I learned

- make needs only "which is later", and a file system with copy-on-write commits already counts. A clock would have been more work and less sure.
- A version that older readers cannot read should be asked for, not handed out. Keeping version 4 opt-in kept the host's C tool working with no change to it.
- Refusing out loud is better than doing something close. `echo one && echo two` run as five words would print a line and succeed.
- A path that is "resolved" twice is resolved wrong once, in a directory nobody tested from. The E5 test ran only from `/`.

## Testing note

**1. The PC: make builds the C programs**

```
make run-disk
make run64
```

`make run-disk` now puts `/make` on the disk, and `/src/libc/programs/Makefile` with the programs' sources. Close QEMU before running it. The disk must be one TheTaaJ mounts, for its files to get stamps: any run disk is.

| Type | You should see |
|---|---|
| `cd /src/libc/programs` | the prompt says `taaj:/src/libc/programs>` |
| `time with programs make` | `tcc chello.c -o chello` and ten more lines like it, then `took` about 25 s |
| `with programs make` | `make: nothing to be done for all` |
| `with files append cprimes.c /* changed */` | `cprimes.c: 2507 bytes` |
| `with programs make` | `tcc cprimes.c -o cprimes`, and nothing else |
| `./cprimes 100` | `cprimes: 25 primes below 100` (the one just built; a bare `cprimes` is the root's) |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1; the first make takes about 43 s.

**3. The tests by themselves**

```
make make64-test
make aarch64-make-test
```

| Look for | Meaning |
|---|---|
| `make on TheTaaJ (x86_64) built prog with tcc, then only what each change put out of date; -n, -t, a failure, a refusal; main.o, util.o and prog byte for byte the host's tcc's` | every step, and the comparison (`aarch64` likewise) |
| `make64-test passed`, `aarch64-make-test passed` | each machine |

Automated: make test64
