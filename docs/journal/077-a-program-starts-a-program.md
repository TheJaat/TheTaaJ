# A program starts a program: `clang x.c -o x` on TheTaaJ

*Journal entry #77 · Era 6, step E5. A C program can now start another and wait for it: `posix_spawn` and `waitpid`, through init. So clang, given the right, starts `ld.lld` itself, and `clang @cflags @lflags chello.c lib/libc.a -o chello` compiles and links in one step, as on the host. The program it makes is the build's `chello`, byte for byte, on both machines.*

## Why

E4 left building a C program on TheTaaJ in two steps typed by hand: `clang -c`, then `ld.lld`. That is because clang links by starting the linker, and a C program could not start anything. Starting programs is also what `make`, a shell script or a test runner needs, so this is the piece the rest of Era 6's "TheTaaJ builds TheTaaJ" waits on.

## How it fits together

```
 shell:  with programs clang @cflags @lflags chello.c lib/libc.a -o chello3
           |  services: files, and init (the right to start programs)
           v
 clang   compiles chello.c to /tmp/chello-XXXXXX.o
           |  posix_spawn("/ld.lld", argv, ...)          libc/src/spawn.rs
           |    reads /ld.lld into memory, then asks init:
           |    Run { kind: IMAGE | REPLY, name, directory, file: "/ld.lld",
           |          arguments }  + stdout, services, stderr, a reply channel
           v
 init    starts ld.lld, replies "ok" + the process on clang's own channel
           |
 clang   waitpid(child) -> the process's exit code -> WEXITSTATUS
           |  0: removes the .o, exits 0
```

What it took:

