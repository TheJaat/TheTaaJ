# First Words on AArch64

*Journal entry #50 · the first step of Era 3. The same kernel crate is built for a second kind of CPU, started by QEMU's `virt` machine, and gets as far as the x86_64 kernel did in entries 2 and 3: it turns on the MMU, runs in the high half of the address space, prints through a serial port, and catches its own exceptions. Nothing else works yet, and the entry says exactly what.*

```
Hello from TheTaaJ!
64-bit kernel on AArch64, kernel_main at 0xffffffff80081f1c
[cpu] exception vectors loaded
[cpu] breakpoint at 0xffffffff8008113c - continuing
[cpu] resumed after the breakpoint - exceptions round-trip
[cpu] Arm Cortex-A72 (id 0x410fd083), at exception level 1; the loader entered at level 1
[cpu] 44 bits of physical address; MMU on, the kernel loaded at 0x40080000 and running at 0xffffffff80080000
[boot] device tree at 0x48000000: 1048576 bytes, version 17, boot CPU 0

[kernel] this is as far as the AArch64 port goes - halting
```

---

## Why now

Entry 49 closed Era 2: a C program, compiled on my laptop, running on four CPUs. The roadmap's next era is one line:

> **3 · Portable.** AArch64 on QEMU `virt`, then a Raspberry Pi; RISC-V after; every architecture booted on every commit. *Done when the same user space boots on x86_64 and AArch64.*

Entry 1 gave the reason, and it has not changed: every feature I add to an x86-only system is a feature to port later. The kernel has had a directory called `arch/` from its first day (entry 2), with a comment at the top:

> Adding AArch64 means adding `arch/aarch64/` with the same functions - and nothing outside `arch/` changes.

This era is where I find out whether that comment was true.

## The plan for the era

The x86_64 `arch` module exports about twenty things. I am not going to write twenty things and then boot. Each step below ends with a kernel that boots and a test that says so.

| Step | What | The x86_64 entry it repeats |
|---|---|---|
| **3a** | **Start, MMU, serial port, exceptions - this entry** | 2, 3 |
| 3b | Read the device tree: memory, the serial port, the interrupt controller, the CPUs | 6 (its ACPI half) |
| 3c | Memory: the frame allocator, the kernel's own tables with real permissions, the heap | 4 |
| 3d | Interrupts and time: the GIC and the timer every AArch64 CPU has | 6 |
| 3e | Threads: the context switch | 7 |
| 3f | Programs: exception level 0, `svc`, the user library and `libc` built for AArch64 | 8, 36–42 |
| 3g | Devices: virtio over memory-mapped registers, then the disk and the shell | 10–15 |
| 3h | The other CPUs, through PSCI | 44–47 |
| 3i | My own loader for AArch64 (UEFI), instead of QEMU's | 5 |

The order may change. The rule will not: `test64` boots both architectures from this entry on.

## What is different about this CPU

A table of the differences this entry meets. I will explain each where it is used.

| | x86_64 | AArch64 |
|---|---|---|
| Privilege | rings 3 (program) and 0 (kernel) | exception levels: EL0 program, EL1 kernel, EL2 hypervisor, EL3 firmware |
| CPU configuration | control registers, MSRs, GDT, IDT, TSS | **system registers**, read and written with `mrs` and `msr` |
| Page tables | one root, `CR3` | two roots: `TTBR0_EL1` for low addresses, `TTBR1_EL1` for high ones |
| Exception entry | a table of *addresses* (the IDT); the CPU pushes a frame on the stack | a table of *code* (the vectors); the CPU pushes nothing |
| Devices | I/O ports and memory | memory only |
| Where memory is | always at 1 MiB | wherever the board put it |
| Instruction size | 1 to 15 bytes | always 4 |

---

## Part 1 · The image

### What a loader expects

On a PC my own loader reads `kernel.elf`, copies its segments to 1 MiB and jumps. I have no AArch64 loader. QEMU has one built in (`-kernel`), and so does every real board; they all understand the format the Linux kernel made standard:

- a **flat image** - no ELF, just the bytes of the kernel as they should lie in memory;
- which **begins with a 64-byte header**;
- loaded at any 2 MiB boundary plus an offset the header asks for;
- entered at its **first byte**, with the address of the device tree in register `x0`.

