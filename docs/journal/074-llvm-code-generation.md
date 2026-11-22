# LLVM code generation on TheTaaJ

*Journal entry #74 · Era 6, step E2. LLVM's code generators run on TheTaaJ. `llc` turns a C program, already in LLVM's own language, into PC assembly or an AArch64 object file. `llvm-mc` assembles for either machine. Every output is byte for byte what the host's LLVM 18.1.3 writes, on both machines. `llc` is 34 MB, eight times the old program limit. To run it, the kernel now loads programs straight from the memory object they arrive in and gives each the stack it asks for. TaajFS gained files bigger than 4 MiB (format version 3). The file service can fill a whole memory object in a few exchanges. The C library gained the rest of `math.h`, taken from musl.*

## Why

Step E1 proved LLVM's Support library on TheTaaJ with `llvm-tblgen`. Clang is a front end (C to LLVM's language) on top of the rest of LLVM: the IR, the optimiser, the code generators and the machine-code layer. E2 brings in that rest, without Clang. `llc` is the smallest program that uses all of it: it reads LLVM's language, optimises, chooses instructions, allocates registers and writes assembly or an object file. `llvm-mc` is the assembler underneath. If both write exactly what the host's copies write, everything Clang will stand on works. Then E3 only has to add Clang's own front end.

## How it fits together

```
 host: clang -S -emit-llvm cwords.c          ->  x86_64.ll, aarch64.ll  (LLVM's language)
                                                    |
 TheTaaJ:  llc -O2 x86_64.ll -o x86_64.s     ------+   llc: 61 libraries in all three
           llc -O2 -filetype=obj aarch64.ll -o aarch64.o   tools, 1,508 files compiled;
           llvm-mc -triple=... -filetype=obj x86_64.s       34 MB (PC), 29 MB (AArch64)
                                                    |
 host:     cmp with what /usr/lib/llvm-18/bin/llc and llvm-mc write: identical
```

Both machines carry both code generators. `llc` on AArch64 writes PC code, and the reverse. `llc --version` on TheTaaJ lists the targets and says `Default target: x86_64-unknown-taaj` (or `aarch64-unknown-taaj`).

What it took, from the bottom up:

**The kernel loads big programs.**
- Programs could be 4 MiB, and the kernel copied each one into its own heap before loading it. That heap never gives memory back, so a 34 MB program would have left it 34 MB bigger for good.
- The ELF reader now reads through a small trait, `elf::Source`, with two implementations: a byte slice (programs built into the kernel) and a `MemoryObject` (programs from disk), which it reads page by page.
- `spawn` loads straight from the caller's memory object if nobody else holds it. Otherwise it first copies the object into fresh frames, so that the bytes it checks are the bytes that run.
- `PROGRAM_IMAGE_MAX` and a memory object's limit are now 128 MiB.

**A program says how much stack it wants.**
- LLVM recurses deeply, and 64 KiB was not enough. The kernel now reads the `PT_GNU_STACK` header's size and gives that much, from 64 KiB to 16 MiB, in whole pages.
- `tools/build-llvm.py` writes 8 MiB into LLVM's programs, which is what Linux gives a process. lld leaves that header's size at 0 when the linker script lists the headers itself, so the script patches the size after linking.

**TaajFS version 3: big files.**
- An inode had room for 1,020 block numbers, so files were at most 4 MiB.
- When a file needs more blocks than that, its inode holds index blocks instead. Each index block lists 1,024 data blocks, so a file can now be 256 MiB.
- Before the first big file is written, both superblock slots are raised to version 3. That way an older implementation, which would misread an index block as data, refuses the disk instead.

**READ_INTO.**
- Reading a program meant one 1 KiB request after another. llvm-mc's `--version` took 9.88 s, almost all of it reading.
- The new request `READ_INTO` carries a memory object. The file service maps it and reads the file straight into it, 4 MiB per exchange.
- The shell's program loader uses it. Assembling with llvm-mc went from 10.3 s to 1.76 s.

