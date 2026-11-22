# Clang on TheTaaJ

*Journal entry #75 · Era 6, step E3. `clang -c` runs on TheTaaJ. It compiles TheTaaJ's own eleven C programs, with the options TheTaaJ's build gives them, and writes the same objects as the host's clang 18.1.3, byte for byte, on both machines. It also writes the same assembly, the same preprocessed C, and the other machine's code. Clang is 82 MB, built from 574 of Clang's files and 88 libraries in all. To find its own headers, a program now learns which file it was started from, and a C program's `argv[0]` is its real name.*

## Why

E1 put LLVM's Support library on TheTaaJ, and E2 the code generators: everything under Clang. E3 adds Clang's front end, which reads C, and its driver, which turns `clang -c x.c` into the compile. With it, TheTaaJ has the compiler the project is built with, not only TCC. What is left of the self-hosting step is a linker (E4, lld).

## How it fits together

```
 /clang                         82 MB (PC), 67 MB (AArch64): driver + front end + LLVM
   started as "clang", file "/clang"   <- init tells it both (abi::startup::ARGUMENTS)
   resource dir: /lib/clang/18          <- from its own file + CLANG_RESOURCE_DIR
 /lib/clang/18/include/*.h      clang's own headers: stddef.h, stdarg.h, the intrinsics
 /include/*.h                   the C library's headers

 clang @cflags -c src/chello.c ... src/clib.c
   -> eleven jobs, run one after the other inside this process
   -> chello.o ... clib.o                  == the host's clang, byte for byte
```

What it took:

**Clang's sources, in `ports/llvm/clang`.**
- From the same 18.1.3 release tarball as LLVM, unchanged, in their own commit (`include/`, the libraries the `clang` program is made of, `lib/Headers`, `tools/driver`, `utils/TableGen`, the licence). Not the tests, the static analyser, the ARC migrator, the interpreter or the other tools. 1,826 files, 60 MB.
- TheTaaJ's changes come after, in a commit of their own: two places that read a file with `std::ifstream` (TheTaaJ's libc++ has no `<fstream>`), and the driver's handling of several jobs (below).

**clang-tblgen, for the host.**
- Clang writes its tables (diagnostics, attributes, options, builtins, `arm_neon.h`) with its own TableGen, `clang-tblgen`, as LLVM does with `llvm-tblgen`. Ubuntu does not package it.
- `tools/build-clang-tblgen.sh` builds it from `ports/llvm/clang/utils/TableGen`, against the host's LLVM 18.1.3 (`llvm-config-18`). The host's shared libLLVM lacks LLVM's TableGen library, so that one is linked from its archive. 20 files, a few seconds.

**`build-llvm.py` builds Clang too.** Adding `clang` to its tools needed:
- Clang's own way of declaring libraries: `add_clang_library` (with `LINK_LIBS` and the `LLVM_LINK_COMPONENTS` set before it), and `clang_target_link_libraries`.
- Clang's TableGen jobs, run with `clang-tblgen`: `clang_tablegen(...)` lines, the macro `clang_diag_gen(Component)`, and `clang_generate_header` in `lib/Headers`. About 90 jobs.
- The headers CMake would configure: `Version.inc`, `Config/config.h` (from `ports/llvm/taaj/clang-config.txt`), an empty `VCSVersion.inc`, and LLVM's `Extension.def` (no extensions).
- The `main` that CMake writes for a tool marked `GENERATE_DRIVER`, from LLVM's template.
- Clang's headers copied to `lib/clang/18/include`, beside the built programs, with the ones generated from `.td` files.