The first byte is both "the start of the header" and "the first instruction". So the header's first field is an instruction: a jump over the rest of the header.

```asm
_start:
    b       .Lbegin                 // "code0": over the header
    .long   0                       // "code1"
    .quad   0x80000                 // how far into a 2 MiB step to load this
    .quad   __image_size            // how much memory it needs, .bss included
    .quad   0b1010                  // little-endian, 4 KiB pages, anywhere in memory
    .quad   0
    .quad   0
    .quad   0
    .ascii  "ARM\x64"               // the magic number loaders look for
    .long   0
```

The build makes the ELF file as before, and then `llvm-objcopy -O binary` writes out just the section bytes. The result is 19,459 bytes. The header says it needs 180,224 in memory: the difference is `.bss`, which a flat image does not carry.

### No fixed address

A PC always has memory at 1 MiB. AArch64 boards put memory where they like: QEMU's `virt` at 1 GiB, a Raspberry Pi at 0. So the linker script cannot say where the kernel is loaded. It says only where it *runs*:

```
virtual 0xFFFFFFFF80000000 + x   ->   physical (the 2 MiB step it was loaded in) + x
```

and the kernel finds the physical side out when it starts.

---

## Part 2 · The first instructions

`arch/aarch64/entry.rs` is one block of assembly. When it starts:

- the MMU is off, so every address is physical;
- there is no stack;
- the code was linked for `0xFFFFFFFF80080000` and is running at `0x40080000`.

That last point sounds fatal and is not. An instruction like "load from address `0xFFFFFFFF80085000`" would fail. But AArch64 code almost never says that. It says "the page 5 pages after the one I am in" (`adrp`) or "the label 40 bytes back" (`adr`, `b`). Those work wherever the code is. This is **position-independent** code, and it is why this part is assembly: I can see that every instruction is of that kind. Rust makes no such promise.

It does five things.

### 1. Come down to exception level 1

A kernel runs at EL1. QEMU enters at EL1 by default, but real boards (and `-M virt,virtualization=on`) enter at EL2, where a hypervisor would live. There is no instruction "go to EL1". There is only *return from exception* (`eret`), which goes to whatever level the saved state says the exception came from. So the kernel forges that state:

```asm
    mov     x1, #(1 << 31)
    msr     hcr_el2, x1             // EL1 is 64-bit; nothing it does traps to EL2
    ...
    mov     x1, #0x3c5              // "came from": EL1, its own stack pointer, interrupts off
    msr     spsr_el2, x1
    adr     x1, .Lat_el1
    msr     elr_el2, x1             // "return" to the next instruction
    eret
```

It returns from an exception that never happened, to a level it was never in.

### 2. Zero `.bss`

Rust assumes a `static` that starts as zero *is* zero. In an ELF file the loader sees to that. In a flat image nobody does. The loop is four instructions, and it has to come first: the page tables and the stack are in `.bss`.

### 3. Build the tables

Four levels, nine bits of the address each, 4 KiB pages - the same shape as x86_64. Two differences:

- **Two roots.** Addresses whose top bits are zero are translated through `TTBR0_EL1`; those whose top bits are one, through `TTBR1_EL1`. The kernel's half and a program's half are separate trees, and switching programs will change only one register.
- **An entry can map a whole 2 MiB or 1 GiB**, a "block", with no table below it. (x86 has this too; I had not needed it.)

```
TTBR1 (high)  boot_l0_high[511] -> boot_l1_high
                boot_l1_high[510] -> boot_l2_kernel    2 MiB blocks: the image
                boot_l1_high[511] -> boot_l2_device    empty, filled later (part 3)

TTBR0 (low)   boot_l0_low[0]    -> boot_l1_low
                boot_l1_low[n]   =  1 GiB mapped to itself, for the GiB the
                                    image is in and the GiB the device tree is in
```

Six pages, all in `.bss`. The low mapping exists for one instruction - see the next step - and, until the kernel has a memory manager, to read the device tree.

### 4. Turn the MMU on, and jump

