# Descriptors for a child: inheritance, close-on-exec, here-documents and `exec`

*Journal entry #92 · Era 6, step E11. A program started on TheTaaJ now gets every descriptor of its parent that is not marked close-on-exec, as Unix's exec leaves them:
- a file at 3 and up is handed down by name, with where the parent had got to;
- a pipe's end is handed down as itself.

posix_spawn's file actions now reach any descriptor (`addopen` to 3, `adddup2` to 5), and `dup` copies share one offset. On top of that, `/sh` gets:
- here-documents (`<<`, `<<-`, quoted delimiters);
- `<>`;
- `exec`: `exec 3< list`, `exec > log`, `exec cmd`;
- builtins, functions and compound commands with redirections, run in the shell itself, so `read x < f` and `while read l; do ...; done < f` keep their variables.

Still refused: `&`.*

## Why

Until now, a child got exactly three descriptors: 0, 1 and 2. Everything else stayed behind. That ruled out a lot of ordinary shell:
- `exec 3< list; while read l <&3; ...`: there was no 3 to read;
- `cat <<END`: refused;
- `read x < file`: refused, because `read` must set the variable in *this* shell, and a redirection meant running a copy;
- `while read l; do n=...; done < f`: it ran in a copy, so `n` was lost afterwards.

Configure scripts and Makefiles lean on these all the time. They are the last shell pieces between TheTaaJ and building bigger things on itself.

## How it fits together

```
 parent (libc)                                   child (libc start)
 descriptor table, a close-on-exec bit each      State::new -> inherit()
   0 Input  1 Output  2 Error                      reads the DESCRIPTORS channel:
   3 File{path, offset, flags}  ──────┐              one message per descriptor
   4 PipeWrite(handle)  ──────────────┤                FILE  /t/in.txt @4990 O_RDONLY
   5 PipeRead (O_CLOEXEC) ── dropped  │                WRITE + a channel handle
                                      │              -> its table at 3, 4, ...
 posix_spawn: standard_for(actions)   │
   1. start from the whole table      │
   2. do the file actions in order    │
   3. drop what is close-on-exec      │
   4. 0-2 -> handles (a file -> a stream from fs)
      3+  -> descriptors_channel() ───┘  (abi::descriptors, one message each)
   5. ask init: Run{ ..., kind |= DESCRIPTORS }  -> startup flag DESCRIPTORS
```

**The ABI** (TheKernel, `abi/src/lib.rs`):
- A new startup flag, `startup::DESCRIPTORS`, says the program was given one more handle: a channel holding its descriptors beyond 2.
- `startup::index(flags, flag)` finds that handle among the others. It counts the flags below it that bring a handle, so a new flag never moves the old ones.
- `abi::descriptors::Entry` is one message:
  - `File{descriptor, flags, offset, path}`, with no handle: the child opens the file by name;
  - `Read{descriptor}` or `Write{descriptor}`, with one handle: a stream channel's end.
- A Run request asks init for it with `init::DESCRIPTORS`. The kernel needed no change: it already passes start-up handles through.

**The parent** (`libc/src/spawn.rs`, `descriptors.rs`, `pipes.rs`):
- Each descriptor has a close-on-exec bit (one `u32`, since OPEN_MAX is 32). The bit is set by `O_CLOEXEC`, `F_SETFD`, `F_DUPFD_CLOEXEC` and `pipe2(fds, O_CLOEXEC)`, and cleared by `dup2`.
- `standard_for` now starts from the parent's whole table, does the file actions, then drops the close-on-exec entries:
  - a file at 0, 1 or 2 still becomes a stream from fs (read from the offset the parent had reached, or appended);
  - a file at 3 and up goes by name;
  - a pipe end goes as a copy of its handle.
