# Pipes and redirections: a program's input and output, chosen by the program that starts it

*Journal entry #79 · Era 6, step E7. A C program can now give a program it starts a pipe or a file as its input or output, and an environment:
- `pipe`;
- posix_spawn's file actions;
- the file service's streams;
- `envp`.

`system` and `popen` run pipelines with redirections, still without a shell, so make's recipes can have `<`, `>`, `|` and `2>&1`. 58 checks on both machines.*

## Why

E6 left make able to run a recipe only as one bare command. `cc x.c > log`, `prog | wc` or `$(shell ...)` were refused, because nothing could give a started program anything but this program's own output. Building software needs both: commands that write files, and commands that feed each other. A make that runs commands needs its variables to reach them too.

## How it fits together

```
 make:  system("pipes emit made by make | pipes count > out 2>&1")
          |  words.rs: two commands; "> out" and "2>&1" for the second
          |  pipe(): a channel, two descriptors            libc/src/pipes.rs
          v
 first   stdout = the pipe's write end  -----DATA----->  second  stdin
                                                          stdout = a stream:
                                                            Files::stream_out("out")
                                                            a channel whose other end
                                                            the file service keeps
                                                          stderr = a copy of stdout
          |                                                        |
          v                                                        v DATA
                                                     fs: appends each message to
                                                         "out" - and has written all
                                                         queued before it answers
                                                         the next request
```

What it took:

**The file service's streams** (`fileserver`, `user/src/bin/fs.rs`):
- A stdout channel carries DATA messages, which a program cannot tell from a pipe or the screen. For a program to write into a file with nobody copying, something has to be at the other end.
- That something is now the file service. `STREAM_OUT name keep` brings one end of a channel. The service watches it alongside its request channels, and appends every DATA message on it to the file until it closes. `keep` 0 empties the file first (`>`), 1 keeps it (`>>`).
- `STREAM_IN name` is the other direction. The service sends the file as DATA messages and closes the channel at the end. When the channel is full it tries again a tick later, since the kernel has no "wake me when there is room".
- At most 16 streams are open at once; one more gets the new `Busy` status.
- Before the service answers any request, it writes every message already waiting on an output stream into its file. A program that waited for another to end and then reads the file finds all of it there.
- The logic (`open_stream`, `Stream::take`, `next`, `sent`) is in the crate, tested on both stores. The user library has `Files::stream_out` and `stream_in`.

**`pipe`** (`libc/src/pipes.rs`):
- A pipe is a channel, two new kinds of descriptor. Each end is shared by the descriptors `dup` makes of it, and its handle closes with the last of them.
- So the reader sees the end only when every write end, here and in every child, is closed. That is Unix's rule.
- Writing with no reader is `EPIPE`, and `fstat` says FIFO.

**posix_spawn's file actions** (`libc/src/spawn.rs`, `standard_for`):
- The actions run in order on what would be the child's descriptors. That table starts as this program's own, without standard input.
- Only 0, 1 and 2 reach the child: a program here is given no other descriptors.
- `adddup2(fd, to)` can take this program's standard ones, a pipe's end, a file open here (made into a stream), or what an earlier action made. `addopen` opens a stream: read with `O_RDONLY`, otherwise written, from empty with `O_TRUNC`. Without `O_CREAT`, a missing file is `ENOENT`. `addclose` takes a descriptor away.
- The child gets a duplicate of each with the rights it needs, and a standard input only when an action gives it one.
- init now lets a program hand on a standard input that is not the keyboard, as it already did the outputs.
- spawn.h's `posix_spawn_file_actions_t` keeps a pointer to the list.

**`system` and `popen`, pipelines** (`libc/src/words.rs`, `pipeline_of`):
- The same word splitting as E6, now with `|` between commands and, anywhere in a command, `< f`, `> f`, `>> f`, `N> f`, `N>&M`.
- Each command becomes file actions; a pipeline's status is its last command's.
- Still refused, out loud: `;`, `&&`, `||`, `&`, `$`, wildcards, here-documents, `>&-`, a pipe with nothing on one side.
- `popen` starts the pipeline with a pipe to this program and makes a `FILE` of the other end. `pclose` closes it, so a command reading from it sees the end, then waits.