```asm
    msr     sctlr_el1, x1           // the MMU is on
    isb
    ldr     x1, =.Lhigh             // an address as the linker wrote it: a high one
    br      x1
.Lhigh:
```

The instruction after the MMU comes on is fetched from where the program counter points: still `0x4008....`. If that address were not mapped the CPU would fault with no handler. That is the whole reason for the low mapping. Then `ldr x1, =.Lhigh` loads the *linked* address of the next line and jumps there, and from that instruction on the kernel is where it was built to be.

### 5. Call Rust

Set the stack pointer, pass along three things learned on the way (the device tree, the level entered at, the load address), and `bl kernel_main`.

### The numbers live in Rust

The assembly needs values like "the bits of a 2 MiB kernel block" and "translation control: 48 bits, 4 KiB pages, cached". Those are defined once, in `src/aarch64.rs` in the kernel's library, and handed to the assembly as constants:

```rust
global_asm!(r#" ... ldr x4, ={normal_block} ... "#,
    normal_block = const descriptor::NORMAL_BLOCK,
    translation  = const translation_control(0),
    ...
);
```

The library is the part of the kernel that is compiled for my laptop and unit-tested. So the page-table bits are checked by `cargo test`, against the values in Arm's manual, before any CPU sees them.

---

## Part 3 · A serial port that is memory

A PC's serial port is reached with `in` and `out`: I/O ports, a second address space. AArch64 has none. A device's registers are at memory addresses.

The port is a **PL011**, Arm's own UART, on nearly every AArch64 board. On QEMU's `virt` it is at physical `0x0900_0000`.

Two things follow from "registers are memory".

**It needs a mapping, and a particular kind.** Each table entry names one of the kinds of memory listed in the `MAIR_EL1` register. Ordinary memory is cached: the CPU may keep a copy, merge two writes, read ahead. For a device every one of those is a bug - writing the data register *sends a byte*. So `MAIR_EL1` gets a second kind, "device", and `mmu::map_early_device` puts the UART's 2 MiB into the empty table from part 2 with that kind. The top 1 GiB of the address space is now the "early device window".

**The compiler must be told too.** `write_volatile` and `read_volatile` mean: do this access, here, every time. Without them the compiler is entitled to decide that a loop which reads the same address until a bit changes can read it once.

```rust
fn write_byte(byte: u8) {
    while read(FLAGS) & TRANSMIT_FULL != 0 {
        core::hint::spin_loop();
    }
    write(DATA, u32::from(byte));
}
```

With that, `kprintln!` works. Not one line of `console.rs` changed for it - it calls `arch::console_write`, and there is now a second `arch`.

---

## Part 4 · Exceptions

On x86_64 the IDT is a table of 256 addresses, and on an exception the CPU pushes five values on the stack before jumping.

On AArch64:

- The table (`VBAR_EL1` points at it) is **code**: 16 slots of 128 bytes. The CPU jumps to the *start of a slot*.
- Sixteen is four kinds of event (a synchronous exception, an interrupt, a fast interrupt, a system error) from four places (the kernel on the program stack pointer, the kernel, a 64-bit program, a 32-bit program).
- The CPU **pushes nothing**. It puts the return address in `ELR_EL1`, the old state in `SPSR_EL1`, the reason in `ESR_EL1`, and - for a bad address - the address in `FAR_EL1`.

So a slot has 32 instructions to save what it is about to destroy. Mine uses four:

```asm
    .macro vector number
    .balign 0x80
    sub     sp, sp, #272            // room for a Frame
    stp     x0, x1, [sp]            // two registers to work with
    mov     x0, #\number            // which slot this is
    b       exception_common
    .endm
```

`exception_common` saves the other 29 registers, `ELR` and `SPSR`, and calls the Rust function `exception(vector, &mut frame)`.

### Reading the reason

`ESR_EL1`, the *syndrome*, packs the reason into one number: the top six bits are the class, the rest are details. `kernel64::aarch64::Syndrome` decodes it - also in the library, also host-tested:

```rust
let abort = Syndrome(0x9600_0005).abort().unwrap();
assert_eq!(abort, Abort { from_user: false, access: Access::Read, cause: Cause::NotMapped { level: 1 } });
```

