# User Mode: Ring 3 Processes, System Calls, ELF Loading

*Journal entry #8 · the first code the kernel does not trust*

Everything so far ran in ring 0, the CPU's most privileged level, where any instruction is allowed and any memory can be touched. A microkernel's whole idea is that almost nothing should run there. This entry builds the other side: **ring 3**, where programs run in their own address space, can touch only their own memory, and must *ask* the kernel for anything else.

From this point on the kernel has an adversary. Every value that comes from a program — a pointer, a length, a file — is treated as possibly wrong and possibly hostile.

---

## What a process is

A process is three things:

1. **An address space**: its own page tables. The lower half holds the program; the upper half is the kernel, the same in every process.
2. **A thread** (one, for now) that runs in it.
3. Later (entry #9): a table of handles — what it is allowed to reach.

### Per-process page tables

Each process gets its own top-level table. Its upper 256 entries are *copies of the kernel's* — pointing at the same lower-level tables, so the kernel is mapped identically everywhere and a system call needs no address-space switch. Its lower 256 entries are the process's own.

The kernel half is mapped **without the "user" bit**, so ring 3 cannot read or write a byte of it, though it is in the same address space.

Destroying a process walks only the lower half and frees every frame and table the process **owned**. "Owned" is a real distinction — later a process can map memory it shares or merely borrows (entry #11), and that must not be freed with it.

## Loading a program: ELF, from an untrusted file

Programs are ELF64 executables — the format compilers already produce. An ELF file is a header plus a list of **segments**: "these bytes of the file go at this address, this size, with these permissions".

A program file is input from a stranger. `elf.rs` checks **every number before it is used**: offsets stay inside the file, sizes do not overflow, segments stay inside user space and do not overlap or share a page. A bad file is an error, never a kernel crash.

Two rules go beyond the format:

- **W^X**: no segment may be both writable and executable. Memory that can be written and then run is how injected code runs. Refusing it at load time closes that door for every program, forever.
- **Nothing below 64 KiB**: page 0 stays unmapped, so a null pointer — and null plus a small offset — faults.

The parser is pure logic, tested on the host — later (entry #15) with every truncation and every single-bit flip of a good file's header, none of which may panic.

## Getting into ring 3

The CPU has no "enter user mode" instruction. It has `iretq` — *return from interrupt* — which pops an instruction pointer, a code segment, flags, a stack pointer and a stack segment. If the popped code segment says ring 3, the CPU is in ring 3 afterwards.

So `enter_user` builds, by hand, the frame an interrupt *would have* pushed — user SS, user RSP, RFLAGS with interrupts enabled, user CS, the program's entry point — and executes `iretq`. It "returns" to a place the thread has never been.

## Getting back: `syscall`

A program asks the kernel for something with the `syscall` instruction. It is fast because it does very little:

- RIP is saved in RCX, RFLAGS in R11;
- the new CS and SS come from a register called STAR, the new RIP from LSTAR;
- the flags named in FMASK are cleared — interrupts among them.

(STAR is why the GDT's segment order in entry #3 was fixed in advance: `syscall` and `sysret` derive their selectors from it by fixed offsets.)

What `syscall` does **not** do is switch stacks. RSP still holds the *user's* stack pointer — a value chosen by the program, which could point anywhere, including into kernel memory. The kernel must never push to it. So the entry stub's very first act is to swap in the thread's kernel stack; interrupts are off (FMASK) until that is done, so nothing can run on the wrong stack in between.

Interrupts and exceptions from ring 3 have the same need, handled by the CPU: it takes the kernel stack pointer from **TSS.RSP0** — which the kernel updates on every thread switch.

### The ABI: one crate, both sides

The system call number goes in RAX, arguments in RDI, RSI, RDX, R10, R8, R9; the result comes back in RAX — a value, or a negative error code. These numbers and the error codes are defined **once**, in a small crate `abi/` that both the kernel and the user runtime depend on. They cannot disagree, because there is only one definition.

The first calls: `exit`, `debug_write`, `yield`, `sleep`, `process_id`, `ticks`.

## Pointers from user space are never trusted

`debug_write(pointer, length)` asks the kernel to read the program's memory. If the kernel simply dereferenced that pointer, a program could pass a *kernel* address and have the kernel print its own secrets — or pass garbage and crash it.

So the kernel never dereferences a user pointer directly. It **translates** it by walking the *process's own* page tables, page by page, and requires that each page is mapped, **user-accessible**, and (for writes) writable. Then it copies the data through its own direct map. A pointer to kernel memory fails the "user-accessible" test; an unmapped one fails "mapped". The program gets an error code; the kernel is unharmed.

## When a program misbehaves

A fault in ring 3 — a bad pointer, a privileged instruction — arrives at the same exception handler as in entry #3. The difference is one check: *was the CPU in user mode?* If so, the fault is the program's problem, not the kernel's. The process is killed, its memory freed, the fault logged, and everything else carries on.

This is the single most important behaviour of this step: **a broken program cannot take the system down.**

## The runtime, and four test programs

`user/` holds `taaj`, the small library a program links: the entry point, the system call wrappers, `println!`. With it a program is ordinary Rust with a `main`.

| Program | Does | Must end with |
|---|---|---|
| `hello` | Prints, sleeps, makes every call — including with bad pointers (null, kernel code, unmapped), which must be *refused* | exit code 42 |
| `spin` | Loops for a while with no system calls | exit 0 — and it must have been **preempted in ring 3** |
| `peek` | Reads kernel memory | killed: page fault |
| `cli` | Executes `cli` (disable interrupts), a privileged instruction | killed: general protection fault |

The boot self-test runs all of them **twice** and then requires the count of free frames to be exactly what it was before: no process leaks memory, however it ended.

## What I learned

- **The boundary is the design.** Almost all the work here is deciding what crosses between ring 3 and ring 0, and checking it as it crosses.
- **`syscall` leaves you on the user's stack.** The few instructions before the kernel is on its own stack are the most delicate in the kernel.
- **Killing a process cleanly is the feature.** Fault isolation is not an extra; it is what user mode is *for*.

---

## Testing note

```
make run64
```

In the terminal log:

| Line | Meaning |
|---|---|
| `[process] page fault at 0x… in user mode` then `[process] N "peek" killed: page fault` | A program read kernel memory; it was killed, the kernel was not |
| `user read at 0xffffffff80100000: protection violation` | …and this is what it tried |
| `[process] N "cli" killed: general protection fault` | A privileged instruction in ring 3 is refused by the CPU |
| `[process] N "hello" exited with code 42` | A well-behaved program ran and exited |
| `[selftest] user: hello exited 42, spin and heapcheck exited 0, peek, cli, port and memro were killed; 14 processes, nothing leaked` | All of them, twice, and every frame came back |

In the QEMU window:

| Type | You should see |
|---|---|
| `run hello` | `[hello] hello from user mode! …`, lines about refused pointers, then `hello exited with code 42` |
| `run peek` | `peek exited with code -1000` (killed by the kernel) — and the shell is still there |
| `ps` afterwards | No `hello` or `peek` left |
| `mem` before and after | The same "used" figure |

Automated: `make kernel64-test` (ELF validation), `make boot64-test`.