**A program knows its name and its file.**
- Until now a C program's `argv[0]` was the placeholder `program`, and LLVM's `getMainExecutable` returned nothing on TheTaaJ. Clang finds its headers from where its own file is, so it needs the real answer.
- The shell tells init the program's file as a path from the root (`/clang`, `/docs/tools/given`), in a new field of the start request, `abi::init::Run::file`.
- init now always gives the program its arguments channel, with two messages: the arguments as before (perhaps none), then the name it was started by and its file.
- `taaj::command_line()` reads both; `taaj::arguments()` is the arguments alone, as before, so no Rust program changed.
- libc gives the name as `argv[0]`, and keeps the file for `__taaj_executable()`, which LLVM's `getMainExecutable` calls on TheTaaJ, as Linux's reads `/proc/self/exe`.
- So errors now read as on the host: `clang: error: no such file or directory: 'nothere.c'`, and `llc: error: llc: bad.ll:2:7: ...` instead of `program: error: program: ...`.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How a program learns its file | A second message on the arguments channel, from init, which the shell fills in | A new startup handle: the flags' bits 6 and 7 are not handles, so every count of handles would change. Putting the name first among the arguments: every Rust program reads `taaj::arguments()` as the words after the name |
| `argv[0]` | The name as typed (`llc`, `tools/given`) | The file (`/llc`): the host's programs say `llc: error`, because that is what was typed, and the file is a separate question |
| Where clang finds its headers | `CLANG_RESOURCE_DIR=lib/clang/18`, a configuration value | Upstream's default, `../lib/clang/18` from clang's directory: with clang at `/clang` the root has nothing above it, and the answer came out relative (`lib/clang/18`), right only in the root. Putting clang in `/bin`: the shell runs bare names from the root |
| `clang -c a.c b.c` | Patch the driver: on TheTaaJ several jobs run in the one process, one after another, without `-disable-free` | Upstream starts a new process per job when there are several, and TheTaaJ's libc cannot start one yet. One clang per file: works, but loads 82 MB each time |
| `<fstream>` in Clang | Two small patches, reading the file into a string stream | Turning on libc++'s filesystem library, which `<fstream>` is tied to in 18: it wants a great deal more of POSIX than TheTaaJ has, for two options nobody here uses (`-foverride-record-layout`, `-frandomize-layout-seed-file`) |
| The vendor | "TheTaaJ clang version 18.1.3", as LLVM's tools say "TheTaaJ LLVM" | "Ubuntu clang ... (1ubuntu1)", to make `.comment` match: that would be a false statement. The test passes `-fno-ident` to both compilers instead |
| How many files a long command line takes | Response files (`clang @cflags`), as on any system | Raising the shell's 256-byte line limit: not needed, and response files are what clang's users do anyway |

## Bugs

- **`no library or group called version`.** Clang's driver links Windows' `version.dll`, in a `set()` inside an `if(WIN32)` block. My reading of CMake ignores `if`, so it saw a library called `version`. Clang's `LINK_LIBS` now count only for Clang's own libraries; the rest are the system's.
- **`'llvm/Support/Extension.def' file not found`.** CMake writes this list of linked-in extensions (Polly, ...). There are none here, and the file is now written with nothing in it.
- **`implicit instantiation of undefined template 'std::basic_ifstream<char>'`** in two files. See the design table.
- **`-print-resource-dir` said `lib/clang/18`.** See the design table.
- **`clang: error: unable to execute command: Couldn't fork: TheTaaJ does not do that`**, eleven times, for `clang -c` with eleven files. Upstream turns off running the compiler in the driver's own process when there is more than one job (review D74447), because each in-process compile skips freeing its memory. The patch keeps them in the process and lets each free its memory.
- **TaajFS names are at most 27 bytes.** Ten of clang's 198 headers have longer names: nine are CUDA's, HIP's and Hexagon's, and one is the PC's `avx512vlvp2intersectintrin.h`, which `<immintrin.h>` includes. They are left off the disk, and the test says which.
- Two test bugs of my own: QEMU's serial line ends lines with `\r`, so `^/lib/clang/18$` never matched, and the first compile line was 329 bytes, longer than the shell takes.

## How I checked it