`0x96000005` is what the CPU really reports when the kernel reads an address with no level 1 entry. An "abort" is what x86 calls a page fault, and the syndrome is more helpful than x86's error code: it says at which level of the tables the walk gave up.

### The breakpoint

The one exception the kernel expects is the `brk` it executes on purpose, to prove the path. One trap: on x86 the saved address after `int3` is the *next* instruction. On AArch64 it is the `brk` itself. Return there and it happens again, for ever. Since every instruction is 4 bytes:

```rust
frame.elr += 4;
```

Everything else is reported and the kernel stops:

```
*** CPU EXCEPTION 0x25: data abort
    kernel read at 0xffffffff00000000: not mapped (the level 1 table has no entry)
    from the kernel (vector 4), syndrome 0x96000005
    pc  0xffffffff800822f8  state 0x600003c5  sp  0xffffffff8009af10
    x0  0xffffffff80084953  x1  0xffffffff8009b000  x2  0x0000000000000000
    ...
```

---

## Part 5 · What happened outside `arch/`

The comment said "nothing outside `arch/` changes". Here is the count.

| File | Change | Why |
|---|---|---|
| `main.rs` | every module but `console`, `arch` and `irqlock` is marked x86_64-only; a second, short `kernel_main` | the port has not reached them yet |
| `main.rs` | an allocator that always answers "no" | see Bugs |
| `console.rs` | two calls to the screen's "last words" marked x86_64-only | there is no screen yet |
| `lib.rs` | two new modules, `aarch64` and `devicetree` | pure logic, with tests |
| `build.rs`, `.cargo/config.toml`, `Cargo.toml` | a linker script, flags and dependencies per architecture | |

So the comment was nearly true for what exists: the console, the lock that turns interrupts off, the panic handler and the whole library compiled for AArch64 unchanged. The marks in `main.rs` are a list of what is left, and each later step removes some.

The Rust target is `aarch64-unknown-none-softfloat`. "softfloat" forbids the compiler the floating-point and vector registers, for the reason the x86 kernel is built without SSE: nothing saves them on a switch. Floating point is also left switched off in the CPU, so a kernel that used it by accident would take an exception at once rather than corrupt a program quietly.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How to be loaded | A flat image with the Linux AArch64 header, by QEMU's `-kernel` | *An ELF file through `-kernel`.* QEMU then jumps to the ELF entry address - a high one, with the MMU off - and does not pass the device tree in `x0`. *My own UEFI loader first.* Right in the end (step 3i), but it would have been two unknowns at once. The header is what real boards load too. |
| Load address | Found at run time; only "0x80000 into a 2 MiB step" is fixed | *Link for QEMU's 0x40080000.* One line shorter, and wrong on the first real board. |
| The tables before Rust | Built in assembly | *In Rust, called before the MMU is on.* Rust code may hold absolute addresses (in a jump table, a `static` that points at another); it would run until the day the compiler chose to. |
| Kernel mapping | 2 MiB blocks, read-write-execute | *4 KiB pages with real permissions now.* That is the memory manager's job (step 3c), exactly as on x86_64, where the loader's tables are also crude and the kernel replaces them. |
| The low mapping | Only the GiB of the image and of the device tree | *All of the first 4 GiB, as on x86.* On `virt` the first GiB is devices; mapping devices as ordinary cached memory invites the CPU to read them ahead of need. |
| The serial port's address | A constant for QEMU `virt`, named as such | *Read it from the device tree now.* That is a parser, and it is the next entry. Until then the constant has a name that says what it is. |
| One crate or two | The same crate, `cfg` on the modules not yet ported | *A separate `kernel-aarch64` crate.* It would start small and clean and then grow a second copy of everything. The marks are ugly, and they are a to-do list that the compiler checks. |
| Where constants live | In the library, passed to the assembly with `const` | *Written in the assembly as numbers.* Then no test ever looks at them. |
| Exception at EL3 | Hang | Leaving EL3 is firmware's work, and differs per board. No loader I will use enters there. |

## Bugs

It booted the first time I ran it. That says more about how much of this entry is the x86 kernel's entries 2 and 3 over again than about care. The bugs were all in what I had *not* thought about.

