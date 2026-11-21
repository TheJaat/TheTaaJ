# The same system on the other CPU

*Journal entry #60 · Era 3, step B. Everything above the kernel now runs on AArch64: init, the registry, the file system on the virtio disk, the shell, and C programs loaded from that disk. The serial port is the keyboard and the screen, driven by a program. The boot test types the PC's whole session at it and expects the same answers. And a terminal that froze whenever a program was computing, which the test found one run in six.*

```
[kernel] serial port at 0x9000000, interrupt 33 (level): handed to init, for a driver
[virtio] 0xa003e00 block device: handed to init
[kernel] idle - the services take it from here
[init] started: registry fs blk shell serial; supervising
[blk] virtio disk at 0xa003e00: 8192 sectors (4096 KiB), queue of 1024, interrupts
[serial] serial port ready: keys in, text out (driver in user mode, process 40)
[shell] ready - type a command (try help)
Welcome to TheTaaJ. Type help for the commands.

taaj:/> cat welcome
Hello from the host
```

---

## Why

Until this entry the AArch64 kernel booted, tested itself, ran a handful of programs it carried inside its own image, and stopped. Entry 59 gave it a disk driver. But nothing *used* the disk, nobody could type, and the programs were baked into the kernel - including the C ones, behind a build feature.

The PC side has been a whole system for a long time: init starts services, the registry hands out capabilities, `fs` keeps files on a disk, the shell runs programs from it. None of that is written for a CPU. So the question of this step was a simple one: what is actually missing for the same user space to come up on the other machine?

The answer turned out to be three things, and one of them I did not know about until a test failed:

1. A keyboard and a screen. QEMU's `virt` machine has neither. It has a serial port.
2. Somebody to start init, and to hand it the serial port and the disk.
3. A kernel that still writes programs' output when the CPU is never idle. (The surprise. See Bugs.)

I did these as one step instead of five small ones. The parts only make sense together anyway: a shell without a keyboard cannot be tested, and a keyboard without a shell has nobody to type to.

## A serial port is a keyboard and a screen

On the PC two programs do input and output: `kbd` reads the PS/2 keyboard, `display` paints text into the framebuffer. The shell talks to both over channels, with two small protocols: keys arrive as terminal bytes on one channel, and `TEXT` messages go out on another.

On `virt` there is one device for both directions, an Arm PL011. What I type in the terminal QEMU runs in arrives there a byte at a time; what the system prints leaves through it. So one new program, `serial`, speaks both protocols:

```
                    PC                                AArch64 virt

   PS/2 ──► kbd ──keys──► shell               terminal ──► PL011 ──► serial ──keys──► shell
            shell ──TEXT──► display ──► screen           shell ──TEXT──► serial ──► kernel log ──► PL011 ──► terminal
```

Because `serial` speaks exactly what `kbd` and `display` speak, the shell did not change at all for input, and init only had to learn which program to start.

### In: bytes that are nearly keys

A PS/2 keyboard sends scancodes and `kbd` has to turn them into characters. A serial line already has a terminal on the other end, so the bytes are almost right as they come. Almost:

- Enter arrives as carriage return (`0x0D`); programs here read `\n`.
- Backspace arrives as DEL (`0x7F`); here it is `0x08`.
- An arrow key arrives as three bytes, `ESC [ A`, one at a time.

The first two are a `match`. The third needs care. If the driver passed on `ESC`, then `[`, then `A` as three messages, a reader could see half an arrow key. So the bytes of a sequence are held back until its last one has come, and sent as one piece. This lives in the `input` crate next to the scancode decoder, as plain code with no hardware in it, so it has host tests:

```rust
match self.length {
    0 if byte == ESCAPE => self.hold(byte),
    0 => Input::Bytes(Bytes::new(&[plain(byte)], &[])),
    // After ESC, only '[' goes on with a sequence of the kind kept together here.
    1 if byte == b'[' => self.hold(byte),
    ...
    // Inside ESC [ ...: numbers and separators go on, a letter or '~' ends it.
    _ if (0x20..=0x3F).contains(&byte) && self.length < HELD_MAX => self.hold(byte),
    _ => { /* the sequence is complete: pass it on whole */ }
}
```

(`match` with `if` after a pattern is a *match guard*: the arm is taken only if the pattern fits and the condition holds. It lets "how many bytes am I holding" and "which byte came" be decided together, top to bottom.)

There is one thing bytes cannot say: `ESC` is also a key. Whether an `ESC` is the Escape key or the start of an arrow, only time tells - a sequence's bytes come together, a person's next key does not. So while the decoder holds something, the driver waits only 3 ticks for more, and then calls `flush`, which hands out the lone `ESC`.

Two sequences are not passed on: F12 (`ESC [ 2 4 ~`) and F11 (`ESC [ 2 3 ~`). As in `kbd` they are deliberate bugs - read address 0, loop for ever - so that the boot test can check that init notices a dead or hung driver and starts a new one.

### In: never dropping a key

`kbd` drops keys when the shell's queue is full. It has no choice: a keyboard cannot be told to wait. A serial line can. If the shell is not reading, `serial` keeps the one key it could not send, stops reading the port, and tries again a tick later. The port's own queue fills, and then the far end has to wait:

```rust
while unsent.is_none() {
    let Some(byte) = port.next() else { break };
    match decoder.feed(byte) {
        Input::Bytes(bytes) => match send(&bytes) {
            Ok(true) => {}
            Ok(false) => unsent = Some(bytes),   // no room: keep it, stop reading
            Err(code) => return code,
        },
        ...
    }
}
```

This is called *backpressure*: instead of losing data, each stage refuses more until the next one has taken some, and the refusal travels all the way back to the sender. I had not planned it. The first full session lost keys (see Bugs), and this was the fix.

### Out: one writer for one register

The obvious way out would be for `serial` to write to the port's data register itself. I did not do that. The kernel writes its log to the same register. Two writers that each check "is there room?" and then write would lose each other's bytes.

So text for the screen is handed to the kernel with the existing `debug_write` call, and the kernel stays the only one who transmits. On this machine the log *is* the screen.

That has a consequence in every program. The user library's `println!` writes to the log and, if the program has a screen, to the screen. Here that would show everything twice. So the library has one constant:

```rust
/// Whether what goes to the log is already what the user sees.
pub const LOG_IS_SCREEN: bool = cfg!(not(target_arch = "x86_64"));
```

(`cfg!(…)` is evaluated by the compiler and becomes plain `true` or `false`. Unlike `#[cfg(…)]` it does not remove code, so both branches are still type-checked on both machines - a mistake in the AArch64 branch is found when building for the PC too.)

### The interrupt

The port's interrupt, number 33, is a level-triggered wire: it stays raised for as long as there are bytes not yet read. Entry 59 built the answer for that: the kernel's handler switches the line off at the interrupt controller and counts, and the line comes back on when the driver waits again - by which time it has emptied the port.

`serial` waits for three things at once (the interrupt, text to show, a ping from init), with `object_wait`, not with `interrupt_wait`. In entry 59 only `interrupt_wait` switched a line back on. So the readiness check that `object_wait` uses does it too now:

```rust
pub fn is_pending(&self) -> bool {
    let pending = self.pending.load(Ordering::Relaxed) > 0;
    if !pending {
        self.rearm();      // about to wait for it: it must be able to fire
    }
    pending
}
```

Only when nothing is pending. If an interrupt is still waiting to be taken, the driver has not heard of it yet, and a line that is still raised would only fire again.

## Starting init

The AArch64 kernel now ends its boot the way the PC's does. It starts `init` with five handles:

