# Interrupts and a Tick on AArch64

*Journal entry #54 · Era 3, step 3d. The AArch64 kernel sets up Arm's interrupt controller - both versions in use - starts the CPU's own timer, and is interrupted a hundred times a second. For the first time on this CPU something happens that the running code did not ask for.*

```
[irq] GIC version 2: 288 interrupt lines, all off but the timer's
[timer] the CPU's own: counts at 62500000 Hz, interrupt 27, 100 ticks a second
[selftest] timer: 10 ticks in 99 ms (expected 100)
[selftest] interrupts: 5 taken with a known value in each of 28 registers, none changed
[selftest] interrupts masked for 35 ms: no tick arrived; unmasked: 3 counted at once
```

---

## Why

Until now the AArch64 kernel ran from its first instruction to its last in one straight line. Everything that makes an operating system more than a program needs that line to be *interruptible*:

- **taking the CPU back** from a thread that does not give it up (step 3e needs a tick);
- **sleeping** until a time;
- **devices** - a key pressed, a disk block arrived (step 3g).

On the PC this was entry 6. This is the same step on the other CPU, and almost nothing of entry 6 could be reused: interrupt hardware is the least alike part of the two machines.

## How a PC and an AArch64 machine differ here

| | PC (entry 6) | AArch64 |
|---|---|---|
| Who collects the devices' wires | the I/O APIC | the GIC's *distributor* |
| The per-CPU part | the local APIC | GIC v2: a *CPU interface* in memory. GIC v3: CPU registers, plus a *redistributor* in memory |
| How the kernel finds them | ACPI tables | the device tree |
| How the CPU learns *which* interrupt | it does not ask: the CPU jumps to a different handler for each (256 vectors) | **one** entry for all; the handler asks the controller |
| The timer | a device (the local APIC timer), calibrated against another (the PIT) | part of the CPU, with its rate in a register |
| "Done" | write to the local APIC | write the interrupt's number back to the controller |

The fourth row shapes the code. On x86 the *vector number* is the interrupt's identity and the IDT is the dispatch table. On AArch64 the vector table (entry 50) has just one slot that means "an interrupt", and what follows is a conversation:

```text
   device raises its line
        │
   GIC: enabled? urgent enough? for which CPU?  ──►  raises the CPU's IRQ input
        │
   CPU: jumps to the "interrupt" slot of the vector table
        │
   kernel:  "which?"   read  the acknowledge register   ──►  27
            handle it  (the timer: move its deadline on, count a tick)
            "done"     write 27 to the end register
            "which?"   ──►  1023, meaning "nothing more"
        │
   eret: back to whatever was running
```

## Interrupt numbers

```text
      0 - 15    SGI   one CPU poking another ("software generated")
     16 - 31    PPI   private to each CPU - its timer is one
     32 - 1019  SPI   shared: devices
   1020 - 1023        not interrupts: answers meaning "nothing, after all"
```

The device tree does not use these numbers. It writes each interrupt as three: a kind (1 = private, 0 = shared), a number *within the kind*, and flags. Entry 51 already converts them: the timer's "1, 11" is 16 + 11 = **27**.

## Two versions, one driver

QEMU's `virt` machine has GIC version 2 by default and version 3 on request; real boards are split (a Raspberry Pi 4 has v2, most servers and newer boards v3). The tree says which. I did both now, because they differ exactly where a mistake is easiest:

| | Version 2 | Version 3 |
|---|---|---|
| "which?" / "done" | registers in memory | CPU registers `ICC_IAR1_EL1` / `ICC_EOIR1_EL1` |
| A private interrupt's enable bit | in the distributor (which shows each CPU its own) | in that CPU's **redistributor** |
| Finding the per-CPU part | one address, the same for every CPU | a row of redistributors; each names its CPU, and the kernel looks for its own name |
| Groups | not needed | every interrupt must be put in "group 1" or it is firmware's, not the kernel's |
| Naming CPUs | a bit per CPU, at most 8 | the CPU's `MPIDR` ("affinity routing") |
| Waking | - | the redistributor must be told its CPU is awake, and agree |

