# Control flow in sh: if, loops, case, functions, `${X%.c}` and `$((...))`

*Journal entry #82 · Era 6, step E9. TheTaaJ's `/sh` now has what a recipe or a script uses to decide and repeat:
- `if`/`elif`/`else`, `while`, `until`, `for`, `case`;
- `{ ...; }` groups and `( ... )` subshells;
- functions, with `return`, and `break` and `continue`;
- `${NAME%pattern}`, `%%`, `#` and `##`;
- `$((...))`, whole-number arithmetic;
- the builtins `read` and `eval`.

A compound command can be in a pipeline or have redirections (`for ...; done > list`, `... | while read l; do ...; done`). Still refused by name, before anything runs: here-documents, `&`, `<>`, `${NAME?}`, `>& file`.*

## Why

E8's shell refused every `if`, `for` and `case`, so a recipe with a loop, or a script that tests something before acting, could not run on TheTaaJ. The C library's Makefile, and most make-driven builds, use them: `for f in $(OBJS); do ...; done`, `case $CC in ...`, `${f%.c}.o`. Building the C library with make on TheTaaJ, the next step, needs them.

## How it fits together

```
 sh -c 'for f in *.c; do case $f in x*) ;; *) cc -c $f -o ${f%.c}.o;; esac; done > log'
   sh.rs lexer:   each token with where it starts and ends in the script
   sh.rs parser:  lists -> and-or -> pipelines -> commands, a command one of
                    Simple       words, assignments, redirections
                    Compound     Group | Subshell | If | For | Loop | Case,
                                 its redirections, and its source text
                    Function     name() compound-command, and its source text
   run:
     a compound command alone, no redirections  -> run here (a group's cd stays)
     with redirections, in a pipeline, or ( )   -> a copy of sh:
                    sh -c 'FUNCTIONS; SOURCE' $0 "$@"     with the redirections
     a function in a pipeline or redirected     -> sh -c 'FUNCTIONS; "$@"' sh f args
```

