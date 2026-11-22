# A text editor

*Journal entry #68 · Era 5, step C. `edit FILE` opens a file in a window of its own: type, delete, move with the arrows, Home, End and the page keys, click to place the cursor, and save with Ctrl+S. The editing itself, the lines, the cursor, the view that follows it and the keys as they arrive in bytes, is a new module, `gui::text`, with no machine in it and tested on the host. A boot test types into a new file and a 300-line one on both machines, then reads the saved files back off the disk on the host and compares them byte for byte.*

```
[edit] new file note
[display] window 2 "edit note" opened, 924x512 at (98, 150)
[edit] window 2 open, 924x512: 76 x 24 characters
[edit] saved note: 22 bytes, 2 lines
[edit] quit
[edit] opened long: 301 lines, 4692 bytes
[edit] view from line 24
[edit] view from line 47
[edit] saved long: 4696 bytes, 301 lines
[edit] unsaved changes: ctrl+q again to drop them
[edit] quit
```

## Why

Era 5's goal is to write a journal entry in a text editor running in a window on TheTaaJ. Step A gave the system a screen and a pointer, step B gave it windows. This step is the editor. With it, writing a file on TheTaaJ no longer means `write FILE TEXT`, one line at a time, replacing what was there.

What an editor needs, at the least:

- **to read a file and write it back unchanged** where nothing was edited, down to whether it ends in a newline;
- **to take keys**: characters, Enter, Backspace, Delete, the arrows, Home, End and the page keys, which reach a window as bytes and escape sequences;
- **a view** that follows the cursor through a file longer and wider than the window;
- **a way to save and a way out**, the way out asking first if there are unsaved changes, and the close box doing the same.

## The picture

```
  keyboard driver ──bytes──▶ display ──KEY──▶ edit ──▶ gui::text::Editor
                                                         │  lines, cursor, view
                                     ◀──DAMAGE── draws ◀─┘  "these rows changed"
                                                         │
                  files ◀──READ / TRUNCATE + WRITE── save ◀┘  Ctrl+S
```

`edit` is a program on the disk, like `counter`. The shell gives it two services by name: "windows", and "files", the channel that can change files. It also passes on the directory it was started in, so `edit notes` inside `docs` means `docs/notes`.

## The text: `gui::text`

The text is a list of lines, each a list of `char`s: `Vec<Vec<char>>`.

- **Not one long string.** Nearly every key changes one line, and a line is short, so inserting into it is cheap. A file of 4,700 bytes is 301 small vectors.
- **`char`, not bytes.** A Rust `String` is UTF-8: "é" is two bytes, "€" three. The cursor's column would then be a byte offset that has to land on a character's start, and every move would walk the bytes. With a `Vec<char>`, column 7 is index 7. It costs four bytes per character in memory, so the editor refuses files over 1 MiB (`FILE_MAX`).
- **A file is the lines joined by `'\n'`.** Reading splits on `'\n'` with `str::split`, not `str::lines`. The difference matters: `"one\n".split('\n')` gives `"one"` and `""`, two lines, and joining them gives back `"one\n"`. `lines()` would drop that last empty line, so a file's final newline would be lost on every save. That is why `long` (300 lines, each ending in a newline) opens as 301 lines.
- **Text only.** A file that is not UTF-8 is refused (`NotText`): editing bytes as if they were characters would corrupt them. The font has only ASCII, so other characters are drawn as `?` (`glyph`), but kept in the text and saved as they were.

The cursor is a line and a column. The column can be one past the last character: that is "at the end of the line". Beside it, the editor keeps `wanted`, the column that Up and Down try to stay in. Going down through a short line moves the cursor to that line's end, and the next line down puts it back in its old column. The arrows, Home, End, typing and clicks set `wanted`; Up, Down and the page keys keep it.

Each key is one arm of a `match` in `Editor::command`:

| Key | What it does |
|---|---|
| a character | inserted at the cursor |
| Tab | spaces, to the next multiple of 4 |
| Enter | the line split at the cursor (`Vec::split_off`), the rest becomes the next line |
| Backspace | the character before the cursor; at a line's start, this line joined onto the one above |
| Delete | the character under the cursor; at a line's end, the next line joined onto this one |
| Left / Right | across line ends too |
| Up / Down, Home / End | |
| Page Up / Page Down | the view and the cursor together, by the view's height less one line, so the line that was last is now first |
| Ctrl+S | `Outcome::Save`: the program writes `text()`, then calls `saved()` |
| Ctrl+Q | `Outcome::Quit` if there is nothing unsaved; else `Outcome::Warned`, and a second Ctrl+Q straight after quits. Any other key in between starts that over |

### The view

After every key, `follow` moves the view so the cursor is in it: up if the cursor went above the first row, down if it went below the last, and sideways the same way for a line wider than the window. Then it records what must be drawn again:

