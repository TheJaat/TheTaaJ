# A debugger: stop a program, read its registers and memory, catch it at a breakpoint

*Journal entry #NNN · K142 (Era 22), its first two parts. Written on TheJat-WSL/ci, by day, with quick checks only. The item has three: "a debug capability over another process (stop, read and write its memory and registers, breakpoints), given only by its launcher; a small debugger program; a gdb stub on the serial line for the kernel on a real PC". The third, for the kernel itself on a machine no QEMU runs, is a new item of its own: K?-gdb-stub.*

## A session

From a boot like `debug64-test`'s (the commands it types, one at a time):

```
debug forever spin
debug: started forever; commands: stop, wait, cont, regs, x, w, break, kill, quit
stop
debug: thread 0 of pid 49 stopped as asked, at 0x40043b
regs
debug: thread 0: pc 0x40043b  sp 0x7ffffffefb40  flags 0x10297
 rax 0x0000000000000000   rbx 0x0000100000000000   rcx 0x0000000000402a67   rdx 0x0000000000000026
 ...
x 400000 16
0x400000: 55 41 57 41 56 41 55 41 54 53 48 81 ec 78 04 00
break pc
debug: a breakpoint at 0x40043b
cont
debug: going on
wait
debug: thread 1 of pid 49 stopped at a breakpoint, at 0x40043b
regs
debug: thread 1: pc 0x40043c  ...
cont
debug: going on
```

After the second `cont`, `forever` is back in its loop. The test stops it once more to see that, then `quit` ends it.

## The capability

- **`rights::DEBUG`** is a new right on a process handle. The handle `PROCESS_SPAWN` gives its caller now carries it, beside `TRANSFER` and `KILL`. init gives that handle to whoever asked for the program. So the one who started a program can debug it, and a copy of the handle made without the right cannot.
- **`call::PROCESS_DEBUG`** (48, the next free number, rule 16): `process_debug(process, op, a, b, c)`, its operations in `abi::debug::op`:
  - `STOP`: each thread stops at its next return to the program.
  - `WAIT`: up to N ticks for a stopped thread; gives its number, why it stopped (asked, or at a breakpoint) and where.
  - `CONTINUE`: every stopped thread goes on, with its registers as last set.
  - `READ` and `WRITE`: up to 4096 bytes of the process's memory. `WRITE` works on its code too: each process's code is its own copy, so nobody else sees the change.
  - `GET_REGISTERS` and `SET_REGISTERS`: a stopped thread's registers, as a `signal::Context`.

## How a thread stops

Every return to a program, from a system call, an interrupt or a fault, already goes through `signals::on_return`, which delivers signals there. Its first step now is `debug::checkpoint`. If the process is to stop, the thread records its registers (`UserFrame::context()`, as for a signal), wakes whoever waits, and waits there until it is let go or killed. Then it goes back to the program with the registers as the debugger left them (`restore()`). A thread waiting in a system call (asleep, receiving) stops when the call returns.

## Breakpoints

A breakpoint is `int3` on a PC, or `brk #0` on AArch64, written into the program's code with `WRITE`. When a program reaches one and a debugger has used the call on it, the process stops, that thread first, with the breakpoint's address. Without a debugger it is passed over, as before.

- **On a PC, a program could not take `int3` before.** Every IDT gate allowed ring 0 only, so a program's `int3` was a general protection fault: the first try ended `forever` with one. The breakpoint's gate now allows ring 3 (DPL 3). Any other `int N` from a program is still a fault.
- **The CPU reports the address after the `int3`.** `debug`'s `cont` puts the original bytes back and sets the pc to the breakpoint, so the instruction runs. So a breakpoint is caught once, and set again to catch it again (`break pc`, at the stopped thread's pc).
- **On AArch64**, code written through the kernel's map is followed by an instruction-cache invalidation. Nothing there is tested yet: no AArch64 test runs `debug`.

## `debug`

A program on the disk. It reads the program's file, as libc's `spawn` does: init's `IMAGE` request carries the file as a memory object, which the first try left out, so init refused. It asks init to start the program with its own output, and gets the handle with `DEBUG`. Then it takes commands a line at a time: `stop`, `wait [TICKS]`, `cont`, `regs`, `x ADDR [N]`, `w ADDR BYTE...`, `break ADDR` or `break pc`, `kill`, `quit`. The shell gives it the names `init` and `files` (read only), in `DEBUG_PROGRAMS`.

## In other lanes' files

- `ipc.rs`: `DEBUG` on the spawner's handle.
- `process.rs`: the `debug` field and `debug_memory()`.
- `signals.rs`: the checkpoint.
- `syscall.rs`: the call.
- `arch/x86_64/idt.rs`: the breakpoint's gate, and the breakpoint.
- `arch/aarch64/vectors.rs`: the breakpoint.
- `shell.rs`: `DEBUG_PROGRAMS`.

`ROOT_FILES` is 45, or 33 without the C compiler.

## Checked

- **`debug64-test`, new:** the session above, checking each step:
  - the stop at an address A, the registers, 16 bytes of code;
  - the breakpoint at A, caught at A with pc A+1;
  - going on past it, and stopping again in the loop;
  - `forever` ended by request, never by a fault.
- **Host tests:** `abi::debug`, a stop survives its words, the AArch64 breakpoint's bytes, `DEBUG` is in `ALL`.
- **Quick checks:**
  - `kernel64-test`, `coverage` and `unsafe-check`;
  - `boot64-test` and `boot64-uefi-test`, with C programs' signals: every return to a program now passes the checkpoint;
  - `aarch64-test`, `kill64-test`, `fault64-page-test`, `trace64-test`, `profile64-test`;
  - the timing tests `schedbench64-test` and `times64-test`.
- The full `make test64` is for the night.

## What it does not do yet

- **A debugger that ends leaves its program stopped.** `kill` from the shell still ends it.
- **No single-stepping.** To go past a breakpoint and keep it, set it again after.
- **Programs have no symbols** (K140's note), so addresses are numbers.
- **The gdb stub** for the kernel on a real PC: K?-gdb-stub.

## Testing note

| Do | You should see |
|---|---|
| `debug forever spin`, then `stop`, `regs` | Where it is, its registers |
| `break pc`, `cont`, `wait` | Stopped at the breakpoint |
| `cont`, then `quit` | It goes on past it; then it ends |

Automated: `make debug64-test` (part of `make test64`).