| | What | Why |
|---|---|---|
| 1 | the launcher | the right to start programs |
| 2 | the serial port's interrupt line | for the driver |
| 3 | a memory object over the port's registers | for the driver |
| 4 | a channel with one message per device: `"mmio"`, a description, the device's handles | so far: the disk from entry 59 |
| 5 | the system handle | `mem`, `ps`, `time` |

The interrupt number comes from the device tree, which now parses the serial node's `interrupts` property. If the tree has no serial port with an interrupt, nothing is started and the kernel says `no programs can run on this machine - halting` - a terminal-less init would be a system nobody can reach.

init itself changed little. Where the PC's init passes the keyboard's two I/O ports to `kbd`, this one passes the interrupt and the register memory to `serial`. A disk can now arrive as an `"mmio"` message as well as a PCI one. The list of services differs in its last entry only.

## Programs come from the disk

With `fs` running on the virtio disk, the shell loads programs from files, as on the PC. So the kernel image no longer needs to carry them. The AArch64 image now holds only what must exist before a disk does: init, the registry, `fs`, `blk`, the shell, `serial`, and four small programs the self-tests start. The C programs left the image completely, and with them the `c-programs` build feature and the C self-test in the kernel: the typed session runs the same C programs from the disk, which is a better test of more.

## The same session, typed at a serial port

The PC's boot test types 231 lines through QEMU's monitor and checks what came of each. I wanted exactly that list on AArch64, not a copy that would drift. Two small scripts do it:

- `serial-type.py` takes the same key names (`shift-h i ret`) and writes the bytes a terminal would send for them into a pipe that QEMU reads as the serial port's input. One key every 20 ms, like a person.
- `check-boot.sh` got an `ARCH=aarch64` mode. The expectations are the PC's list, passed through eight `sed` substitutions for the lines that name a PC driver (`[kbd] F11` becomes `[serial] F11`, the disk's `MSI-X` becomes `interrupts`, `the 4 CPUs worked for` becomes `the one CPU worked for`).

The checks that are really about the PC - four CPUs, the framebuffer, the screenshot - are skipped. Everything about files, programs, pipes, jobs, crashes and restarts is the same, including reading the disk image from outside afterwards.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Who writes to the serial port | the kernel only; `serial` hands text to the log | the driver writing too: two writers of one register lose bytes; taking the port away from the kernel: then a panic could not be printed |
| One program or two for the serial port | one, speaking both the `kbd` and the `display` protocol | two programs sharing one device's registers and one interrupt, for no gain |
| Shell reader too slow | hold the key, stop reading the port (backpressure) | dropping keys like `kbd`: the first session lost characters |
| Escape sequences | held and sent whole; `ESC` alone decided by a 3-tick wait | passing bytes through one by one: readers would see half an arrow key |
| Where the decoder lives | `input` crate, no hardware, host tests | inside the driver: only testable by booting |
| Double output on a machine whose log is the screen | one constant in the user library | a second "screen" capability pointing at the log: more moving parts for the same effect |
| The AArch64 session's expectations | the PC's list, translated by `sed` | a second list: two things to keep in step |
| C programs on AArch64 | from the disk only | still embedded behind a feature: two ways to run them, one of which nobody would use |
| Programs' output on a busy CPU | the timer writes up to 32 bytes each tick | writing at once in `debug_write`: a program could then keep interrupts off for as long as it likes to print; only when idle: the bug below |

## Bugs

**1. Keys were lost.** In the first full session a line came out as `cprimes 24000wc`: the end of one command and the start of the next. The shell was waiting for a program to load while three others computed, its queue of 32 keys filled, and `serial` did what `kbd` does - dropped. Fixed with the backpressure described above. Afterwards all lines arrived whole.

**2. The PC's expectations named the PC's drivers.** Not a kernel bug, but the first comparison failed on lines like `[kbd] F11`. Hence the translation in `check-boot.sh`. One check had to be dropped for this machine rather than translated: "four programs at once take less than twice as long as one" is a statement about four CPUs.

