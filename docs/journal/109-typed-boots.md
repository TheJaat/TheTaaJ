# The typed boots wait for the right thing: test64 in 6½ minutes

*Journal entry #109 · a test fix, in TheKernel's boot scripts. Written on TheJat-WSL. The typed boots (`boot64-test`, `boot64-uefi-test`, `aarch64-keys-test`, and the screen and editor tests) spent more than half their time in waits that could only end by running out: about 240 s of a 434-second boot. A wait now ends when the line it waits for has run, by that line's own text. `make test64` on TheJat-WSL: 626 s after entry 108, 389 s now; 994 s this morning.*

## Why

The serial log of `boot64-test`, every line timestamped as it came, showed the work done by 285 s and long silences that were waits running out:

| Silence | At | What waited, and for what |
|---|---|---|
| 94 s | before the first key | `/` after F12: one more "keyboard ready" than now. Under KVM the crashed driver is back within the 50 ms after F12, so "now" already counted it, and the wait was for one more that never came - 90 s, `FIRMWARE_SECONDS` since entry 105 |
| 86 s | after `time wait` | `~~`: every Enter so far to have run. Enters typed into programs (`cask`, `cat`, `wc`) never "run" as the shell's lines, so that count is never reached: 90 s |
| ~10 s, five times | before the Ctrl+C lines | `~`, the same count: 10 s each |
| 10 s | at the end | the same count, for all 237 Enters |

## How it fits together

`kernel/kernel64/scripts/` (TheKernel):

- **`typed_line.py`** (new): from the keys, for each Enter, the line the shell logs once it has run that line - `[shell] ran "TEXT"`, TEXT quoted as Rust quotes it. Backspace removes a character; a Ctrl key, a pointer event or a monitor command starts a new line (what came before went to a program or a window); F11 and F12 are not text. A line with arrows or other escapes has no text, and the wait falls back to counting.
- **`~` and `~~`** wait until the line just entered has run: its own "ran" line appears once more than at its Enter. Counting "ran" lines was not enough: on emulated AArch64 the shell falls behind the typing, and the "ran" of `cfloat 4 &` came after `time wait`'s Enter - the wait ended at once, and the next keys went into `wait` and stopped it.
- **`/`** waits for one more "ready" line than there was before the last key was sent, not when the wait begins.
- **Ctrl+C** in `serial-type.py` waits until a program has started since the last Enter (`... for the shell` from init), then 0.3 s, as before. The fixed pause alone let a Ctrl+C reach the shell before `cat` had started, under load.
- `qemu-boot.sh` (PS/2), `serial-type.py` (AArch64's serial port) and `qmp-type.py` (the screen tests) all do the same; `serial-type.py` takes its key table from `typed_line.py`. `.gitignore` gains `__pycache__/`.

What is typed, and the 50 ms between keys, are unchanged.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Wait for the line's own "ran" | One "ran" more than at the Enter | Tried first: under emulation another line's late "ran" ended the wait - `aarch64-keys-test` failed twice in two runs |
| The text worked out from the keys | Marking program input in the key lists | No key list changes; every list in the tests is covered at once |
| Counted before the key is sent | When the wait begins | Under KVM what the key causes can already be in the log by then |
| A program seen started before Ctrl+C | A longer fixed pause | Right on any machine, at any load |
| Typing kept at 50 ms a key | Typing faster | "A real typist, not a burst": the waits were the cost, not the typing (about 170 s of the 434) |

## Bugs

- **`/` after F12 waited 90 s every boot.** Evidence: 94 s of silence after "[kbd] PS/2 keyboard ready" at 26.7 s, the next ready line at 26.74 s. Cause: the ready lines counted when the wait began, after the restart. Fix: counted before each key. Proof: typing starts within a second of the restart.
- **`~` and `~~` ran out every time** once a line had gone to a program. Evidence: silences of 86 s and five of about 10 s, ending at their limits. Cause: they waited for as many "ran" lines as Enters. Fix: the line's own "ran". Proof: the typing phase of `boot64-test` 431 s before, 196 s after.
- **A later "ran" could end a wait early** (my first fix). Evidence: `aarch64-keys-test`, twice: `time wait` stopped by keys typed into it, then `abc` and Ctrl+C reaching the shell before `cat` ran, and `cat` taking every line after - "lsout is not empty". Fix: the line's own text; Ctrl+C after a started program. Proof: three full runs passed.
- **The editor tests' last line**: a `z` typed into the editor, closed with a click, was taken as the start of `echo done`. Fix: a pointer event starts a new line. Proof: no "did not run" from `edit64-test` or `aarch64-edit-test`.
- Seen once, not again: `tcc64-test` failed `clib`'s five file-time checks in one full run. It passed three times alone with these changes, three times without, and in the seven full runs after. Its log was written over; noted, not explained.

## How I checked it

On TheJat-WSL (KVM, 24 CPUs), with Ubuntu 24.04's LLVM 18.1.3:

- `typed_line.py` on `TEST_KEYS`: 237 Enters, every one decoded; every shell line's expected "ran" line is in a real `serial64.log`, and the ones that are not are the lines typed into programs.
- `make test64`, three times in a row after the last change to the waits: all passed, 389, 396 and 389 s; the last with no "did not run" from any typer.
- `bash -n` and `dash -n` for `qemu-boot.sh` (it runs under `/bin/sh`), `py_compile` for the Python.

| | Before (entry 108) | After |
|---|---|---|
| `boot64-test`'s typing | 431 s | 196 s |
| `make test64` | 626 s | 389 s |

## What is still missing

- The longest parts now: the emulated AArch64 ones (clang, libmake, llvm, `aarch64-keys-test`) and `e1000-64-test`, whose seven boots run one after another and one of which takes 98 s where the same check with the other card takes 17.
- `make test64` relinks LLVM's tools even when nothing changed.
- Not yet timed on TheJat-VM and MaK-WSL; the waits saved are the same everywhere.

## What I learned

- **A wait that ends by running out looks like work.** Nothing failed; the suite was just slow. Only timestamps on every line showed it.
- **"Some line ran" is not "this line ran".** Count what you mean.
- **Faster tests move the load around.** Removing waits put the AArch64 typing next to the heavy compile tests, and a fixed pause that had always been enough was not.

## Testing note

From the root of TheTaaJ, with nothing else booting on the machine:

```
make test64 2>&1 | tee build/test64.out
grep "did not run" build/test64.out
```

| Look for | Meaning |
|---|---|
| `test64: all passed`, about 6½ minutes under KVM (before: about 10½) | The waits end when their lines have run |
| Nothing from the `grep` | No typer waited out its time for a line |