**The C library has the rest of `math.h`.**
- `llc` calls `sin`, `exp`, `log`, `pow`, `cbrt`, the hyperbolic functions and `fma`. These are musl 1.2.5's, in `libc/c/libm`: 38 files kept as musl wrote them, with three small patches.
- The float versions (`sinf`, `powf`) are TheTaaJ's own `floats.c`: the double function, rounded once more.
- TCC on TheTaaJ compiles all of them as part of `/src/build`.
- `uname` was added too, with `sys/utsname.h`.

**build-llvm.py reads CMake properly now.**
- In E1 it knew `llvm-tblgen`'s libraries by name. Now it scans every `CMakeLists.txt` under `lib/` for libraries and their sections. It expands `AllTargetsCodeGens` and similar group names into the two targets, and follows `LINK_COMPONENTS` to get the whole set in link order.
- It also finds which TableGen outputs each library depends on, and runs the host's `llvm-tblgen` for them first.
- Adding a tool is now one line naming its directory.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where `math.h`'s approximations come from | musl 1.2.5, its files kept as written, 3 small patches | Writing my own: error bounds of under one ulp take years of numerical work. FreeBSD's msun: bigger, and more of it depends on its own headers. glibc's: LGPL, and tied to its build |
| The float functions | `floats.c`: the double function, then rounded to float | musl's own float files: 30 more files, for programs that do not use them. Rounding a double result can be off by one in very rare cases; nothing on TheTaaJ needs correctly rounded `sinf` |
| What libm is tested against | glibc's `long double` functions, rounded to double; glibc's `fma` for `fma` | glibc's double functions: its `cbrt` is up to 2 ulps off, where musl's is right, so my test blamed musl for glibc's error |
| Getting a big program into memory | `READ_INTO`: the file service fills the memory object | Bigger `READ` messages: a message is at most 1 KiB by design. Mapping files: there is no page cache to map from |
| How much one `READ_INTO` reads | 4 MiB, and the client asks again | All of it at once: the service answers nothing else meanwhile, and init stops a service that does not answer for 4 s. Reading 34 MB took longer than that. A longer watchdog: it would hide real hangs |
| A big program in the kernel | Read through `elf::Source` straight out of the memory object, or out of a copy in fresh frames | Copy into the kernel heap as before: the heap never shrinks |
| When to copy first | Only if someone else also holds the object | Always: the copy only matters when someone could change the file while it is being checked, and for programs loaded from disk the shell has just let go of the object |
| A program's stack | Its `PT_GNU_STACK` size, 64 KiB to 16 MiB | A bigger fixed stack for everyone: every small program would pay for LLVM |
| Files over 4 MiB | One level of index blocks (256 MiB) | Extents: a bigger change to the format and its checker. Two levels: no file needs more than 256 MiB yet |
| A disk for trying LLVM by hand | `make llvm-put` makes its own 128 MiB disk, `build/llvm-disk.img` | Growing the usual run disk: everyone's would become 128 MiB for three programs few people run |
| RISC-V's `.td` files | Imported (49 files, no C++) | Patching TargetParser: every LLVM program has it, and it is generated from `RISCV.td` whichever targets are built |

The three patches to musl:
- `acosh.c` returns NaN for every x below 1. musl's could return a number for x below -2.
- The four `*_data.h` headers include TheTaaJ's `libm.h` instead of `<features.h>`.
- `fma.c` includes `libm.h` instead of `atomic.h`, which only `a_clz_64` needed. TheTaaJ's version of it is plain C that TCC can compile.

## Bugs