**It would not link: "no global memory allocator found".** The AArch64 kernel allocates nothing. But the kernel's library uses `Vec`, and Rust refuses to link a program that *could* ask for memory when there is nothing to ask. The x86 kernel's allocator lives in `mm.rs`, which is one of the modules not ported. So for now there is an allocator that always answers "no memory". If anything ever asks, the kernel panics and says so.

**A test that could not fail.** I removed the loop that zeroes `.bss`, to see the test catch it. Every boot passed. QEMU's memory *starts* as zeroes, so the loop makes no difference there - and would make all the difference on a real board after a restart. The test now has a boot called `dirty`: it has QEMU write random bytes over the kernel's `.bss` before starting it. Without the loop that boot prints nothing at all.

**Another.** I changed the exception return to restore two registers from the wrong place in the frame. Every boot passed again: the code after the breakpoint did not happen to use those two. "Exceptions round-trip" was a claim, not a check. Now `arch::breakpoint()` puts a different number in each of the 31 registers, takes the exception, and counts the registers that came back wrong:

```
*** KERNEL PANIC: 2 of 31 registers were changed by an exception that should have restored them all
```

Both of these are the lesson of entry 46 again, and I expect to keep relearning it: a test proves something only after I have seen it fail.

## How I checked it

**Automated** (`make test64`, which now ends with `make aarch64-test`):

