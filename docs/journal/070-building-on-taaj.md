# TheTaaJ builds its C library and C programs

*Journal entry #70 · Era 6, step B. The shell runs scripts now: `source FILE` runs a file's lines one by one and stops at the first that fails. One such script, `/src/build`, compiles the C half of TheTaaJ's C library with tcc, on TheTaaJ, puts it together with the Rust half into a new libc.a, and builds all eleven C programs with it: 24 seconds on the PC, 46 on AArch64. A program typed into `edit` is compiled and run without leaving the system. Every one of the 28 files tcc writes on TheTaaJ is checked, byte for byte, against the cross compiler's.*

```
taaj:/> time source /src/build
+ cd /src/libc/c
+ tcc -c format.c stdio.c fcntl.c ctype.c stdlib.c inet.c float.c setjmp.c time.c abort.c fp128.c
+ tcc -ar rcs ../libc.a /lib/libc-rust.o format.o stdio.o fcntl.o ctype.o stdlib.o inet.o float.o setjmp.o time.o abort.o fp128.o
+ cd /src/libc/programs
+ tcc -L.. chello.c -o chello
  ...
+ tcc -L.. clib.c -o clib
+ echo built: the C library and 11 programs
built: the C library and 11 programs
took 23.78 s; the 2 CPUs worked for 16.19 s in all
taaj:/src/libc/programs> cd /
taaj:/> with files src/libc/programs/clib
library: 53 checks, all passed
```

## Why

Step A put a C compiler on TheTaaJ, and it compiled a few test programs. Step B makes it compile TheTaaJ's own C: the C library and the programs in `libc/programs`. Then, when one of them changes, it can be rebuilt on the system it runs on. Two things were missing for that:

- **a way to run a list of commands.** A build is fifteen commands, and typing them each time is not a build;
- **a C library that tcc can compile.** The C half had only ever been compiled by clang, and it used two things tcc does not understand.

## The picture

```
  /src/build (libc/build.taaj)          the shell: source /src/build
        │
        ├─ tcc -c format.c stdio.c ...   ─▶ 11 objects          /src/libc/c
        ├─ tcc -ar rcs ../libc.a  /lib/libc-rust.o + the 11      /src/libc/libc.a
        └─ tcc -L.. chello.c -o chello   ─▶ 11 programs          /src/libc/programs
               (-L.. : the new libc.a, before /lib/libc.a)
```