**The language** (`libc/src/sh.rs`, 2,977 lines with its tests):
- The lexer records where each token starts and ends, so the parser can take a compound command's text as written. A copy of the shell is given that text to run, as a Unix shell's fork would run the same parsed command.
- The parser reads `if`, `while`, `until`, `for`, `case`, `{` and `(` where a command starts, and `name()` before a compound command. A word that only closes or continues one (`then`, `fi`, `done`, `esac`, `}`, `in`...) where nothing is open is a mistake, refused with where it was found. After a command's first word they are plain words: `echo if then fi` prints them.
- `break N`, `continue N` and `return` leave a mark that the loops and the function call around them take off. Every list stops running when it sees one.
- A condition (`if`'s, `while`'s, `until`'s) does not stop the shell under `set -e`, as POSIX says.
- Patterns (`case`, `${NAME%pattern}`) keep, for each character, whether it was quoted: `*`, `?` and `[...]` (with `!` and ranges) work only unquoted. `case $f in '*')` matches a star.
- `$((...))` has C's operators but assignment and `?:`, in C's order. A name is its variable's value, 0 if unset or empty. Division by zero is an error: the command does not run and the shell exits with status 2, as a non-interactive sh does.
- `read [-r] names` splits a line of standard input as words are split. The last name takes the rest of the line.
- Precedence, as POSIX has it: special builtins (`cd`, `exit`, `read`, `eval`, `return`, `break`...), then functions, then the other builtins, then programs.

**On TheTaaJ** (`libc/src/sh_system.rs`):
- A copy is now `sh -c 'FUNCTIONS SCRIPT' ZERO WORDS...`. The functions' definitions go first, so a function in a pipeline, or called inside a redirected loop, is defined in the copy.
- `read` reads standard input one byte at a time. A loop's next command reads what follows the line, as sh does when its input is not a file it can seek.
- `$(...)` is now a copy like the others, given the functions too.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| A compound command with redirections, or in a pipeline | A copy of sh runs its source text | Redirecting this shell's own descriptors and putting them back: more state to restore after every command, and a loop in a pipeline must run beside the others anyway |
| What a copy is given | The source text as written, and every function's definition | The parsed command, serialised: a second format to keep right, for the same result |
| `{ ...; }` alone | Run here, so its `cd` and assignments stay | A copy: a group is not a subshell in sh |
| A special builtin with a redirection (`read x < f`) | Refused by name, status 2 | A copy: `read`'s variable would be set in the copy and lost |
| Arithmetic errors | The command is not run; the shell exits, 2 | 0 for the value: `rm -f file$((n/0))` would act on a wrong name |
| `$((x = 1))`, `$((a ? b : c))` | Refused by name | Half of C's assignment rules; nothing that make or a build runs uses them yet |
| Patterns | One matcher in sh.rs, for case and `${N%p}` | glob(): it matches paths, and cannot say which characters were quoted |

## Bugs

- **My own test expectations, four times:** which token a mistake is said at. `while a; b; done` fails at `done` (the condition list ran into it), not at the end. Fixed in the tests: the messages name the token that was really found.
- **The pretend system assumed every command has words.** A copy for `{ ...; }` has none when there are no arguments.
- **A disk directory of mine clashed with a check.** I made `t/sub` on the pipe test's disk for the subshell check, but an older check makes `sub` itself and expects to succeed. Removed.

## How I checked it

On the host, `sh.rs`: 30 tests against the pretend system (17 before), in `make libc-test`. The new ones cover:
- `if`/`elif`/`else`, nested;
- `for` over words, wildcards and `"$@"`, and with no words;
- `while`, `until`;
- `break`, `continue`, `break 2`, `continue 2`, and `break` outside a loop;
- `case` with `|`, `[A-Z]*`, `[!y]`, quoted patterns, `(pattern)`;
- 15 patterns against the matcher alone;
- groups (here) and subshells (a copy), and both with redirections and in pipelines;
- functions: arguments, `$#`, `return` from inside a loop, in a pipeline (the copy has the definition), with a redirection, and a special builtin before a function of the same name;
- `$((...))`: operators, precedence, hex and octal, unset names, `/0`, a value that is not a number, assignment refused;
- `${f%.*}`, `%%`, `#`, `##`, a quoted pattern;
- `read`: splitting, `-r`, the end of input, `while read`, and `read x < f` refused;
- `set -e` in conditions;
- `eval`;
- 5 refusals and 23 syntax errors, each with its message.

Mutation experiments, against those tests:

| Mutation | Result |
|---|---|
| M1: `continue` ends the loop | 1 test fails |
| M2: conditions not counted (set -e stops in `if`) | 3 tests fail |
| M3: `[!...]` not negated | 2 tests fail |
| M4: `##` the same as `#` | 1 test fails |
| M5: a function's caller's arguments not put back | 1 test fails |
| M6: `+` tighter than `*` | 1 test fails |
| M7: `read`'s last name not given the rest of the line | 1 test fails |
| M8: `case` always takes its first arm | 1 test fails |
| M9, on TheTaaJ: a copy not given the functions | `pipe64-test`: "a function, with a redirection" fails |

By booting, both machines (`pipe64-test`, `aarch64-pipe-test`):
- `pipes.c` now has 82 checks; 9 new ones go through `system` to sh:
  - `for` with its output a file;
  - `if`/`else` on `[ -f ]`;
  - `while` with `$((...))`;
  - `pipes emit one two | while read a b; ...` (a loop in a pipeline, reading the pipe);
  - a function with a redirection;
  - `case`, its status;
  - `${f##*/}` and `${f%.txt}`;
  - `( cd sub && ... ) > file`: the subshell's `cd` is its own, the file is here;
  - a here-document refused, 2.
- `script.sh` now also runs `for`/`if` over files, a function, `$((6 * 7))`, `${f%.sh}`, `pipes emit x y | while read a b`, and `case "$1"`.
- Typed: `sh -c 'cat << x'` is refused: "sh: a here-document, <<, is not in this shell yet", exit code 2.
- make: a fourth recipe, `for w in a b; do echo $$w; done > made-loop.txt`, wrote `a` and `b`.

`make64-test` and `aarch64-make-test` pass as before: every recipe goes through the new sh.

clippy: libc on x86_64 and AArch64, clean.

`make test64`: every part passed, the first time, in 1073 s.

The testing note below was run by typing its steps on both machines, each on a disk of its own.

Not tested:
- A function that calls itself deeply. Each call is a Rust call; there is no depth limit.
- `IFS` set to something other than blanks, for `read`.

## What is still missing

- **Here-documents, `&`, `<>`, `${NAME?}`, `>& file`.** Refused, by name.
- **A special builtin with a redirection** (`read x < file`, `exec 3< f`). Refused; `while read l; do ...; done < file` works, since the loop is a copy.
- **`$((...))` assignment and `?:`**, `$(...)` inside `$((...))`.
- **`.` (source) and `exec`.**
- **Every redirected loop starts a program**, sh again.

## What I learned

- Keeping each token's place in the script made "run this in a copy" nearly free: the copy is given the text as written, not a second format.
- Quoting has to survive until a pattern is matched: a pattern is characters *and* whether each was quoted, not a string.
- `break`, `continue` and `return` are one mechanism: a mark that every list checks, and that the right loop or call takes off.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `cd /src/libc/programs` | |
| `with programs sh -c 'for f in c*.c; do echo "C: $f"; done'` | `C: cask.c` ... `C: cwords.c`, one line for each |
| `with programs sh -c 'n=0; while [ $n -lt 3 ]; do n=$((n + 1)); echo "round $n"; done'` | `round 1`, `round 2`, `round 3` |
| `with programs sh -c 'f() { echo "f got $# arguments: $*"; }; f a "b c"'` | `f got 2 arguments: a b c` |
| `with programs sh -c 'for f in cask.c notes; do case $f in *.c) echo "$f: C";; *) echo "$f: other";; esac; done'` | `cask.c: C`, `notes: other` |
| `with programs sh -c 'f=/src/libc/programs/cask.c; echo ${f##*/} ${f%.c}'` | `cask.c /src/libc/programs/cask` |
| `with programs sh -c 'cprimes 20 \| while read a b c; do echo "[$c] [$a]"; done'` | `[primes below 20] [cprimes:]` |
| `with programs sh -c 'cat << END'` | `sh: a here-document, <<, is not in this shell yet`, `sh exited with code 2` |

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
| `pipes on TheTaaJ (x86_64): 82 checks ... passed; sh ran a list and a script with if, for, while read, case and a function` | the shell, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64