Automated, by booting: `clang64-test` and `aarch64-clang-test` (in `make test64`). Each makes a 192 MiB disk laid out as above, with TheTaaJ's eleven C programs in `/src` and the options in response files. The host's clang 18.1.3 (checked) writes the reference files from the same names in a work directory. Then TheTaaJ runs:

| Step | Writes | PC, alone | AArch64, alone |
|---|---|---|---|
| `clang --version` | `TheTaaJ clang version 18.1.3`, `Target: x86_64-unknown-taaj` (`aarch64-...`) | | |
| `clang -print-resource-dir` | `/lib/clang/18`, found from its own file | | |
| `clang @cflags -c` the eleven programs | eleven objects, in one run of clang | 56.05 s | 116.57 s |
| `clang @oflags -c src/cwords.c -o other.o` | the other machine's object | 17.99 s | 25.01 s |
| `clang @cflags -S src/cwords.c` | assembly | | |
| `clang @cflags -E -P src/cstdio.c` | the C after the preprocessor: every header as read | | |
| `clang -c bad.c` | `bad.c:2:12: error: use of undeclared identifier 'x'`, exit code 1 | | |
| `clang -c nothere.c` | `clang: error: no such file or directory: 'nothere.c'`, exit code 1 | | |

All fourteen output files are compared byte for byte with the host's, and are identical on both machines. `@cflags` is the Makefile's `TAAJ_CFLAGS` (or `TAAJ_A64_CFLAGS`) with `-fno-ident`, so these are the objects TheTaaJ's own build makes for its programs, apart from `.comment`.

`llvm64-test` and `aarch64-llvm-test` now expect `llc: error: llc: bad.ll:2:7: ...`, as the host prints it. The ABI's host tests cover the start request with its file: round trips, a file too long for its length byte, a request cut off before its file, a file that is not text, and the request from before files.

Mutation experiments, 4:
- M1, no `CLANG_RESOURCE_DIR`: `clang did not find its headers from its own file (-print-resource-dir)`. Caught.
- M2, libc never records the program's file: the same failure. Caught.
- M3, init does not send the program's name and file: the same failure. Caught.
- M4, the driver patch for several jobs removed: `clang did not finish all six of its jobs with exit code 0`. Caught.

Not mutated, because no test can see it: keeping `-disable-free` for several jobs in one process only costs memory.

The testing note's steps were run as scripted boots on fresh disks made by `make llvm-put` and `make llvm-put-aarch64`, with 4 CPUs. `cwords.o` from each was compared with the host's and was identical. Clippy is clean for `user` and `libc` on both CPUs and for `abi`. `make test64` passes.

Not tested:
- A person typing.
- C++: `clang -c x.cpp` would need libc++'s headers on the disk, and many of their names are longer than 27 bytes.
- Any program that includes `<immintrin.h>` or `<x86intrin.h>`.
- Bigger programs than TheTaaJ's own: the biggest of the eleven, `clib.c`, is 435 lines.

## What is still missing

- **A linker**: E4, lld. Until then the objects clang writes on TheTaaJ are linked by TCC or on the host.
- **A C program cannot start another.** clang runs its jobs in its own process; `clang x.c -o x` (compile and link) needs to start a linker.
- **TaajFS names longer than 27 bytes**, for the ten headers left out, and for libc++'s.
- **`<fstream>` and `<filesystem>`** in TheTaaJ's libc++.
- **Clang is 82 MB in a 512 MB machine**, within the kernel's 128 MiB program limit.

## What I learned

- Reading CMake without running it works because LLVM's lists are regular. Where it fails is where CMake's language is a language: an `if(WIN32)` was the whole of this step's surprises from the build.
- `argv[0]` looked like a cosmetic gap in E2, a placeholder in error messages. It was the one thing clang's driver could not do without. A program's idea of where it lives is part of what an OS gives it.
- The decisions that kept this small were about where to put a value, not code: one configuration line for the resource directory, one field in the start request.
- Upstream's reasons matter when patching. The driver starts a process per job for memory, not for correctness, so the patch could keep the correctness and pay for the memory differently.
- Byte for byte with the host held again, now for the compiler the project itself is built with. These objects are the build's own objects, made on TheTaaJ.

