# A small shell: `sh -c`, so that system, popen and make run commands as POSIX says

*Journal entry #81 · Era 6, step E8. TheTaaJ has a POSIX shell for programs now: `/sh`. It has:
- lists (`;`, `&&`, `||`), pipelines and `!`;
- redirections;
- variables, `$NAME`, `${NAME:-word}`, `"$@"` and `$?`;
- `$(...)` and backquotes;
- quoting and wildcards;
- the builtins a recipe uses: `cd`, `echo`, `test`, `export`, `set -e` and a few more.

`system` and `popen` hand it whatever a plain pipeline cannot express, and make is back to running each recipe as `set -e;` and the command, as pdpmake does upstream. Not yet: `if`, `for`, `while`, `case`, subshells, here-documents, `&`. Those are refused by name, before anything runs.*

## Why

E7 gave `system` pipes and redirections. Everything else a command line can say was still refused with ENOSYS:
- `cd sub && make`;
- `X=1; cc -DX=$X`;
- `rm -f *.o`;
- `CC=tcc make`.

A make recipe is a shell command, and pdpmake had a TheTaaJ change that dropped its `set -e;` because the `;` would be refused. TheTaaJ's own shell (`user/src/bin/shell.rs`) is for people, not for programs: it gives rights (`with files`), runs jobs, and does not speak POSIX sh. A C program needs `sh -c`.

## How it fits together

```
 make: system("set -e;N=$(pipes count < in.txt); test -n \"$N\" && echo ...")
         |
         | words.rs: ';' -> not a plain pipeline       (a plain one, or no /sh:
         v                                              started from here, as before)
 spawn.rs start_jobs:  /sh -c 'set -e;N=$(...); ...'
         |
         v
 sh  (libc/programs/sh.c: main -> __taaj_sh)
   sh.rs:  parse   ->  lists of and-or lists of pipelines of simple commands
           run     ->  expand each word when its command runs:
                         $N, ${N:-w}, "$@"     the variables
                         $(...)                System::capture
                         field splitting, wildcards (System::glob)
                       builtins here:     cd export set -e exit ...
                       programs:          System::pipeline
   sh_system.rs (TheTaaJ):
           pipeline  -> spawn.rs start_jobs, the same code system uses
           capture   -> another /sh -c '...' with its output a pipe, read to the end
           builtin in a pipeline or with a redirection  (echo hi > f, echo x | wc)
                     -> /sh -c '"$@"' sh echo hi       a copy of itself, as a fork would be
```

**The language** (`libc/src/sh.rs`, 1,790 lines with its tests):
- A lexer turns the script into words and operators. A word keeps its parts apart: plain text, quoted text, `$NAME` or `${...}` (and whether inside double quotes), `$(...)`, `~`.
- A parser makes and-or lists of pipelines of simple commands: assignments, words and redirections. Reserved words (`if`, `for`, `while`, `case`, `{`) said first in a command, `(`, `&`, `<<` and `$((` are refused. So is a mistake. Nothing has run by then: the whole script is read first.
- Words are expanded when their command runs, so `X=1; echo $X` sees the 1. An expansion outside quotes is split at `IFS`. Then each field with a wildcard outside quotes is matched against the files. A quoted `*` goes to `glob` behind a backslash. If nothing matches, the field stays as it was, as sh leaves it.
- `set -e` stops at a failure that is not a condition: not one followed by `&&` or `||`, and not one negated with `!`.
- It asks a system, the `System` trait, for what it cannot do itself: start a pipeline, capture a script's output, match a wildcard, change directory, say what a path is, write. So the whole shell is tested on the host against a pretend system that records what it was asked.

**On TheTaaJ** (`libc/src/sh_system.rs`):
- A pipeline is started by `start_jobs`, which E7's `system` code became. One loop now serves `system`, `popen` and `sh`: pipes between the commands, file actions for the redirections, an environment for each command.
- A builtin alone runs inside the shell. In a pipeline, or with a redirection, it is run by a copy of the shell: the file it was started from (`__taaj_executable`), with `-c '"$@"' sh echo hi`. That is what a Unix shell's fork does, and its output goes where the redirections say.
- `$(...)` runs another copy with its output a pipe, reads the pipe to its end, and waits for it. It is given every variable, exported or not, so it sees what this shell sees.

