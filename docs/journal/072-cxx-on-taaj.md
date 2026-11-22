# C++ on TheTaaJ

*Journal entry #72 · Era 6, step D. C++ programs run on TheTaaJ, on both machines. They are cross-compiled by clang and linked with LLVM's own C++ runtimes: libunwind, libc++abi and libc++, release 18.1.3, imported into `ports/llvm` as they are upstream with two small changes. Exceptions, RTTI, global constructors and destructors, the containers, iostream, `<format>` and `<regex>` all work. To get there the C library grew most of what a hosted C library has: math.h, sscanf, locale.h, strftime, atexit, aligned_alloc and clock_gettime.*

```
taaj:/> cxxhello pear apple pear fig
Hello from C++
4 words, 3 different, in order: apple fig pear pear
pear 2, apple 1, fig 1
the longest: apple (5 letters)
taaj:/> cxxlib
c++: 56 checks, all passed
c++: the objects outside main were destroyed
taaj:/> cxxlib throw
libc++abi: terminating due to uncaught exception of type std::runtime_error: nobody catches this
cxxlib exited with code 134
```

## Why

Era 6 is "TheTaaJ builds TheTaaJ". The C half of that is done: TCC builds the C library and itself (entries 070 and 071). The rest of TheTaaJ is Rust, and the Rust compiler is built on LLVM, which is C++. So does Clang, which comes next. Before either can be compiled *for* TheTaaJ, a C++ program has to be able to *run* on it, and that needs three things C did not:

- **Unwinding.** A `throw` walks back up the stack, frame by frame, to the `catch`, running destructors on the way. It needs tables the compiler writes for every function (`.eh_frame`) and code that reads them (libunwind).
- **The language's runtime.** `throw` and `catch`, `typeid` and `dynamic_cast`, `new` and `delete`, and the guard on a function's `static` locals are library calls, not instructions (libc++abi).
- **The standard library** (libc++), which is mostly templates in headers. Its compiled part asks a lot of the C library underneath.

I chose to do this before Clang itself: Clang is a million lines of C++, and running it would be the first test of everything above at once. A small program that checks each part in turn tells me which part is broken.

## How it fits together

```
  libc/programs/cxxlib.cpp ──clang++──► cxxlib.o
                                           │
  ports/llvm ──tools/build-cxx.sh──► libc++.a (libunwind + libc++abi + libc++, 58 objects, 2.0 MB)
                                           │
  libc/ (Rust + C)  ──────────────► libc.a │
                                           ▼
                     ld.lld --eh-frame-hdr -T user/cxx.ld ──► cxxlib (750 KB)
```

- **`ports/llvm`** holds libcxx, libcxxabi and libunwind from the 18.1.3 release, only `include/`, `src/` and the licences (13 MB). That is the release of the host's clang: libc++'s headers expect a compiler of their own release or newer. The import is one commit, exactly as upstream. TheTaaJ's changes come in the next commit, so `git diff` shows them alone: a `__config_site` (the configuration CMake would write) and two lines in libc++. One line picks the "no locales" fallbacks for `__TAAJ__`; the other says TheTaaJ has `clock_gettime`.
- **`tools/build-cxx.sh`** compiles the three libraries into one `libc++.a`, using the source lists from upstream's CMakeLists.txt. I did not use CMake: a bare-metal target with a C library CMake has never seen means fighting its compiler checks, and the lists are short. Left out: threads, `<filesystem>`, `random_device`, wide characters, time zones and modules. libunwind is built `_LIBUNWIND_IS_BAREMETAL`: there is no dynamic loader to ask where the tables are, so it reads them between four symbols that the linker script defines.
- **`user/cxx.ld`** is `user.ld` with `.init_array`, `.fini_array`, `.eh_frame` and `.eh_frame_hdr` kept, and those symbols around them. `user.ld` throws the unwinding tables away; a C program never needs them.
- **Before and after `main`.** `_start` in the Rust half now calls `__taaj_run_constructors` (in the new `atexit.c`), which runs every function in `.init_array`. That is how a global `std::string`, or `std::cout` itself, gets built before `main`. `exit` runs the `atexit` and `__cxa_atexit` handlers newest first, then `.fini_array` in reverse, and only then flushes the C streams, so a destructor's last `printf` still comes out.

