# Taking Over the CPU: GDT, TSS, IDT and Exception Handlers

*Journal entry #3 · when something goes wrong, say what and where*

The kernel from entry #2 had a property that makes development miserable: any mistake — a bad pointer, a division by zero — made the machine silently reboot. This entry is about why that happens, and about replacing it with a report that names the fault, the address, and every register.

---

## Why a fault reboots the machine

When the CPU hits something it cannot do, it raises an **exception** and looks in a table — the IDT, Interrupt Descriptor Table — for the handler. The kernel had not set one up. So:

1. the fault finds no handler — which is itself a fault;
2. that becomes a **double fault**, which also finds no handler;
3. that is a **triple fault**, and the CPU's response to a triple fault is to reset.

No message, no clue. Three tables fix it.

## The GDT: almost nothing, but required

In 64-bit mode, segmentation is nearly gone: the base and limit of a segment are ignored. But two things still depend on the Global Descriptor Table:

- **Privilege.** The code segment register says whether the CPU is running kernel code (ring 0) or user code (ring 3).
- **The TSS.**

The kernel's GDT has kernel code and data segments, user code and data segments, and the TSS. The *order* of the user segments is not free: the `syscall`/`sysret` instructions, which arrive in entry #8, compute their selectors by fixed offsets from one register, so the segments must be laid out the way those instructions expect. I ordered them correctly now so nothing would need to move later.

## The TSS: a table of stacks

In 64-bit mode the Task State Segment is no longer about tasks. It is a table of stack pointers the CPU switches to when an interrupt arrives. The important part for this step is the **Interrupt Stack Table**: an IDT entry can say "run this handler on stack number N".

Why that matters: suppose the kernel overflows its stack. The CPU tries to push the exception frame — onto the broken stack. That faults again: a double fault. If the double fault handler also runs on the broken stack, it is a triple fault and a reset. So the double fault handler gets **its own stack**, IST 1. A stack overflow then produces a report instead of a reboot.

## The IDT, and making every exception look the same

The IDT has 256 entries. The first 32 are CPU exceptions. There is an annoyance: for some of them (page fault, general protection fault, and a few more) the CPU pushes an **error code** onto the stack, and for the others it does not. A handler written in Rust wants one frame layout, not two.

So each vector gets a tiny assembly stub:

```
CPU          pushes SS, RSP, RFLAGS, CS, RIP (and, for some vectors,
             an error code), then jumps to the IDT entry
isr_stub_N   pushes a 0 if the CPU pushed no error code - so every
             frame has the same shape - then the vector number
isr_common   pushes all 15 general registers, calls Rust
Rust         reads and may change the saved state
isr_common   pops everything back and `iretq`s to the saved RIP
```

After the stubs, one Rust function handles everything:

```rust
extern "C" fn exception_handler(frame: &mut InterruptFrame)
```

`InterruptFrame` is a `#[repr(C)]` struct whose fields are in exactly the order the stubs pushed them. The handler gets a mutable reference, so it can also *change* the saved state — which is how a system call will later return a value.

## What the handler does

- **Breakpoint (`int3`)**: report it and return. Execution continues after the instruction. This is the test that the whole path works in both directions — into the handler and back out with every register intact.
- **Anything else**: print the exception's name, the address (`RIP`) where it happened, the error code, and all the registers — then halt.

Page faults get extra decoding. The CPU puts the address that was accessed in register `CR2`, and the error code's bits say what kind of access failed: was the page present, was it a read or a write, was the CPU in user or kernel mode, was it an instruction fetch. "Page fault" tells you little; *"write to 0xdeadbeef, page not present, in kernel mode"* tells you the bug.

## Testing things that are supposed to crash

The problem with testing a crash handler is that a correct test ends in a crash. The solution: Cargo **features** that compile a deliberate fault into the kernel — one build per fault — and a script that boots each build and checks the report.

At this step there were two: a general fault and a double fault. (Entry #4 adds more: a write to read-only data, executing data, a stack overflow.) I checked each by hand as well: the reported `RIP` mapped back to the exact source line that faulted.

## A bug this step left behind

The stub table (`isr_stub_table`) is defined in assembly and used from Rust. I forgot to mark it `.global`. It linked anyway — because the Rust code and the assembly happened to end up in the same compilation unit. When later changes moved code around, the link failed. The lesson: "it links" is not proof that a symbol is exported correctly. (Fixed in entry #4.)

## What I learned

- **An exception is just a function call the CPU makes for you**, with a fixed stack layout. Once the stubs make that layout uniform, the handler is ordinary Rust.
- **Order matters in places where nothing checks it**: the GDT layout that `syscall` will need, the push order that the frame struct must mirror.
- **A separate stack for the double fault is the difference between a diagnosis and a reboot.**

---

## Testing note

```
make fault64-test
```

This boots one kernel per deliberate fault and checks that each is caught and named. Each boot must end in the report for its own fault; the command fails if any does not.

In a normal boot (`make run64`, terminal log), look for:

| Line | Meaning |
|---|---|
| `[cpu] breakpoint at 0x… - continuing` | An `int3` went into the handler and came back |
| `[cpu] resumed after the breakpoint - exceptions round-trip` | …and execution really continued after it |

If you ever see `*** CPU EXCEPTION` followed by a register dump in the log of a normal boot, that is this handler doing its job — and a bug for me to fix; send me the lines.