**`posix_spawn`, `posix_spawnp`, `waitpid`, `wait`, `wait4`, in the C library** (`libc/src/spawn.rs`, `libc/include/spawn.h`):
- `posix_spawn` reads the program's file into memory (as the shell does) and asks init to start it, through the `init` service, the same request the shell sends. `posix_spawnp` finds a bare name in the root, as the shell does.
- The new program gets its arguments with `argv[0]`, its file (so clang's `__taaj_executable` works in a child too), the current directory, standard output (and whether it is the screen), standard error, and this program's services, no more. It gets no standard input: a keyboard line, file or pipe would be read by both.
- Without the `init` service, `posix_spawn` returns `EPERM`, and clang says `unable to execute command: posix_spawn failed: not permitted`.
- File actions (redirections) and attribute flags are refused (`ENOSYS`); an empty set of each is fine. There is no environment (`environ` is an empty list).
- A process ID is the program's own number for a child, counting from 2. `waitpid` waits on the child's process handle (`WNOHANG` too, and `-1` for any child). The status is the Unix one: the exit code shifted by 8, or a signal: `SIGSEGV` for a process the kernel killed, `SIGKILL` for one stopped.
- `fork` and `exec` still fail with `ENOSYS`: a process here cannot copy itself.

**init's reply, apart** (`abi::init::REPLY`):
- The shell is the only one asking on its channel, so init replies on the channel the request came on. Programs that start programs all ask on the one `init` service channel, and one could read another's reply.
- A request whose `kind` has `REPLY` (0x80) in it carries one more handle, a channel of the asker's own, and init replies there, then closes it.
- init's log line now says who asked and how: `[init] started /ld.lld for a program, from a file`.

**Rights that can be passed on.** A child gets copies of the handles its parent got. Before, init gave a program its standard output, services and standard error with SEND only, so clang could not copy them for ld.lld (`AccessDenied`). They now have `SEND | DUPLICATE | TRANSFER`, and the shell's `with` copies the services it hands out the same way.

**`with programs`, in the shell.** `with files COMMAND` gives the files; `with programs COMMAND` gives the files and `init`. A program gets the right to start programs only when the person says so.

**clang's own linker job for TheTaaJ** (`ToolChains/Gnu.cpp`, `TaajLinker`). For `--target=*-unknown-none-elf` clang otherwise drives a GNU `ld` with crt files and `-lgcc`. On TheTaaJ it runs `ld.lld` (`CLANG_DEFAULT_LINKER=lld`) with the inputs, `-L`, `-T`, `-s`, `-Bstatic` for `-static`, `-Wl,` options, and `-L<sysroot>/lib -lc` unless `-nostdlib`. So `@lflags` is the build's `TAAJ_LDFLAGS` with `-Wl,` in front of the linker's own options. LLVM's `Program.inc` now uses `posix_spawn` (`HAVE_POSIX_SPAWN=1`).

**`/tmp`.** clang writes the object it is about to link to `/tmp`, and removes it afterwards. The test disk and `make llvm-put`'s have one.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How a program starts another | `posix_spawn`: the whole request in one call | `fork` + `exec`: a process here cannot copy itself, and LLVM uses `posix_spawn` when it has one |
| Who starts it | init, asked the way the shell asks | A kernel call: init already reads files into processes, hands out handles and logs; one way in |
| Who may | Whoever was given the `init` service (`with programs`) | Every program: a program that compiles must not start anything unless the person said so |
| Where init replies | On a channel the asker sends with the request, when it asks so | On the service channel: two programs asking at once could swap replies |
| What the child gets | stdout, stderr, the parent's services, its own file and arguments | Standard input too: two readers of one keyboard or pipe |
| clang's link job | `TaajLinker`: ld.lld with the build's kind of options and `-lc` | The generic GNU job: it asks for `crt1.o`, `-lgcc` and a dynamic linker TheTaaJ does not have |
| The exit status | Unix's encoding (`code << 8`, or a signal number) | The code as it is: every `WIFEXITED` in C code would be wrong |

## Bugs

- **`ld.lld: not given the files`**, then **`AccessDenied`**: the handles clang got could not be copied for ld.lld. See "Rights that can be passed on".
- **`use of undeclared identifier 'AddLinkerInputs'`**: it lives in `tools::`.
- **The failing link only warned.** With the build's `--gc-sections` and no `_start`, nothing is kept and nothing is missing, as in E4. The failing case is linked with `-nostdlib` and without `@lflags`.
- **A mutation that survived** (M4 below): after `posix_spawn` itself fails, clang says `ld.lld command failed with exit code 1` too, so the test's one check for that line was answered by the wrong case. It now wants the line twice.
- **`ld.lld: error: failed to open chello3: the file system is full`**, in the testing note. `make llvm-put` asked for a 256 MiB disk since E4, but an existing one was kept, and mine was the 192 MiB one from before. The tools fit; a link's output did not. `llvm-put` now makes a disk smaller than 256 MiB again.

## How I checked it

Automated, by booting: `clang64-test` and `aarch64-clang-test` (in `make test64`) put `/tmp` and `lflags` (the build's linker options, as clang takes them) on the disk, and after E4's steps run:

| Step | Shows |
|---|---|
| `with programs clang @cflags @lflags libc/programs/chello.c lib/libc.a -o bin/chello2` | `[init] started /ld.lld for a program, from a file`, ld.lld and clang exit with code 0 |
| `bin/chello2 one two` | it runs, exit code 0 |
| `with programs clang @cflags -nostdlib libc/programs/cwords.c -o bin/y` | ld.lld exits with code 1, `clang: error: ld.lld command failed with exit code 1` |
| `with files clang @cflags @lflags libc/programs/cwords.c lib/libc.a -o bin/z` | `clang: error: unable to execute command: posix_spawn failed: not permitted`; no `bin/z` |

`bin/chello2` is compared byte for byte with the build's `chello` (`build/c/`, `build/c-aarch64/`): identical on both machines. The test also counts clang's exits (7 with code 0, 4 with code 1) and ld.lld's started by clang (exactly one with code 0). A host test in `abi` decodes a request with `REPLY` and refuses `REPLY` alone.

Mutation experiments, 4, each run against `clang64-test`:
- M1, init gives a program's handles with SEND only: clang could not finish the one-step build (`clang did not finish all seven of its jobs with exit code 0`). Caught.
- M2, `with programs` without `init`: caught the same way.
- M3, clang's TheTaaJ linker job taken out (the generic GNU one): caught the same way.
- M4, the exit code not shifted (`waitpid` gives 1 for exit code 1): clang said `ld.lld command failed due to signal`, but the test passed. Not caught at first (see Bugs). With the check counting two lines: `clang did not say its linker failed with exit code 1`. Caught.

Not tested:
- A person typing.
- `posix_spawn` from a program other than clang (no other C program here starts one yet).
- A child killed: `WIFSIGNALED` is not checked by any test.

## What is still missing

- **Redirections.** `posix_spawn_file_actions_addopen` and its kin return `ENOSYS`, so a program cannot give a child a file as its output, and there are no pipes between programs. `make` needs neither for simple rules, but a shell script would.
- **`system` and `popen`**: not there; they need a shell a C program can start, and pipes.
- **An environment**: `getenv` finds nothing, and a child gets none.
- **lld is slow on AArch64** (E4). The one-step build of `chello` took 79 s there, by hand, against 48 s on the PC.
- **`make` on TheTaaJ**, next: it now has the one thing it lacked from the system.

## What I learned

- A capability system changes what "may a program start another" means: not a permission bit, but whether it was handed the one handle that does it. `with programs` is that, said by the person.
- Handing on a handle needs the right to copy it at every step. A handle that is fine to use can still be a dead end for a child.
- A check that one line appears is weak when two cases print the same line. Count it, or pin it to the case.
- A disk kept "as it is" is a disk made under old rules. A size that can grow must be checked, not just asked for.

## Testing note

**1. The PC: compile and link in one**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

`make llvm-put` now also puts `/tmp` and `/lflags` on the disk, and makes the disk again if it is one from before E4 (smaller than 256 MiB). Close QEMU before running it.

| Type | You should see |
|---|---|
| `time with programs clang @cflags @lflags libc/programs/chello.c lib/libc.a -o chello3` | `[init] started /ld.lld for a program, from a file`, then `took` about 48 s, no errors |
| `chello3 one two` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two` |
| `with files clang @cflags @lflags libc/programs/chello.c lib/libc.a -o x` | `clang: error: unable to execute command: posix_spawn failed: not permitted`, `clang exited with code 1` |
| `with programs clang @cflags -nostdlib libc/programs/cwords.c -o y` | ld.lld's `undefined symbol` errors, then `clang: error: ld.lld command failed with exit code 1` |
| `ls tmp` | `empty`: clang removed the objects |

**2. The same program as the build's**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img chello3 > /tmp/chello3
cmp /tmp/chello3 build/c/chello && echo same
```

| Look for | Meaning |
|---|---|
| `same` | clang on TheTaaJ, starting ld.lld itself, made the build's program |

**3. AArch64**

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1; the first step takes about 80 s. Compare with `build/c-aarch64/chello`.

**4. The tests by themselves**

```
make clang64-test
make aarch64-clang-test
```

| Look for | Meaning |
|---|---|
| `clang on TheTaaJ (x86_64) compiled and linked chello in one, starting ld.lld itself: the build's chello, byte for byte` | the one-step build, compared (`aarch64` likewise) |
| `clang64-test passed`, `aarch64-clang-test passed` | each machine |

Automated: make test64