- **llvm-tblgen lost its `main`.** The new CMake reader treated every word after `DESTINATION` and `EXPORT` as a source file. Those two keywords take one word and then go back to the source list. The real source files came after them, so `main.cpp` was being read as a destination.
- **"ScalarOpts" was an unknown library.** A library can be named by its component name (`ScalarOpts`) or its library name (`Scalar`). Both are registered now.
- **`-combiners="..."`**: quotes inside a word reached `llvm-tblgen` literally. They are removed now.
- **TargetParser needs `RISCV.td`** even when RISC-V is not built, so RISC-V's `.td` files are imported.
- **`llc` linked with undefined `sin`, `exp`, `pow`...** This is why libm came in. My first try compiled all of musl's files as one big file, and their private constants collided: each file has its own `static const` tables with names it shares with other files. Each file is compiled on its own now, as musl does.
- **musl's `acosh(-730000)` was 2.29, not NaN.** My test found it by comparing with glibc. musl leaves the domain check to the `log` it calls, but for x below -2 what it hands to `log` is 2x + 2|x|. That should be zero, but it can round to a small positive number, so `log` returns a number. My patch checks `x < 1` first, as glibc does.
- **My test blamed musl for glibc's `cbrt`.** glibc's double `cbrt` is up to 2 ulps off. The reference is now glibc's `long double` result rounded to double. The hyperbolic functions and their inverses are allowed 2 ulps, as musl's own comments say.
- **`fma` was undefined in AArch64's `llc`.** Only the AArch64 link asked for it. I added musl's exact software `fma`, tested against glibc's (both exact, so they must agree to the bit).
- **`llc` did not start: "does not fit in memory".** The memory-object limit was 16 MiB, which had been plenty until now.
- **The 4 MiB file limit.** `llc` could not be put on a disk at all. That led to TaajFS version 3.
- **lld wrote `PT_GNU_STACK` with size 0**, so the kernel's new stack rule would have given LLVM's programs 64 KiB anyway. `build-llvm.py` now writes the size into the header itself.
- **The file service was stopped while it read llc.** The first `READ_INTO` read everything in one go. Reading 34 MB took more than init's 4 s, so init stopped the service as hung and `llc` never started. Reads are now 4 MiB at a time.
- **The AArch64 test could not type `=`.** Its serial typist knew only some punctuation keys. It now knows every key `key-names.py` can name.
- **AArch64's TCC test ran out of time inside `make test64`.** The libm lines made `/src/build` longer: on the PC, with every other test booting beside it, the build took 640 s. The test waits for the build in 90-second steps and allowed six of them. AArch64's build was stopped while it compiled `clib.c`, its last program. A step returns as soon as the command is over, so the test now allows twelve for `/src/build` and eight for TCC building itself, and a spare step costs nothing.
- **The TCC test looked for `/src/build line 19`.** libm's lines moved the C library's own line down to 30. It now looks for the line's text, whatever its number.

## How I checked it

Automated, by booting: `llvm64-test` and `aarch64-llvm-test` (in `make test64`) now make a 128 MiB disk with all three tools. The host's clang 18 turns `libc/programs/cwords.c` into LLVM's language for each machine. The host's `llc` and `llvm-mc` (18.1.3, checked by the test) write the reference files. Then TheTaaJ runs:

| Step | Writes | PC, alone | AArch64, alone |
|---|---|---|---|
| `llvm-tblgen`: three jobs, as in E1 | `vt.inc`, `enums.inc`, `impl.inc` | 0.86 s, 6.43 s, 23.93 s | 1.15 s, 15.18 s, 39.42 s |
| `llc -O2 x86_64.ll -o x86_64.s` | 15,906 bytes of PC assembly | 5.91 s | 8.53 s |
| `llc -O2 -filetype=obj aarch64.ll -o aarch64.o` | a 6,712-byte AArch64 object | 6.49 s | 8.94 s |
| `llvm-mc ... x86_64.s -o x86_64.o` | a 5,848-byte PC object, from llc's own output | 1.76 s | 2.84 s |
| `llvm-mc ... aarch64.s -o mc-aarch64.o` | 6,712 bytes, from the host llc's AArch64 assembly | 1.76 s | 3.05 s |
| `llc bad.ll`, `llvm-mc bad.s` | the error, with the file, line and column; exit code 1 | | |

All seven output files are compared byte for byte with the host's, and are identical on both machines. The tblgen times include loading `llvm-tblgen`, which is now quick: its first job went from 1.52 s to 0.86 s.

