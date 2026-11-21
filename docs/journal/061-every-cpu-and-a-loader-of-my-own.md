# Every CPU, and a loader of my own

*Journal entry #61 · Era 3, step C. The AArch64 kernel starts the other CPUs - by asking the firmware, which is the only way there is - and runs threads on all of them. Four programs now compute at once on that machine too. Running the typed session on four CPUs tore the terminal's lines apart, so the kernel's log is now merged by line. And TheTaaJBoot has a second UEFI loader: `BOOTAA64.EFI`, which starts the same kernel on a machine whose firmware is UEFI.*

```
TheTaaJBoot (UEFI, AArch64)
  kernel 1044 KiB at 0x5c280000
  device tree 1052672 bytes at 0x47ef6000, copied to 0x5c08e000 with 3 ranges the firmware keeps
  firmware at exception level 1 - exiting boot services

Hello from TheTaaJ!
[cpu] Arm Cortex-A72 (id 0x410fd083), at exception level 1; the loader entered at level 1, with the MMU off
[tree] reserved    0x5c770000 - 0x5cb40000
[tree] reserved    0x5fe20000 - 0x5feb0000
[tree] reserved    0x5fec0000 - 0x5ffe0000
[smp] CPU 1 awake: named 0x1, stack below 0xffffd80000010000
[smp] CPU 2 awake: named 0x2, stack below 0xffffd80000020000
[smp] CPU 3 awake: named 0x3, stack below 0xffffd80000030000
[smp] 4 CPUs awake: CPU 1, 2, 3 started by the firmware (PSCI 1.1, asked by hvc)
[smp] counting together: 4 CPUs reached 400000 of 400000 with a lock; without one 399838, 162 lost
[smp] 4 CPUs run threads: each has its own timer and idle thread, and all take from one queue
[selftest] wake-ups: a thread woken by a busy CPU ran on an idle one 153 us later (median of 40; slowest 485 us; 41 interrupts between CPUs)
```

---

## Why

After entry 60 the whole system ran on AArch64, on one CPU. QEMU was giving the machine four and the kernel left three of them where the firmware had put them: off. And the kernel could only be started one way, by QEMU's `-kernel` option, which is not something a real machine has.

Both are the last pieces of "the same system on the other CPU":

1. **The other CPUs.** The scheduler has been ready for several CPUs since the PC got them. What was missing is everything under it that is different on Arm: how a CPU is started, how it knows which one it is, where its interrupts come from, and how one CPU interrupts another.
2. **A loader.** On the PC I have my own for BIOS and for UEFI. For AArch64 I had none.

I did them as one step with one round of tests. In between, the first made a mess of the terminal, which became a third piece of work I had not planned.

## Part 1: the other CPUs

### Who holds a CPU that is off

On the PC the kernel starts the other CPUs itself: it sends them messages through the interrupt controller, and they begin in 16-bit mode at an address the kernel chose.

On AArch64 a kernel cannot do that. A CPU that is off is held by software that is *more privileged than any kernel*: firmware at exception level 3, or a hypervisor at level 2. Only that software can let it go. Arm wrote down one way of asking for it, **PSCI** (Power State Coordination Interface).

Asking is a function call that crosses exception levels. A number that names the function goes in `x0`, arguments in `x1` to `x3`, and then one instruction:

- `smc` ("secure monitor call") traps to level 3
- `hvc` ("hypervisor call") traps to level 2

Which of the two a machine wants, its device tree says (`/psci`, property `method`). The answer comes back in `x0`.

```rust
unsafe fn firmware(call: FirmwareCall, function: u64, first: u64, second: u64, third: u64) -> u64 {
    let answer: u64;
    unsafe {
        match call {
            FirmwareCall::Hvc => asm!(
                "hvc #0",
                inout("x0") function => answer, inout("x1") first => _, inout("x2") second => _, inout("x3") third => _,
                clobber_abi("C"), options(nostack),
            ),
            FirmwareCall::Smc => asm!( "smc #0", /* the same */ ),
        }
    }
    answer
}
```

