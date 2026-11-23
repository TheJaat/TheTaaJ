# lld on TheTaaJ: TheTaaJ builds its C programs

*Journal entry #76 · Era 6, step E4. lld, LLVM's linker, runs on TheTaaJ as `ld.lld`. With clang from E3, TheTaaJ now compiles and links its own eleven C programs, with the options its build uses, and the programs it makes are byte for byte the ones the build makes on the host. One of them, linked there, runs there. Both machines.*

## Why

E3 left TheTaaJ with a compiler and no linker: the objects clang wrote there still had to be linked by TCC or on the host. lld is the linker TheTaaJ's build uses for every program, so with it on TheTaaJ, the whole of building a C program happens there, with the same tools as on the host.

## How it fits together

```
 TheTaaJ:  clang @cflags -c @sources                 eleven objects (E3)
           ld.lld @ldflags chello.o lib/libc.a -o bin/chello      x 11
           bin/chello one two                         "Hello from C", ...

 host:     cmp bin/chello build/c/chello              identical, all eleven
```

`@ldflags` is the Makefile's `TAAJ_LDFLAGS`: `-static -nostdlib --gc-sections -s -T user.ld` (and `-z max-page-size=4096` on AArch64). The linker script throws `.comment` away, so a program does not say which linker or compiler made it: what TheTaaJ links is the build's program exactly.

What it took:

**lld's sources, in `ports/llvm/lld`.**
- From the same 18.1.3 release tarball, unchanged, in their own commit: `include/`, `Common/`, `ELF/`, `tools/lld`, the licence. 100 files, 1.9 MB.
- Only the ELF linker. lld also links for Windows (COFF, MinGW), Apple (MachO) and WebAssembly. TheTaaJ has ELF files only, as only two of LLVM's targets were taken.
- TheTaaJ's changes, in a commit of their own: `lld.cpp` offers only the ELF linker on TheTaaJ, and `Writer.cpp` seeds `--shuffle-sections=...=0` from the clock, since there is no `std::random_device`.

**`build-llvm.py` builds lld.**
- lld's own way of declaring libraries, `add_lld_library` (with `LINK_COMPONENTS` inside it), `add_lld_tool` and `lld_target_link_libraries`. The four linkers not imported are left out by name.
- `Version.inc` and an empty `VCSVersion.inc`, and `-DLLD_VENDOR="TheTaaJ"`: `ld.lld --version` says `TheTaaJ LLD 18.1.3 (compatible with GNU linkers)`.
- The same generated `main` as clang's.
- Clang and lld each write an `Options.inc` (the driver's options, the linker's). The script searched every generated directory in every compile, so lld found Clang's. Now Clang's and lld's generated directories are searched only by their own files; Clang names its own by path anyway (`clang/Driver/Options.inc`).
- An object is now compiled again when its command changes, not only its sources. Each object keeps the command it was compiled with, beside it, in a `.cmd` file. Before, a new `-D` changed nothing until a source was touched (I found this when a mutation did not take).

**`__clear_cache`, in the C library.**
- On AArch64, lld pulls in LLVM's `Memory::protectMappedMemory`, which calls `__clear_cache`: a compiler-runtime function that makes freshly written code safe to run. TheTaaJ had none.
- On TheTaaJ a program cannot make memory executable at all (`mmap` refuses `PROT_EXEC`), so code a program writes never runs, and there is nothing to flush. `__clear_cache` does nothing, with that reason written beside it (`libc/c/mmap.c`).

**The name matters again.** lld decides which linker to be from the name it is run by: `ld.lld` is the ELF one, `lld` alone asks which. E3's `argv[0]` is what makes `ld.lld` work.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Which of lld | The ELF linker only | All five: three of them need more of LLVM (COFF's Windows libraries, MachO's libunwind headers), and TheTaaJ has no use for them |
| How the program is named | `/ld.lld` on the disk, chosen by its name as on the host | `/lld -flavor gnu`: works, but every command would carry it, and the build calls `ld.lld` |
| `__clear_cache` | A no-op, because a program cannot make memory executable | Real cache maintenance (`dc cvau`, `ic ivau`): it needs the kernel to let EL0 do it, for memory that can never hold code |
| What the test compares | The linked programs with the build's own (`build/c/*`) | With programs the host's ld.lld links in the test: the build's are the ones that ship, and comparing with them says TheTaaJ makes TheTaaJ's programs |
| Where the sources are on the test disk | `/libc/programs/*.c`, the names the build compiles them by | `/src/*.c`: `clib.c` uses `assert`, which writes the file's name into the program. With `src/clib.c` it was the one program of eleven that differed |
| Rebuilding when a command changes | A `.cmd` file per object; an object with none keeps its build | Rebuilding everything once to write them: twenty minutes for nothing |

## Bugs

- **`use of undeclared identifier 'OPT_reproduce'`**, and fifty more: lld's ELF driver found Clang's `Options.inc`. See above.
- **`no member named 'random_device' in namespace 'std'`** in `Writer.cpp`.
- **`undefined symbol: __clear_cache`**, on AArch64 only.
- **A mutation that did not take**: changing `-DLLD_VENDOR` in the script left every object as it was, and the test passed. The `.cmd` files fix it.
- **`blk has not answered for 2 s - stopping it`**, in three boots of the first full `make test64` (`aarch64-clang-test`, `tcc64-test`, `aarch64-tcc-test`), and the step that was reading the disk then failed. Each passed alone. The suite runs no more emulated CPUs at once than the host has CPUs (`boot-slots.sh`), but QEMU does a disk's work in a thread of its own, which the slots did not count, and these boots read tens of megabytes of programs. They now ask for three slots for their two CPUs (`SLOTS=3`, which `qemu-boot.sh` now takes as `aarch64-boot.sh` did). The next `make test64` passed.
- Two of the test's own: `chello` prints its arguments from `argv[1]`, so it cannot show `argv[0]`; and a link with `--gc-sections` and no `_start` keeps nothing, so nothing is missing and it only warns. The failing link is now one without the build's options.

