# Era 20 · The command line and libc

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: one shell language is typed at the keyboard and run in scripts, with history, Tab completion and line editing; `&` works in scripts as at the keyboard; a full-screen program (`vi`, `less`) runs in raw mode on a pseudo-terminal; a conformance suite (libc-test, Open POSIX) runs on TheTaaJ with its pass rate recorded and over a level set when it is first run; and a port that uses `regex` and `iconv` builds unchanged. Whose it is, and when, is for the user to say. Pointed to, not moved: J32-J34, J36, J40-J42 (C and C++ library), J43-J50 (developer tools), J57 (exec), K44 and K45 (pseudo-terminals, job control), K31 (DNS), K33 (UDP and loopback from C).

What is right. **libc** is one implementation with Rust: the system half (descriptors, sockets, spawn, threads, poll, signals, memory) is Rust on the `taaj` runtime, so C and Rust programs share it; what needs varargs (printf, stdio, scanf, math, locale, fenv) is C; libm is musl's. Linux's flag values, errno numbers and structure layouts, so ported code compiles as it is. Host tests for both halves. tcc, LLVM, clang and libc++ build and run on it. **`sh`** (`libc/src/sh.rs`) is nearly POSIX's language - lists, `if`, `while`, `for`, `case`, functions, variables and `${...}`, `$((...))`, `$(...)`, here-documents, `N>&M`, `set -e`, `eval` - tested on the host against a pretend system. **`shell`**, the one at the keyboard, grants each program only its services, gives a redirect a channel rather than the file service, and runs jobs in the background.

What is missing:

- **Two shells, two languages**: a script that runs in `sh` doesn't run typed at the keyboard. `shell` has no variables, `if` or `for` (J43), no `2>` (J44), and **no history, Tab completion or arrow keys**; `sh` refuses `&`.
- **libc gaps**: no `termios` (so no raw mode, no `vi` or `less`); no `regex.h`, `iconv.h`, `wctype.h`, `threads.h`, `complex.h`, `search.h`, `ftw.h`, `wordexp.h`, `syslog.h` or `sys/ioctl.h`; the "C" locale only; `malloc` a linked list with no debug mode; every program linked with its own copy; and nobody knows its coverage, since no conformance suite has been run.
- **Its own overview is out of date**: `libc/src/lib.rs` still says a process has one thread and names `emutls.c`, both gone (entry 099, step E).

All of it is user space: the shells and libc are programs and a library.

| # | Item | Size | Notes |
|---|---|---|---|
| J81 | **`libc/src/lib.rs`'s overview brought up to date**: real threads, native thread-local storage, what each file holds now | S | First: small, and the rest of the era reads it |
| J82 | **One shell language**: the shell at the keyboard parses and runs with `sh.rs`, keeping what only it does (grants, jobs, the keyboard); one parser left | M | Supersedes J43 and J44 |
| J83 | **Line editing**: history (kept in a file), Tab completion of commands and names, arrow keys, Ctrl+R search, a start-up file (`.shrc`) | M | After J82 |
| J84 | **`&` and job control in scripts** as at the keyboard | M | With K45 |
| J85 | **`termios`**: raw and cooked modes, echo, the window size (`TIOCGWINSZ`), on K44's pseudo-terminals; then a port of a small `vi` and `less` | M | With K44 |
| J86 | **The missing headers**: `regex` (POSIX regular expressions), `iconv` (UTF-8, Latin-1, UTF-16), `wctype`, `threads.h` (C11), `complex.h`, `search`, `ftw`/`nftw`, `wordexp`, `syslog`, `sys/ioctl.h` | L | Each with host tests; `semaphore.h` is J59's |
| J87 | **A UTF-8 locale**: `setlocale` takes `C.UTF-8`, `mbrtowc` and the wide functions done properly | M | With Era 19's K117 |
| J88 | **A faster `malloc`**: size classes and per-thread caches, a debug mode for double frees and overruns, host tests for the family | M | Covers J41 and J42 |
| J89 | **A conformance suite** (libc-test, Open POSIX test suite) run on TheTaaJ, its pass rate kept in the journal and watched | M | Early: it measures the era |
| J90 | **A shared libc**: dynamic linking for programs that want it | L | After Era 16's shared code pages (J29, K87) |
| J91 | **The everyday tools**: J47 and J48, and `sed`, `awk`, `tar`, `xargs` - ported or our own | L | After J86 (`regex`) |