Two Rust things here. `inout("x0") function => answer` says: put `function` in `x0` before the instruction, and read `answer` out of `x0` after it. And `clobber_abi("C")` tells the compiler that this instruction may change every register a C function is allowed to change - which is exactly what the PSCI calling convention promises and no more. Without it the compiler would assume `x4` to `x17` survive the call, and sometimes they would not.

The one function I need is `CPU_ON` (number `0xC4000003`): `x1` names the CPU, `x2` is the *physical* address it starts running at, `x3` is a number it finds in its own `x0` when it does.

The part with no instruction in it - the numbers, what an answer means (`-2` is "no such CPU", `-4` "already on"), and *which* CPUs to ask for - is in `kernel64/src/psci.rs`, portable, with host tests. Which CPUs: all the device tree lists, except the one already running, up to the 16 this kernel has room for.

QEMU showed me that the instruction really does depend on the machine. Entered at level 1 the tree says `hvc`. Started with `virtualization=on` the kernel is entered at level 2, there is no hypervisor above it to call, and the tree says `smc`.

### What a CPU needs when it starts

The firmware starts a CPU the way a loader starts the first one: MMU off, at a physical address, maybe at exception level 2. So it needs a small copy of what `_start` does. But much less:

| | the first CPU (`_start`) | every other (`secondary_start`) |
|---|---|---|
| come down from level 2 | yes | yes - each CPU has its own level-2 registers |
| zero `.bss` | yes | **no** - it is in use |
| build page tables | yes | no - adopt the kernel's |
| a stack | the boot stack | its own, allocated for it |

The level-2 part was already written, so I turned it into an assembler macro (`leave_el2`) and both entries use it.

What the new CPU must be *told* is in one block of memory, filled in by the boot CPU just before it asks. The address of the block is the number passed through `CPU_ON`:

```rust
#[repr(C)]
struct Start {
    tables: AtomicU64,         //  0  the kernel's page tables (TTBR1_EL1)
    translation: AtomicU64,    //  8  TCR_EL1
    control: AtomicU64,        // 16  SCTLR_EL1 - "MMU on" and the rest
    memory_kinds: AtomicU64,   // 24  MAIR_EL1
    features: AtomicU64,       // 32  CPACR_EL1
    counter_access: AtomicU64, // 40  CNTKCTL_EL1
    stack_top: AtomicU64,      // 48
    index: AtomicU64,          // 56  the kernel's number for this CPU
}
```

`#[repr(C)]` matters: the assembly reads the fields by their offsets (`ldr x1, [x19, #24]`), and without it Rust may put the fields in any order it likes. The boot CPU simply reads its own registers and stores them, so every CPU ends up configured exactly like the first.

One problem is left. The moment the MMU comes on, the CPU is still executing at a *physical* address, and the kernel's tables only map high addresses. The first CPU solved this at boot with a small identity map in the low half (`boot_l0_low`), used it for a few instructions, and switched the low half off. But it never took the tables apart. So a starting CPU borrows them:

```
    adrp    x1, boot_l0_low
    msr     ttbr0_el1, x1           // low half: the old identity map, for a moment
    ldr     x1, [x19, #0]
    msr     ttbr1_el1, x1           // high half: the kernel's real tables
    ...
    msr     sctlr_el1, x2           // MMU on - the next instruction is found through the low half
    ldr     x1, =.Lsecondary_high
    br      x1                      // ...and this jumps to a high address
```

Then Rust (`secondary_main`) switches its low half off again.

CPUs are started one at a time: fill in `Start`, call `CPU_ON`, wait until the CPU says it is awake, go on to the next. That way one block is enough.

### "Which CPU am I?"

Every CPU runs the same code and sees the same memory, so the answer cannot be in a variable. On x86_64 I use the `GS` segment for this. AArch64 has a register that exists for exactly this purpose: `TPIDR_EL1`. Each CPU has its own, and the CPU itself never reads or changes it. The entry code puts the CPU's number there, and `arch::cpu()` is one instruction:

```rust
pub fn cpu() -> usize {
    let number: u64;
    unsafe { asm!("mrs {}, tpidr_el1", out(reg) number, options(nomem, nostack, preserves_flags)) };
    number as usize
}
```

The kernel's numbers are not the hardware's names. The hardware names a CPU by `MPIDR_EL1` (on QEMU: 0, 1, 2, 3, but on real machines numbers like `0x100` and `0x101`). The kernel numbers CPUs 0, 1, 2 … in the order they woke, so the numbers have no holes even if a CPU does not answer.

### An interrupt controller with a part for each CPU

A GIC has a part all CPUs share and a part each CPU has for itself. Until now I had set up "the" per-CPU part, because there was one CPU.

- **Version 2.** The CPU interface is at the same address for every CPU, and the hardware shows each CPU its own registers there ("banked"). So a new CPU runs the same few writes as the first did, at the same address, and they land in different registers.
- **Version 3.** Each CPU has a *redistributor*: a pair of pages in a row of such pairs, and the CPU must find its own by comparing the name stored in each with its own `MPIDR`. Then it wakes it. The CPU interface is in system registers, which are per CPU by nature.

Devices' interrupts (the serial port, the disk) all go to the boot CPU, as before. Spreading them is not needed for anything yet.

The timer needed nothing new at all. The Arm generic timer is part of each CPU, so "start it here" is the same code run on each one.

### One CPU interrupting another

When a busy CPU wakes a thread, an idle CPU should run it *now*, not when its own timer next ticks. On the PC that is an inter-processor interrupt through the APIC. On a GIC it is a **software generated interrupt** (SGI): interrupt numbers 0 to 15 are reserved for CPUs to send to each other. I use two: 0 for "look at the run queue", 1 for "stop, the kernel has panicked".

Sending differs between the versions, and both are small pure functions with host tests:

```rust
/// Version 2: one write to the distributor. Bits 16-23 say which CPUs, a bit each.
pub fn software_v2(targets: u8, id: u32) -> u32 {
    u32::from(targets) << 16 | (id & 0xF)
}

/// Version 3: a write to the CPU's own ICC_SGI1R_EL1, which names the target by its MPIDR.
pub fn software_v3(mpidr: u64, id: u32) -> Option<u64> { ... }
```

In version 2 a CPU learns its own bit by reading a register that answers differently on each CPU. In version 3 the register addresses a group of 16 CPUs by the upper parts of their name and one in the group by a bit, so a CPU whose lowest name part is 16 or more cannot be addressed that way: the function returns `None` instead of sending to the wrong one.

### What did not change

The scheduler, the run queue, the idle threads, the locks, the self-tests for "threads really run on every CPU" and "an idle CPU is woken by an interrupt, not by its timer": all of that is the PC's code. The two self-tests used the PC's `tsc` to measure time; they now use `arch::clock()`, which each architecture provides, and that was the whole port.

## Part 2: lines, kept whole

The typed session passed on one CPU. On four it failed, and when I looked at the terminal I saw this:

```
Hello from C
argc [process] 132 "chello" exited with code 3
2
argv[[shell] ran "chello fail"
1] fail
```

On this machine the kernel's log *is* the screen. Three writers share the one serial line: the `serial` driver (showing what programs print), other programs' log lines, and the kernel. A C program's `argc 2` arrives at the driver as two or three pieces. With one CPU the pieces nearly always got through before anyone else ran. With four, the program exits on one CPU, the kernel announces it at once, and the driver on another CPU is still holding the `2`.

This is not a locking bug. Every piece is written whole. The pieces are just not lines.

So the kernel now merges the log **by line** (`kernel64/src/logmerge.rs`):

- Each piece comes with its writer: the kernel, or a process id.
- A writer whose last piece did not end in a newline has the line *open*.
- While a line is open, what other writers send is **held**, and written when that line ends.