**system and popen** (`libc/src/spawn.rs`):
- A command `words.rs` can split, with no builtin in it, is still started from here. That is what sh would do with it, without starting sh.
- Anything else becomes `sh -c command`. If there is no `/sh` on the disk, it is refused as before: -1, ENOSYS, and a line saying why.
- `exit 7` and `echo hi` are plain words, but they are builtins, not programs: they go to sh too.

**make** (`ports/pdpmake/make.c`): TheTaaJ's one change to pdpmake is gone. Recipes run as `set -e;` and the command, through `system`, as upstream.

`/sh` is one of the C programs, so it is on every disk `make run-disk` makes, and on the make and pipe tests' disks.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where the shell runs | A program, `/sh`, started for each command that needs it | Inside `system`: `cd` and assignments would change the caller, and nothing could type `sh -c` |
| What language the shell is in | Rust, in libc, behind a `System` trait; `sh.c` only calls it | C: the language would not be testable on the host without a system |
| A simple command in system | Started by system itself, as before | Always `sh -c`, as POSIX describes: one more program to start for every command, for the same result |
| A builtin in a pipeline | A copy of the shell runs it (`sh -c '"$@"'`) | Running it here into the pipe: it can fill the pipe before its reader is started |
| Expanding words | When the command runs | Before the script runs: `X=1; echo $X` would see nothing |
| Unsupported syntax | Refused by name, the whole script, exit 2 | Running the part understood: half of `if x; then rm y; fi` would run `rm y` |
| `$(...)`'s variables | All of them, exported or not | Only exported: sh's subshell sees every variable, and `X=1; echo $(echo $X)` is a common recipe |
| `test -r`, `-w`, `-x` | True if the path exists | Inventing permissions: TheTaaJ has none to read |

## Bugs

- **`exit 7` and `echo` from system ran nothing.** They are plain words, so `system` started them as programs and found none: "system: exit: not found". It now hands a builtin to sh.
- **Each command substitution ran twice.** A simple command's words were expanded once to see whether it was a builtin, and again when it was prepared. `$(list)` ran twice, shown by the pretend system counting captures.
- **`$?` was always 0.** The status was reset before the command's words were expanded, to make a substitution's status the command's. A substitution's status is now kept apart.
- **`"$@"` with no arguments made an empty word.** An empty `""` part was added at every opening quote, so that `""` is a word. Now it is added only when the quotes hold nothing at all.
- **`${X:-a  b}` was not split.** The plain text of the word inside is split as the value would be, when the expansion is not quoted.
- **Wrong expectations of my own:** `echo piped` is 6 bytes, not 7. A line I added to the test's Makefile moved its last recipe to line 15. And `a && b || c && d` runs `c` and `d` after `b` fails.
- **Two mutations survived at first** (M1 and M3 below). The tests now cover them.

## How I checked it

On the host, `sh.rs`: 17 tests against the pretend system, in `make libc-test`:
- splitting and quoting;
- lists, statuses, `!`, `exit`;
- `set -e`;
- pipelines and every redirection;
- builtins here and in a pipeline;
- variables, exported and for one command;
- `${...}` with defaults and lengths, `$$`, `$#`, `$0`;
- field splitting with `IFS`, `"$@"` and `$*`;
- `$(...)` and backquotes;
- wildcards, quoted and not;
- `cd`, `pwd`, `~`;
- 20 `test` expressions;
- `set -x`;
- 22 scripts refused, with the message for each;
- `export` and `set` listing.

Mutation experiments, against those tests:

| Mutation | Result |
|---|---|
| M1: `set -e` stops at a negated failure | survived; caught after `! good` was added |
| M2: no field splitting | 4 tests fail |
| M3: a quoted `*` not escaped for glob | survived (`"*".z` has no unquoted wildcard); caught with `"*".?` |
| M4: `&&` and `\|\|` swapped | 3 tests fail |
| M5: `"$@"` one word | 1 test fails |
| M6: a command's redirections dropped when it has no words | 1 test fails |
| M7, on TheTaaJ: system not handing builtins to sh | `pipe64-test`: 2 checks fail (`exit`, a builtin in a pipeline) |