- `addopen` to fd 3 and up makes the file ready in the parent, as `open` would (creating or emptying it), then hands it down by name.
- The library's own pipes (`system`'s, `popen`'s, the shell's) are now `private_pipe`: both ends close-on-exec. The end meant for the child arrives by `dup2`, which clears the mark. Without this, a child could inherit the write end of its own input and never see the end of it.

**The child** (`libc/src/descriptors.rs`):
- `inherit()` reads the channel until it is empty and fills the table from 3 up. The parent filled the channel and closed its end before the child started, so the reading never waits.

**`dup` shares the offset now.** `Open::File` holds an `Rc<RefCell<File>>`. A `dup` copies the `Rc`, so the copies share one offset, as POSIX's "open file description" does. Before, a copy got its own offset, and `exec 3< f; read a <&3; read b <&3` read the first line twice.

**The shell** (`libc/src/sh.rs`, `sh_system.rs`):
- **Here-documents in the lexer.**
  - After `<<` or `<<-`, the next word is a delimiter, and the lexer notes it as pending.
  - When the line ends, the lexer reads each pending here-document's lines up to its delimiter.
  - With `<<-`, it takes away leading tabs first.
  - The text becomes a `Word` of quoted parts, so it is not split or globbed. `$`, `` ` `` and `$((...))` still work in it unless the delimiter was quoted.
  - The parser takes the texts in the order of their `<<`s.
- **Two new `Redirection`s:** `ReadWrite{fd, path}` and `Here{fd, text}`.
- **Redirections done in the shell itself.** `System` has two new methods: `redirect(&[Redirection], save) -> Saved` and `restore(Saved)`.
  - A builtin, a function or a compound command with redirections, alone, now runs inside the shell (`Shell::redirected`).
  - On TheTaaJ, `redirect` copies each descriptor it changes to 10 and up with `F_DUPFD_CLOEXEC`, then `dup2`s the new one in. `restore` `dup2`s the copies back.
  - A subshell `( ... )` and anything in a pipeline still run in a copy of the shell.
- **`exec`** with only redirections calls `redirect(.., save: false)`, so they stay. `exec cmd` runs `cmd` and ends the shell with its status: from outside, that is what a Unix shell becoming `cmd` looks like.
- **A here-document on TheTaaJ** is a private pipe: the text is written into it and the write end closed before the command starts.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What a child gets | Every descriptor without close-on-exec, as Unix's exec | Only 0–2 plus what file actions name: simpler, but `exec 3<` scripts and every program that hands a pipe by number would not work |
| How a file at 3+ travels | By name, offset and flags; the child opens it itself | A stream from fs, as for 0–2: a stream goes one way and has no offset, so `lseek` and `<>` would break |
| How descriptors travel | One channel, one message per descriptor, handles inside | One startup handle per descriptor: the kernel allows 8 startup handles and 7 are used |
| The library's own pipes | Close-on-exec, the meant end given by `dup2` | Closing them in each child: there is no fork, so the child cannot close what it never knew it had |
| `dup` and offsets | Shared (`Rc<RefCell<File>>`), as POSIX says | A copy each: simpler, but `read <&3` twice reads the same line |
| Builtins and compound commands with redirections | Run in the shell, descriptors saved to 10+ and put back | A copy of the shell, as in entry 082: it loses `read`'s variables and a loop's assignments. 082 chose copies because a child could not be given fd 3 yet; now it can |
| `>` in the shell itself | Emptied, then opened `O_APPEND` | Plain `O_TRUNC`: in `{ echo a; prog; echo c; } > f`, `prog` appends to the end, and the shell's next write, at its own old offset, would land on top of `prog`'s output |
| A here-document | A pipe filled before the command starts | A temporary file: TheTaaJ has no `/tmp` convention yet. Writing while the command runs: the shell would need a second thread or a loop |
| A here-document longer than a pipe holds (32 messages, about 32 KB) | Refused with its size | Blocking: nobody reads until the command starts, so the shell would wait forever |
| `exec cmd` | Run `cmd`, wait, exit with its status | Replacing the process: TheTaaJ starts programs fresh and has no exec |

## Bugs

- **Inherited files could not be read.** In the first run, the three checks that read an inherited file failed: the child said `3: closed`. `read` and `write` expect the file service connection to exist already, because `open` makes it. An inherited file never went through `open`. Fixed: `inherit()` asks for the file service when it meets a file (`if self.files().is_none() => continue`).
- **The offset bug in `dup`**, described above: it had been there since `dup` was written. It only showed once the shell started using `<&3`.
- **`pipes.c` relied on not inheriting.** Its second test makes two plain pipes and gives the child one end of each. With inheritance, the child also got the write end of its own input, and would wait forever, exactly as on Linux. The tests now use `pipe2(..., O_CLOEXEC)`, the way real programs do.

## How I checked it

**The host tests** (`make libc-test`): sh.rs went from 30 to 33 tests, and six old ones were changed.
- New:
  - `here_documents`: expansion, four kinds of quoted delimiters, `<<-`, two here-documents on one line, `$(...)` inside, `read` from one, a loop reading one, and a function containing one passed to a copy;
  - `builtins_and_groups_redirected_run_here`: in order, put back after, a missing file runs nothing and undoes what was done;
  - `exec_changes_this_shells_descriptors_or_ends_it`.
- The pretend system got a descriptor table of its own, so these run with no OS underneath.
- The abi crate went from 27 to 30 tests (`cargo test` in `kernel/abi`).

**On TheTaaJ** (`make pipe64-test`, `make aarch64-pipe-test`): `pipes.c` went from 82 to 102 checks. The new `inherited()` section covers:
- a file open at 4990 bytes: the child reads the last 10;
- `F_SETFD FD_CLOEXEC`: the child has no such descriptor;
- `addclose` of an inherited descriptor;
- a plain pipe's write end, written to by number;
- `addopen` to 3, and `adddup2` to 5;
- through sh: `exec 3<`, a here-document, `<<-` with a quoted delimiter, `read <&4` twice, `while read ... done <<E` keeping its variable, a group writing to a file with a program in the middle, a program reading on from where `read` stopped, `<>`, `exec >`, `exec cmd`, and `read < missing`.

`pipe-test.sh`'s script gained `exec 3<`, `read <&3`, `pipes fdcount 3`, a here-document with `$((1 + 1))` in it, and a group redirected to a file.

**Mutation experiments**, each undone afterwards:

| What I broke | What happened |
|---|---|
| `dup` copies a file with its own offset again | 1 check failed: "sh: read <&4 twice: one offset" |
| The shell's `>` without the empty-then-append trick | 1 check failed: "a group's output a file - the shell's and a program's, in order" |
| (Both of the above were in one run: exactly those two of 102 failed) | |
| spawn hands close-on-exec descriptors down anyway | The test hung at `between_programs`' second case: `pipes count` waited for the end of its input while holding that input's write end itself. Stopped by `timeout` after 300 s |
| `Shell::redirected` never restores | 2 host tests failed: `builtins_and_groups_redirected_run_here`, `groups_and_subshells` |
| (The real bug) `inherit()` without asking for the file service | 3 checks failed, the inherited-file ones |

`make test64` passed, in 31 min 11 s. Entry 091's run took 13 min 52 s; nothing in this step should slow the other tests, so I take the difference to be this machine's load at the time.

## What is still missing

- **An offset is shared only within one program.** A file handed to a child goes by name and offset. After that, the child's reads don't move the parent's offset. `{ read a; prog; read b; } < f` gives `prog` the rest after `a`, but `b` is read from where `a` ended, not from where `prog` stopped. Sharing across programs would need fs to keep open-file state.
- **Here-documents over about 32 KB** are refused.
- **`read` reads one byte per request**, and from a file that is one fs round trip per byte. Fine for short files, slow for long ones.
- **`<>` as a program's standard output** appends instead of writing over the start: a stream to fs has no offset. In the shell itself, `<>` writes over the start correctly.
- **`&`**: background jobs.
- **Writes**: every `write` to a file is still one commit. That is the next step of Era 6.

## What I learned

**Rust, as used here:**
- **`Rc<RefCell<T>>`: shared and changeable.** `Rc` lets several owners point at one value, and `clone()` copies the pointer, not the value. `RefCell` lets you change what's inside through a shared pointer, checking at run time that nobody else is changing it at the same moment (`borrow()` to read, `borrow_mut()` to change). Two descriptors holding the same `Rc<RefCell<File>>` see the same `offset`: that is the whole "open file description" of POSIX in one type. Pipes already worked this way. Files now do too.
- **An associated type in a trait.** `trait System { type Saved; fn redirect(..) -> Result<Self::Saved, String>; fn restore(&mut self, saved: Self::Saved); }`. Each system decides what "what is needed to put things back" is: on TheTaaJ, `Vec<(c_int, c_int)>` (each descriptor and its saved copy); in the tests, a list of pretend table entries. The shell never looks inside. It only hands the value back. Generics with no `Box<dyn Any>` and no casting.
- **A closure taking the shell itself.** `fn redirected(&mut self, .., run: impl FnOnce(&mut Self) -> i32)`. The caller passes what to run (`|shell| shell.run_compound(body)`), and `redirected` does the saving and restoring around it. The closure gets `&mut Self` as an argument instead of capturing `self`, because Rust won't let a closure hold `self` mutably while `redirected` uses `self` too.
- **`Option<Option<Files>>`** for "not asked yet" against "asked, and there is none". The inherited-file bug was exactly a value left in the "not asked yet" state.
- **`core::mem::take(&mut self.pending)`** moves the list out and leaves an empty one behind. That let the lexer loop over the pending here-documents while calling `self.here_document(..)`, which needs `self` mutably.
- **`vec::IntoIter<Word>`** in the parser: each `<<` takes the next text with `.next()`, so nobody keeps an index.

**About Unix:**
- Close-on-exec isn't a detail. With inheritance, a program that forgets it can hang a pipeline, and the hang shows up somewhere else: in the reader, waiting for an end that a third program holds.
- "Run a builtin in a copy" was a shortcut that looked free, until `read` needed to set a variable. Doing redirections in place, with saved copies, is how real shells do it, and it turned out smaller than the copy machinery.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. Type each line in TheTaaJ's shell, exactly as written (the `\$` in the first line is meant).

| Type | You should see |
|---|---|
| `with programs sh -c 'echo "cat <<END" > h.sh; echo "six sevens: \$((6 * 7))" >> h.sh; echo END >> h.sh'` | nothing; `sh` exits with code 0. h.sh now holds a here-document |
| `with programs sh h.sh` | `six sevens: 42` |
| `with programs sh -c 'exec 3< h.sh; read -r a <&3; read -r b <&3; echo "[$a] [$b]"'` | `[cat <<END] [six sevens: $((6 * 7))]`: two lines read through descriptor 3 |
| `with programs sh -c '{ echo first; cprimes 10; echo last; } > g.txt; cat g.txt'` | `first`, `cprimes: 4 primes below 10`, `last`, in that order |
| `with programs sh -c 'n=0; while read l; do n=$((n + 1)); done < h.sh; echo "$n lines"'` | `3 lines`: the loop's variable kept |
| `with programs sh -c 'exec cprimes 10; echo never'` | `cprimes: 4 primes below 10`, and no `never` |
| `with programs sh -c 'a &'` | `sh: running a command in the background, &, is not in this shell yet`, `sh exited with code 2` |

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
| `pipes on TheTaaJ (x86_64): 102 checks ... passed; sh ran a list and a script with if, for, while read, case, a function, exec 3< and a here-document` | inheritance and the shell, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64