**3. The terminal froze while a program was computing.** This one took most of the time, and most of what I believed on the way was wrong, so I am writing the way down.

*What I saw.* A short session - start three `cprimes 24000000 &` jobs, then twenty `echo` lines - sometimes stopped dead in the log right after the first job started. One run in six, then six of six, then none for forty runs.

*Wrong idea 1: a lost interrupt.* I had just changed when interrupt lines are switched back on, so I suspected that. I added a dump of every thread's state to the timer tick. With the dump in, the problem vanished.

*Wrong idea 2: the timer died.* I moved the dump to tick 3300 so that it would not disturb anything. Hung runs never printed it, and I concluded that ticks had stopped. They had not: the test script gives up 10 seconds after the last key and kills QEMU, well before tick 3300.

*Wrong idea 3: it depends on the exact binary.* One build hung 8 times in 30, another 0 in 30, run alternately. But the same image that hung 7 of 12 was clean for the next 12. It also clustered in time: bad for some minutes after a compile, then fine.

*What settled it.* I stopped guessing and read the kernel's counters straight out of a hung machine's memory through QEMU's monitor (`xp` at the physical address of the statics, from the symbol table). Three numbers:

| | hung run | good run |
|---|---|---|
| ticks, half a second apart | 2658 → 2718 | 2296 → 2357 |
| device interrupts taken | 300 | 296 |
| CPU | in `cprimes`, user mode | in `cprimes`, user mode |

Ticks were advancing. And the "hung" machine had taken *more* device interrupts than a good one that had finished. It had read every key and run every command. Nothing was stuck. The output was.

*The cause.* Programs' output (`debug_write`) goes into a queue in the kernel, and the queue is written to the serial line by the idle thread - "this is time nobody else wants". Kernel messages flush it too. That was designed for the PC's log, long ago, and it has a hole: a machine whose every CPU is busy is never idle. With one CPU and one program computing, the idle thread never runs again, and the queue sits there until the program ends.

On the PC nobody noticed: there are four CPUs, and the log is not the screen. Here the log is the screen. Type `forever &` and the terminal is dead.

And the test passed most of the time for a silly reason: `cprimes 24000000` takes about as long as the session plus the script's 10 second grace. When the jobs ended in time, the kernel printed `[process] … exited`, which flushed everything at once, and the log looked perfect. When the host was a little slower - hot after a compile - they did not, and the script gave up on an empty-looking log. That is why it came and went by the minute, and why my dumps "fixed" it: every dump is a kernel message, and flushes the queue.

*The fix* is small:

```rust
/// Writes a little of what is queued, from the timer interrupt: what
/// keeps programs' output moving on a machine that is never idle.
pub fn write_on_tick() {
    flush(&mut QUEUE.lock(), TICK_CHUNK);
}
```

called on every tick of the CPU that keeps the time. `TICK_CHUNK` is 32 bytes: a tick is 10 ms, in which a 115200 baud line carries 115 bytes, so at worst a quarter of one CPU goes on writing, and it is still about 3000 characters a second.

*The test* that would have caught it is now part of the suite: a second boot that types `forever &` and then `echo still here`, and requires the echo to appear. Without the fix it never does.

**4. A dump that could not be trusted.** My first thread dump cloned each process's name - allocating memory inside the timer interrupt. It happened to work, but it is exactly the kind of thing that deadlocks when the interrupted code is inside the allocator. I changed it to borrow the name. All of that instrumentation is gone again from the code.

## How I checked it

**Automated (`make test64`, which now has eight parts):**