As with the page tables (entry 52), what the registers *are* went into the library, `kernel64::gic`, where it can be tested on my own machine: where interrupt 27's enable bit is, how to read the acknowledge register's answer, how a CPU recognises its redistributor. `arch/aarch64/gic.rs` only reads and writes.

```rust
/// Where interrupt `id`'s bit is among registers of one bit each
/// starting at `base`: the register's offset, and the bit as a mask.
pub fn bit(base: usize, id: u32) -> (usize, u32) {
    (base + (id as usize / 32) * 4, 1 << (id % 32))
}
```

### The first use of `map_mmio` on this CPU

The controller's registers are at physical `0x8000000`. Entry 53 left `mm::map_mmio` compiled for AArch64 and never called. Here it is called for the first time, and the pointer it returns is used at once:

```rust
let dist = mm::map_mmio(controller.distributor.start, controller.distributor.size.max(0x1_0000)) as *mut u8;
```

Device registers are read and written with `read_volatile` / `write_volatile`. To Rust an ordinary read of memory is a *question about a value*: it may be skipped if the answer is known, or done twice. Reading the acknowledge register is an *action* - it takes the interrupt. `volatile` says: do exactly this access, exactly once.

### One thing the start code had to learn

On version 3 the kernel talks to the controller through CPU registers. If the kernel was entered at exception level 2 (as on real boards), level 2 decides whether level 1 may use them - through one more register, `ICC_SRE_EL2`. The forty instructions that come down from level 2 (entry 50) now set it, if the CPU has such registers at all.

## The timer

Every AArch64 CPU has a counter that runs at a fixed rate and never stops, and a timer that compares the counter with a value. When the counter reaches the value the timer raises its interrupt - and *keeps* raising it until the value is moved.

```rust
let frequency = /* CNTFRQ_EL0 */;          // 62,500,000 on QEMU
let interval  = frequency / HZ;            // 625,000 counter steps = 10 ms
// compare value = counter + interval; control = enabled
```

That is all of it. On the PC, entry 6 needed the PIT to measure the APIC timer's unknown rate before it could ask for 100 Hz. Here the rate is a register.

The handler has one decision to make: what the next compare value is.

```rust
pub fn next_deadline(deadline: u64, now: u64, interval: u64) -> (u64, u64) {
    let behind = now.saturating_sub(deadline) / interval;
    (deadline + (behind + 1) * interval, behind + 1)
}
```

Two wrong answers are tempting:

- **`now + interval`.** Each tick would then be late by however long the handler took to start. A few microseconds, a hundred times a second, for ever: the clock drifts.
- **`deadline + interval`.** No drift. But if the kernel had interrupts masked for 35 ms, three deadlines have passed; stepping one at a time, the timer fires again immediately, and again - three interrupts back to back to "catch up".

The function steps from the *old deadline* (no drift) by as many intervals as have gone by (no pile-up), and says how many that was, so the tick count stays true.

I use the **virtual** timer. A CPU has a "physical" one too. They differ only under a hypervisor, which can shift the virtual one; the virtual one is what a guest is always allowed, so it works both on bare metal and in a VM.

## What the handler must not lose

An interrupt arrives between two arbitrary instructions. The interrupted code has no idea; every register must come back exactly as it was. Entry 50 checked this for a breakpoint - at one chosen instruction. For interrupts there is now a second check: 28 registers each get a known number, a loop spins until five ticks have gone by, and the registers are compared.

```
[selftest] interrupts: 5 taken with a known value in each of 28 registers, none changed
```