## Testing note

**1. The PC: clang**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

The first `make llvm-put` builds Clang for the PC: about 620 files more than E2, which takes a while. It makes a new 192 MiB run disk, `build/llvm-disk.img`, with what E2's had, plus:
- `clang`, its headers in `/lib/clang/18/include`, and the C library's headers in `/include`;
- TheTaaJ's C programs in `/src` (`src/cwords.c`, `src/chello.c`, ...);
- `cflags`: the options TheTaaJ's build compiles them with, as a response file;
- a broken `bad.c`.

Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly clang --version` | `TheTaaJ clang version 18.1.3`, `Target: x86_64-unknown-taaj`, `Thread model: posix`, `InstalledDir: /` |
| `with readonly clang -print-resource-dir` | `/lib/clang/18` |
| `time with files clang @cflags -fno-ident -c src/cwords.c` | `took` about 20 s, nothing else printed |
| `time with files clang @cflags -c src/chello.c src/cmem.c src/cprint.c` | `took` about 35 s: three objects, from one clang |
| `with files clang @cflags -S src/chello.c -o chello.s` | nothing printed |
| `with files clang -c bad.c` | `bad.c:2:12: error: use of undeclared identifier 'x'`, the line `    2 \|     return x;` with a `^` under the `x`, `1 error generated.`, then `clang exited with code 1` |
| `with files clang -c nothere.c` | `clang: error: no such file or directory: 'nothere.c'`, `clang: error: no input files`, then `clang exited with code 1` |
| `ls` | `cwords.o` 5808, `chello.o` 2976, `cmem.o` 12864, `cprint.o` 2936, `chello.s` 7997, `clang` 82217968 (the last four were made without `-fno-ident`, so they carry `TheTaaJ clang version 18.1.3` in `.comment`) |

**2. The same object from the host's clang**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img cwords.o > /tmp/taaj-cwords.o
mkdir -p /tmp/e3/src && cp libc/programs/cwords.c /tmp/e3/src/
cd /tmp/e3
clang-18 --target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding \
    -fno-stack-protector -fno-pic -mno-red-zone -nostdlibinc -isystem ~/Workspace/TheTaaJ/libc/include \
    -fno-ident -c src/cwords.c -o /tmp/host-cwords.o
cmp /tmp/taaj-cwords.o /tmp/host-cwords.o && echo same
cd -
```

The host compiles from `src/cwords.c` too, in a directory of its own: an object holds the name of the file it was compiled from.

| Look for | Meaning |
|---|---|
| `same` | the object TheTaaJ wrote is the host's, byte for byte |

**3. AArch64**

Close any AArch64 QEMU first.

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1, typed at the terminal. `clang --version` says `Target: aarch64-unknown-taaj`. The `cwords.c` step takes about 40 s and the three-file step about 55 s. `clang` is 67332776 bytes, `cwords.o` (with `-fno-ident`) 6648, `chello.o` 3336, `cmem.o` 13720, `cprint.o` 3432.

**4. The tests by themselves**

```
make clang64-test
make aarch64-clang-test
make llvm64-test
```

| Look for | Meaning |
|---|---|
| `clang on TheTaaJ (x86_64) wrote, byte for byte as the host's: chello.o cmem.o cprint.o cfile.o cstdio.o cwords.o cask.o cprimes.o cnet.o cfloat.o clib.o other.o cwords.s cstdio.i` | every output equal to the host's (`aarch64` for the other machine) |
| `clang's headers not on the disk (names over 27 bytes): avx512vlvp2intersectintrin.h ...` | the ten headers TaajFS cannot name yet |
| `clang64-test passed`, `aarch64-clang-test passed`, `llvm64-test passed` | each machine; llvm64-test now checks `llc: error: llc: bad.ll` |

Automated: make test64