The C library is two halves, and only one can be built here. The C half (`libc/c`: printf, stdio, strtod's C side, setjmp, the calendar) is C. The Rust half (system calls, files, malloc, strings) is Rust, and TheTaaJ has no Rust compiler. So the Rust half comes ready-made as one object, `/lib/libc-rust.o`. `tools/build-tcc.sh` makes it from libc.a: every member that is not one of the C objects, merged by `ld.lld -r` (relocatable output: an object, not a program). tcc's own archiver joins the two halves into a new libc.a.

## `source FILE`

`source` is one of the shell's own commands. It reads the file, and runs each line through the same `run` that a typed line goes through, after printing it with a `+` in front. Blank lines and lines starting with `#` are skipped. It stops at the first line that fails and says where: `source: broken: stopped at line 3`.

To know whether a line failed, the shell now keeps a flag, `failed`, that is cleared when each line starts. It is set when:

- a program ends with a code other than 0, or is killed;
- a command does not exist;
- a quote is never closed;
- a redirection's file cannot be opened or written;
- a pipeline cannot be started;
- `cd` cannot go where it was told.

The lines run in the shell itself, not in a new one, so a `cd` in a script is still in effect when the script ends. After `source /src/build` the prompt is `taaj:/src/libc/programs>`. That is how a Unix shell's `source` behaves too. A script inside a script works, up to four deep, so a script that sources itself stops instead of running forever. `time source /src/build` times the whole build, because `time` runs whatever words follow it.

Stopping at the first failure is a deliberate difference from a Unix shell, which goes on unless told `set -e`. A script here is a build. A build that keeps going after a compiler error only buries that error under the errors that follow from it.

## What tcc could not compile

- **`setjmp.c` on the PC was in Intel syntax** (`mov [rdi + 8], rbp`). Clang reads both syntaxes; TCC's assembler reads only AT&T (`movq %rbp, 8(%rdi)`: source first, registers with `%`). So it is AT&T now, which both read. TCC also wanted `jmp *56(%rdi)` rather than `jmpq`.
- **`setjmp.c` on AArch64 used `cmp` and `csinc`**, which TCC's AArch64 assembler does not have yet. A `cbnz` around a `mov` does the same job: "the value, or 1 if it is 0".
- **`stdio.c` used `__builtin_mul_overflow`**, a clang built-in, in fread and fwrite. It is now a small C function: `size > SIZE_MAX / count` means the product would not fit.

And one thing tcc compiled happily, but the result would not link: **`__va_arg`**. When TCC compiles `va_arg` on x86-64, it calls a helper in its own library, in `lib/va_list.c`. My libtcc1.a did not have it. Step A never noticed, because printf's file had been compiled by clang. The first tcc-compiled `format.c` gave `unresolved reference to '__va_arg'`. Upstream's own Makefile lists `va_list.o` for x86-64, and now build-tcc.sh does too.

## Checking against the cross compiler

The test runs the same script on the host, with the cross compiler substituted for `tcc` and host paths for `/src` and `/lib` (`sed` on `libc/build.taaj`). The archive keeps only file names, not paths, so the two libc.a files can be compared directly. Then all 28 files must be identical: the 5 from step A's part of the test (including `hi`), libc.a, its 11 objects and the 11 programs.

That comparison says the compilation is right. That the library works is shown by running it: clib, linked with the libc.a built on TheTaaJ, passes its 53 checks. Those checks cover setjmp from the AT&T rewrite, printf's floating point from a tcc-compiled `format.c`, and strtod.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Running a list of commands | `source FILE` in the shell | A `make` port: a program and a language of its own, for fifteen lines. A separate script program: it would need the shell's job control and grants all over again |
| On a failing line | Stop, and say which line | Go on, as `sh` does: a build's later errors hide the first |
| What counts as failing | A non-zero exit or a kill, a missing command, a bad redirection, a failed `cd` | Exit codes only: a typo in a command name would pass |
| The Rust half on TheTaaJ | Built on the host, shipped as one object, `/lib/libc-rust.o` | Rust on TheTaaJ: far beyond this era. Shipping its 18 archive members: 18 more files to name in the script, with the shell's lack of wildcards |
| Joining it to the C half | `ld.lld -r` on the host, `tcc -ar` on TheTaaJ | Linking programs against two archives: works, but the result would not be a libc.a that compares with the host's |
| x86 setjmp | AT&T syntax | Teaching TCC Intel syntax: a second assembler parser. Two versions of the file: twice the places for a mistake |
| Where the build writes | Beside the sources: `/src/libc/libc.a`, the programs in `/src/libc/programs` | Over `/lib/libc.a` and the root's programs: a broken build would break the compiler's own library |
| Testing the editor | A second boot, with the window system's keyboard; the program's output in `hi.out` | One boot on AArch64 with a screen: the compiler steps' output would go to the terminal window and not the log |

## Bugs

- **`unresolved reference to '__va_arg'`.** This was libtcc1.a missing `va_list.c`, as described above. It was a step A bug that only step B could reveal.
- **`unknown opcode '.intel_syntax'`**, then **`unknown opcode 'jmpq'`**, then **`ARM64 instruction 'cmp' not implemented`**: the setjmp rewrite, one error at a time.
- **The editor step failed** because `edit` was not on the test disk. `tcc` and the test programs were, but the editor is built with the user programs. The Makefile now passes it in (`EDIT=`).
- **On AArch64 the first steps "printed nothing".** With a screen, programs' output goes to the terminal window, not the serial log. The test now has two boots, as described above.
- **The host half failed in a directory it had left.** The build script changes directory, and the test's work directory was a relative path. It is absolute now. I made the same mistake in step A.
- **thin-archive could keep two definitions of one symbol.** It pulled in a support member for any symbol something needed, even one another kept member already defined. That is harmless in an archive, where the first definition wins, but an error when `ld -r` merges the members into one object. Now it only takes a member for a symbol nothing kept defines. In today's library this changes nothing (still 29 members), so no test can show it.

## How I checked it

Automated, by booting: `tcc64-test` and `aarch64-tcc-test` grew from one boot to two.

- First boot, after step A's steps:
  - `source broken`: a script whose second command (`tcc nofile.c`) fails. `first line` is printed, then `source: broken: stopped at line 3`, and `never printed` is not.
  - `time source /src/build`: the build. It takes 23.8 s on the PC and 46.0 s on AArch64, both with 2 CPUs under QEMU.
  - clib built by it passes 53 checks; `cprimes 100` says 25 primes; `chello built here` gets its arguments.
- Second boot, with the window system's keyboard:
  - `edit hi.c`, two lines typed, Ctrl+S (74 bytes), Ctrl+Q;
  - `tcc hi.c -o hi`;
  - `hi > hi.out`.
- On the host:
  - `hi.c` read back off the disk is exactly what was typed, and `hi.out` is `0.125`;
  - the build script run with the cross compiler, and all 28 files compared byte for byte. They are identical on both machines.

Mutation experiments, 3, all caught by `tcc64-test`:
- M1, `source` not stopping at a failing line: "source went on after a command failed".
- M2, a program's non-zero exit not counted as a failure: "source did not stop at the command that failed".
- M3, libtcc1.a without `va_list.c`: the build script stopped at the first program, and "tcc failed where it should not have".

`make libc-test` still passes: printf's 2127 formats, stdio's 124 checks with the new overflow test, stdlib, inet. Clippy is clean. `make test64` passes.

The testing note's steps were run as scripted boots on scratch disks.

Not tested:
- A person typing. The boot scripts sent every key.
- `source` on its other ways to fail (a missing file, a directory, a script over 64 KiB, four scripts deep), and a script given as a path with `..`. They are a few lines each and not booted.
- Whether anything still links the C half compiled by clang. The programs built on TheTaaJ use the new libc.a (`-L..`), but the byte comparison would not notice if one did not, because the host does the same.

## What is still missing

- Step C: tcc compiling itself on TheTaaJ.
- Wildcards in the shell, so a script can say `*.c`; variables; `if`.
- Rebuilding only what changed: every `source /src/build` compiles all 22 files.
- The Rust half built on TheTaaJ.
- Installing what was built: `/lib/libc.a` and the programs in the root are still the ones from the host.

## What I learned

- A compiler change can expose a library bug from the step before. `__va_arg` was missing all along. It only mattered once printf's file was compiled by TCC instead of clang.
- Two assemblers are two dialects. The same instructions written for clang did not parse for TCC. AT&T syntax is the one both understand.
- A script's most important feature is stopping. Without the `failed` flag the shell could run lines, but it could not tell a build that worked from one that printed an error and went on.
- Merging objects is stricter than archiving them. An archive tolerates two definitions of a symbol; one merged object does not.
- Comparing with a cross compiler checks that the compiler is right, not that the program is. For that, the library built on TheTaaJ still has to run its own tests.

## Testing note

**1. The PC: build the C library and the programs on TheTaaJ**

`make run64` puts tcc, the sources in `/src` and the script `/src/build` on the run disk.

```
make run64
```

| Type | You should see |
|---|---|
| `ls /src/libc/c` | the C library's `.c` files and `format.h` |
| `time source /src/build` | each command after a `+`, no errors, `built: the C library and 11 programs`, then `took` about 20 to 40 s |
| `ls ..` | the prompt is `taaj:/src/libc/programs>` now: the script's `cd` stays done. `libc.a` (about 225,000 bytes), `c/`, `programs/` |
| `./cprimes 100` | `cprimes: 25 primes below 100` |
| `./chello built here` | `Hello from C`, `argc 3`, `argv[1] built`, `argv[2] here` |
| `cd /` | the prompt is `taaj:/>` again |
| `write broken tcc nofile.c` | `broken: 13 bytes` |
| `source broken` | `+ tcc nofile.c`, `tcc: error: file 'nofile.c' not found`, `tcc exited with code 1`, `source: broken: stopped at line 1` |

**2. Change a program on TheTaaJ, and rebuild it there**

```
make run64
```

| Type | You should see |
|---|---|
| `edit /src/libc/programs/chello.c` | the editor, the file's first lines |
| Page Down twice, then Down seven times | the status line says `line 54/…`, on `print("Hello from C\n");` |
| End, Left five times, Backspace, `TheTaaJ` | the line now reads `print("Hello from TheTaaJ\n");` |
| Ctrl+S, Ctrl+Q | saved, and the shell again |
| `cd /src/libc/programs` | the prompt says `taaj:/src/libc/programs>` |
| `tcc -L.. chello.c -o chello` | nothing printed |
| `./chello` | `Hello from TheTaaJ` |
| `chello` | `Hello from C`: a bare name is always the program in `/`, wherever the shell is |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64
```

The same as part 1, typed at the terminal. The build takes about 45 s, and this `libc.a` is about 276,000 bytes.

**4. The tests by themselves**

```
make tcc64-test
make aarch64-tcc-test
```

| Look for | Meaning |
|---|---|
| `took … s; the 2 CPUs worked for … s in all` in `build/tcc64.log` | how long the build took on TheTaaJ |
| `source: broken: stopped at line 3` in the log | a script stops at the command that fails |
| `tcc on TheTaaJ wrote all 28 files byte for byte as tcc on the host does: …` | everything built on TheTaaJ is exactly what the cross compiler builds |
| `tcc64-test passed`, `aarch64-tcc-test passed` | all of it, both boots, on each machine |

Automated: make test64