On the host:
- `make libc-test` runs `libm_test.c`: 45,602,992 checks of the 22 double functions and `fma` against glibc, with 43,614,069 exactly the nearest double and 0 too far off. The worst cases: 1 ulp for most functions, 2 for sinh, tanh, asinh, acosh and atanh. `fma`: 1,000,000 checks, all equal to glibc's to the bit.
- TaajFS: 51 tests, 6 of them new (a big file over a remount, growing a file into a big one and changing its middle, deleting one returns its index blocks too, both slots raised to version 3, a damaged big inode refused at mount, and a power cut in the middle of putting a big file leaving the old state or the new).
- The file server: 32 tests, 2 of them for `READ_INTO` (the window and the 4 MiB limit).
- The kernel's crate: 190 tests, including the stack a program asks for and the limit on how many headers are read.

`tcc64-test` and `aarch64-tcc-test` compile the 39 libm files with TCC on TheTaaJ, byte for byte as on the host. `cxx64-test` still passes.

Mutation experiments, 7:
- M1, musl's `acosh` without my patch: `libm: acosh(0x0p+0) = 0x0p+0, the nearest double is -nan`. Caught.
- M2, `a_clz_64` without its 16-bit step: `libm: fma(0x1.39e8916f106dcp+32, ...) = 0x1.ba7c617c8bd58p+57, not 0x1.ba7c617c8bd53p+57`. Caught.
- M3, `a_clz_64` without its last, 1-bit step: not caught. This mutation is harmless. `fma` shifts by the count minus one and lowers the exponent by the same amount, so a count one short only means one bit less of headroom in a 64-bit number that has plenty. Its result is the same.
- M4, `READ_INTO` without its 4 MiB limit: `one_read_into_reads_no_more_than_its_limit` failed (4294304, not 4194304). Caught. Without the limit, the boot shows what happens: init stops the file service while it reads `llc`.
- M5, the free map not marking a big file's index blocks: `deleting_a_big_file_returns_every_block_index_blocks_too` failed. Caught.
- M6, the version not raised before the first big file: four TaajFS tests failed, `the_first_big_file_raises_both_slots_to_version_3` among them. Caught.
- M7, the `PT_GNU_STACK` size ignored: `a_program_gets_the_stack_it_asks_for` failed. Caught.

The testing note's steps were run as scripted boots on fresh disks made by `make llvm-put` and `make llvm-put-aarch64`, on both machines. On each, the four files written were compared with the host's and were identical. Clippy is clean for every crate that changed: `user` and `libc` on both CPUs, the kernel for both and for host tests, `abi`, `taajfs`, `fileserver` and `tfstool-rs`. `make test64` passes.

Not tested:
- A person typing.
- Bigger inputs than `cwords.c`: one C file of 200 lines, about 25 KB of LLVM's language.
- The C implementation of TaajFS (`servers/tfs`) with a version 3 disk. It refuses the disk by its version number, which is the point of raising the version, but I did not boot it against one.

## What is still missing

- **Clang itself**: E3, `clang -c` on TheTaaJ. Then lld, E4.
- **A program does not know its own name.** `argv[0]` is "program", so `llc`'s errors begin `program: error: program:`. Clang's driver needs its real name, to find itself and its headers.
- **The C TaajFS cannot read version 3 disks.**
- **`erf`, `erfc`, `tgamma`, `lgamma` and every `long double` function** are declared but not there.
- **The kernel's limits are now 128 MiB per program and per memory object**, in a 512 MB machine. Clang will be bigger than `llc`, so it will come closer to them.
- **Index blocks hold 1,024 blocks each, one level deep**, so files can be at most 256 MiB.

## What I learned

- A file's size limit, a message's size, a heap's growth, a watchdog's patience: each was a sensible choice for small programs. A 34 MB program ran into all of them, one after another. Each limit was fine; the trouble was that each one was written down somewhere I had to find.
- A good reference matters as much as a good test. My first libm test compared against glibc's doubles and blamed musl for glibc's errors. Comparing against higher precision fixed that.
- Raising a format's version before using it is what keeps old readers safe. Nothing in the old code can be changed, so the only defence it has is to refuse what it doesn't know. That only works if the version goes up first.
- A mutation that survives is not always a gap in the tests. M3's change cannot change any result, and working out why taught me how musl's `fma` keeps its rounding exact.
- Comparing byte for byte with the host was again the test that mattered. One wrong bit anywhere in the C library, libc++, libm, the kernel's loader or the file system would have shown up as a different object file.

