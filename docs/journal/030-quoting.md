# Quoting: Who Decides Where One Argument Ends

*Journal entry #30 · the shell divides a line into words — spaces, two kinds of quotes, the backslash — and programs receive a list of arguments instead of a string to pick apart*

This is the first of four steps I am calling *shell plumbing* (quoting, then standard input and output as handles, then redirection, then pipes and background jobs). It is the smallest, and it fixes something that had been wrong since programs first took arguments in entry #16.

Try this on the system as it was yesterday:

```
taaj:/> write notes hello   world
```

Is that the file `notes` and the text `hello   world`? Or four arguments? `write` decided: it cut the text at the first space and kept the rest, spaces and all. Now:

```
taaj:/> rm my file
```

`rm` decided differently: it cut at *every* space, and tried to remove `my` and `file`. There was no way at all to name a file with a space in it — not to `rm`, not to `cat`, not to anything — because **every program was doing its own splitting of the line, each in its own way, and none of them could be told "this space is part of the name"**.

---

## The idea: split once, in one place

A line of text has to become *a command and its arguments* somewhere. There are two places it can happen:

| | Who splits | What a program receives |
|---|---|---|
| **Windows / DOS** | Each program (the C runtime does it at startup) | One string: the whole command line |
| **Unix** | The shell, before the program starts | A list: `argv[0]`, `argv[1]`, … |

TheTaaJ had drifted into the first by accident: the shell passed "everything after the command name" as one piece of text. That design has a long and unhappy history. On Windows, how quotes and backslashes are understood depends on which runtime a program was built with, so the same command line can mean different things to different programs, and passing an arbitrary string safely to another program is famously hard.

The Unix design puts the rule in **one** program — the shell — and gives everything else a list that is already divided. A program never sees a quote. It cannot get the rule wrong because it does not apply the rule.

So:

1. the shell divides the line into words, understanding quotes;
2. the words travel to the program **still divided**;
3. `taaj::arguments()` returns a list.

## 1. Dividing a line: `input::line::words`

The rules are the Unix shell's, as far as they go:

| Typed | Words | Rule |
|---|---|---|
| `write notes hello` | `write` · `notes` · `hello` | spaces separate, any number of them |
| `write notes "two  words"` | `write` · `notes` · `two  words` | inside `"…"` spaces are ordinary characters |
| `write 'my file' x` | `write` · `my file` · `x` | inside `'…'` *everything* is ordinary |
| `cd my\ docs` | `cd` · `my docs` | `\` makes the next character ordinary |
| `echo "say \"hi\""` | `echo` · `say "hi"` | `\` works inside double quotes too |
| `echo 'a\b'` | `echo` · `a\b` | …but not inside single quotes |
| `a"b c"d` | `ab cd` | pieces that touch are one word |
| `given "" x` | `given` · *(empty)* · `x` | empty quotes are a word with nothing in it |

Why two kinds of quote? Because each is how you write the other: `"it's"` and `'say "hi"'`. And single quotes having *no* exceptions means there is always one way to write text exactly as it is, without thinking about what might be special inside.

What is **not** there yet: variables (`$HOME`), wildcards (`*.txt`), and the characters the next steps will give meaning to (`>`, `<`, `|`, `&`). Today those are ordinary characters.

### The code: a small state machine

```rust
enum Within { Nothing, Single, Double }