What libc++ needed from the C library, found by compiling it until it stopped complaining:

| Area | Added |
|---|---|
| math.h | the full C99 list of declarations, `FP_*` and the classifying macros, `M_*`; `math.c` with a correctly rounded bit-by-bit `sqrt`, `fmod`, `remainder`, `floor`/`ceil`/`trunc`/`round`/`rint`, `hypot`, `fmin`/`fmax` and more |
| stdio.h | `sscanf`, `vsscanf` (new `scan.c`); `asprintf`, `vasprintf` |
| stdlib.h | `div`/`ldiv`/`lldiv`, `llabs`, `atoll`, `aligned_alloc`, `atexit`, `MB_CUR_MAX` |
| time.h | `mktime`, `difftime`, a full C99 `strftime` (ISO weeks too), `asctime`, `ctime`, `clock`, `clock_gettime`, `timespec_get` |
| new headers | `locale.h` and `langinfo.h` (the "C" locale only), `wchar.h` and `uchar.h` (types only), `bits/types/mbstate_t.h` |
| the rest | `strcoll`, `strxfrm`, `isascii`, `toascii`, 44 more `errno` codes (Linux's numbers), `extern "C"` round every header, `[[noreturn]]` for C++ |

`malloc` changed shape to support `aligned_alloc`, which `new` of an over-aligned type calls. A block now starts with a header as big as its alignment, with the size at `p - 16` and the alignment at `p - 8`, so that `free` can find where the heap's block really begins.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Which C++ library | LLVM's: libc++, libc++abi, libunwind | GNU's libstdc++: it is built with GCC's own build system against its own runtime, and TheTaaJ's toolchain is clang. A small hand-written C++ runtime: exceptions alone are a large job, and Clang will need the real library anyway |
| Version | 18.1.3, the host clang's | The newest release: libc++'s headers would then use builtins this clang does not have |
| Building it | A shell script with upstream's file lists | CMake: its checks of the compiler and C library expect a known system; teaching it TheTaaJ is more work than the 58 files |
| Changing upstream | `__config_site` plus two `__TAAJ__` lines, in a commit after the plain import | Patching freely: the next LLVM update would then mean finding my changes by hand |
| Locales | libc++'s "no locale" fallbacks; the C library has only "C" | Real locales: nothing on TheTaaJ needs a language other than the one it is written in |
| Where tables are found | `_LIBUNWIND_IS_BAREMETAL` and symbols from the linker script | `dl_iterate_phdr`: there is no dynamic loader, and a static program knows where its own tables are |
| new/delete | libc++abi's (libc++'s `new.cpp` left out) | Both: the same symbols twice. Hand-written ones: libc++abi's already throw `bad_alloc` and handle alignment |
| Order at exit | handlers, then `.fini_array`, then flush | Flush first: a destructor's output would be lost |
| `strtold` | Still through a double (see Bugs) | An exact one now: it is a parser for two more formats (x87's 80 bits, AArch64's 128), and nothing needs it yet |

## Bugs

- **Four functions in `math.c` were wrong**, all caught by the host test that compares every function with glibc's over 150 million inputs:
  - `fmin` and `fmax` of `+0` and `-0` returned a different zero from glibc's, which returns the first argument on a tie; so does mine now.
  - `hypot` overflowed for sides near 1e300 when it squared them: it now scales above 1e150 and below 1e-150.
  - `sqrt` put its exponent one off.
