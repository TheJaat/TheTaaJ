# A crash report per program

*Journal entry #165 · K138 (Era 22). Written on TheJat-WSL/ci, by day, with quick checks only; it stands on K136 (the system log) and K188 (the crash record).*

## Before

A program that breaks a rule, with no handler for the signal, is ended by the kernel with two lines on the serial line. It may have read memory it was not given, run data, or divided by zero. The two lines read:

```
[process] page fault at 0x400015 in user mode
    user read at 0xffffffff80100000: protection violation
```

That says where, but not what the program was doing, what it had printed, or what its memory looked like. Those are the questions anyone debugging it asks first.

## Now

```
2026-11-28 07:44:45.124 peek       info  [peek] reading the kernel's code at 0xffffffff80100000...
2026-11-28 07:44:45.127 peek       error crash report: peek (pid 2)
2026-11-28 07:44:45.127 peek       error broke a rule: page fault at 0x400015, reading 0xffffffff80100000: not allowed
2026-11-28 07:44:45.127 peek       error registers:
2026-11-28 07:44:45.127 peek       error     rip 0x0000000000400015    rsp 0x00007ffffffeffd0    rbp 0x0000000000000000 rflags 0x0000000000010202
...
2026-11-28 07:44:45.127 peek       error stack at 0x7ffffffeffd0: words that point into its code (likely return addresses, newest first):
2026-11-28 07:44:45.127 peek       error    sp+0x18   0x400058
2026-11-28 07:44:45.127 peek       error memory:
2026-11-28 07:44:45.127 peek       error    0x000000400000-0x000000402000 r-x its program
2026-11-28 07:44:45.127 peek       error    0x000000402000-0x000000403000 r-- its program
2026-11-28 07:44:45.127 peek       error    0x000000403000-0x000000404000 rw- its program
2026-11-28 07:44:45.127 peek       error    0x7fffff7f0000-0x7fffffff0000 rw- stack
```

The same report is also kept in a file of its own, `/var/log/crashes/2026-11-28-074445-peek-2`, with the program's last lines, and the newest also as `/var/log/crashes/latest`:

```
crash report: echod (pid 19), 2026-11-28 07:45:22.777
broke a rule: page fault at 0x40039c, reading 0x0: nothing there
...
the last lines it printed (3):
   [echod] a client opened a private channel
   [echod] upper-cased text in shared memory, in place
   [echod] asked to crash: reading address 0
```

The serial line says what it said before.

## How

- **The kernel gathers** (`process::report_current`), at the fault, just before ending the program:
  - what it did, in words: the fault, and for a page fault whether it was reading, writing or running, which address, and whether there was nothing there or it was not allowed;
  - every register, by name: on x86_64 `rip`, `rsp`, `rbp`, `rflags`, the fifteen general ones and the error code; on AArch64 `pc`, `sp`, `spsr` and `x0`-`x30`;
  - up to 512 bytes from the top of its stack (`copy_from_user`, inside its stack only);
  - its memory: the loadable segments from its ELF program headers, which the kernel already keeps, with what each allows; its stack; and its mappings (memory objects, reserved memory).
- **`kernel64::crashreport`** (the kernel's library, host-tested) turns that into text.
  - The ELF segments are read from the headers.
  - The backtrace is the honest part. Programs are not built with frame pointers, so there is no chain of frames to walk. Turning them on would change every program's code, and Era 9 compares compiler output byte for byte. So the report lists the words near the top of the stack that point into the program's code, which are the likely return addresses, says that that is what they are, and gives their places on the stack. When there is none, it says so.
- **Into the system log's ring** (`logtap::report`) as a new kind of record, `abi::log::kind::REPORT`, with the process id. This is the "user-space service the kernel tells" the item asked for: the log service, told through the ring it already reads.
- **`logd`** (`syslog::Lines::report`):
  - It writes out the program's unfinished line first.
  - It logs the report a line each, under the program's name, all `error`: a program broke a rule, and the system went on. `crit` is for the system stopping.
  - It has always seen every line every program printed. Now it keeps each program's last 10 lines (for up to 64 programs, the ones that printed last). It adds them to the report's file, and writes that file and `latest`, retrying if the disk is not there yet.

## In other lanes' files

- **`kernel64::regions`**: `Regions::iter()`, to list a program's mappings. It is the memory manager's, and nothing else changed there.
- **`process.rs`**: `report_current()`, the gathering. It needs the process's private fields.
- **`arch/x86_64/idt.rs`**, **`arch/aarch64/vectors.rs`**: a call to it before `kill_current`, with the registers.

## Checked

- **The self-test makes it real at every boot.** `peek`, `cli`, `port`, `memro` and `gs` break rules on purpose, and `echod` is asked to crash, so a boot leaves twelve reports, and the tests read them.
- **`log64-test`**: its first boot now also
  - lists `/var/log/crashes` and finds `peek`'s two;
  - shows `peek`'s report with `log -s peek`, checking what it did, its registers and its memory;
  - reads `latest`, checking it is a report with its program's last lines.
- **Host tests:**
  - `crashreport`: segments from headers, including torn ones; words into the code, at most twelve; the report's form, including when nothing points into the code;
  - `syslog`: the last lines kept, ten each for up to 64 programs; the report's lines and file; its file name.
- **Quick checks:**
  - `kernel64-test`, `coverage` (`kernel/kernel64` at 91.9%, `syslog` at 97.5%) and `unsafe-check`;
  - `boot64-test` and `aarch64-test` (the report on both);
  - `crash64-test` and `fault64-page-test` (the kernel's own stops are unchanged);
  - `kill64-test`: it still passes, with disk and network drivers made to die in the middle of their work while `logd` writes.
- The full `make test64` is for the night.

## For later

- **Frame pointers in programs** would give a real backtrace. It is one flag, once Era 9's comparison can take it.
- **Symbols:** user programs' names for their addresses, from their ELF files on the disk. The log service could look them up.
- **A report for a program a signal ends** (its handler not set, the default being to end). Today only the CPU's faults make one.

## Testing note

| Do | You should see |
|---|---|
| Boot, then `ls /var/log/crashes` | A report for each program the self-test made break a rule |
| `log -s peek` | Its report as `error` lines: what it did, its registers, its stack, its memory |
| `cat /var/log/crashes/latest` | The newest report, with the last lines its program printed |

Automated: `make log64-test` (part of `make test64`).