(28, not 31: the loop itself needs three - a pointer to the tick count, the count to wait for, and one to load into.)

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| GIC versions | Both, now | *Version 2 only (QEMU's default).* The Raspberry Pi 4 would work and most other boards would not; and "add v3 later" means changing interrupt code under a kernel that by then has threads and drivers depending on it. |
| Where register knowledge lives | In the library, tested on the host | *All in the driver.* Then "is bit 27 of the register at 0x100 the timer's enable?" could only be answered by booting. |
| Which timer | Virtual | *Physical.* Not available to a guest under every hypervisor. |
| The next deadline | From the old deadline, skipping what was missed | See above: `now + interval` drifts; one step at a time piles up. |
| An interrupt nobody asked for | Counted, said once, and *disabled* | *Ignore it.* A device holding its line up would then interrupt for ever, and the kernel would do nothing else. |
| No controller the kernel knows | Say so; go on without interrupts | *Panic.* The kernel can still print and check memory; a board with an unknown controller should get as far as saying which. |
| The tick counter | In `arch/aarch64/timer.rs` for now | *Use `interrupts.rs`, as the PC does.* Its tick calls the scheduler, and there are no threads on AArch64 yet. It moves there in step 3e. |
| Priorities | One for everything | The kernel has no use for nested interrupts. Handlers are short and run with interrupts masked. |

## Bugs

**One, caught by a host test before any boot.** `acknowledged` decides whether the controller's answer is an interrupt or one of the four "nothing" numbers. I wrote it as "an interrupt if below 1020". My own test then asked about 8192, which on version 3 is a real interrupt (the kind sent as messages, which start there), and failed. The rule is "not one of 1020 to 1023". Nothing in this entry's boots would have shown it; the first message-signalled interrupt, whenever that comes, would have been silently dropped.

**A wrong number in a comment.** I wrote in the timer's source that the PC's timer was "entry 13". It was entry 6; entry 13 is the user heap. Found when I checked the journal's index for this entry. Fixed.

**Everything booted the first time, on both versions and from both exception levels.** As in entry 53, that says more about the emulator than I would like - see the two experiments below that it could not tell apart.

## How I checked it

**Automated** (`make test64`):

- **Host tests**: 166 in the kernel's library; 9 new, all in `gic`:
  - where an interrupt's bit and byte are, including the first of a register and the last interrupt there can be;
  - how many lines a controller has, from its type register (QEMU's 288; the maximum, 1020);
  - the acknowledge register's answer, for both versions: a number, a number with a sending CPU above it, the four "nothing" values, a message interrupt;
  - which kind a number is;
  - a CPU recognising its redistributor, with a simple name and a four-part one;
  - the tick interval for two real frequencies, and for frequencies that cannot work;
  - the next deadline: on time, exactly on it, early, late by 3½ intervals, late by exactly one;
  - counter steps as milliseconds, without overflow.
- **AArch64 boots**: thirteen ordinary ones. Eleven have a controller and must print the five lines at the top - the right version, ten ticks in 90 to 299 ms, no register changed, no tick while masked, and at least three counted afterwards. Any "unexpected interrupt" fails the boot.

  | Boot | What it adds |
  |---|---|
  | plain, a53, smp4, dirty, … | version 2, entered at level 1 |
  | el2 | version 2, entered at level 2 |
  | max | **version 3**, entered at level 1 |
  | **el2v3** (new) | version 3, entered at level 2: the start code's new lines |
  | **nogic** (new) | a tree whose controller has no `compatible`: the kernel must say it has no interrupts, run no timer test, and finish |
  | **notimer** (new) | a tree without the timer's interrupts: the usual number, 27, is used |

- **x86_64**: unchanged; the whole suite as before.

**By hand - breaking it on purpose** (each put back):

| # | Change | Result |
|---|---|---|
| 1 | level 2 does not allow level 1 the controller's registers (el2v3) | **passes** - see below |
| 2 | the timer's compare value is not moved on | nothing after the `[timer]` line: the same interrupt for ever; boot test times out |
| 3 | the kernel never says "done" | one tick, then silence; times out |
| 4 | v3: private interrupts not put in group 1 | **passes** - see below |
| 5 | one register restored from the wrong place in the frame | panic: *2 of 31 registers were changed* (the breakpoint check gets there first) |
| 6 | ticks counted one at a time however late | panic: *after 35 ms masked only 2 ticks were counted, not the 3 that passed* |
| 7 | v2: the CPU interface not switched on | no tick ever; times out |

**Not tested, and why:**

- **Experiments 1 and 4.** QEMU's version 3 controller starts with the level-2 permission already given and with every interrupt already in group 1 - as firmware would leave them on some machines, and not on others. Both lines are from the controller's manual; neither can be shown necessary here.
- **A device's interrupt** (a shared one, 32 and up). `enable` has a path for them - on version 2 it also chooses the CPU - and nothing takes it until there is a device driver (step 3g).
- **The "unexpected interrupt" path.** Nothing unexpected arrived, which is the point; I did not make one arrive.
- **Interrupts from a program** (the vector table's other "interrupt" slot). There are no programs yet.
- **More than one CPU.** Each needs its own CPU interface or redistributor set up; only the boot CPU's is.
- **A machine whose firmware leaves the counter's rate at 0.** The kernel panics with a message saying so. Some boards give the rate in the device tree instead; that is not read.
- **Real hardware**, as for the whole port so far.

## What is still missing

- **The tick does nothing but count.** No scheduler listens. That is step 3e: threads, and with them a separate stack for exceptions, so that a kernel stack overflow can be reported (entry 53).
- **`interrupts.rs` is still PC-only.** It carries device interrupt lines and the scheduler's hooks; it un-marks with threads.
- **The serial port's interrupt.** Input still does not exist on AArch64.
- **Shutting the machine down** from inside (the firmware call the tree describes) - the boot tests still stop QEMU from outside.

## What I learned

- **x86 asks the hardware to dispatch; Arm asks the kernel to.** 256 vectors against one. Neither is simpler overall - x86 has an IDT to build, Arm has a question to ask on every interrupt - but the Arm way puts the whole decision in code I can read.
- **The interrupt controller is where "the same architecture" stops meaning the same.** Two AArch64 boards can have the same CPU core and controllers that share a name and little else. The device tree is what makes one kernel possible.
- **A timer that is part of the CPU removes a whole class of work.** No calibration, no second device, the same on every board. What remained was one small function about deadlines, and it was the only part with real decisions in it.
- **"Late" has two wrong fixes.** Drift and pile-up are opposite mistakes, and each obvious formula makes one of them.
- **Test the answer that is not an answer.** The bug in this entry was in the meaning of "nothing". A value that says "no result" is still a value, with an exact range.
- **Twice now the emulator pre-did my work.** Entry 53: a TLB that needs no flush. This entry: permissions and groups already set. Each "Not tested" section since entry 50 has one or two of these; together they are the list to go through on the day this runs on a board.

## Testing note

**1. The AArch64 kernel is interrupted by its timer**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[irq] GIC version 2: 288 interrupt lines, all off but the timer's` | the controller the device tree describes is set up |
| `[timer] the CPU's own: counts at 62500000 Hz, interrupt 27, 100 ticks a second` | the timer is started |
| `[selftest] timer: 10 ticks in 99 ms (expected 100)` | interrupts arrive, at the right rate (anything from 90 to about 130 is normal; more on a busy machine) |
| `[selftest] interrupts: 5 taken with a known value in each of 28 registers, none changed` | being interrupted loses nothing |
| `[selftest] interrupts masked for 35 ms: no tick arrived; unmasked: 3 counted at once` | masking works, and the time is not lost |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

**2. The other version of the controller, and the other way in**

```
make run-aarch64 RUN_A64="-M virt,gic-version=3"
make run-aarch64 RUN_A64="-M virt,virtualization=on,gic-version=3"
```

| Look for | Meaning |
|---|---|
| `[irq] GIC version 3: 288 interrupt lines, all off but the timer's` | version 3 found and set up |
| the same three `[selftest]` lines | it interrupts just the same |
| in the second run: `the loader entered at level 2` | the kernel came down from level 2 and may still use the controller |

**3. A machine with no controller the kernel knows**

`make aarch64-test` leaves `build/aarch64/nogic.dtb`:

```
make aarch64-test
make run-aarch64 RUN_A64="-dtb build/aarch64/nogic.dtb"
```

The first command should end `aarch64-test: all passed`.

| Look for | Meaning |
|---|---|
| `[tree] interrupts  no controller that this kernel knows` | the tree was read, and has nothing usable |
| `[irq] the device tree describes no interrupt controller this kernel knows: no interrupts` | said plainly |
| no `[selftest] timer` line, then `[kernel] this is as far as the AArch64 port goes - halting` | the kernel did not wait for a tick that cannot come |

**4. The PC is unchanged**

```
make run64
```

| Type | You should see |
|---|---|
| `uptime` | the time since boot, counting up |
| `cprimes 100` | `cprimes: 25 primes below 100` |

**5. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu gic
```

should end `test result: ok. 10 passed` (the nine new ones, and one device-tree test with "gic" in its name).

Automated: `make test64`