- **`__udivti3` undefined at link.** libc++ divides 128-bit numbers, and `thin-archive.py`, which keeps only the members of Rust's compiler_builtins that something uses, had already dropped them. They are now in its "always keep" list.
- **A test that tested nothing.** `cxxlib` checks that `new` of an impossible size throws `bad_alloc`. Clang saw a `new` whose result was never used and removed the pair, so nothing was ever asked of the heap. The pointer now goes into a `volatile` global, so the allocation has to happen.
- **`uchar.h` including `wchar.h`** reached libc++'s own `wchar.h`, which refuses to work without wide characters. `mbstate_t` now lives in `bits/types/mbstate_t.h`, and both include that.
- **`__assertion_handler` missing.** It comes from `libcxx/vendor`, which my first import left out. The import commit now includes it.
- **The TCC test broke**, and that was a real finding. `clib` gained `sscanf("…1e300…", "%Lf")` and compared with `1e300L`. tcc parses `1e300L` with the C library's `strtold`, which goes through a double. So tcc on TheTaaJ wrote a different constant from tcc on the host, which uses glibc's exact `strtold`, and `clib.o` differed in two bytes. I changed the check to a number that is exact in both (2^-20). The inexact `strtold` is now an open issue: a C program built on TheTaaJ can differ from the same program built elsewhere in the last bits of a long double literal.
- **`cxxhello` said "1 words".** Found by the testing note's boot; fixed.

## How I checked it

Automated, by booting: `cxx64-test` and `aarch64-cxx-test` (in `make test64`). Each makes a 4 MB disk and runs:

- `cxxlib`, with 56 checks in five groups:
  - before and after `main`: global objects made before it, in order, and a C constructor too;
  - exceptions: through frames with destructors, `throw;`, a half-made object's members destroyed, `exception_ptr`, what the library throws (`vector::at`, `stoi`), `bad_alloc`;
  - types at run time: virtual calls, `dynamic_cast` down and across (to a reference too, which throws), `typeid`;
  - memory: over-aligned `new` and `delete`, `shared_ptr` and `weak_ptr`, a function-local static made once;
  - the library: strings, `to_chars`/`from_chars`, `<format>`, containers, algorithms, `mt19937`, string streams, `optional`/`variant`/`any`, `std::function`, `<regex>`, `steady_clock`.

  Then the destructor of a global object prints its last words after `main`.
- `cxxlib throw`: nobody catches it. libc++abi's `std::terminate` names the type and the message, and the exit code is 134.
- `cxxhello` with four words (sorted, counted) and with a 28-letter word (an exception thrown and caught).

The same `cxxlib.cpp` compiles on the host against libstdc++ and passes there too (56 checks). The checks test C++, not my library's quirks.

On the host, `make libc-test` adds two tests that compare with glibc. `math-test` makes 150,135,396 checks and finds 0 different. `scan-test` makes 200,461 checks, all the same as glibc's `sscanf` (except `%3c` on a short string, where glibc does not follow C; the test says so). The boot test of the C programs now also expects `clib`'s `library: atexit ran its handlers, newest first`, and so does `tcc-test.sh` for both of its `clib` runs. `tcc64-test` (42 files byte for byte) and `aarch64-tcc-test` (40) pass with the larger C library: 15 C objects now, up from 11.

Mutation experiments, 5:
- M1, `_start` without `__taaj_run_constructors`: `cxxlib: FAILED: objects outside main made before it, in order`, then a page fault, because libc++'s own `std::cout` was never built either. Caught.
- M2, linked without `--eh-frame-hdr`: **survived**, and rightly. libunwind uses the index when there is one; when it is empty, it searches `.eh_frame` from the start. The flag makes a throw faster, not possible. I corrected the comment in `cxx.ld`, which had implied it was needed.
- M2b, `.eh_frame` discarded by the linker script: the first throw finds no handler: `libc++abi: terminating due to uncaught exception of type Failure: from the bottom`. Caught.
- M3, the 128-bit helpers out of `thin-archive.py`'s list: `ld.lld: error: undefined symbol: __udivti3`. Caught at link.
- M4, `free` ignoring the stored alignment (always 16): at first it **survived**. Freeing at `p - 16` instead of `p - 64` gives the heap a smaller piece of the same block, so nothing overlaps and nothing crashes; 48 bytes are just lost each time. I added a check that a 64 KiB block aligned to 4096 is given back whole, by asking for the same again and getting the same address. Then M4 gave `cxxlib: FAILED: delete of an over-aligned block gives all of it back`. Caught.

The testing note's steps were run as scripted boots on fresh scratch run disks on both machines, including making `docs` and `welcome` for `clib`. Clippy is clean for `libc` and `user` on both CPUs. `make test64` passes.