```
   serial:  "argc "            -> written, line open (serial)
   kernel:  "[process] ...\n"  -> held
   shell:   "[shell] ran\n"    -> held
   serial:  "2\n"              -> written, line ends -> the held text follows

   argc 2
   [process] 132 "chello" exited with code 3
   [shell] ran "chello fail"
```

Lines are kept whole, not kept in any particular order *between* writers: the kernel's "exited" line can still come before the program's last lines, because it really was sent first. That is only untidy.

It cannot hold for ever. A shell prompt is a line that stays open until somebody presses Enter, and a kernel message must not wait for that. So held text waits three timer ticks at most (30 ms) and is then written into the open line anyway. A torn line is better than a silent log.

One consequence I had to accept and write down: a kernel message is no longer *always* on the wire when `kprintln!` returns. It can be held for those few ticks behind a program's half line. When the kernel is stopping (a panic, a fatal exception) nothing is held any more: the first thing that path does is let everything go and switch holding off.

The bookkeeping has no hardware in it, so it is tested on the host: eight tests, among them "a line written in pieces is not torn by another writer" and "held text waits a few ticks and no longer".

## Part 3: a UEFI loader for AArch64

### What the kernel expects, and what UEFI gives

The AArch64 kernel is not started with my `TaajBootInfo` structure. It follows the convention every AArch64 loader uses: a flat image with a 64-byte header, entered at its first byte with the **MMU off** and the address of a **device tree** in `x0`. That is why QEMU's `-kernel` could start it with no loader of mine.

A UEFI application starts in a very different world: MMU on, caches on, every address mapped to itself, and the firmware still owning the machine. So `BOOTAA64.EFI` (`bootloader/src/boot/uefi/aarch64.c`) has six things to do:

1. read `\kernel\Image` from the partition it came from
2. copy it to an address of the kind its header asks for (some multiple of 2 MiB, plus the header's offset)
3. find the device tree
4. take the final memory map and exit boot services
5. write a copy of the device tree that lists the memory the firmware keeps
6. turn the MMU off and jump

The x86_64 loader and this one share their console and file code now (`common.h`). `efi.h` needed one change: on x86_64 every firmware function is marked with the Microsoft calling convention; on AArch64 there is only one convention and nothing to mark.

### The device tree, and why `acpi=off`

UEFI firmware offers tables through a list of "configuration tables", each with an identifying GUID. The device tree is one of them.

But only if the firmware has one to give. QEMU's firmware for `virt` describes the machine with ACPI tables by default, and then hands a loader no device tree at all. My kernel reads no ACPI on AArch64. So the machine has to be started with `-M virt,acpi=off`, and when it is not, the loader says so and starts nothing:

```
TheTaaJBoot: the firmware has no device tree to give (on QEMU: start the machine with -M virt,acpi=off) (status 0x800000000000000e)
```

### Memory the firmware keeps

This is the part that a UEFI machine adds, and the part I find most interesting.

The device tree says where the RAM is: all of it. But part of that RAM the firmware still uses *after* it has handed over - its runtime services live there. Only the firmware's memory map knows which part. My kernel does not read that map. It reads the tree.

I did not want to teach the kernel about UEFI. And there is no need: a device tree blob already has a list of reserved memory, and the kernel already stays out of it. So the loader writes the firmware's "keep out" ranges into that list.

A blob is four parts, and the header says where each one starts:

```
   old:  [ header ][ reserved list ][ structure ............ ][ strings ]
   new:  [ header ][ reserved list + mine ][ structure ............ ][ strings ]
                                            ^ moved along; the header is told
```

The list is not at the end, so it cannot simply grow. The loader writes a fresh copy: header, the old list, its own ranges, the end marker, then the structure and the strings copied further along, and the three offsets and the total size in the header corrected. Nothing inside the structure refers to a position in the blob, so nothing else has to change. Everything in a blob is big-endian, so each number is read and written a byte at a time.

Which ranges? Everything in the firmware's map except free memory, the boot services' memory (they are gone when the kernel reads this), the loader's own, and device registers (not RAM). Sorted, and touching ranges joined. On QEMU that makes three.

All memory for this is allocated *before* the final memory map is read, because allocating changes the map, and `ExitBootServices` only succeeds with the current one. The copy itself is written after the firmware is gone.

### Turning the MMU off

Turning a data cache off loses whatever is in it and not yet in memory. So before the jump, everything the kernel is about to read - its image, the new tree - is written through to memory, one cache line at a time (`dc cvac`; `CTR_EL0` says how long a line is). The instruction cache is told to forget those addresses too, in case firmware code ran there before.

The last instructions are assembly, with no stack: once the cache is off, a value just written to the stack may not be what a read finds there.

```c
__attribute__((naked, noreturn)) static void EnterKernel(uint64_t Entry, uint64_t Tree)
{
    __asm__(
        "msr    daifset, #0xf\n\t"      /* no interrupts of any kind */
        "mov    x10, #5\n\t"            /* the MMU (bit 0) and the data cache (bit 2) */
        "mrs    x9, CurrentEL\n\t"
        "cmp    x9, #8\n\t"             /* level 2? */
        "b.eq   1f\n\t"
        "mrs    x9, sctlr_el1\n\t"
        "bic    x9, x9, x10\n\t"
        "msr    sctlr_el1, x9\n\t"
        ...
        "br     x4\n\t");
}
```

The code keeps running across "MMU off" because the firmware had every address mapped to itself: the next instruction is at the same address with or without translation. The firmware may be at level 1 or level 2, and each level has its own control register, so the loader checks which.

### The kernel says what it was given

When I tested the loader by breaking it, leaving the MMU *on* changed nothing. The kernel's first instructions use only relative addresses and set up their own tables, so they survive it. That is nice for the kernel and bad for the test: a loader with this bug would pass.

So the kernel now looks, in its very first instructions, at whether the loader's MMU was on, and says so:

```
[cpu] Arm Cortex-A72 (id 0x410fd083), at exception level 1; the loader entered at level 1, with the MMU off
```

and the tests ask for "with the MMU off" - for every loader, QEMU's included.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How to start CPUs | PSCI only | The older "spin table" method (CPUs spin in firmware watching an address): some old boards still use it, QEMU does not, and I could not test it. |
| What a starting CPU maps low | Borrow the boot CPU's old identity tables | Build new ones per CPU: more code, and the boot ones were already there and correct. |
| Start order | One at a time, each confirmed | All at once: needs a `Start` block and a stack hand-over per CPU, for a second saved at boot. |
| CPU number | `TPIDR_EL1` | A table searched by `MPIDR`: a search on every "which CPU am I", which is asked constantly. |
| Devices' interrupts | All to the boot CPU | Spread over CPUs: nothing needs it yet, and a driver is a program that may run anywhere anyway. |
| Torn lines | Merge by line in the kernel, with a time limit | Loosen the test: the terminal really was unreadable. A lock programs hold across pieces: a program that died mid-line would hold it for ever. Give `serial` whole lines only: a prompt is not a whole line. |
| Hold time | 3 ticks | Until the line ends: a prompt never ends by itself. 1 tick: a tick can be nearly over when the piece arrives. |
| Loader protocol on AArch64 | The standard one: flat image, MMU off, tree in `x0` | My `TaajBootInfo`: then QEMU, U-Boot and the Raspberry Pi's firmware could not start the kernel without my loader. |
| Firmware's memory | Written into the tree's reserved list | The kernel reads the UEFI map: a second source of truth about memory, for one loader. |
| No device tree | Say so and stop | Read ACPI: a large piece of work, for a machine that offers a tree when asked. |
| Where the loader lives | Next to the x86_64 one, sharing `common.h` | A separate directory: the two share most of what is not about the CPU. |

## Bugs

**Ctrl+C before the program had said anything.** The first failure on four CPUs: `forever sleep`, then Ctrl+C, and the line `forever: sleep until stopped` never appeared. The test's typist sent keys a fiftieth of a second apart. On one CPU the new program got to print before the shell saw the Ctrl+C. On four, the shell handles the key on another CPU and ends the program before it has printed. Nothing is wrong with the system here: a person presses Ctrl+C at something they see running. The typist now waits 0.3 s before a Ctrl+C.

**Torn lines.** Part 2.

**A background job's first line after the prompt.** With lines kept whole, one check still failed: `forever yield &` printed its first line after the next prompt had been drawn (`taaj:/> forever: yield until stopped`). That is what any terminal does with a background job, and with several CPUs the job really may start after the shell has gone on. The check for that one line no longer insists it begins a line - on AArch64 only, where the screen is in the log.

**Held text that nothing proved was ever let go.** Found by breaking the code on purpose: I removed the line that releases held text after three ticks, and every test still passed, because every line in the session eventually ended. The second boot of the typed test now starts a silent program in the background (`cprimes 100 > out &`). The kernel's "exited" message for it arrives while the prompt is open and nothing more is typed: it must appear anyway.

**The firmware did not find the loader.** My first partition image was made with `mformat -F` (FAT32) at 16 MiB, which is too small to be a valid FAT32. The firmware said `failed to load Boot0001 … Not Found` and tried the network. Made with `mkfs.fat` like the PC's, it worked.

**"No PSCI" cannot be staged on QEMU.** I wanted a boot with a device tree that names no firmware. QEMU puts the `/psci` node back into any tree it is given. I removed that boot from the test; see below.

## How I checked it

**Automated** (`make test64`, which must end `test64: all passed`):

- Host tests: PSCI answers and the choice of CPUs (5), sending a software interrupt in both GIC versions, the line merge (8).
- `aarch64-test`: boots with 4 CPUs on GIC version 2 and 3, entered at level 1 and 2 (so both `hvc` and `smc`), with 8 CPUs, and with 20 (16 used, 4 "left asleep"). Each must show every CPU awake, the count under a lock exact, threads on all CPUs, and the wake-up test.
- `aarch64-keys-test`: the PC's whole typed session, now on four CPUs, including that four copies of a C program really ran at once (`took 9.47 s; the 4 CPUs worked for 34.63 s in all`), then the one-CPU boot with a program that never stops.
- `aarch64-uefi-test`: three boots through the firmware and my loader - four CPUs with a disk and a program run from it; firmware at level 2 with GIC version 3; and firmware with ACPI, where the loader must refuse. The number of reserved ranges the loader says it wrote must be the number the kernel reads, and at least one.

**By breaking it on purpose.** Each of these changes made a test fail, as it should:

| Change | Caught by |
|---|---|
| no interrupt sent to an idle CPU | wake-up self-test: `only 0 CPUs were interrupted in 40 wake-ups` |
| a started CPU does not start its timer | SMP self-test panics |
| a started CPU does not set up its part of the interrupt controller | SMP self-test panics |
| every CPU thinks it is CPU 0 | SMP self-test panics |
| GIC version 3: a started CPU does not wake its redistributor | SMP self-test panics |
| anyone may write into an open line; held text released a tick late; released text forgets who has the line; the wait counted from the newest piece | host tests |
| held text never released | the new check in the second typed boot |
| the loader writes no ranges into the tree | `the loader wrote 3 reserved ranges, the kernel read 0` |
| the loader leaves the MMU on | `the kernel was not entered at level 1 with the MMU off` (only after the kernel learned to say so) |
| the image not placed where its header asks | the boot never finishes |
| the tree's strings not copied | the kernel cannot read the tree: memory self-test missing |

**By hand:** the boots in the testing note below.

**Not tested:**

- Any real hardware. Everything here ran in QEMU, which does not model caches: the cache cleaning in the loader has never been needed by anything that ran it, so it is unproven.
- That the loader's list of the firmware's memory is *complete*. I removed one kind of range from it on purpose and the test still passed: the test only knows that what the loader computed reaches the kernel. Nothing checks it against the firmware's map from outside.
- A machine whose tree names no PSCI, and a firmware that refuses `CPU_ON`. The code for both is there and prints a message; QEMU cannot be made to do either.
- The "stop" interrupt between CPUs (a panic on one CPU halting the others) on AArch64.
- CPUs whose hardware names are not small numbers (`software_v3` returning `None`) outside the host test.
- A device tree from UEFI firmware other than QEMU's.

## What is still missing

- ACPI on AArch64, which is what UEFI firmware offers by default.
- Devices' interrupts all go to the boot CPU.
- The loader does not pass a command line, and loads no ramdisk.
- The kernel image must be named `\kernel\Image`; there is no menu and no configuration file.
- No framebuffer on AArch64: the serial port is still the only screen.
- Raspberry Pi and RISC-V.

## What I learned

- On AArch64 the kernel does not own the CPUs that are off. Something above it does, and the kernel asks. That felt like a restriction at first; it is also much less code than the PC's 16-bit start-up page.
- A CPU's "who am I" has to come from a register, never from memory, and Arm provides a register just for it.
- `#[repr(C)]` is what makes it safe for assembly to read a Rust struct by offsets.
- `clobber_abi("C")` on inline assembly: the compiler only knows what an instruction changes if I tell it.
- More CPUs do not only make things faster. They change *orders*: who prints first, whether a program has started when the next key arrives. Two of my tests had been relying on an order that one CPU gave them for free.
- A lock keeps a *write* whole. It does not keep a *line* whole if the line is several writes. Those are different problems.
- A device tree blob is simple enough to edit by hand: four parts and a header of offsets.
- UEFI's rule that nothing may allocate between reading the memory map and exiting decides the order of the whole loader.
- A test that passes when I break the thing it is about is not testing it. The MMU check and the held-text check both exist because of that.

## Testing note

**1. Four CPUs on AArch64**

```
make run-aarch64
```

The terminal you started it from is the machine's keyboard and screen. To leave QEMU: **Ctrl+A, then X**. The machine now has four CPUs.

In the boot log:

| Look for | Meaning |
|---|---|
| `[cpu] … the loader entered at level 1, with the MMU off` | new: the kernel says what state its loader left |
| `[tree] firmware    called with hvc` | how this machine's firmware is asked |
| `[smp] CPU 1 awake: named 0x1, stack below 0x…` (and CPU 2, CPU 3) | each CPU says so itself, on its own stack |
| `[smp] 4 CPUs awake: CPU 1, 2, 3 started by the firmware (PSCI 1.1, asked by hvc)` | all started |
| `[smp] counting together: 4 CPUs reached 400000 of 400000 with a lock; without one …, … lost` | the lock works; without one, counts are lost (the lost number differs each time) |
| `[smp] 4 CPUs run threads: each has its own timer and idle thread, and all take from one queue` | the scheduler has them |
| `[selftest] smp: threads ran on all 4 CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked` | threads really ran everywhere |
| `[selftest] wake-ups: a thread woken by a busy CPU ran on an idle one … us later (median of 40; …; 41 interrupts between CPUs)` | one CPU interrupts another |

| Type | You should see |
|---|---|
| `cprimes 24000000 &` four times | `[1] cprimes 24000000` … `[4] cprimes 24000000` |
| `time wait` | four lines `cprimes: 1507122 primes below 24000000`, then `took … s; the 4 CPUs worked for … s in all` - the second number is three to four times the first (I saw `took 9.91 s; the 4 CPUs worked for 36.93 s in all`) |
| `chello one` | `Hello from C`, `argc 2`, `argv[1] one` - each on a line of its own. Log lines (`[process] … "chello" exited with code 0`, `[shell] ran "chello one"`) may come before, after or between them, but not inside one |

**2. Other machines**

```
make run-aarch64 RUN_A64="-smp 1"
make run-aarch64 RUN_A64="-M virt,gic-version=3 -smp 8"
make run-aarch64 RUN_A64="-M virt,virtualization=on"
make run-aarch64 RUN_A64="-smp 20"
```

| Look for | Meaning |
|---|---|
| first: `[smp] 1 CPU: no others to start` | one CPU still works |
| second: `[irq] GIC version 3: …` and `[smp] 8 CPUs awake: CPU 1, 2, 3, 4, 5, 6, 7 started by the firmware (PSCI 1.1, asked by hvc)` | the other interrupt controller, eight CPUs |
| third: `the loader entered at level 2, with the MMU off` and `… (PSCI 1.1, asked by smc)` | entered at level 2: the firmware is called the other way |
| fourth: `[smp] 4 more CPU(s) than the 16 this kernel uses: left asleep` and `[smp] 16 CPUs awake: …` | more CPUs than the kernel has room for |

(A later `-smp` wins over the `-smp 4` the Makefile gives.)

**3. Booted by UEFI firmware, through my loader**

```
make run-aarch64-uefi
```

The firmware prints first: `UEFI firmware (version …)`, then `BdsDxe: failed to load Boot0001 "UEFI Misc Device" … Not Found` (that is the system's own disk, which has nothing on it to boot), then `BdsDxe: starting Boot0002 "UEFI Non-Block Boot Device" …`. Then:

| Look for | Meaning |
|---|---|
| `TheTaaJBoot (UEFI, AArch64)` | my loader is running |
| `kernel … KiB at 0x…` | where it put the image |
| `device tree … bytes at 0x…, copied to 0x… with 3 ranges the firmware keeps` | the tree it found, and what it added |
| `firmware at exception level 1 - exiting boot services` | the last thing it can say |
| `Hello from TheTaaJ!` | the kernel |
| `[cpu] … the loader entered at level 1, with the MMU off` | the loader turned it off |
| three lines `[tree] reserved    0x… - 0x…` | the firmware's memory, read by the kernel out of the tree |
| the `[smp]` lines of 1, and then the shell | everything else is the same |

| Type | You should see |
|---|---|
| `cat welcome` | `Hello from the host` |
| `chello` | `Hello from C` and `argc 1` |

**4. Firmware that has no device tree**

```
make aarch64-esp
cp /usr/share/AAVMF/AAVMF_VARS.fd build/aarch64/vars.fd
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.fd \
    -drive if=pflash,format=raw,file=build/aarch64/vars.fd \
    -drive file=build/aarch64/esp.img,format=raw,if=none,id=esp -device virtio-blk-pci,drive=esp
```

(`-M virt` without `acpi=off`.) You should see `TheTaaJBoot: the firmware has no device tree to give (on QEMU: start the machine with -M virt,acpi=off) (status 0x800000000000000e)`, and no `Hello from TheTaaJ!`. After five seconds the firmware says `BdsDxe: failed to start Boot0001 … Not Found` and tries the network (`>>Start PXE over IPv4.`). Leave with Ctrl+A, then X.

**5. Lines kept whole, and not held for ever**

In `make run-aarch64`:

| Type | You should see |
|---|---|
| `chello fail` | `Hello from C`, `argc 2`, `argv[1] fail`, `chello exited with code 3` - each line whole; no `[process]` or `[shell]` text in the middle of one |
| `cprimes 100 > out &` then nothing | `[1] cprimes 100 > out`, a new prompt `taaj:/> `, and a moment later, on the prompt's line, `[process] … "cprimes" exited with code 0` - the kernel's message waited for the prompt's line to end, briefly, and then did not |
| Enter | `[1] done  cprimes 100 > out` |

**6. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu psci
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu logmerge
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu gic
```

should say `5 passed`, `8 passed` and `13 passed`.

**7. The three AArch64 tests alone**

```
make user-aarch64 cprogs-aarch64 kernel-aarch64
make aarch64-test
make aarch64-keys-test
make aarch64-uefi-test
```

should end `aarch64-test: all passed`, `aarch64-keys-test passed` and `aarch64-uefi-test passed`. The UEFI test needs QEMU's firmware (`sudo apt install qemu-efi-aarch64`); without it the test says it was skipped.

Automated: `make test64`
