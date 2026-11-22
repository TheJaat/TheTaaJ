# Writing a C program for TheTaaJ

How to write a program in C, build it on Linux, put it on TheTaaJ's disk and run it. The story of how the library came to be is in journal entries 036 to 043; this page is only the recipe.

## What you need

- `clang` and `lld` on the host (`sudo apt install clang lld`). GCC will not do: it has no target for a bare x86-64 machine without building a cross-compiler.
- TheTaaJ built once (`make run64` does it).

## The short way

Put the program in `libc/programs/`, one file:

```c
/* libc/programs/mine.c */
#include <stdio.h>

int main(int argc, char **argv)
{
    printf("hello from mine, with %d arguments\n", argc - 1);
    return 0;
}
```

Close QEMU if it is running (the disk is QEMU's while it runs), and:

```
make cput C=mine
make run64
```

and type `mine one two`. `make cput` builds `libc/programs/mine.c`, links it with the library, and puts the result on `build/taaj-disk.img`, the disk `make run64` uses.

The program stays on that disk. It is *not* added to the programs the tests use or to the disk a fresh checkout builds; for that, add its name to `C_PROGRAMS` in the Makefile.

## The long way: any files, anywhere

`make cput` is three commands. For a program of several files, or one that lives outside this repository, run them yourself from the root of TheTaaJ:

```
make libc64        # once, and after the library changes: makes build/c/libc.a

CFLAGS="--target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra \
        -ffreestanding -fno-stack-protector -fno-pic -mno-red-zone \
        -mgeneral-regs-only -nostdlibinc -isystem libc/include"

clang $CFLAGS -c ~/shout/main.c  -o /tmp/main.o
clang $CFLAGS -c ~/shout/shout.c -o /tmp/shout.o

ld.lld -static -nostdlib --gc-sections -s -T user/user.ld \
       /tmp/main.o /tmp/shout.o build/c/libc.a -o /tmp/shout

tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img shout /tmp/shout
```

Three things are needed from TheTaaJ, and only three:

| What | Where | What it is |
|---|---|---|
| The headers | `libc/include/` | what the library's functions are called and take |
| The library | `build/c/libc.a` | the functions themselves, and the code that runs before `main` |
| The linker script | `user/user.ld` | where in memory a TheTaaJ program is put |

### What the flags are for

| Flag | Why |
|---|---|
| `--target=x86_64-unknown-none-elf` | Code for an x86-64 machine with no operating system the compiler knows about. Without it Clang builds a Linux program. |
| `-ffreestanding` | Do not assume the host's C library: no "I know what `printf` does, I will call `puts` instead". |
| `-nostdlibinc -isystem libc/include` | Do not look in `/usr/include`; look in TheTaaJ's headers. Clang's own (`stddef.h`, `stdint.h`, `stdarg.h`, `stdbool.h`, `limits.h`) are still found. |
| `-mgeneral-regs-only` | Use no floating-point or vector registers: the kernel does not save them when it switches programs. `float` and `double` still work, in software. |
| `-mno-red-zone` | Do not use the memory just below the stack pointer without moving it. |
| `-fno-pic` | The program is linked for one fixed address; it does not need to work at any other. |
| `-fno-stack-protector` | The library has no stack-check function for it to call. |
| `ld.lld -static -nostdlib` | One file with everything in it, and no host libraries. |
| `--gc-sections -s` | Leave out what is not used, and the symbol names. |
| `-T user/user.ld` | Lay the program out as TheTaaJ loads it. |

The object files come before `libc.a` on the linker's command line: a library is searched for what the files before it still need.

## What a program is given

- **Its arguments**, as `argc` and `argv`, divided as the shell divided them (`mine "two words" three` is two arguments). **`argv[0]` is always `program`**: the shell does not yet tell a program its name.
- **Standard output** (`stdout`, descriptor 1): the screen, or a file (`mine > out`), or the next program (`mine | wc`).
- **Standard error** (`stderr`, descriptor 2): always the screen. Complaints go here, so that they are seen even when the output is going into a file.
- **Standard input** (`stdin`, descriptor 0): a file (`mine < notes`), the program before it (`cat notes | mine`), or — with neither — **the keyboard**. From the keyboard a program gets whole lines: the shell shows what is typed and lets Backspace correct it, and Enter sends the line. Ctrl+D at the start of a line ends the input (`fgets` returns `NULL`); Ctrl+C stops the program. A program started in the background (`mine &`) has no input at all. `isatty(0)` says whether the input is being typed.
- **Its exit code**: what `main` returns, or what `exit` is given. The shell says so when it is not 0.

### Files are not given unless asked for

A program started by name can open **no files at all**. `fopen` and `open` fail, and `strerror(errno)` says `this program was not given the files`. Whoever starts the program decides:

```
mine notes                  no files: fopen fails with EACCES
with readonly mine notes    may read every file, change none (writing fails with EROFS)
with files mine notes       may read and change files
```

A program that only reads standard input and writes standard output needs neither: `mine < notes > out` works, because the *shell* opened those files.

## What the library has

| Header | What is in it |
|---|---|
| `stdio.h` | `printf` `fprintf` `snprintf` `sprintf` and their `v` forms; `puts` `fputs` `putchar` `putc` `fputc`; `fopen` `fclose` `fflush` `setvbuf`; `fgetc` `getc` `getchar` `ungetc` `fgets`; `fread` `fwrite`; `fseek` `ftell` `rewind`; `feof` `ferror` `clearerr` `fileno`; `perror`; `remove` `rename`; `stdin` `stdout` `stderr` |
| `stdlib.h` | `malloc` `calloc` `realloc` `free`; `exit` `_Exit` `atexit`; `getenv` (always NULL: there is no environment); `rand` `srand`; `strtol` `strtoul` `atoi` `atol`; `abs` `labs`; `qsort` `bsearch` |
| `string.h` | `strlen` `strnlen` `strcmp` `strncmp` `strcpy` `strncpy` `strcat` `strncat` `strchr` `strrchr` `strstr` `strspn` `strcspn` `strdup` `strerror` `strerror_r` `strsignal`; `memcpy` `memmove` `memset` `memcmp` `memchr` |
| `ctype.h` | `isalpha` `isdigit` `isalnum` `isupper` `islower` `isxdigit` `isspace` `isblank` `isprint` `isgraph` `ispunct` `iscntrl`; `toupper` `tolower` |
| `errno.h` | `errno`, and the numbers Linux uses for the errors that can happen here |
| `unistd.h` | `read` `write` `close` `lseek` `unlink` `isatty` `_exit`; `dup` `dup2` `fsync`; `getcwd` `chdir` `rmdir` `access` `truncate` `ftruncate` `readlink`; `getpid` `sysconf` `getpagesize` `usleep`; `fork` `execv` `execve` (which fail with `ENOSYS`) |
| `fcntl.h` | `open` `fcntl`, and `O_RDONLY` `O_WRONLY` `O_RDWR` `O_CREAT` `O_EXCL` `O_TRUNC` `O_APPEND` and the rest of the `O_` flags |
| `sys/stat.h` | `stat` `fstat` `lstat` `mkdir` `chmod` `umask`: a file's kind, size and a number for it; no owners, permissions or times are kept |
| `dirent.h` | `opendir` `readdir` `rewinddir` `closedir` |
| `sys/mman.h` | `mmap` `munmap`: a file is read into memory, not mapped; changes are not written back |
| `signal.h` | `signal` `sigaction` `raise` `kill` `sigprocmask`: only signals a process sends itself |
| `pthread.h` | mutexes, condition variables, `pthread_once`, keys: a process has one thread, so `pthread_create` fails with `EAGAIN` |
| `time.h` | `time` `clock` `clock_gettime` `nanosleep` |
| `sys/types.h` | `ssize_t` `off_t` `pid_t` `uid_t` `mode_t` and the other POSIX types |

Each header says in its comments what its functions do and where they differ from other systems.

## What it does not have

Code that needs any of these will not compile or link:

- **`scanf` from a stream** (`scanf`, `fscanf`). `sscanf` reads from a string: read a line with `fgets`, then `sscanf` it.
- **Single keys.** The keyboard arrives a line at a time; a program cannot see a key as it is pressed, the arrow keys, or turn the showing of typed text off (no `termios`). An editor or a game cannot be written yet.
- **The transcendental functions** of `math.h`: `sin` `cos` `exp` `log` `pow` and their kind are declared but not there yet. `sqrt` `fmod` `floor` `ceil` `round` `trunc` `hypot` `fmin` `fmax` and the classifying macros are.
- **An exact `strtold`**: a long double is read through a double, so text with more digits than a double keeps is not rounded to the nearest long double.
- **Processes**: `fork`, `exec` and `wait` are declared but fail (`ENOSYS`, `ECHILD`); there is no `system` or `pipe`. Signals reach a process only from itself (`raise`, or `kill` of its own number).
- **Threads**: `pthread_create` fails with `EAGAIN`. The locks are there, and count, so code written for threads runs with one.
- **Links, owners, permissions and file times**: `link` and `symlink` fail with `EPERM`; `stat` says every time is 0 and every owner 0. `flock` and `fcntl`'s locks fail with `ENOLCK`.
- **Sleeping by seconds**: `nanosleep` and `usleep` are there, `sleep` is not. (`time`, `clock` and `clock_gettime` count from the moment the system started.)
- Wide characters (`wchar.h` has only `mbstate_t`), locales other than "C".

And things that are there but are not what a Unix program expects:

- `rename` onto a file that exists fails with `EEXIST`; it does not replace it.
- An open file follows its *name*: if another program renames or removes it meanwhile, the descriptor reads whatever has that name now, or fails.
- No single block of memory can be bigger than 16 MiB.
- `printf` and `write(1, …)` do not come out in the order they were called: `printf`'s text waits in a buffer (for the end of the line on the screen, for 4096 bytes otherwise). Use one or the other, or `fflush(stdout)` between.
- Text still in a buffer is lost if the program is stopped with Ctrl+C or ends with `_exit`.

## When it does not work

| What you see | What it means |
|---|---|
| `fatal error: 'something.h' file not found` | The library has no such header. See the two lists above. |
| `ld.lld: error: undefined symbol: something` | The header may exist, but the library has no such function — or `libc.a` is not last on the linker's line. |
| Arithmetic with `float` or `double` that is wrong now and then | Most likely a flag is missing: `-mgeneral-regs-only`. Without it the program uses registers the kernel does not save when it switches programs. (I have not made this happen; it is what the missing flag would cause.) |
| `unknown command: mine (try help)` in the OS | It is not on the disk: `tfstool ls build/taaj-disk.img`. |
| `tfstool put` fails or the file does not appear | QEMU was running. Close it, put the file again. |
| `this program was not given the files` | Start it `with readonly mine …` or `with files mine …`. |
| The program seems to hang, doing nothing | It is reading standard input, and that is the keyboard: type a line, or Ctrl+D for "no more". |
| Output is missing at the end | The program ended by `_exit` or was stopped; or it printed without a new line and is still running. |
| `mine exited with code N` | `main` returned N. |
| A line like `[process] page fault at 0x… in user mode` in the terminal `make run64` was started from | The program crashed (a bad pointer, most often) and the kernel ended it. The address is the instruction that did it. |

To see what a program printed after the fact, look in the terminal `make run64` was started from: everything shown on TheTaaJ's screen is also written there.

## A program that also runs on Linux

If a program uses only what is in the table above, the same file compiles on the host with the host's compiler and library:

```
cc -std=c11 -Wall -Wextra mine.c -o mine-host
```

That is the quickest way to find out whether a bug is in the program or in TheTaaJ's library: run both on the same input and compare. `libc/programs/cwords.c` is tested exactly this way on every `make test64`.

## C++

A C++ program is built the same way, from one `.cpp` file in `libc/programs/`, and linked with LLVM's C++ library (`ports/llvm`, built for TheTaaJ by `tools/build-cxx.sh`) as well as the C library:

```
make cxx64 CXX_PROGRAMS=mine        # libc/programs/mine.cpp -> build/cxx/x86_64/bin/mine
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img mine build/cxx/x86_64/bin/mine
```

The first `make cxx64` builds `libc++.a`, which takes a few minutes; after that only the program is compiled. Exceptions, `dynamic_cast` and `typeid`, global constructors and destructors, the containers and algorithms, `iostream`, `<format>` and `<regex>` work; `libc/programs/cxxlib.cpp` checks them all. `std::mutex` and `thread_local` work too, with one thread; `std::thread` throws `resource_unavailable_try_again`. What is left out: `<filesystem>`, `std::random_device`, wide characters, and every locale but "C". The linker script is `user/cxx.ld`, not `user.ld`: it keeps the tables an exception needs to find its way up the stack, and `--eh-frame-hdr` makes the linker add a sorted index to them, so that a throw finds its way faster.