By booting, both machines (`pipe64-test`, `aarch64-pipe-test`):
- `pipes.c` now has 74 checks. The 17 new ones go through `system` and `popen` to sh:
  - `&&`, `||` and `;`;
  - a variable, quoted and not, and one for a single command;
  - `> file` alone;
  - a wildcard and a quoted one;
  - `cd` in sh, not in the caller;
  - `$(...)`, backquotes, and `echo` with a redirection;
  - a builtin in a pipeline;
  - `set -e`, `test` and `[`, `exit`;
  - 127 in a list, `if` refused;
  - popen of a list.
- Typed: `with programs sh -c 'echo a && echo b; false || echo c'` prints a, b and c.
- A script, `sh script.sh one 'two words'`: its arguments, `cd`, `$(...)`, `test`, `*.mk`, `${...}`, a pipeline, then `set -e` stops it at `pipes exit 3`, exit code 3.
- `sh -c 'if x'`: "sh: if is not in this shell yet", exit code 2.
- make's recipes, each through sh now. A new one, `N=$$(pipes count < in.txt); test -n "$$N" && echo "counted: $$N" > made-shell.txt`, wrote `counted: 5000 bytes`.

`make64-test` and `aarch64-make-test` (each recipe now through `sh`, with `set -e;`):
- the same compiles and links as before, byte for byte the host's tcc's;
- `echo one && echo two`, refused until now, prints both;
- make not given the right to start programs still cannot run a recipe.

clippy: libc on x86_64 and AArch64, and `sh.rs`'s tests, clean.

`make test64`: every part passed but one. `aarch64-keys-test` failed: two typed lines ran into each other (`echo "p |s | cat > big4 &`), so its file was `big4`, not `big`. That is the typed-key loss under load noted in entry 080, in a test that does not start sh. It passed when run again alone, with the four parts make had not yet reported when it stopped. The run took 2724 s, not 15 minutes: two headers' comments changed, so the shared phase built the C++ library and LLVM again for both machines.

The testing note below was run by typing its steps on both machines.

Not tested:
- `system` with no `/sh` on the disk. Every disk the tests make now has one, and the code is the refusal E7 tested.
- A program stopped by a signal inside sh (`$?` 128 + N).

## What is still missing

- **`if`, `for`, `while`, `case`, functions, `( ... )`, `{ ... }`, here-documents, `&`.** A recipe with a loop is refused, and says so.
- **`${NAME%pattern}` and the other pattern operators, `$((...))`, `${NAME?}`.**
- **`sh` reads no script from its standard input.** `sh` alone prints its usage.
- **Each builtin with a redirection starts a program**, the shell again. `echo x > f` costs a program start.
- **`sh` is built on the host only.** `/src/build` and the Makefile for make on TheTaaJ still build the 11 other C programs.

## What I learned

- A shell's language and the system it runs on separate cleanly: the language needs about eight things from the system. Behind a trait, the whole language is tested in milliseconds on the host, and every TheTaaJ boot only checks the eight.
- Running a builtin elsewhere by starting yourself again with `"$@"` is a fork without fork.
- A command that looks plain can still need a shell: `exit 7` is words, and still no program.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `with programs sh -c 'echo one && echo two; false \|\| echo three'` | `one`, `two`, `three` |
| `cd /src/libc/programs` | |
| `with programs sh -c 'echo c*.c'` | `cask.c cfile.c ... cwords.c`, the C files that start with c |
| `with programs sh -c 'N=$(cprimes 100); echo "said: $N"'` | `said: cprimes: 25 primes below 100` |
| `with programs sh -c 'cd /nowhere \|\| echo no such place'` | `sh: cd: /nowhere: no such directory`, then `no such place` |
| `with programs sh -c 'for f in *; do echo $f; done'` | `sh: for is not in this shell yet`, `sh exited with code 2` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 74 checks ... passed; sh ran a list and a script` | the shell, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64