Not tested:
- A person typing.
- Adding the C++ programs to a run disk made before this step (a fresh one has room; an old one that is nearly full might not).
- Anything in libc++ that `cxxlib` does not call. The library is large, and 56 checks touch a small part of it.

## What is still missing

- **Transcendental math:** `sin`, `exp`, `log`, `pow` and the rest are declared but not written. Nothing has needed them yet; Clang may.
- **libc++ without threads, `<filesystem>`, `random_device` or wide characters**, because the C library has none of these yet.
- **`strtold` is not exact** for long double (see Bugs).
- **`scanf` reads only from strings.** `scanf` and `fscanf` need a stream that can push back what it read too far.
- **C++ is cross-compiled only.** No C++ compiler runs on TheTaaJ yet; that is Clang, the next steps.
- Building `libc++.a` takes a few minutes and is redone whenever any header in `libc/include` changes.

## What I learned

- Most of "C++ support" is the C library. The C++ runtimes compiled almost unchanged; what took the time was the long list of things libc++ expects any C library to have.
- An exception is data, not control flow. The compiler writes a table for each function, and at run time a library reads the tables to find its way up the stack. Throw those tables away in the linker script and the code is all still there, but every exception ends in `std::terminate`.
- Global objects need help from the C library: something has to call their constructors before `main`, and their destructors from `exit`, in reverse order and before the streams are flushed.
- A test can pass because the compiler removed what it tested. When the compiler can prove a result is unused, the allocation is gone.
- Two survivors showed two different things. M2 showed I had misunderstood what a flag was for. M4 showed a quiet kind of bug that my checks could not see: a leak where nothing crashes. Only the second needed a new check.
- Comparing a self-hosted build byte for byte found a precision problem in `strtold` that no test of `strtold` itself had found.

## Testing note

**1. The PC: C++ programs**

```
make run64
```

The first `make run64` after this step builds `libc++.a` for the PC, which takes a few minutes.

| Type | You should see |
|---|---|
| `cxxhello pear apple pear fig` | `Hello from C++`, `4 words, 3 different, in order: apple fig pear pear`, `pear 2, apple 1, fig 1`, `the longest: apple (5 letters)` |
| `cxxhello` | `Hello from C++`, `give me some words: cxxhello pear apple pear fig` |
| `cxxhello antidisestablishmentarianism` | `1 word, 1 different, …`, `the longest: antidisestablishmentarianism (28 letters)`, then `caught: no word may be longer than 12 letters, and "antidisestablishmentarianism" is 28` |
| `cxxlib` | after a moment, `c++: 56 checks, all passed`, then `c++: the objects outside main were destroyed` |
| `cxxlib throw` | `libc++abi: terminating due to uncaught exception of type std::runtime_error: nobody catches this`, then `cxxlib exited with code 134` |

**2. The C library's new parts**

`clib` reads `welcome` and `docs/`, which the test disks have and the run disk does not; make them first (skip the first two lines if they are already there):

| Type | You should see |
|---|---|
| `mkdir docs` | nothing printed |
| `echo Hello from the host > welcome` | nothing printed |
| `with files clib` | `library: 70 checks, all passed`, then `library: atexit ran its handlers, newest first` |
| `ls` | `cxxhello` (about 427,000 bytes), `cxxlib` (about 751,000) and `clib` among the programs |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64
```

The same as parts 1 and 2, typed at the terminal. `cxxhello` is about 410,000 bytes and `cxxlib` about 714,000.

**4. The tests by themselves**

```
make libc-test
make cxx64-test
make aarch64-cxx-test
```

| Look for | Meaning |
|---|---|
| `math: 150135396 checks against this machine's, 0 failed` | math.c gives glibc's answers, bit for bit |
| `scan: 200461 checks, all the same as the host's sscanf` | sscanf reads what glibc's does |
| `C++ on TheTaaJ (x86_64): cxxlib's 56 checks passed, the destructors ran after main, an uncaught exception ended in std::terminate, cxxhello sorted, counted and caught` | all of part 1 on the PC (`aarch64` for the other) |
| `cxx64-test passed`, `aarch64-cxx-test passed` | each machine |

Automated: make test64