## Testing note

**1. The PC: llc and llvm-mc**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

The first `make llvm-put` builds LLVM's code generators for the PC: about 1,500 files, which takes a long time. It makes a new 128 MiB run disk, `build/llvm-disk.img`, with everything the usual run disk has, plus:
- `llvm-tblgen`, `llc`, `llvm-mc`;
- `cwords.c` in LLVM's language for each machine (`x86_64.ll`, `aarch64.ll`) and in AArch64 assembly (`aarch64.s`);
- a broken `bad.ll` and `bad.s`.

Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly llc --version` | `TheTaaJ LLVM version 18.1.3`, `Default target: x86_64-unknown-taaj`, and under `Registered Targets:` `aarch64`, `arm64`, `x86` and `x86-64` among others |
| `time with files llc -O2 x86_64.ll -o x86_64.s` | `took` about 6 s |
| `time with files llvm-mc -triple=x86_64-unknown-none-elf -filetype=obj x86_64.s -o x86_64.o` | `took` about 2 s |
| `time with files llc -O2 -filetype=obj aarch64.ll -o aarch64.o` | `took` about 7 s: AArch64 code, made on the PC |
| `with files llvm-mc -triple=aarch64-unknown-none-elf -filetype=obj aarch64.s -o mc-aarch64.o` | nothing printed |
| `with files llc bad.ll -o bad.out` | `program: error: program: bad.ll:2:7: error: value doesn't match function result type 'i32'`, the line `  ret i64 0` with a `^` under `i64`, then `llc exited with code 1` |
| `with files llvm-mc -triple=x86_64-unknown-none-elf -filetype=obj bad.s -o bad.o` | `bad.s:2:2: error: invalid instruction mnemonic 'notaninstruction'`, the line with `^~~~~~~~~~~~~~~~` under it, then `llvm-mc exited with code 1` |
| `ls` | `x86_64.s` 15906, `x86_64.o` 5848, `aarch64.o` 6712, `mc-aarch64.o` 6712, `llc` 34185400, `llvm-mc` 11067720 |

**2. The same files from the host's LLVM**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img aarch64.o > /tmp/taaj-aarch64.o
/usr/lib/llvm-18/bin/llc -O2 -filetype=obj build/llvm64.log.work/aarch64.ll -o /tmp/host-aarch64.o
cmp /tmp/taaj-aarch64.o /tmp/host-aarch64.o && echo same
```

| Look for | Meaning |
|---|---|
| `same` | the object TheTaaJ wrote is the host's, byte for byte (`build/llvm64.log.work` is there once `make llvm64-test` has run) |

**3. AArch64**

Close any AArch64 QEMU first.

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1, typed at the terminal. `llc --version` says `Default target: aarch64-unknown-taaj`. The two `llc` steps take about 9 s each, and the `llvm-mc` steps about 3 s. `llc` is 28930096 bytes and `llvm-mc` 10395896. The four files it writes are the same sizes as on the PC, and the same bytes.

**4. The tests by themselves**

```
make libc-test
make llvm64-test
make aarch64-llvm-test
```

| Look for | Meaning |
|---|---|
| `libm: 45602992 checks against this machine's long double, 43614069 the nearest double, 0 too far off` | musl's functions on the host, against glibc's |
| `llc and llvm-mc on TheTaaJ (x86_64) wrote, byte for byte as the host's: x86_64.s (15906 bytes) aarch64.o (6712 bytes) x86_64.o (5848 bytes) mc-aarch64.o (6712 bytes)` | llc and llvm-mc's outputs equal to the host's (`aarch64` for the other machine) |
| `llvm64-test passed`, `aarch64-llvm-test passed` | each machine |

Automated: make test64
