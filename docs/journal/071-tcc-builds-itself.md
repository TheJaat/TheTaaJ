# TCC builds itself on TheTaaJ

*Journal entry #71 · Era 6, step C. tcc on TheTaaJ compiles its own source, 1.3 MB of C, into a new tcc. That new tcc compiles the same source again, and the two compilers it made are the same file, byte for byte. The third generation then rebuilds TCC's library and compiles a program that runs. 13 seconds on the PC, 26 on AArch64. Every file is the cross compiler's, byte for byte.*

```
taaj:/> time source /src/tcc/build
+ cd /src/tcc
+ tcc tcc.c -o tcc2
+ with files ./tcc2 tcc.c -o tcc3
+ cd lib
+ with files ../tcc3 -c libtcc1.c va_list.c alloca.S builtin.c stdatomic.c atomic.S
+ with files ../tcc3 -ar rcs libtcc1.a alloca.o atomic.o builtin.o libtcc1.o stdatomic.o va_list.o
+ echo built: tcc by tcc, tcc by itself, and its library
built: tcc by tcc, tcc by itself, and its library
took 13.17 s; the 2 CPUs worked for 9.81 s in all
taaj:/src/tcc/lib> ls ..
  ...
  502425  tcc2
  502425  tcc3
  ...
```

## Why

A compiler that can compile itself is the usual test of a self-hosting system. If the compiler, the C library under it, the file system and the kernel are good enough to build the compiler, they are good enough for nearly anything written in C. It is the last step of Era 6's TCC half. Clang comes later.

## Three generations

```
  /tcc      compiled by clang, on the host
    │  tcc tcc.c -o tcc2
    ▼
  tcc2      TCC, compiled by TCC, on TheTaaJ
    │  tcc2 tcc.c -o tcc3
    ▼
  tcc3      TCC, compiled by itself
    │  tcc3 -c libtcc1.c ...; tcc3 -ar rcs libtcc1.a ...
    ▼
  libtcc1.a TCC's library, rebuilt
```

Comparing tcc2 with tcc3 is the check. /tcc and tcc2 are different programs: one is clang's machine code, the other TCC's. If both are correct, they do the same thing, so given the same tcc.c they must write the same file. If tcc2 and tcc3 differ, TCC compiled itself wrongly somewhere, or its output depends on something it should not. It is the same check GCC's build does in its three stages.

The test checks more than that tcc2 equals tcc3. On the host, the cross compiler compiles the same tcc.c with the same config.h, and tcc2 and tcc3 must both be identical to its output. libtcc1.a and its six objects (four on AArch64) built by tcc3 must be the ones `build-tcc.sh` made on the host and shipped in `/lib/tcc`. Finally, tcc3 compiles and links `hello.c` into `hello3`, which must be the same file `hello` was earlier in the test, and which runs.

## What it took

Less than I expected. TCC is one file when compiled: `tcc.c` includes `libtcc.c`, which includes the rest (`ONE_SOURCE`). The first try, on the host, with the cross compiler and TheTaaJ's headers, compiled with no errors. The first boot built all three generations, and they matched. Step A's work on the C library (strtod, setjmp, time, realpath) and step B's `source` did the hard part. What was left:

- **TCC's sources on the disk.** `tools/put-sources.sh` now takes the CPU and the TCC build directory. It puts the 24 files that tcc.c reads for that CPU in `/src/tcc` (1.28 MB on the PC), the helpers' sources in `/src/tcc/lib`, and the build script as `/src/tcc/build`. It also writes `/src/tcc/config.h`: the very file `/tcc` was compiled with (`TARGETOS_TheTaaJ`, `/lib/tcc`, static, no threads), plus the CPU's `TCC_TARGET_…`.
- **Two scripts, one per CPU** (`ports/tcc-x86_64.taaj`, `ports/tcc-aarch64.taaj`). Only the helper lists differ, but the shell has no variables or `if` to choose between them.
- **`with files ./tcc2`.** The shell gives the file service only to programs it knows by name: `tcc` is on the list, `./tcc2` is not. A program run by its path gets nothing unless it is asked for. Without `with files`, tcc2 starts and says `tcc: error: file 'tcc.c' not found`, because it cannot see any files.
- **Compiling the helpers by their bare names.** TCC writes the name of the source file it was given into the object, as its file symbol. `build-tcc.sh` used to compile `/home/…/ports/tcc/lib/libtcc1.c`, and TheTaaJ compiles `libtcc1.c` in `/src/tcc/lib`. So the objects differed by that one string, and the libraries could not be compared. Now `build-tcc.sh` changes into `ports/tcc/lib` and compiles each helper by its bare name. The shipped library is then exactly the one TheTaaJ rebuilds.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| The check | tcc2 = tcc3, both = the cross compiler's tcc.c, libtcc1.a = the shipped one, hello3 = hello | Only "tcc3 runs": a compiler that runs can still be wrong. Only tcc2 = tcc3: two wrong compilers can agree, and the host's compile is a check from outside |
| config.h on TheTaaJ | The one `/tcc` was built with, plus `#define TCC_TARGET_…` | TCC's own guess of the target from `__x86_64__`: it works, but the build would then depend on a guess rather than say what it builds |
| Which sources | The 24 files tcc.c reads for that CPU | All of `ports/tcc`, 2.2 MB, including other CPUs, Windows and Mach-O: a third more disk for files nothing reads |
| Running tcc2 | `with files ./tcc2 …` | Adding "tcc2" and "tcc3" to the shell's list: names in the shell for a build's temporary files |
| Installing tcc3 over `/tcc` | Left to a person (the testing note does it, and puts `/tcc` back) | In the script: one wrong build, and the system has no working compiler to fix it with |
| The helpers' file symbols | Compiled by bare name on the host too | Comparing the libraries with the file symbols ignored: a comparison that skips parts checks less |