- **Host tests** (125 in the kernel's library, 19 of them new): the page-table bits against the manual's values; the translation-control value for a Cortex-A72; which table entry an address uses; the syndrome decoder on real syndromes (breakpoint, kernel read, kernel write, user write, instruction fetch); the device tree header - a good one, the wrong byte order, all zeroes, a newer version, parts that lie outside the blob.
- **The image header**: the magic at byte 56, the load offset, and that the size it claims covers the file and is whole pages.
- **Six boots**, each of which must print every line at the top of this entry, take the breakpoint exactly once, and print no panic or exception:

| Boot | Machine | What it is for |
|---|---|---|
| `plain` | `virt`, Cortex-A72, 512 MiB | the ordinary case; entered at EL1 |
| `el2` | `virt,virtualization=on` | entered at EL2: the drop to EL1 |
| `a53` | Cortex-A53, 1 GiB | 40 bits of physical address instead of 44 |
| `max` | QEMU's `max` CPU, 128 MiB, GICv3 | 52 bits (the kernel must ask for no more than 48); the device tree at another address |
| `smp4` | four CPUs | the other three must stay out of the way |
| `dirty` | random bytes over `.bss` | the zeroing loop |

- **One fault boot**: a kernel built with `fault-page` reads `0xFFFFFFFF00000000`; the log must name a data abort, a kernel read, that address, "level 1", and must not go on.

**By hand - breaking it on purpose.** Six changes, one at a time, each put back afterwards:

| Change | Caught by |
|---|---|
| the handler does not step over the `brk` | every boot: 9,000 breakpoint lines in five seconds |
| EL1 is not told to be 64-bit | `el2` only: silence |
| the kernel's level 2 table is not linked into the tree | every boot: silence |
| `.bss` is not zeroed | `dirty` only (after I added it) |
| two registers restored from the wrong slot | every boot (after I added the check): the panic above |
| the link register not restored; two registers not saved | every boot: the same panic, "1 of 31" and "2 of 31" |

**Not tested, and not testable in QEMU as far as I can see:**

- **The caches.** On a real CPU the data cache may hold stale lines when the MMU comes on; a careful kernel invalidates it first. QEMU does not model the cache. This is not done.
- **The "device" kind of memory.** Mapping the UART as ordinary memory works in QEMU exactly as well.
- **The physical address size.** If the kernel asked for 32 bits it would still boot: everything it touches is below 4 GiB.
- **A load address other than `0x40080000`**, and a device tree in a different GiB from the kernel. The code computes both; QEMU only ever produces one case.
- **Entry at EL3.** `-M virt,secure=on` still enters this kind of image at EL1.
- **Real hardware.**

**The x86_64 kernel**: all of `test64` as before - the changes to shared files must not have moved it.

## What is still missing

Nearly everything; the plan at the top is the list. Specific to what this entry built:

- **No guard page under the boot stack**, and an overflow would not be reported: the handler runs on the same stack that just overflowed, faults again, and so on. x86 solved this with a separate stack for double faults; AArch64's answer is a second stack pointer, in step 3e.
- **Only one exception is handled.** An interrupt, were one enabled, is "unexpected" and stops the kernel.
- **The kernel is mapped writable and executable**, all of it.
- **The low mapping is still there** after boot. It belongs to programs.
- **The serial port's address is a constant.**
- **No way to stop QEMU from inside.** The test kills it when it sees the last line.

## What I learned

- **Exception levels, and that "down" is a return.** There is no instruction to lower privilege, on any CPU I know now; there is only returning from an exception, and you can write the return address yourself.
- **Position-independent code is not a compiler feature I switch on; it is a property of instructions.** `adr` and `adrp` say "relative to me". For its first few dozen instructions the kernel runs nowhere near the address it was linked for, and nothing notices.
- **Why there is an identity mapping at boot.** Not for convenience: for the one instruction after the MMU comes on. On x86 my loader has made one since entry 2, and I had never had to say what it was *for*.
- **A table of code.** The vector table made me see the IDT as a choice. x86 does more for you on entry (it pushes a frame, switches stacks); AArch64 does almost nothing and lets the kernel decide. Less to learn, more to write.
- **Memory-mapped I/O needs two promises**, one from the page tables (do not cache) and one from the compiler (do not optimise). Either alone is not enough.
- **Big-endian is not dead.** The device tree is big-endian on every machine. `u32::from_be_bytes` is the whole cost.
- **The `arch/` boundary held.** Forty-odd entries of "put it behind `arch::`" are why this entry is a new directory and not a rewrite.
- **QEMU is kind in ways hardware is not.** Zeroed memory, no cache, forgiving devices. Each kindness is a test that cannot fail. I can remove some (`dirty`); the rest have to be written down as untested until there is a board on my desk.

## Testing note

**1. Run the AArch64 kernel**

```
make run-aarch64
```

The serial port is your terminal. There is nothing to type yet. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `Hello from TheTaaJ!` | the serial port works, through its mapping as a device |
| `kernel_main at 0xffffffff8008…` | Rust is running in the high half: the MMU is on |
| `[cpu] breakpoint at 0xffffffff8008… - continuing`, once | an exception was taken… |
| `[cpu] resumed after the breakpoint - exceptions round-trip` | …and returned from, with all 31 registers intact |
| `[cpu] Arm Cortex-A72 (id 0x410fd083), at exception level 1; the loader entered at level 1` | |
| `[cpu] 44 bits of physical address; MMU on, the kernel loaded at 0x40080000 and running at 0xffffffff80080000` | where it was put, and where it thinks it is |
| `[boot] device tree at 0x48000000: 1048576 bytes, version 17, boot CPU 0` | the loader's `x0` pointed at a real device tree |
| `[kernel] this is as far as the AArch64 port goes - halting` | the end, for now |

**2. Entered at exception level 2**

```
make run-aarch64 RUN_A64="-M virt,virtualization=on"
```

| Look for | Meaning |
|---|---|
| `…at exception level 1; the loader entered at level 2` | the kernel came down a level by itself |
| every other line as in step 1 | |

**3. A fault, caught**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-page
```

The first command runs all the automated boots (it should end `aarch64-test: all passed`) and leaves the faulting kernel in `build/aarch64/`. The second runs it where you can watch.

| Look for | Meaning |
|---|---|
| `[test] forcing a page fault...` | |
| `*** CPU EXCEPTION 0x25: data abort` | the class, from the syndrome register |
| `kernel read at 0xffffffff00000000: not mapped (the level 1 table has no entry)` | who, what, where, and how far the table walk got |
| `from the kernel (vector 4), syndrome 0x96000005` | |
| a register dump, `pc` to `x30`, and then nothing | the kernel stopped |

**4. The PC is as it was**

```
make run64
```

| Type | You should see |
|---|---|
| `uptime` | `up … s (… ticks), on 4 CPUs` |
| `cprimes 100` | `cprimes: 25 primes below 100` |

**5. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu aarch64
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu devicetree
```

should end `test result: ok. 13 passed` and `test result: ok. 6 passed`.

Automated: `make test64`