pub fn words(line: &str) -> Result<Vec<String>, LineError> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut within = Within::Nothing;
    let mut characters = line.chars();

    while let Some(c) = characters.next() {
        match (within, c) {
            (Within::Nothing, ' ' | '\t') => words.extend(word.take()),
            (Within::Nothing, '\'') => {
                within = Within::Single;
                word.get_or_insert_with(String::new);
            }
            (Within::Nothing, '"') => {
                within = Within::Double;
                word.get_or_insert_with(String::new);
            }
            (Within::Single, '\'') | (Within::Double, '"') => within = Within::Nothing,
            (Within::Nothing | Within::Double, '\\') => {
                let next = characters.next().ok_or(LineError::TrailingBackslash)?;
                word.get_or_insert_with(String::new).push(next);
            }
            (_, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    ...
```

The reader is always in one of three states — outside quotes, inside single quotes, inside double quotes — and what a character means depends on the state. **`match (within, c)`** matches on the *pair*, so each arm reads as a line of the table of rules: "outside quotes, a space: the word is finished"; "inside single quotes, a single quote: back outside". The last arm, `(_, c)`, is "anything else, in any state, is part of the word". The order of the arms matters — the first that fits wins — which is why the specific ones come first.

Three pieces of Rust earn their place here:

**`word: Option<String>`, not `String`.** I first thought of the word being collected as a string that is empty between words. But `""` is a real argument — a word with no characters — and an empty string cannot tell "no word in progress" from "a word that happens to be empty". `Option` says it directly: `None` is *between words*, `Some("")` is *an empty word*. Opening a quote calls `get_or_insert_with(String::new)`, which makes the word exist without putting anything in it.

**`words.extend(word.take())`.** `take()` moves the value out of the `Option` and leaves `None` behind — "hand over the word and go back to being between words" in one call. And `extend` accepts an `Option` because an `Option` is an iterator of zero or one items: if there was no word (several spaces in a row), nothing is added. No `if`.

**`while let Some(c) = characters.next()`, not `for c in line.chars()`.** After a backslash the loop body has to take *one more* character itself. A `for` loop borrows the iterator for its whole run, so the body cannot touch it. `while let` asks for one item at a time and leaves the iterator free in between.

### A quote that is never closed

```
taaj:/> echo "unclosed
the line has a " that is never closed
```

The line is **not run**. A real Unix shell would print a second prompt and wait for the rest; that needs line editing I do not have. The other choice — quietly treating the rest of the line as quoted — means a typo runs a command I did not write: `rm "old notes` would try to remove something. Refusing is the only answer that cannot do harm. The same for a line ending in a backslash.

The errors are a closed set, so they are an enum, and the message is chosen from it:

```rust
pub enum LineError {
    OpenQuote(char),
    TrailingBackslash,
}
```

### Where the code lives

In the `input` crate, as `input::line`, next to the keyboard decoder: scancodes become characters there, and now a line of characters becomes words. It could not go in the shell itself, because the shell is built only for the OS and its code cannot be tested on the host; `input` is tested on the host already. I did not make a new crate for forty lines.

## 2. Keeping the words divided on the way

The shell hands the arguments to init in a message, init puts them in a channel, the program reads them (entry #16). All of that carried one piece of text. Now it has to carry a list — in a flat run of bytes.

The encoding is in `abi`, because it is part of the agreement between whoever starts a program and the program:

```rust
/// How a program's arguments travel: each one followed by a zero byte.
pub mod arguments {
    pub fn encode<'a>(words: impl Iterator<Item = &'a str>, out: &mut [u8]) -> Option<usize> { ... }
    pub fn decode(bytes: &[u8]) -> impl Iterator<Item = &[u8]> { ... }
}
```

```
write "my file" hello      →      m y ␠ f i l e \0 h e l l o \0
```

Three decisions in that:

- **A zero byte**, because it is the one byte that cannot be typed, so it can never be confused with part of an argument. Separating by spaces would have brought the whole problem back; separating by newlines would break the day an argument contains one.
- **After each argument, not between them.** With a separator *between*, no arguments and one empty argument would both be zero bytes long. With a terminator *after each*, nothing is `` and one empty argument is `\0`. This is exactly a C string, and exactly how Unix lays out `argv` — which will matter when the C library arrives (roadmap step 4): `main(argc, argv)` can point straight into this message.
- **An unfinished last argument is dropped.** If a message were cut short, the bytes after the last zero are not an argument. Half a file name is worse than none.

`decode` is nine lines and worth reading for its shape:

```rust
pub fn decode(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = bytes;
    core::iter::from_fn(move || {
        let end = rest.iter().position(|&byte| byte == 0)?;
        let (word, tail) = rest.split_at(end);
        rest = &tail[1..];
        Some(word)
    })
}
```

`abi` has no heap (the kernel uses it too), so it cannot return a `Vec`. It returns an **iterator** instead: `from_fn` turns a closure into one, calling it each time the next item is wanted. The closure owns `rest` (that is what `move` does) and shrinks it each call. `impl Iterator<Item = &[u8]>` in the signature means "some iterator of byte slices — you do not need to know which type". Every slice it yields is a view into the original message; nothing is copied. The `?` on `position` ends the iteration the moment there is no further zero.

init needed **no change at all**. It already treated the arguments as bytes it does not look at.

## 3. What programs get

```rust
pub fn arguments() -> Vec<String>      // was: -> String
```

Every program that took arguments got simpler, because each had been carrying its own little parser:

```rust
// cat, rm, mkdir, ls
for name in &arguments { ... }                    // was: arguments.split_whitespace()

// mv, cp
let [from, to] = &arguments[..] else {            // was: three calls to words.next()
    println!("usage: mv FROM TO");
    return 2;
};
```

That `let [from, to] = … else` is a **slice pattern**: it matches only a slice of exactly two elements and names both. Any other length takes the `else`. "Exactly two arguments, or print the usage" became one line that reads as what it means.

The shell's own commands use the same patterns:

```rust
"cd" => match arguments {
    [] => self.cd("/"),
    [directory] => self.cd(directory),
    _ => self.write(b"usage: cd [DIR]\n"),
},
"run" => match arguments {
    [name, arguments @ ..] => self.run_program(name, arguments, true, Grant::Usual),
    [] => self.write(b"usage: run NAME ARGS\n"),
},
```

`[name, arguments @ ..]` is "a first element, and call the slice of all the rest `arguments`" — the rest may be empty. Before, this was `split_once(' ')` and `trim()` and hoping.

`time COMMAND` and `with files COMMAND` take the words after them and run *those*; they are not turned back into text and divided again, so a quoted argument stays one argument through `time cat "my file"`.

### One visible change: `write` and extra spaces

```
taaj:/> write plain two  words         →  the file holds "two words"   (one space)
taaj:/> write quoted "two  words"      →  the file holds "two  words"  (both)
```

`write FILE TEXT` used to keep the text exactly as typed after the file name. Now `two` and `words` arrive as two arguments and are joined with one space — what `echo` does in every Unix shell. To keep spacing, quote the text. I think this is the right behaviour and it is the standard one, but it *is* a change, and the boot test now checks both lines above so that it cannot change back unnoticed.

`given` — the program that reports what it was started with — now shows each argument in quotes, so the division is visible:

```
taaj:/> given 'a b' c ""
given: the screen, no files
given: arguments "a b" "c" ""
```

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Who divides the line | The shell, once | Each program — every one would have its own rules, and none could be told a space belongs to a name |
| What travels | A list, each argument ending in a zero byte | One string; or space- or newline-separated — both can appear in an argument |
| Terminator or separator | Terminator | Separator — cannot tell "none" from "one empty" |
| A quote never closed | The line is refused | Guessing where it ends — runs a command nobody wrote |
| Single vs double quotes | Both, with the Unix difference | One kind — then the other character needs an escape everywhere |
| Unquoted text of several words (`write`, `echo`) | Joined by one space | Keeping the original spacing — impossible once the line is divided, and not what any shell does |
| Where the splitter lives | `input::line`, host-tested | In the shell — could not be tested without booting |

## The bugs

Small ones this time, and both mine in the first minute:

- **A quote inside the help text.** I added a help line showing `"two words"` — inside a Rust string literal that is itself delimited by `"`. The compiler's error was a wall of red showing the whole help text as "unexpected". Writing about quoting, I had failed to quote. The fix is `\"`, which is the very rule the new code implements.
- **`Vec` in a test of a crate that has no heap.** `abi` is `#![no_std]`, and its tests run on the host where `std` exists but is not in scope by itself. The test needed `extern crate std;` — a reminder that "no_std" is a property of the crate, not of the machine the tests run on.

No bug was found in the splitter itself by the OS tests; the ten host tests were written alongside it and passed. I am noting that because it is the kind of sentence that should make one suspicious rather than satisfied — so the "not tested" list below is where to look.

## How I checked it

**On the host** (`make kernel64-test`, part of `make test64`):

- `input`: 10 new tests of `words` — spaces and tabs; both quotes; each quote holding the other; pieces that touch; empty quotes (alone, repeated, and next to text); the backslash outside quotes, inside double quotes, and *not* inside single quotes; seven lines that must be refused; non-ASCII text.
- `abi`: the argument encoding round-trips; none versus one empty; a buffer one byte too small; an argument containing a zero byte refused; a message cut short.

**In the boot test** (BIOS and UEFI), 12 new lines are typed — they are the testing note below. After QEMU exits the host tool reads the disk and must find:

- `quoted` containing `two  words` with **two** spaces;
- `plain` containing `two words` with **one**;
- a file named `a file` in a directory named `my docs`, containing `x`.

The directory was made with double quotes, the file written with single quotes, and `cd` into it done with a backslash, so all three ways of writing a space are exercised against the real disk.

`make test64` passed. Every earlier command in the boot test — more than a hundred lines of them — runs through the new splitter, which is its own kind of test: nothing that worked before stopped.

**Not tested:**

- I have not typed any of this into a `make run64` window myself. The testing note is taken from the serial log of the automated boot, except the lines marked.
- **A backslash inside double quotes** (`"say \"hi\""`) is tested on the host only, never typed in the OS.
- **Arguments too long for the message** — the shell should say `the arguments are too long`. The line limit (256 characters) is smaller than the buffer, so I believe this cannot actually be reached, and I have not tried.
- **Typing the characters on a real keyboard.** The test sends key names to QEMU (`shift-apostrophe`, `backslash`); a keyboard with a different layout than US would produce different characters, and the driver knows only the US one.
- **Tab as a separator** is tested on the host; the shell ignores the Tab key when typing, so it cannot currently be typed.

## What is still missing

- **No continuation line**: an unclosed quote is refused rather than asked to be finished.
- **No variables, no wildcards**, no `~`.
- **`>`, `<`, `|` and `&` are ordinary characters** — the next steps. When they get meaning, the splitter must report whether a word was quoted, so that `">"` stays an ordinary argument. It returns plain strings today; that will change.
- **No line editing** beyond Backspace: no arrow keys, no history.
- **`write`/`append` take text only from arguments.** With standard input (next step) they will not need to.
- **A name with a space is legal on TaajFS** — and now reachable. `ls` prints it without quotes, so `a file` in a listing is ambiguous to the eye. Real `ls` has the same problem and a dozen options for it.

## What I learned

- **Put a rule in exactly one place.** Every program split its own arguments, so there were as many slightly different rules as programs, and no way to add quoting without changing each of them. With the split in the shell, programs got *shorter* and a file name with a space works everywhere at once.
- **This is an old fork in the road.** Windows passes a string and Unix passes a list, and I had taken the Windows turn without knowing there was one.
- **Terminate, do not separate**, when "empty" is a legal value. It is why C strings and `argv` look the way they do.
- **An `Option` is the honest type for "maybe nothing".** The moment `""` had to be a real word, an empty string could no longer double as "no word".
- **When input is ambiguous, refuse it.** A half-quoted line has no safe meaning.
- In Rust: matching on a tuple to write a state machine as a table; `Option::take` and `get_or_insert_with`; `extend` from an `Option`; `while let` when the body needs the iterator; slice patterns (`[a, b]`, `[first, rest @ ..]`, `[]`); `iter::from_fn` and `impl Iterator` to return a sequence without a heap.

---

## Testing note

```
make run64
```

Start in the root (`cd`). The characters: `"` is Shift+`'`; `\` is the key above Enter (US layout).

**Quotes keep spaces**

| Type | You should see |
|---|---|
| `write quoted "two  words"` (two spaces inside) | `quoted: 11 bytes` |
| `cat quoted` | `two  words` — both spaces *(the `cat` is not in the automated test; the host reads the file instead)* |
| `write plain two  words` (two spaces, no quotes) | `plain: 10 bytes` |
| `cat plain` | `two words` — one space *(same)* |
| `echo a"b c"d` | `ab cd` |
| `echo 'single "inside" quotes'` | `single "inside" quotes` *(not in the automated test)* |
| `echo "it's"` | `it's` *(not in the automated test)* |
| `echo "say \"hi\""` | `say "hi"` *(host-tested only)* |

**Names with spaces**

| Type | You should see |
|---|---|
| `mkdir "my docs"` | Nothing |
| `write 'my docs/a file' x` | `my docs/a file: 2 bytes` |
| `cd my\ docs` | Prompt `taaj:/my docs> ` |
| `pwd` | `/my docs` |
| `ls` | `       2  a file` and `1 file, 2 bytes` |
| `cat "a file"` | `x` *(not in the automated test)* |
| `mv "a file" b` then `ls` | `       2  b` *(not in the automated test)* |
| `cd` | Prompt `taaj:/> ` |

**What a program receives**

| Type | You should see |
|---|---|
| `given a b` | `given: the screen, no files` then `given: arguments "a" "b"` |
| `given 'a b' c ""` | `given: arguments "a b" "c" ""` — three arguments, the last one empty |
| `given a\ b` | `given: arguments "a b"` *(not in the automated test)* |
| `time given "a b"` | `given: arguments "a b"` then `took 0.0N s` — still one argument through `time` *(not in the automated test)* |

**What is refused**

| Type | You should see |
|---|---|
| `echo "unclosed` | `the line has a " that is never closed` |
| `echo it's` | `the line has a ' that is never closed` *(host-tested only)* |
| `echo end\` | `the line ends in a \ with nothing after it` |
| `cd a b` | `usage: cd [DIR]` |
| `mv "one two"` | `usage: mv FROM TO` — one argument, though two words *(not in the automated test)* |

**`help`** — the whole list should still fit on the screen, and include the line `(a word with spaces: "two words", 'two words', two\ words)`.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img quoted
tools/tfstool-rs/target/release/tfstool ls  build/taaj-disk.img "my docs"
```

The first prints `two  words` with two spaces; the second lists what you left in the directory.

**Cleaning up**, in the OS: `rm quoted plain "my docs/b"` (or `"my docs/a file"` if you did not rename it), then `rm "my docs"`.

Automated: `make test64`.