## Bugs

None in TheTaaJ this time. The two failures were in my own setup:

- **The first host try ran a cross compiler that was not there.** `make test64` had cleaned `build/tcc`. `make tcc64 tcc-aarch64` brought it back.
- **The libraries were different by one string.** This was the file symbol described above, found by comparing the host's libtcc1.a with one compiled by bare name. It is fixed in `build-tcc.sh` and was never a problem on TheTaaJ.

## How I checked it

Automated, by booting: `tcc64-test` and `aarch64-tcc-test`. After step B's steps, the first boot now also runs:

- `time source /src/tcc/build`: 13.98 s on the PC, 25.80 s on AArch64.
- `with files src/tcc/tcc3 hello.c -o hello3`, then `hello3`: `hello from tcc: 6 * 7 = 42, 1/8 = 0.125`.

On the host, `tcc.c` is compiled with the cross compiler. Then every file is compared byte for byte: 38 on the PC, 36 on AArch64. That is step B's 28, plus tcc2, tcc3, libtcc1.a, its objects and hello3. The test disk grew from 8 to 16 MB.

Mutation experiments, 3, all caught by `tcc64-test`:
- M1, a TCC whose output depends on who compiled it: one byte of the ELF header is set only `#ifdef __TINYC__`. tcc2, compiled by clang's tcc, still matches the host's; tcc3, compiled by TCC's, does not. "src/tcc/tcc3, written by tcc on TheTaaJ, is not what the same tcc on the host writes". This is the kind of mistake that only comparing generations finds.
- M2, the script without `with files`: tcc2 cannot see `tcc.c`, `source: /src/tcc/build: stopped at line 22`, and "tcc3 did not get to building TCC's library".
- M3, the helpers compiled by their full path again: "src/tcc/lib/libtcc1.a, written by tcc on TheTaaJ, is not what the same tcc on the host writes".

The testing note's steps were run as scripted boots on scratch disks, on both machines. That covers putting tcc3 in place of `/tcc`, compiling `cprimes` with it, and putting `/tcc` back. `make test64` passes. No Rust changed in this step.

Not tested:
- A person typing.
- tcc3 compiling all of step B's build. It compiled hello and TCC's library. That it writes the same files as `/tcc` follows from tcc3 being the same file as tcc2, but I did not run it.

## What is still missing

- Installing: nothing puts a rebuilt tcc or libc in place, and nothing checks it before it does.
- The Rust half of TheTaaJ, which is most of it, still cannot be built on TheTaaJ. "TheTaaJ builds TheTaaJ" is true of its C. Clang, and later Rust, are the rest of Era 6.
- The shell has no variables or `if`, hence two scripts.
- Rebuilding only what changed.

## What I learned

- A compiler is just a C program. TCC needed nothing a C program would not: files, memory, `setjmp`, a C library with the usual functions. Once those were right, it compiled itself on the first try.
- The check is comparing generations, not that the compiler runs. The two compilers being the same file says more than any single test program can.
- The file name a compiler was given ends up inside what it writes. So "the same build" means the same names as well as the same sources.
- Capabilities show up even in a build. The shell's rule that only programs it knows by name get files made the script say `with files ./tcc2`. That one phrase is the build's whole grant of authority.

## Testing note

**1. The PC: TCC builds itself**

```
make run64
```

| Type | You should see |
|---|---|
| `ls /src/tcc` | `build`, `config.h`, `tcc.c`, `tccgen.c` and the rest: 24 files, `lib/`, about 1,277,000 bytes |
| `time source /src/tcc/build` | the six commands, each after a `+`, then `built: tcc by tcc, tcc by itself, and its library`, then `took` about 13 to 20 s |
| `ls ..` | the prompt is `taaj:/src/tcc/lib>`; `tcc2` and `tcc3` are both `502425` bytes: the same size, because they are the same file |
| `cd /` | the prompt is `taaj:/>` |

**2. Use the compiler TCC compiled, then put the old one back**

| Type | You should see |
|---|---|
| `mv tcc tcc.clang` | nothing printed: the compiler clang built is set aside |
| `cp src/tcc/tcc3 tcc` | `tcc: 502425 bytes` |
| `tcc src/libc/programs/cprimes.c -o p` | nothing printed, after a second or so |
| `p 1000` | `cprimes: 168 primes below 1000` |
| `rm tcc` | nothing printed |
| `mv tcc.clang tcc` | nothing printed: `/tcc` is the one from the host again |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64
```

The same as parts 1 and 2, typed at the terminal. The build takes about 26 s. The sources are `arm64-*` instead of `x86_64-*` and `i386-*`, and tcc2 and tcc3 are both `598669` bytes.

**4. The tests by themselves**

```
make tcc64-test
make aarch64-tcc-test
```

| Look for | Meaning |
|---|---|
| `built: tcc by tcc, tcc by itself, and its library` in `build/tcc64.log` | all three generations were built on TheTaaJ |
| `tcc on TheTaaJ wrote all 38 files byte for byte as tcc on the host does: …` (36 on AArch64) | tcc2 and tcc3 are the same file and the cross compiler's, and libtcc1.a is the one TheTaaJ was given |
| `tcc64-test passed`, `aarch64-tcc-test passed` | all of it, on each machine |

Automated: make test64