- Host tests: the serial decoder (5 tests: plain bytes with Enter and Backspace changed, a sequence out whole or not at all, one too long passed on, F11 and F12 taken out, a lone `ESC` as a key), and the device tree's serial interrupt on a small tree and on QEMU's own.
- `aarch64-test`: the earlier AArch64 boots, which now also check that the serial port is handed to init and that the boot ends idle - and, on the machine without an interrupt controller, that it ends with `no programs can run`.
- `aarch64-keys-test`, new. First boot: the PC's 231 typed lines, every expectation of the PC's session (translated), the disk image read from outside afterwards. Plus three checks only this machine needs:
  - `[serial] 8 interrupts taken` must appear - keys came by interrupt.
  - The shell's greeting appears once per shell start, and no typed command shows four doubled letters in a row (`cpprriimmeess`).
  - Second boot: `forever &`, then `echo still here` must be answered.
- The PC parts are unchanged in what they check; the console change runs there too.

**Mutations** - each one change, rebuilt, run against a short typed session (and the busy boot for the last):

| Change | Result |
|---|---|
| serial line not switched off in the handler | caught: 0 commands run (interrupt storm) |
| `serial` drops a key when the queue is full | caught: typed lines arrive damaged |
| `is_pending` does not switch the line back on | **survived at first** - init's once-a-second ping woke the driver, which then read the port. Now caught: no `interrupts taken` line |
| `LOG_IS_SCREEN` false | **survived at first** - the doubled text was in the log but nothing looked. Now caught: doubled letters |
| no `write_on_tick` | caught by the busy boot: `echo still here` never shows |

**By hand:** the reproduction that started bug 3 - six sessions side by side, repeated - went from hanging in bursts to 24 of 24 sessions complete, with the output arriving while the three jobs were still computing (none had printed its result when the session ended).

**Not tested:**

- Real hardware. Everything here is QEMU's `virt` machine and QEMU's PL011, which is more forgiving than a real one (no real baud rate, a receive interrupt as soon as one byte is there).
- A person typing in `make run-aarch64`. Every command in the testing note below was typed by script through the same pipe as the test, against the same disk `make run-aarch64` makes, and what it lists is what came out - but no human hand was on a keyboard.
- The full typed session on interrupt controller version 3. The automated session runs on version 2; on version 3 I ran only a short one by script (`ls`, `echo hi`), which worked.
- A slow line. `write_on_tick` assumes 32 bytes fit in the port's queue without waiting much; on QEMU writing never waits at all.

## What is still missing

- The kernel's log and the user's screen are the same stream on this machine, so `[shell] line: …` and `[init] started …` appear between the things I type. The PC hides them in the log. A proper fix is a console that knows log from screen.
- One CPU. Three computing jobs make everything else slow, because the scheduler has no priorities: the shell waits its turn behind them. The other CPUs are the next step.
- Output under load is limited to about 3000 characters a second. The real answer is to let the port's transmit interrupt drive the writing.
- The register page is handed to the driver whole, and the kernel still uses the same registers to transmit.
- The session takes about four minutes on AArch64, most of it waiting for typed keys.

## What I learned

- **A queue needs someone who is guaranteed to run.** "Do it when idle" is a fine optimisation and a terrible only-plan. If the consumer can be starved, the queue is a place where data waits for ever.
- **A design can be right for one machine and wrong for the next without changing a line.** The idle-time log was written for a PC with four CPUs and a separate screen. Both assumptions went away, quietly.
- **Order checks cannot see a stall.** The log of a "hung" run, once flushed, is byte for byte the log of a good one. Only something that waits for an answer notices.
- **A test that passes for the wrong reason is worse than one that fails.** Mine depended on a prime sieve finishing within a script's grace period.
- **Adding output to a system whose bug is about output changes the bug.** Every dump I printed flushed the queue I was looking for. Reading memory from outside, through the emulator's monitor, changes nothing.
- **Measure before the third theory.** I had three explanations, each fitting what I had seen so far. Three numbers out of the machine's memory ended it in a minute.
- **Backpressure**: when a stage cannot keep up, refuse input instead of dropping it, and let the refusal travel back to the source.
- **`cfg!` versus `#[cfg]`**: the first is a boolean and keeps both branches compiled; the second removes code.
- **Match guards** let a state machine read as a table: state, input, condition.

