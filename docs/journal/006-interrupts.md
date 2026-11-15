# Interrupts: ACPI, the APIC Timer, the I/O APIC and the Keyboard

*Journal entry #6 · the kernel stops polling and starts being told*

Until this step the kernel ran from its first instruction to its last and then halted. Nothing outside could get its attention: no clock, no key. An operating system is mostly a thing that *responds* — to time passing, to devices, to programs — so this is the step where it starts to.

The result: a timer that ticks 100 times a second, a keyboard that echoes what is typed, and a kernel that rests in an idle loop (`hlt`: sleep until the next interrupt) instead of stopping.

---

## What an interrupt is

A device raises a signal. The CPU finishes its current instruction, saves where it was, and jumps to a handler chosen by a number — the **vector** — through the same IDT that entry #3 set up for exceptions. Vectors 0–31 are CPU exceptions; 32–255 are free for devices. The stubs from entry #3 were extended to all 256 vectors (generated with an assembler `.rept` loop rather than typed out).

The hard part is not the handler. It is everything between the device and the CPU: *which* device is wired to *which* vector, through what controller. On a PC that story has two generations.

## Out with the old: the 8259 PIC

The original PC had a pair of 8259 interrupt controllers. Every PC still emulates them, and they are still switched on at boot. I do not use them — they cannot serve more than one CPU — but they cannot simply be ignored: left alone, they deliver interrupts on vectors 8–15, which collide with CPU exceptions (a timer tick would look like a double fault).

So the kernel **remaps them** to vectors `0xE0`–`0xEF`, out of the way, and then **masks every line**. Remapping first matters: even a masked PIC can produce a "spurious" interrupt, and it should land somewhere harmless.

## In with the new: the local APIC and the I/O APIC

- Each CPU has a **local APIC**: it receives interrupts for that CPU, and contains a timer.
- The **I/O APIC** sits on the motherboard: device lines come in, and a table in it says, per line, which vector to raise on which CPU.

Both are controlled through memory-mapped registers — which raised a new need: mapping *device* memory. `map_mmio` maps a physical range, uncached, into a window of its own at `0xFFFF_E000_0000_0000`. (Device registers must not go through the CPU cache: a read has to reach the device every time.)

## Finding the hardware: ACPI

Where are the APICs? How many CPUs are there? Is the keyboard on line 1, or was it rewired? The firmware describes all of this in **ACPI tables**. The bootloader passes one address (the RSDP); from there:

```
RSDP -> XSDT (or the older RSDT): a list of tables
          -> MADT ("APIC"): local APICs, I/O APICs, and
             "interrupt source overrides" - legacy line N
             is really input M, with this polarity and trigger
```

These tables come from firmware, and firmware has bugs. So the parser treats them as **untrusted input**: every table's signature, length and checksum is verified before a field is read, and nothing is read beyond a table's stated length. It reads memory through a small trait, `PhysicalMemory` — so on my development machine, the unit tests build tables in a byte buffer, including deliberately corrupt ones, and check that the parser refuses them instead of crashing.

If there is no usable MADT, the kernel assumes the standard PC addresses and says so.

## A clock: calibrating the APIC timer

The local APIC timer counts down from a number and interrupts at zero. The catch: it counts at the speed of the CPU's bus clock, which differs on every machine and is not written anywhere. It must be **measured against a clock whose speed is known**.

The one clock on a PC with a fixed frequency is the old PIT: 1,193,182 Hz. So the kernel:

1. programs PIT channel 2 for a known interval (10 ms);
2. starts the APIC timer counting from its maximum;
3. waits for the PIT;
4. reads how far the APIC timer got.

That gives ticks-per-10-ms, and the timer is set to interrupt 100 times a second. The CPU's timestamp counter (TSC) is calibrated the same way, for precise timing in tests. Channel 2 is used because it can be read by polling, with no interrupt — interrupts are not on yet.

**Why 100 Hz?** Each tick costs a little; each tick is also the longest a thread can hog the CPU before the scheduler (next entry) can act. 100 Hz — 10 ms — is the traditional middle.

## The keyboard