**The environment** (`abi::init::ENVIRONMENT`):
- A Run request may carry `NAME=value` words after its arguments, laid out by `abi::init::pack`: the arguments' length in two bytes, then both. The whole request must still fit in one message: 1024 bytes, else `E2BIG`.
- init sends the environment as a third message on the program's ARGUMENTS channel, and libc's start makes it `environ`.
- `posix_spawn` passes `envp`, and `system` and `popen` pass `environ`. pdpmake takes a variable from the environment as a macro, and its commands get its environment.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Who writes a redirected output into its file | The file service, from a channel it is given (a stream) | The parent copying it: posix_spawn returns at once, and the parent has no thread to copy with. The child's C library opening the file: then only C programs could be redirected, not `cat` |
| A pipe | A channel, as stdout already is | A new kernel object: a channel already queues, blocks the writer when full, and says when the other end closes |
| When a stream's bytes reach the file | Before the service answers the next request | Whenever it gets to them: `prog > f; cat f` could read half of `f` |
| An input stream with a full channel | Tried again a tick later | Asked for by the reader (WANT): every reader would have to know its input is a file |
| What a child gets | 0, 1 and 2 after the actions | Every descriptor, as Unix: a program here is handed handles, not a table, and a file descriptor is the C library's, not the kernel's |
| `system`'s syntax | Pipes and the common redirections; the rest refused | A shell: still a large job, and the refusals say exactly what is missing |
| Where the environment travels | In init's request, a third message to the program | A handle of its own: one more startup flag and channel for a few short strings |

## Bugs

