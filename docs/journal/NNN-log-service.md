# The system log: the kernel's ring, logd, and a `log` command

*Journal entry #NNN · K136, the first item of Era 22 (observability). Written on TheJat-WSL/ci, by day, with quick checks only. Until now, the debug log was a serial line: whatever it said was gone unless someone was reading the serial port, and gone at every switch-off. Now everything it says is also kept on the disk, a line at a time, with the time, who wrote it and a level, and the `log` command reads it.*

## How it goes

```
kernel's messages ─┐
                   ├─> console.rs ──> serial line (as before)
programs' output ──┘        │
                            └─> logtap.rs ──> the ring (256 KiB, abi::log) ──> init ──> logd ──> fs ──> /var/log/system.log
                                                                                                                 ↑
                                                                                                           `log` reads it
```

1. **The ring** (`kernel/abi/src/log.rs`, shared by the kernel and `logd`). The memory has a header (magic, size, `written`, `first`) and a ring of records. Each record holds a piece of the debug log as its writer gave it, not a line: its kind (the kernel, a program, a program that started), the process id, the time in nanoseconds since 1970, and the name and text.
   - One writer, the kernel, under its own lock; one reader, `logd`, in another address space. Neither waits for the other.
   - When the writer comes round to records not read yet, it moves `first` past them *before* writing over them, and moves `written` on after. A reader that is behind is told how many bytes it lost. A reader that copied a record and then finds `first` past it drops that copy, because it may be torn.
   - Host tests: records read back as written, a long piece cut, a reader left far behind (what is left is the newest, whole and in order, and lost plus read adds up to everything written), a damaged length, and memory that is not a ring.
2. **The kernel** (`kernel/kernel64/src/logtap.rs`, new):
   - `start()` makes the ring as early as there is memory, before the self-tests, so it also has what came before any service. It lives in 64 contiguous pages read through the direct map. The kernel keeps a reference that it never gives back, so the pages outlive whatever init or `logd` do with theirs.
   - `console.rs` hands it each piece where text enters: the kernel's messages, written at once or held behind a program's open line, and `write_later` for programs. It is skipped while the kernel is stopping, so last words never wait for its lock.
   - `process::start` records each program's name as it starts.
   - In `start_init`, `offer()` hands the memory to init as `log`, the way the framebuffer is handed over. It can be read but not written, and init can copy it. The kernel says `[kernel] system log: 256 KiB ring, handed to init`. Both architectures do this.
3. **`logd`** (`user/src/bin/logd.rs`, new; embedded in the kernel like the other services):
   - Five times a second, it reads what is new. Each writer's pieces go back into lines (`syslog::Lines`): a program's `argc `, `1` and newline make one line even with the kernel's messages in between. A line left open for 2 s, such as a prompt, is written as it is.
   - It names programs by their start records, and as `pid N` when it did not see one.
   - It reads a level from each line's text, because programs do not say one yet: `crit` for what stops the system (a panic, a CPU exception, init ending); `error` for what failed ("could not", "would not", "giving up", "FAIL"); `warn` for what went wrong and was dealt with ("restarting", "missing", "no driver"); otherwise `info`.
   - It appends to `/var/log/system.log`. At 1 MiB the file becomes `system.log.1` (the one before is dropped), so the oldest lines go first. It writes `--- the system started ---` at each start, so boots can be told apart.
4. **`log`** (`user/src/bin/log.rs`, new; on the disk):
   - It shows the last 20 lines of `system.log.1` and `system.log`, read as one.
   - `-n N` sets how many lines, and `-a` shows all of them.
   - `-l LEVEL` shows that level and worse, and `-s WRITER` one writer's lines (`kernel` for the kernel's). `-b` shows only what came since the last start.
5. **The format** (the `syslog` crate, new, host-tested; its floor is 97%): `2026-11-28 06:50:59.222 hello      info  [hello] hello from user mode!`. `syslog::parse` reads it back, which is what `log` filters with.

