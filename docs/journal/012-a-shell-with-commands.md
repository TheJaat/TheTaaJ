# A Shell With Commands

*Journal entry #12 · `help`, `echo`, `clear`, `uptime`, `mem`, `ps`, `run`*

The shell until now echoed lines. This entry makes it a shell: it understands commands, can look at the system, and can run programs. Each of those three needed something new underneath — and each raised the same question in a new form: *what is this process allowed to know and do?*

---

## Waiting for more than one thing: `object_wait`

A shell that runs a program must wait for the program to end. init must wait for a service to die *or* a request to arrive. Until now a thread could wait on exactly one thing: receive on *one* channel, wait for *one* set of processes.

`object_wait` takes a list of handles and blocks until **any** of them is *ready*, and says which:

- a channel is ready when it has a message, or its peer has closed;
- a process, when it has ended;
- an interrupt, when one is pending.

It **consumes nothing**: it only reports. The caller then does the receive or the wait, which is now certain not to block. This is TheTaaJ's `select`/`poll`.

Underneath, `threads::wait_any` puts the thread on *several* wait queues at once; when one fires, the thread is taken off all the others. It is built on the same check-and-block step as entry #7, so the lost-wake-up argument holds unchanged.

## Looking at the system: the system handle

`ps` and `mem` need information about *other* processes and about the machine. In a capability system that is itself an authority — a process list tells an attacker what to aim at. So it is behind a handle: the **system handle**. The kernel gives it to init; init gives a copy to the shell and to nobody else.

Two calls need it:

- `system_info`: ticks since boot, frames free and total, kernel heap size, numbers of processes, threads and channels;
- `process_list`: each process's id, name and state.

The kernel keeps a registry of **weak references** to every process. Weak matters: the registry must not keep a dead process alive, or nothing would ever be freed. `process_list` skips entries whose process is gone.

`uptime` is `ticks / 100`. `mem` and `ps` print what those two calls return.

## Running programs

The shell cannot start a program: only init holds the launcher (entry #10). So:

1. the shell sends init a request, `run NAME`, on a channel init gave it;
2. init checks NAME against **its own list of runnable programs** — services are not on it, so the shell cannot ask for a second keyboard driver;
3. init starts the program and replies with a **handle to the new process**;
4. the shell waits on that handle and reports the exit code.

For step 3 a process handle had to be something that can be passed in a message. It carries only the right to *wait* — holding one does not let you touch the process. (The right to kill came later, in entry #17.)

init itself now sits in `object_wait` on its services **and** its request channel at once.

## Where a program's output goes

A program started this way should print to the screen, but it has no handles except what it is given. `process_spawn` gained one more argument: a 64-bit value passed to the child in a register. With the flag `abi::startup::STDOUT` set, the child's handle 1 is a channel to the display, and the runtime's `println!` writes there as well as to the debug log.

That flags value is the beginning of the scheme entry #16 extends to arguments and files.

## Small things

- **`clear`** sends a form feed (`0x0C`), which the console grid treats as "clear and home" — the same convention as real terminals.
- The shell writes everything both to the screen and to the debug log, which is what the tests read.

## Testing a conversation

The boot test now holds a dialogue: it types `ps`, `mem`, `run hello`, `run peek`, and checks each one's output against a list of expected lines (`TEST_EXPECT` in the Makefile). Because commands take time, the script waits for the shell to say it has finished one before typing the next — typing blind at a fixed rate is how tests become flaky (a lesson that came back in entry #17).

`echoc` gained checks that the new calls refuse the wrong kind of handle and handles without the needed right: `system_info` on a channel, `process_list` without the system handle.

## What I learned

- **Information is authority.** "Which processes exist" is not public by default; it flows through a handle like everything else.
- **One waiting primitive, generalised**, was enough for the shell, for init, and later for Ctrl+C and timeouts.
- **A shell is mostly plumbing.** Its commands are a few lines each; the work was in what it is permitted to ask for.

---

## Testing note

```
make run64
```

In the QEMU window:

| Type | You should see |
|---|---|
| `help` | The list of commands |
| `echo hello world` | `hello world` |
| `uptime` | Time since boot; again a few seconds later, it has advanced |
| `mem` | Frames in use and free, and the kernel heap's size |
| `ps` | `init`, `display`, `fs`, `blk`, `shell`, `kbd` with their ids, then `6 processes, 7 threads, 7 channels` |
| `run hello` | The program's output, then `hello exited with code 42` |
| `run peek` | `peek exited with code -1000` |
| `run kbd` | A refusal: services cannot be run from the shell |
| `run nothing` | A refusal: no such program |
| `clear` | An empty screen |
| An empty line (just Enter) | A new prompt, nothing else |

In the terminal log, each command appears as `[shell] line: "…"` and `[shell] ran "…"`, and each program start as `[init] started hello for the shell`.

Automated: `make boot64-test`, `make boot64-uefi-test`.