- if the view moved, everything (`Damage::All`);
- otherwise the row the cursor left and the row it is on (`Damage::Rows`), which covers a typed character too;
- Enter and the joins move every line below, so they mark from their row to the bottom.

The program draws only those rows, and sends the window server one DAMAGE for them and the status line. Typing a character redraws one row of 912 x 20 pixels, not the whole window.

### Keys from bytes

Keys reach a window as the bytes the keyboard drivers make (`input::terminal_bytes`, entry 061). A letter is one byte, but an arrow is three (`ESC [ A`), Delete four (`ESC [ 3 ~`), and "€" three bytes of UTF-8. `gui::text::Keys` turns them back into keys, one byte at a time, because nothing promises a key's bytes arrive in one message. It is a small state machine:

```
Ground ──ESC──▶ Escape ──[──▶ Csi(number) ──digit──▶ Csi(number*10 + digit)
                   │                │
                   │ anything else  └─ A B C D H F ~ ─▶ the key; back to Ground
                   ▼
              that byte, read again in Ground (the Escape key alone means nothing)
```

UTF-8 is the second kind of state. The first byte of a character says how many follow (110xxxxx: one more, 1110xxxx: two, 11110xxx: three), and the parser collects that many continuation bytes (10xxxxxx) before it hands over the character. If something else arrives in the middle, the character was cut short and is dropped. Then the next byte is read as new. The mutation experiments found that my first test of this was too weak; see Bugs.

## The program: `edit`

`edit` is about 300 lines. It reads the file, opens the window, and passes events on:

- **Opening.** `stat` first: a directory is refused, a file over 1 MiB too, and "not found" means a new, empty file. Then `Files::read`, which delivers the file a message at a time, into a `Vec<u8>`, and `Editor::load`.
- **The window.** 76 x 24 characters, each cell 12 x 20 pixels (the font's 5 x 8 at twice the size, with a column and four rows of space), a status line under them, and 6 pixels of space around: 924 x 512. Its title is `edit NAME`, cut at a character's start if it is longer than the protocol's 32 bytes. Cutting in the middle of a character would leave a title that is not valid text.
- **The cursor.** A block in the text colour, with the character under it drawn in the background colour, when the window has the focus. Without the focus, only a line under where it is. A FOCUS event marks the cursor's row (`touch_cursor`) so it is redrawn in its new look.
- **The status line.** The file's name, the line out of how many, the column, `[modified]`, and the last message (`a new file`, `saved, 22 bytes`, `not saved: …`, the Ctrl+Q warning).
- **Clicks.** A press of the left button in the text becomes a cell (`x / 12`, `y / 20`), and `Editor::click` puts the cursor on that cell, or on the nearest place in the text.
- **Saving.** `Files::write`, which truncates the file and writes it a message at a time. On success, `saved()` and `saved, N bytes`. On failure, the error in the status line, and the text stays marked as modified.
- **The close box.** `Editor::close` follows the same rule as Ctrl+Q: it closes at once when everything is saved, and only warns the first time otherwise.

What `edit` says for the record goes to the kernel's log, as `counter`'s does: the file opened, every save with its size, the view moving (`view from line 47`), the warning, quitting. Its standard output is the terminal window, which a test does not read.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| The text in memory | A list of lines, each a `Vec<char>` | One `String`: every insertion moves the rest of the file, and columns are byte offsets. A gap buffer or a rope: faster for huge files, more to explain, and journal entries are small |
| Where the editing lives | `gui::text`, a module of the existing `gui` crate, tested on the host | Inside the program: no host tests. A crate of its own: one more crate for a few hundred lines that already belong with windows |
| Splitting lines | On `'\n'`, joined back with `'\n'` | `str::lines`: drops the final newline (and would take `\r\n` apart), so a file would change on every save |
| Not UTF-8 | Refused | Edited as bytes: every non-ASCII character would be split into pieces |
| Characters beyond ASCII | Kept, drawn as `?` | Refused: a file with one "é" could not be edited at all. A bigger font: later |
| Tab | Spaces to the next multiple of 4 | A tab character: one character as wide as several cells, so the cursor's column and the screen's column would differ. Tabs already in a file are kept and drawn as `?` |
| Page keys | The view and the cursor together, by a page less one line | Only the cursor: Page Down would move it to the bottom of the view without scrolling at all |
| Leaving with unsaved changes | Warn, and drop them on the second Ctrl+Q (or the close box) | Ask with buttons: a dialog needs a toolkit this system does not have yet. Saving without asking: Ctrl+Q would no longer mean "throw it away" |
| Redrawing | Only rows the editor marks, and one DAMAGE | The whole window per key: 924 x 512 pixels copied twice (to the back buffer and to the screen) for one letter |
| Its services | A new shell list, `EDIT_PROGRAMS`: "windows" and "files" | Every window program given the files: `counter` has no need for them |

## Bugs

- **My test of characters cut short was too weak to catch the bug it was about.** When a UTF-8 character is interrupted by a byte that is not one of its continuation bytes, the parser must forget the character. I tested `E2 82 a`: two bytes of "€", then `a`, expecting just `a`. Mutation H6 removed the line that forgets, and every test still passed, because `a` came out right either way. The bug shows only later: a stray continuation byte after the `a` completes the old character, and a "€" appears that nobody typed. The test now sends `E2 82 a AC b` and expects `a`, `b`. With the mutation it gets `a`, `€`, `b` and fails.
- **The first Ctrl+Q was invisible to a test.** The warning was only a message in the status line, and a boot test cannot read the screen's text. So it could not tell "warned, then quit" from "quit at once". `Editor::command` now returns `Outcome::Warned`, and the program logs it. Not a bug in the editor, but in its testability, found while writing the boot test.

## How I checked it

Automated, on the host: `gui` has 38 tests, 16 of them new in `text`:
- every key's bytes, including Insert and a lone Escape (both meaning nothing), and characters of two, three and four bytes, whole and cut short;
- a file coming back byte for byte, with and without a final newline, with tabs and "é"; bytes that are not UTF-8 refused;
- typing, Enter and Backspace undoing each other, Delete joining lines, Tab stops;
- Up and Down keeping the column through a short line; Left and Right across line ends;
- the view following the cursor down and back, the page keys (the view never running past the last line), a long line scrolling sideways;
- Ctrl+S, Ctrl+Q with and without unsaved changes, the close box, a click past the end of a line or below the last line;
- which rows each change marks for drawing.

Automated, by booting: `make edit64-test` and `make aarch64-edit-test`, both added to `make test64`. One boot each:
1. `edit note`: a new file. It types `hello tajj`, two Backspaces and `aj`, a new line, `secnd line`, Home, Right three times and `o`, then Ctrl+S (22 bytes) and Ctrl+Q, which quits at once.
2. `edit long`: Page Down twice (the log says the view is at line 24, then 47), `XYZ ` typed at the start of line 47, Ctrl+S. Then `q`, Ctrl+Q (warned), Ctrl+Q (quits).
3. `edit note` again: it reads back 22 bytes and 2 lines. A click after `hello `, then `big ` and Ctrl+S (26 bytes). Then `z`, the close box (warned), and the close box again (closes).

Then, on the host, `tfstool get` copies `note` and `long` off the test disk. They must be exactly `hello big taaj\nsecond line` and the 300 lines with `XYZ ` before line 47, so the `q` and `z` were not saved. A screenshot taken during step 1 is checked at three pixels: the cursor after `seco`, the status line, and the empty text. The window positions the test clicks at are first checked in the log.

New lines in the editor are typed as Ctrl+J in the test, not Enter. The editor gets the same byte (10) either way. But the boot script counts every Enter it types as a command for the shell, and waits at the end for the shell to have run that many.

Mutation experiments, host: 10 mutants, all caught, each by a named test, H6 only after the test above was strengthened:
- H1: Backspace dropping the line it joins;
- H2: Up and Down forgetting the wanted column;
- H3: Page Down not moving the view;
- H4: sideways scrolling off by one;
- H5: `lines()` for `split('\n')`;
- H6: a cut-short character not forgotten;
- H7: Ctrl+Q not quitting a saved file;
- H8: Enter redrawing only its own row;
- H9: Tab always inserting four spaces;
- H10: a click past a line's end not pulled back.

Mutation experiments, by booting: 4 mutants, all caught by `make edit64-test`.
- Saving one byte short: the log said 21 bytes, not 22.
- The shell giving `edit` no "files": it said so on standard output and opened no window at all.
- The cursor drawn half a cell to the left per column: only the screenshot caught it.
- A click's column counted in cell heights (20 pixels, not 12): the click landed three cells in, not six, and `big ` went into the middle of `hello`. The log was fine; reading the file back off the disk caught it.

The testing note's first three parts, run as scripted boots on a scratch disk with entry 067 on it as `journal.md`. `hello.md` came back off the disk as exactly `# Hello\nwritten on TheTaaJ`, without the `x` typed before the two Ctrl+Qs. `journal.md` opened as 325 lines, and Page Down moved the view to line 24. On the long line below, End put the cursor at column 84, in the view's last cell, with the whole view shifted sideways.

By hand: the screenshots of those boots and of the PC test run, looked at. The window over the terminal, `hello taaj` and `second line` in it, the block cursor on the `n` of "second" with the letter cut out of it, and the status line ` note  line 2/2, column 5  saved, 22 bytes`. Entry 067 in the editor, its box-drawing characters as rows of `?`.

Not tested:
- A person typing in QEMU's window. Every key was sent through QEMU's monitor and QMP. I have not written a journal entry in it yet, and that is Era 5's goal.
- A file over 1 MiB, a directory, a file that is not UTF-8, and a save that fails (a full disk, or the read-only files). The checks exist, and `NotText` is tested on the host, but no boot does any of them.
- Characters beyond ASCII typed on a keyboard: the keyboard drivers have only a US layout, so they never send one. The parser is tested on the host with "é", "€" and an emoji.
- Shift with the arrows, selecting, and anything else the editor does not do.

## What is still missing

- Undo; selecting, copy and paste; search and replace.
- Long lines wrap nowhere: the view scrolls sideways instead.
- Resizing the window, and so the editor's 76 x 24.
- A font beyond ASCII, so `?` stops standing in for every other character.
- Dialogs: "save as", "open", and a question with buttons instead of "press Ctrl+Q again".
- Ctrl+Home and Ctrl+End (the drivers send Ctrl with letters only), and a way to go to a line.

## What I learned

- An editor's hardest parts are small decisions, not data structures. What Backspace does at the start of a line, what column Down lands in, how far a page goes, what "the end of the file" is when the file ends in a newline. Each was one line of code and one test.
- `split` and `lines` look alike and differ exactly where it matters: the last newline of every file.
- A `Vec<char>` per line makes a column an index. Spending four bytes per character to get there is a good trade at this size.
- A byte parser needs a test for what happens *after* a broken input, not only during it. The mutation that survived showed my test checked the wrong moment.
- Testing through the disk closes the loop. The log says "saved, 22 bytes", but only reading the file back on the host shows that the bytes are the ones typed and that the dropped changes were really dropped.

## Testing note

**1. The PC: write a file**

```
make run64
```

| Type | You should see |
|---|---|
| `edit hello.md` | a window "edit hello.md" over the terminal, with a block cursor at the top left and a status line ` hello.md  line 1/1, column 1  a new file`; in the terminal you started QEMU from: `[edit] new file hello.md` |
| `# Hello`, Enter, `written on TheTaaJ` | the text as typed; the status line shows `line 2/2, column 19  [modified]` |
| Backspace a few times, then retype | characters go and come back; Backspace at the start of line 2 joins it onto line 1, Enter splits it again |
| the arrows, Home, End | the cursor moves; Up and Down from the end of a long line through a short one come back to the same column |
| click anywhere in the text | the cursor jumps there (or to the end of that line) |
| Ctrl+S | `saved, N bytes` in the status line, `[modified]` gone; in the log: `[edit] saved hello.md: N bytes, 2 lines` |
| type something, then Ctrl+Q | `unsaved changes: ctrl+q again to drop them` |
| Ctrl+Q | the window closes, and the shell has the keys again |
| `cat hello.md` | what was saved, without the last change |

**2. A long file: scrolling through a journal entry**

Put the previous entry on the disk first, with QEMU closed (the disk exists after the first `make run64`):

```
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img journal.md docs/journal/067-windows.md
make run64
```

| Type | You should see |
|---|---|
| `edit journal.md` | `# Windows` at the top; `[edit] opened journal.md: 325 lines, …`. Its dashes and arrows are drawn as `?`: the font has only ASCII |
| Page Down | the view moves 23 lines, and the old last line is now the first; `[edit] view from line 24` |
| hold Down | the view scrolls a line at a time once the cursor reaches the bottom |
| End on a line longer than 76 characters | the view scrolls sideways |
| the close box with unsaved changes | the status line warns; a second press closes |

**3. Write a journal entry, and copy it to the host**

```
make run64
```

In TheTaaJ: `edit entry.md`, write, Ctrl+S, Ctrl+Q. Close QEMU, then on the host:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img entry.md > entry.md
```

`entry.md` holds exactly what was saved.

**4. AArch64**

```
make run-aarch64-screen
```

The same as 1 to 3, in QEMU's window, with `build/aarch64/taaj-disk.img` as the disk. It is the disk `make run-aarch64` uses, so close any other AArch64 QEMU first.

(I drove these through QEMU's monitor and QMP, as the automated tests do, and looked at a screenshot; not with my own hands on the keys.)

**5. The tests by themselves**

```
make edit64-test
make aarch64-edit-test
```

| Look for | Meaning |
|---|---|
| `check-pixels: 3 pixels as expected` | the cursor, the status line and the text drawn where they should be |
| `edit64-test passed` / `aarch64-edit-test passed` | every step of the three editing sessions, and the files on the disk exactly as typed, the unsaved changes not in them |

The editor's side of the story: `grep -a 'edit\]' build/edit64.log`. The files it saved, as read back: `build/edit64.log.note.got` and `build/edit64.log.long.got`.

Host checks:

```
cd gui && cargo test           # 38 tests, 16 of them the editor's (gui::text)
```

Automated: `make test64`