## What it changed in other lanes' files

The user chose (A) for this: `ci` makes the hooks itself and says so here.

- **`user/src/bin/init.rs`** (`services`):
  - the kernel's `log` offer is taken;
  - grant 57 (the ring, read only);
  - a `logd` service with grants 57 and 11 (the files), started after `dev`, and given up at once if the kernel offered no ring;
  - `logd` added to the switch-off order before `dev`, `tmp` and `fs`, so its last writes go through `fs` before `fs` stops. Before this, it was not stopped at all and was cut off at power-off.
- **`user/src/bin/shell.rs`** (`desktop`'s terminal side): `log` added to `LOOK_PROGRAMS`, so it gets the file service, the channel that can only read.
- **TheKernel**: `console.rs`, `process.rs` and `main.rs`, one or two lines each; `programs.rs`, `logd` in both lists; `abi`'s `lib.rs`, `pub mod log`.
- `Makefile`: `log` in `DISK_PROGRAMS`, `log64-test`, and the `syslog` crate's tests in `kernel64-test`.

## Found on the way

- **The log could not go in `/log`.** The test disk has the `log` program itself at `/log`, so `logd`'s `mkdir log` answered "not a directory". The log now lives in `/var/log`, as on Unix.
- **The first ring was made too late.** It was made in `start_init`, after the self-tests. Their lines, `hello`'s among them, were on the serial line but not in the log. Now it is made before the self-tests.
- **`logd` started before the disk.** It starts after `dev` and before the disk's driver, so its first `mkdir` reached a file service with no disk yet. Now the directory is made whenever a write finds it missing.
- **The boot test counts the disk's root.** `boot64-test` lists it and expected 4 directories and 39 files (27 without the C compiler). With `log` on the disk and `logd`'s `/var`, it is 5 and 40 (28), and `dir  var/` is among them. K188's quick checks found this; K136's had not run `boot64-test`.
- **A line longer than 1024 bytes lost its rest.** The host test caught it before any boot did. Now the rest begins the next line.

## Checked

- **`log64-test`, new:** two boots of one PC on one disk. Each boot's self-test runs `hello` twice, before init and `logd` start.
  - First boot: `log -s hello` shows those two lines, with date, writer and level; `log -s kernel -n 2` shows two of the kernel's lines and no other writer's. Then `poweroff`, and init stops `logd` before `fs`.
  - Second boot: `log -a -s logd` shows both boots' start lines, `log -b -s logd` this boot's only, and `log -a -s hello` both boots' hellos, so the first boot's lines are kept.
- **Quick checks:**
  - `kernel64-test`, with `abi`'s ring tests and `syslog`'s;
  - `coverage`: `kernel/abi` went from 87.2% to 88.9%, `syslog` is at 97.0%;
  - `unsafe-check`, after adding a `SAFETY` comment to each `unsafe impl`;
  - `power64-test`: init's switch-off order changed, and the order the test checks still holds;
  - `aarch64-test`: the ring is made, and offered, there too.
- The full `make test64` is for the night.

## For later

- **Levels from the programs:** a line could carry its own level (`[fs] warn: ...`), and the log service would use it instead of reading one from the text.
- **K188**, a crash record the next boot finds, and **K138**, a crash report per program, now have somewhere to go: the log.
- **The kernel's last words:** while it is stopping, the kernel writes to the serial line only, and the ring is skipped. K188 is how those reach the next boot.

## Testing note

| Do | You should see |
|---|---|
| Boot with a disk, then `log` | The last 20 lines: date, writer, level, text |
| `log -l warn` after F12 (the keyboard driver crashes) | `init ... kbd ended ... - restarting it` as `warn` |
| `log -s kernel -n 5` | The kernel's last five lines |
| `poweroff`, boot again, `log -a -s logd` | `--- the system started ---` once for each boot |

Automated: `make log64-test` (part of `make test64`).