The PS/2 keyboard is ISA interrupt line 1. The kernel looks up line 1 in the MADT's overrides (on some machines it is rewired), programs the I/O APIC to deliver it as a vector, and initialises the PS/2 controller.

The keyboard sends **scancodes** — a number for "key 30 pressed", another for "key 30 released" — not characters. The decoder (`input.rs`) turns them into characters: it tracks Shift and Caps Lock, handles the `0xE0` prefix of extended keys, and maps a US layout. It is pure logic, so it is tested on the host.

## Sharing data with an interrupt handler

Two new problems appear the moment a handler can run *between any two instructions* of the kernel.

### 1. A lock taken by both

Suppose the kernel is printing and holds the console lock. A timer interrupt arrives; its handler wants to print too. It waits for the lock — which is held by the very code it interrupted, which cannot continue until the handler returns. **Deadlock, with one CPU.**

The fix is a lock that **disables interrupts while it is held**: `IrqLock`. It saves whether interrupts were on, turns them off, takes the lock; on release it restores the saved state. The console, the frame allocator, the page tables and the heap all moved to it.

One exception, deliberately: when the kernel is dying — a panic or a fatal exception — it **breaks** the console lock. If the fault happened mid-print, the report must still come out.

### 2. Handing bytes from handler to kernel

The keyboard handler must not do real work — it runs with interrupts off, and everything else waits. It decodes the key and puts a byte in a queue. Something else takes it out later.

The queue is **lock-free, single-producer single-consumer**: a ring of bytes with a head index only the consumer writes and a tail index only the producer writes. With exactly one of each, two atomic indices are enough — no lock at all, so the handler can never be blocked by the code it interrupted. The host test pushes a million bytes through it across two real threads.

## Ending an interrupt

Every handler must tell the local APIC it is done (EOI — "end of interrupt"), or no further interrupt of that priority arrives. Forgetting this gives exactly one keystroke and then silence.

## Testing: typing into a virtual machine

How does an automated test press keys? QEMU has a control channel, the *monitor*, with a `sendkey` command. The boot scripts connect to it and type a line — including a deliberate typo corrected with Backspace — and check that the kernel echoes the corrected line. On BIOS and UEFI.

A boot self-test also times 10 timer ticks with the TSC and requires about 100 ms: proof that the calibration is right, not merely that the timer fires.

## What I learned

- **"Interrupts off" is a lock.** On one CPU it is the strongest one there is. Much of the later kernel (threads, wait queues) is built on that fact — and will need revisiting for several CPUs.
- **Firmware tables are input from a stranger.** Validate, bound every read, and test with corrupt data.
- **Measure, don't assume.** The APIC timer's speed, the TSC's speed: both calibrated on every boot.

---

## Testing note

```
make run64
```

In the terminal log, near the top:

| Line | Meaning |
|---|---|
| `[acpi] revision 0, OEM "BOCHS", tables: FACP APIC HPET WAET` | The ACPI tables were found and passed their checks |
| `[acpi] 1 CPU(s), APIC ids [0] …` | The MADT was parsed |
| `[irq] local APIC id 0; legacy PICs remapped and masked` | The old controllers are out of the way |
| `[irq] I/O APIC 0 at 0xfec00000: GSIs 0-23` | The I/O APIC is mapped |
| `[irq] APIC timer at 100 Hz (bus clock N MHz), TSC N MHz` | Calibration results (the MHz figures depend on your machine) |
| `[selftest] timer: 10 ticks in 99 ms (expected 100)` | The tick really is 10 ms (98–102 is fine) |

In the QEMU window:

| Type | You should see |
|---|---|
| `uptime` | A time since boot; run it again a few seconds later and it has advanced by that much |
| `echo Hello` with Shift and with Caps Lock | Capitals where expected |
| A word, Backspace a few times, other letters | The corrected word |

(The keyboard code described here moved out of the kernel in entry #10; what remains from this step is the interrupt machinery under it.)

Automated: `make kernel64-test` (ACPI parser, decoder, queue), `make boot64-test`, `make boot64-uefi-test`.
