# Kill: Ending a Program From Outside

*Journal entry #17 · `process_kill`, interruptible waits, and Ctrl+C*

Once any file on the disk could be a command (entry #16), one missing piece became obvious within minutes: a program that never ends. A loop with a wrong condition, a program waiting for a message that will never arrive — and the shell, which waits for its foreground program, was stuck until reboot.

Every operating system needs a way to end a process from outside. This entry is about why that is not as simple as "free its memory and remove it from the list", and how TheTaaJ does it.

---

## Why you cannot just destroy a process

Picture the target process at the moment someone wants it dead. Its thread is in one of a few places:

- **Computing in user mode.** Its own code, its own memory. Stopping it here is harmless.
- **In the middle of a system call.** Now it is running *kernel* code. It may hold a lock on its handle table. It may have removed a handle from one table and not yet put it into another. It may be halfway through copying a message.
- **Waiting** — for a message, an interrupt, another process, a timer. It is on a wait queue, with references to the things it waits for held on its kernel stack.

If the kernel simply stopped scheduling that thread and freed its memory, the second and third cases would corrupt the kernel itself: a lock held forever, a handle that exists in no table, a channel that can never be freed because a dead thread's stack still counts as a reference to it.

So the rule I settled on, which is the same rule Unix kernels follow for signals:

> **A kill is a request. The target ends itself, at a point where it holds nothing of the kernel's.**

## Where is "holding nothing"?

There are exactly two moments when a thread is about to go back to user mode, and at both its kernel stack is empty of anything that matters:

1. **The end of a system call.**
2. **The end of an interrupt that arrived while it was in user mode** — the timer tick, most often.

Those are the two checkpoints. In the system call dispatcher:

```rust
    // The way back to user mode: where a process someone asked to end,
    // ends. Every call comes through here ... with nothing of the
    // kernel's left on the stack to lose.
    process::stop_if_killed();
    arch::disable_interrupts();
    abi::encode(result)
```

and in the interrupt handler, after the interrupt has been handled (and possibly after this thread spent a long time off the CPU):

```rust
        super::platform::handle_interrupt(vector as u8);
        if frame.cs & 3 == 3 {
            crate::process::stop_if_killed();
        }
```

`frame.cs & 3` is the privilege level the CPU was at when the interrupt arrived; 3 is user mode. If the interrupt arrived while the thread was *inside the kernel*, nothing happens here — that thread will reach the first checkpoint when its system call finishes.

A program that never makes a system call still passes the second checkpoint a hundred times a second, on every timer tick. So the worst case for stopping a program that is spinning in a loop is one tick: 10 ms.

`stop_if_killed` ends the process the same way `exit` does — record the code, close every handle, wake whoever waits for it — with a different exit code: `STOPPED` (-1001), distinct from `KILLED` (-1000), which still means "the kernel killed it for breaking a rule".

## The request itself

```rust
pub fn kill(target: &Arc<Process>) {
    if target.exit_code().is_some() {
        return; // already ended
    }
    // The flag first: a thread woken before it was set would only go
    // back to waiting.
    target.kill_requested.store(true, Ordering::Release);
    let thread = target.thread.load(Ordering::Acquire);
    if thread != 0 {
        threads::wake(thread); // if it is blocked or sleeping; else nothing
    }
}
```

A flag, and a wake-up. The flag is all that is needed for a thread that is running or ready to run: it will reach a checkpoint by itself. The wake-up is for the third case.

## Waiting threads: the wait has to give up

A thread waiting for a message is blocked: the scheduler will not run it until someone wakes it. Waking it is easy. But look at what every wait in the kernel does when it wakes:

```rust
loop {
    if !condition() { return; }   // "is there a message yet?"
    block();
}
```

It re-checks its condition, finds there is still no message, and goes back to sleep. That re-check is essential — it is what makes spurious wake-ups harmless — and it means a woken thread would never get to the checkpoint.

So `wait_any`, the one function all blocking goes through, got a second way out. It now returns a `bool`: `true` when the condition no longer holds, `false` when the thread's process is being killed.

```rust
        if !condition() {
            arch::restore_interrupts(were_enabled);
            return true;
        }
        // Checked with interrupts off, like the condition: a kill that
        // comes after this finds the thread blocked, and wakes it.
        if being_killed() {
            arch::restore_interrupts(were_enabled);
            return false;
        }
```

Every caller had to decide what `false` means for it. I marked the function `#[must_use]`, so the compiler refused to build until each one did:

```rust
if !end.shared.readable[end.end].wait_while(|| ...) {
    return Err(Error::Interrupted);
}
```

`receive`, `process_wait`, `object_wait` and `interrupt_wait` all give up with a new error, `Interrupted`. The program never sees that error: the call returns to the dispatcher, the dispatcher reaches the checkpoint, and the process ends.

**Why return an error instead of ending the thread right there, inside the wait?** Because of what is on the stack. `receive` holds a reference to the channel while it waits. Returning normally lets Rust drop that reference as the function unwinds. Ending the thread in place would leave the reference count one too high forever — a leaked channel for every killed waiter. Unwinding through ordinary `return`s gets all of that right for free, in every present and future system call.

### Two races, both closed the same way

*The check and the block must be one step.* If a thread checked "am I being killed?", was interrupted, the kill arrived (flag set, wake-up sent to a thread that is not blocked yet — so nothing happens), and then the thread blocked, it would sleep through its own kill. This is the classic lost wake-up. The check happens with interrupts disabled, and stays that way until the thread is off the CPU, so on one CPU nothing can come between.

*Sleep had the same hole.* `sleep` does not go through `wait_any`; it puts the thread in the `Sleeping` state directly. I found this one by reading, not by testing: a thread preempted inside the `sleep` call just before it went to sleep would have slept the full time with the flag already set. `sleep` now makes the same check, the same way:

```rust
pub fn sleep(ticks: u64) {
    let were_enabled = arch::disable_interrupts();
    if !being_killed() {
        suspend(State::Sleeping(interrupts::ticks() + ticks));
    }
    arch::restore_interrupts(were_enabled);
}
```

Both arguments depend on there being one CPU. With several, "interrupts off" no longer means "nothing else runs", and these will need a lock. That is noted for the SMP step.

## Who may kill?

Not everyone. Authority in TheTaaJ is a right on a handle, so I added one: `KILL`. The handle a process gets back from starting a program now carries `KILL`; a copy made without it can only wait.

`init` starts programs for the shell and passes the process handle on to it, with the right. So the shell can stop what it started — and nothing else, because it holds handles to nothing else. There is no "kill by process ID": a number is not authority.

## Ctrl+C

The keyboard driver already sent Ctrl+C as the byte `0x03`. The shell used to wait only for its program; now it waits for **either** the program **or** a key, using `object_wait`, which takes several handles and returns whichever is ready first:

```rust
fn wait_for(&self, process: Handle) -> Result<(usize, i64), Error> {
    let mut input = [0u8; 16];
    loop {
        // Whichever comes first: the program's end, or a key.
        if taaj::object_wait(&[process, self.keys])? == 0 {
            return taaj::wait_any(&[process]);
        }
        match taaj::receive(self.keys, &mut input, &mut []) {
            Ok((n, _)) if &input[..n] == CTRL_C => {
                self.write(b"^C\n");
                if let Err(error) = taaj::kill(process) {
                    say!(self, "cannot stop it: {error:?}\n");
                }
            }
            Ok((n, _)) => { /* kept for later, up to 256 keys */ }
            Err(_) => return taaj::wait_any(&[process]),
        }
    }
}
```

After a kill the loop simply continues: the kill is a request, and the shell waits, like anyone else, for the process to actually end.

Other keys typed while a program runs are not thrown away. They are kept and read as the next line — type-ahead. And Ctrl+C at the prompt abandons the line being typed.

## The bug that was not in the kernel

The first boot test typed `run forever sleep`, Ctrl+C, then `oops`, Ctrl+C, `ps`. The log showed `^C` **twice** before the program stopped, and then the shell tried to run the command `oopsps`. It looked exactly as though the first kill had failed on a sleeping thread and only the second worked.

I added temporary logging to the kill path. The first kill had woken the thread; the thread ended three ticks later; the second `^C` arrived one tick after the first. The kernel was right. The *test* was wrong, in an instructive way:

- Loading a 33 KB program from the disk took about a quarter of a second.
- The test script types a key every 50 ms and does not wait.
- So by the time the program was running, the shell's keyboard queue already held Ctrl+C, `o`, `o`, `p`, `s`, Ctrl+C, `p`, `s`, Enter. The shell — correctly — treated every key that arrived while the program ran as belonging to that run: the first Ctrl+C killed it, the letters were kept as type-ahead, the second Ctrl+C asked again, and the kept letters became `oopsps`.

The fix was in the test: a new token, `~`, makes the script wait until the shell is back at its prompt before typing on. The lesson went into the next step: **the disk path was slow enough to change how the system behaves**, and that is entry #18.

## How I checked it

A new program, `forever`, does one of four things until stopped, depending on its argument: spin in user mode with no system calls, make system calls in a loop, sleep, or wait on a channel it created itself.

- **Kernel self-test, at every boot.** It starts `forever` in each of the four modes, lets it get where it is going, kills it, and requires: exit code `STOPPED`, within 3 ticks. A fifth case kills a process before its thread has run a single instruction. The whole set runs twice, and afterwards the count of free memory frames must be exactly what it was, and the number of live channels zero — the waiting mode makes its own channel, which must die with it.
- **Boot test.** Types `forever`, `forever wait` and `run forever sleep`, each followed by Ctrl+C, and expects `^C`, the kernel's `stopped: killed by request` line, and `forever was stopped`. Then a line abandoned with Ctrl+C must *not* run.
- The `ps` comparison from entry #16 still holds after all of it.

## What is still missing

- Only the foreground program can be stopped. There are no background jobs, and no `kill` command for anything else.
- A killed program gets no chance to clean up. Unix has `SIGTERM` (please stop) before `SIGKILL` (stop). TheTaaJ has only the second so far; the polite version is a message to the program, which needs a convention, not a kernel feature.
- The shell itself still hangs if the *file service* stops answering. Killing is for the shell's children, not its servers.

---

## Testing note

```
make run64
```

Type in the QEMU window. **Press Ctrl+C in the QEMU window, not in the terminal** — in the terminal it ends QEMU.

| Type | You should see |
|---|---|
| `forever` | `forever: spin until stopped (Ctrl+C)` — and no prompt |
| Ctrl+C | `^C`, then `forever was stopped`, then the prompt |
| `forever wait` then Ctrl+C | `forever: wait until stopped`, `^C`, `forever was stopped` |
| `forever sleep` then Ctrl+C | The same, for `sleep` |
| `forever yield` then Ctrl+C | The same, for `yield` |
| `forever`, then type `ps` and Enter *while it runs*, then Ctrl+C | The program stops, and then `ps` runs by itself (type-ahead) |
| `abc` then Ctrl+C (no Enter) | `abc^C` and a fresh prompt; `abc` is not run |
| `ps` | The six services only; no `forever` left |
| `mem` before and after a few rounds of the above | The same "used" figure |

In the terminal log each stop appears as `[process] N "forever" stopped: killed by request`, and near the top of the boot: `[selftest] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked`.

Automated: `make boot64-test` and `make boot64-uefi-test`.