## Testing note

**1. The system, on AArch64**

```
make run-aarch64
```

The terminal you started it from is the machine's keyboard and screen. To leave QEMU: **Ctrl+A, then X**. The first time, it makes a disk (`build/aarch64/taaj-disk.img`) that is kept between runs.

Near the end of the boot:

| Look for | Meaning |
|---|---|
| `[kernel] serial port at 0x9000000, interrupt 33 (level): handed to init, for a driver` | the kernel gave the port away |
| `[virtio] 0xa003e00 block device: handed to init` | and the disk |
| `[init] started: registry fs blk shell serial; supervising` | the same services as on the PC, with `serial` for `kbd` and `display` |
| `[serial] serial port ready: keys in, text out (driver in user mode, process …)` | the driver is a program |
| `Welcome to TheTaaJ. Type help for the commands.` and `taaj:/>` | the shell |

Lines beginning `[shell]`, `[init]`, `[registry]` between your commands are the log; on this machine it shares the terminal.

| Type | You should see |
|---|---|
| `cat welcome` | `Hello from the host` |
| `ls` | the programs on the disk, among them `cat`, `cprimes`, `chello`, and last `22 files, 768884 bytes` (the number of bytes may differ) |
| `chello` | `Hello from C` and `argc 1` - a C program, loaded from the disk |
| `echo one two \| wc` | `1 line, 2 words, 8 bytes` |
| `write note hello` | `note: 6 bytes` |
| `cat note` | `hello` |

Somewhere in the first few keys the log says `[serial] 8 interrupts taken: the port says when it has something`, once for each driver started.

**2. A program computing, and a terminal that still answers**

| Type | You should see |
|---|---|
| `forever &` | `forever: spin until stopped (Ctrl+C)` and `[1] forever` |
| `echo still here` | `still here`, at once - before this entry, nothing more ever appeared |
| `jobs` | `[1] running  forever` |
| `kill %1` | `[process] … "forever" stopped: killed by request` and `[1] stopped  forever` |
| `jobs` | `no jobs` |

**3. A driver that dies, and one that hangs**

| Type | You should see |
|---|---|
| **F12** | `[serial] F12: simulating a driver bug - reading address 0`, `[process] … "serial" killed: data abort in a program`, `[init] serial ended with code -1000 - restarting it`, and a new `[serial] serial port ready` |
| `echo again` | `again` - the new driver works |
| **F11** | `[serial] F11: simulating a driver bug - looping forever`; two seconds later `[init] serial has not answered for 2 s - stopping it`, `[init] serial ended with code -1001 - restarting it`, and a new `[serial] serial port ready` |
| `echo once more` | `once more` |

(If your terminal does not send F11/F12 as `ESC [ 2 3 ~` / `ESC [ 2 4 ~`, these do nothing.)

**4. A machine without a disk, and the other interrupt controller**

```
make run-aarch64 RUN_A64_DEVICES=
make run-aarch64 RUN_A64="-M virt,gic-version=3"
```

| Look for | Meaning |
|---|---|
| first: `[init] started: registry fs shell serial; supervising` and `[fs] no disk (…): files in memory only`; the shell starts, `echo hi` works, `ls` answers `unknown command: ls (try help)` | the system comes up without a disk; programs are files, and there are none |
| second: `[irq] GIC version 3: 288 interrupt lines, all off but the timer's`, and `ls` and `echo hi` work as in 1 | the other interrupt controller |

**5. Host checks**

```
cd input && cargo test serial
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu devicetree
```

**6. The two AArch64 tests alone**

```
make user-aarch64 cprogs-aarch64 kernel-aarch64
make aarch64-test
make aarch64-keys-test
```

should end `aarch64-test passed` and `aarch64-keys-test passed`. The second types 231 lines and takes about four minutes; its logs are `build/aarch64/keys.log` and `build/aarch64/busy.log`.

Automated: `make test64`