## How I checked it

Automated, by booting: `clang64-test` and `aarch64-clang-test` (in `make test64`) now also put `ld.lld`, `libc.a`, `user.ld` and `ldflags` on the disk. After E3's steps, TheTaaJ runs:

| Step | Shows |
|---|---|
| `ld.lld --version` | `TheTaaJ LLD 18.1.3 (compatible with GNU linkers)` |
| `ld.lld @ldflags NAME.o lib/libc.a -o bin/NAME`, for each of the eleven | exit code 0, eleven programs |
| `bin/chello one two` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two`, exit code 0 |
| `ld.lld cwords.o -o bin/x` | `ld.lld: error: undefined symbol: ...`, exit code 1 |

All eleven programs are compared byte for byte with the build's own (`build/c/`, `build/c-aarch64/`), and are identical on both machines. The compile of the eleven took 59.47 s on the PC and 102.94 s on AArch64, alone.

A build from nothing, in a directory of its own, wrote the same five programs (`llvm-tblgen`, `llc`, `llvm-mc`, `clang`, `lld`) byte for byte as the main build, which was built up step by step since E1. That checks the change to which directories each compile searches, for every one of the 2,179 files of LLVM, Clang and lld.

Mutation experiments, 3:
- M1, no `-DLLD_VENDOR`: at first not caught, because nothing was compiled again (the bug above). With the `.cmd` files, the 54 files of lld were compiled again: `ld.lld did not say its version`. Caught; and when the script was put back, the same 54 were compiled again.
- M2, `lld.cpp`'s ELF linker listed under another flavor: `ld.lld did not say its version`. Caught.
- M3, init does not send the program's name: caught by E3's first check (`clang did not find its headers`); without its name lld does not know which linker to be either.

Not tested:
- A person typing.
- Linking C++ on TheTaaJ (libc++.a is not on the disk, and clang cannot read libc++'s headers there yet).
- `clang x.c -o x`, compiling and linking in one: clang would have to start ld.lld, and a C program cannot start another yet.

## What is still missing

- **lld is slow on AArch64, and I do not know why.** On the PC a link takes 7.5 s, and `ld.lld --version` alone takes 7.6 s: nearly all of it is loading the 46 MB program, at about 6 MB/s (clang's 82 MB take 13 s the same way). Once, the first link of a boot took 30.8 s; I could not make it happen again. On AArch64 `--version` takes 11.7 s, but linking `chello` took 59 s in one boot and 108 to 141 s in another, with or without the build's options, and the CPUs were busy for most of it, so it is not waiting for the disk. clang on AArch64 is only about twice as slow as on the PC. The output is right every time. Finding where the time goes needs a way to profile a program on TheTaaJ, which there is not yet.

- **A C program cannot start another.** So `clang x.c -o x` does not work; compile with `-c` and link with `ld.lld`. The same gap keeps `make` from running on TheTaaJ.
- **TaajFS names longer than 27 bytes** (E3).
- **Era 6's milestone, "TheTaaJ builds TheTaaJ"**: TheTaaJ builds its C programs (now with clang and lld too) and its C library (with TCC, entry 070). The kernel, the Rust programs, LLVM itself: not yet. Rust has no compiler on TheTaaJ.

## What I learned

- Every one of lld's three surprises came from something next to it: Clang's file of the same name, a C++ feature this libc++ lacks, a runtime function no one had needed. The linker itself worked first time.
- A build that can't see a change of options gives tests a false pass. The mutation showed it, not a failing build: a mutation that survives for no reason is worth chasing.
- Comparing with what ships, not with a reference made for the test, is the stronger claim, and it found the one real difference: a file name inside `assert`.

## Testing note

**1. The PC: compile, link, run**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

`make llvm-put` now makes a 256 MiB disk with E3's files and `ld.lld`, `lib/libc.a`, `user.ld` and `ldflags` (the build's linker options); the C programs are in `libc/programs`. Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly ld.lld --version` | `TheTaaJ LLD 18.1.3 (compatible with GNU linkers)` |
| `time with files clang @cflags -c libc/programs/chello.c` | `took` about 17 s |
| `time with files ld.lld @ldflags chello.o lib/libc.a -o chello2` | `took` about 8 s (once 30 s: see "What is still missing"), nothing else printed |
| `chello2 one two` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two`, then the lines `chello` always prints |
| `with files ld.lld chello.o -o x` | `ld.lld: error: undefined symbol: write` (and others), each with `>>> referenced by chello.c`, then `ld.lld exited with code 1` |
| `ls` | `chello2` 62456, the same as `chello` |

**2. The same program as the build's**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img chello2 > /tmp/chello2
cmp /tmp/chello2 build/c/chello && echo same
```

| Look for | Meaning |
|---|---|
| `same` | the program TheTaaJ compiled and linked is the one the build made |

**3. AArch64**

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1, but slow: the `clang` step takes about 30 s and the `ld.lld` step a minute or more (see "What is still missing"). `chello2` is 66568 bytes. Compare with `build/c-aarch64/chello`.

**4. The tests by themselves**

```
make clang64-test
make aarch64-clang-test
```

| Look for | Meaning |
|---|---|
| `ld.lld on TheTaaJ (x86_64) linked, byte for byte as the build's own programs: chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib` | all eleven programs equal to the build's (`aarch64` likewise) |
| `clang64-test passed`, `aarch64-clang-test passed` | each machine |

Automated: make test64