- **A test that ran itself inside make.** The recipe `pipes oops and its errors` gave `oops` four words where it takes one, so it fell through to the checker's `main` and ran all its checks again, inside make's recipe. That showed as a log of a hundred extra processes. Now `oops errors`, and the test counts make's children: exactly four.
- **`counted.txt` already up to date**: the checker had written a file of the same name. The Makefile's files are now `made-*.txt`.
- **Two wrong expectations of my own**: `line 00001\n` is 11 bytes, not 10, and the file service test assumed stream DATA is byte 1 (it is the display's TEXT).
- **The host test of unix.c stopped linking**: popen now calls into spawn.rs, which the host test does not have. It has two stubs now.
- **A mutation that survived** (M1 below): the test cannot show why the file service must write a stream's messages before it answers a request.

## How I checked it

Automated, by booting: `pipe64-test` and `aarch64-pipe-test` (`kernel/kernel64/scripts/pipe-test.sh`, in `make test64`). The disk has `pipes` (the C checker, `pipes.c`, which is also the program at the other end of every pipe), `make`, `cat`, and in `/t` a Makefile, `env.mk` and a 5000-byte `in.txt`.

| Step | Shows |
|---|---|
| `with programs pipes` | `pipes: 58 checks, all passed` (below) |
| `with programs make` | `pipes count < in.txt > made-count.txt`; `pipes emit made by make \| pipes count > made-both.txt 2>&1`, then `pipes oops errors >> made-both.txt 2>&1`: four commands, exit code 0 |
| `with programs make` again | `make: nothing to be done for all` |
| `with files make refused` | `make: (Makefile:12): couldn't execute 'pipes emit not run'` |

Afterwards the files are read off the disk: `made-count.txt` is `5000 bytes`, and `made-both.txt` is `13 bytes` and `errors` (the second command's standard error, by `2>&1`).

The 58 checks of `pipes.c`:
- **In one program:** a pipe, a copy of an end, the end of the stream, EPIPE, FIFO.
- **Between programs:**
  - a child's output through a pipe;
  - its input through one (3000 bytes, and the end);
  - `2` a copy of `1`;
  - a writer stopped with SIGPIPE when its reader goes.
- **Files:**
  - 5000 lines into a file, and the whole file is there once the writer ended;
  - `O_APPEND`;
  - input from a file;
  - a file open in the parent, handed on;
  - `ENOENT` for a missing file, with and without `O_CREAT`;
  - `addclose`;
  - `EBADF`.
- **`system`:**
  - a pipe and a redirection;
  - `<` and `>`;
  - `>>`, `2>`, `2>&1`;
  - three commands;
  - TheTaaJ's own `cat` in a pipe;
  - a pipeline's status;
  - 127 for a missing command or file;
  - refusals;
  - 40 redirections in a row, more than the 16 streams the service keeps, so none is kept open.
- **`popen`:** reading, writing, and the status.
- **The environment:**
  - none from the shell;
  - passed by `system` and `popen`;
  - `envp` and only `envp`;
  - make taking a macro from it;
  - `unsetenv`;
  - `E2BIG`.

On the host:
- `fileserver`: 37 tests, streams on both stores among them.
- `abi`: 25 tests, one for an environment in a Run request, a wrong length and one too big.
- `words.rs`: 4 tests, the pipelines and what is still refused.
- `libc-test` passes.

Mutation experiments, 7, each run against `pipe64-test`:
- M1: the file service does not write waiting stream messages before it answers a request. **Not caught**, even with the output made 55 KB. The kernel's wait does give the request channel first, so the bug is real, but in every run the service had written the messages before the parent asked. Making the service late enough to show it would need a stall that a C program cannot ask for. It stays as reasoning, and as a comment in fs.rs and pipes.c.
- M2: a stream's DATA not written to the file. `pipes.c`'s checks fail. Caught.
- M3: a pipe's end never closed. The reader never sees the end: `the steps did not all run`. Caught.
- M4: no standard input given to a child. Caught by the checks.
- M5: init drops the environment. Caught by the checks.
- M6: `2>&1` ignored. Caught by the checks.
- M7: an input stream never closed at the file's end. The reader waits for ever: `the steps did not all run`. Caught.

`make64-test` and the rest of `make test64` pass: the C library's spawning changed under them. The testing note below was run on both machines by typing its steps.

Not tested:
- A person typing.
- `Busy`, a 17th stream at once.
- A program reading a pipe it was given as standard input handing that on to one it starts.

## What is still missing

- **A shell for programs**: `;`, `&&`, `$VAR`, wildcards. `system` refuses them, and says so.
- **Only 0, 1 and 2 reach a child.** A program that passes a fourth descriptor (`3< file`) cannot.
- **`STREAM_IN` reads from the start**: a file open here at an offset and handed on starts again at 0.
- **The environment is small**: arguments and environment together in one 1024-byte request.
- **The shell has no environment**, so a program the shell starts has none; only programs started by programs get one.

## What I learned

- An output is a channel to whoever reads it. Once a file can be that reader, redirection needs no copying program and no new kernel object, and works for programs in any language.
- Ordering between two channels is a promise the server has to make on purpose. A test passing does not show it is kept: the mutation survived because the race is real but narrow.
- A test program that is also its own helper must reject wrong arguments loudly. Here a silent fall-through became a test running inside a test.

## Testing note

**1. The PC: a recipe with a redirection**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. The Makefile on the disk has a new target, `primes.txt`.

| Type | You should see |
|---|---|
| `cd /src/libc/programs` | the prompt says `taaj:/src/libc/programs>` |
| `with programs make primes.txt` | `tcc cprimes.c -o cprimes` (if it was not built yet), then `./cprimes 100 > primes.txt` |
| `with files cat primes.txt` | `cprimes: 25 primes below 100`: what cprimes printed, in the file |
| `with programs make primes.txt` | `make: 'primes.txt' is up to date` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make pipe64-test
make aarch64-pipe-test
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 58 checks of pipes, file actions, system, popen and the environment passed; make ran recipes with <, >, \| and 2>&1` | every check, and make's recipes (`aarch64` likewise) |
| `pipe64-test passed`, `aarch64-pipe-test passed` | each machine |

Automated: make test64
