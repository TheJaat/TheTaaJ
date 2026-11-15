# A Driver in User Mode, and init: a Supervisor That Restarts Crashed Services

*Journal entry #10 · the first time the microkernel idea pays off*

Entries #8 and #9 built the machinery: isolated processes, channels, capabilities. This entry uses it for what a microkernel is *for*. The keyboard driver leaves the kernel and becomes an ordinary program. Then a first process, `init`, starts the services and **restarts any that crash** — and I prove it by crashing the keyboard driver on purpose and typing on.

---

## Why move a working driver?

The kernel's keyboard code worked. But consider what a driver is: code that deals with hardware quirks, written against vague specifications, often by third parties. In Linux and Windows it runs in ring 0, where one bad pointer is a kernel panic or a blue screen. Most crashes of mainstream systems are driver bugs.

In ring 3, the same bug kills one process. The goal in entry #1 — *more reliable than Windows and Linux* — rests on exactly this.

But a ring-3 program cannot touch hardware. A driver needs two things the CPU reserves for the kernel: **interrupts** and **device registers**. Both became capabilities.

## Interrupts as objects

An interrupt always arrives in ring 0. The kernel's handler now does the minimum: **add one to a counter, and wake whoever is waiting.** The driver holds a handle to an *interrupt object* and calls `interrupt_wait`, which blocks until the counter is non-zero and returns it.

The **counter** — rather than a flag or a bare wake-up — is what makes this race-free. If an interrupt arrives while the driver is still busy with the last one, nothing is lost: the count is waiting when the driver asks again.

## I/O ports as capabilities

The PS/2 controller is reached through I/O ports `0x60` and `0x64`. Ring 3 may use a port only if the CPU's **I/O permission bitmap** allows it: one bit per port, stored at the end of the TSS.

The kernel keeps a bitmap per process and loads it into the TSS on every switch to that process. A driver holding a handle to a range of ports calls `ioports_enable`, and from then on reads its device **directly** — no system call per byte — and *only* its device. Any other port is a general protection fault and the process is killed. (A test program, `port`, does exactly that at every boot.)

A full bitmap is 8 KiB; copying that on every context switch would cost more than the switch. So a process's bitmap holds bytes only up to its highest allowed port — the keyboard driver's is 13 bytes, most processes' is empty — and the TSS rewrites only the bytes that differ.

## What the driver is given

Exactly:

- the keyboard's interrupt — **wait only**;
- port `0x60` and port `0x64` — not `0x61`–`0x63` in between, and without the right to pass them on;
- a **send-only** channel to the shell.

That list is the driver's entire power. It cannot read a file, draw on the screen, start a program, or even *receive* from the shell. If the driver were compromised, that list is all an attacker would get.

The scancode decoder left the kernel too and became the `input` crate, extended to produce what terminals expect: UTF-8, control codes (Ctrl+C is byte `0x03`), VT100 escape sequences for arrows.

## init

At first the kernel itself started `kbd` and `shell`. But "which services exist and how they are wired" is **policy**, and policy does not belong in a microkernel. So the kernel now starts exactly one process, **`init`**, and gives it:

- the **launcher** — the right to start programs. Nothing else in the system has it;
- the keyboard's interrupt and ports.

init builds the rest: it creates the channels, and starts each service with only the handles it needs and **fewer rights than init itself holds**.

### `process_spawn` and `process_wait`

`process_spawn` starts a program with a list of startup handles, each with rights the parent chooses — a subset of its own. They *move* into the child, numbered `1..=n`.

One ordering detail matters: **the program is loaded before any handle moves.** If loading fails, nothing has changed, and the parent still holds everything. An operation that can fail halfway must not have side effects before the point of no return.

`process_wait` waits for *any* of several processes to end, and says which, with its exit code.

### Restarting

init waits. When a service ends — a crash, most likely — init starts it again.

The detail that makes this *invisible* to the rest of the system: **init keeps the originals of every handle**, and gives each service *duplicates*. In particular init holds **both ends** of the channel between the keyboard driver and the shell. So when the driver dies and its handles close, the channel is **not** closed — init still holds an end. The shell never sees `PeerClosed`. A new driver gets a fresh duplicate of the same end, and its keystrokes arrive in the same queue the shell was already reading.

The shell never notices that the driver was replaced.

### Giving up

A service that crashes the moment it starts would be restarted in a tight loop — hiding the bug and burning the CPU. So: more than **5 restarts within 30 seconds**, and init gives up on that service and says so.

## Proving it: F12

The driver treats **F12** as a deliberate bug: it reads address 0. The boot test presses F12, waits for the new driver to report ready, and types a line — which the *original* shell must echo.

```
[kbd] F12: simulating a driver bug - reading address 0
[process] page fault at 0x40032f in user mode
[process] 38 "kbd" killed: page fault
[init] kbd ended with code -1000 - restarting it
[kbd] PS/2 keyboard ready (driver in ring 3, process 39)
[shell] line: "Hi there" (process 37)
```

Process 38 died. Process 39 took over. Process 37, the shell, was the same throughout. On Linux or Windows, the equivalent bug in the keyboard driver is a kernel panic.

## What I learned

- **Supervision is a wiring problem.** Recovery works because of *who holds which end of which channel*, not because of clever recovery code.
- **Least privilege is concrete here**: the driver's power is a list of three items I can read.
- **Mechanism in the kernel, policy outside.** The kernel knows how to start a process; only init knows which.

Still untested at the time of writing: the give-up path, and state. A restarted driver starts clean, which is fine for a keyboard; a service with state (files!) needs more — see entries #13 and #15.

---

## Testing note

```
make run64
```

In the QEMU window:

| Do | You should see |
|---|---|
| Type `echo before` | `before` |
| Press **F12** | Nothing changes on the screen. In the terminal log: `[kbd] F12: simulating a driver bug…`, `"kbd" killed: page fault`, `[init] kbd ended with code -1000 - restarting it`, `[kbd] PS/2 keyboard ready (driver in ring 3, process N)` with a **new** number |
| Type `echo after` | `after` — the keyboard works, through the new driver |
| `ps` | One `kbd`, with the new process number; the shell's number unchanged |
| Press F12 **six times**, a second or two apart | After five restarts inside 30 s the log says `[init] kbd ended (code -1000) 5 times in 30 s - giving up on it`, and the keyboard stops responding. *(This path has no automated test — tell me if it does anything else.)* Close QEMU and start again |

Also in the log at boot: `[init] started: display fs blk shell kbd; supervising`, and `[process] N "port" killed: general protection fault` — the program that touched a port it was not given.

Automated: `make kernel64-test` (the port bitmap), `make boot64-test` (presses F12 and types on).
