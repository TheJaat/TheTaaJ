# A New Kernel: 64-bit, in Rust, Booted by the Same Loader

*Journal entry #2 · the first lines of `kernel64`*

Entry #1 ended with a decision: the 32-bit C kernel becomes the reference design, and a new kernel starts — 64-bit, written in Rust. This entry is the first step of that: the smallest kernel that proves the whole chain works, from my bootloader to Rust code printing what the machine looks like.

It does almost nothing. That is the point. Every later step stands on these few hundred lines, so they should be boring and certain.

---

## What "booting a kernel" has to mean

The bootloader (my own, from the 32-bit days, extended to switch the CPU into 64-bit long mode) loads the kernel file from the ISO and jumps to it. At that instant:

- the CPU is in 64-bit mode, with the loader's temporary page tables;
- register `RDI` holds the address of one structure, `TaajBootInfo` — the memory map, the framebuffer, the ACPI pointer, where things were loaded;
- the stack pointer points into the loader's own scratch memory.

The kernel's job in this step: take a stack of its own, get a way to print, check that structure, and show what is in it.

## Where the kernel lives

The kernel is linked to run at `0xFFFFFFFF80100000` — the top 2 GiB of the address space — while being *loaded* at physical 1 MiB. This is the "higher half" layout:

- **The lower half of the address space is left completely free** for user programs, later. A kernel linked low would have to be moved the day the first process arrives.
- The top 2 GiB specifically lets the compiler use short, fast addressing for kernel code (the `kernel` code model).

The loader maps the kernel there before jumping. `linker.ld` describes the layout; the kernel's first section is its entry point.

## The first instruction cannot be Rust

Rust code assumes a valid stack from its very first instruction — the compiler emits pushes and calls freely. So the entry point is a few lines of assembly whose only job is to make that assumption true:

```rust
global_asm!(
    ".section .text.entry, \"ax\"",
    ".global _start",
    "_start:",
    // Stacks grow down, so RSP starts at the END of the array.
    // RIP-relative: correct wherever the kernel is mapped.
    "    lea rsp, [rip + {stack} + {size}]",
    // A zero frame pointer marks the bottom of the call chain.
    "    xor ebp, ebp",
    // RDI still holds the boot info address - the first argument.
    "    call {main}",
    "2:  cli",
    "    hlt",
    "    jmp 2b",
    ...
);
```

The stack is a 64 KiB array in `.bss` — which costs nothing in the kernel file, since `.bss` is just "this much zeroed memory". `RDI` is not touched, so by the calling convention it arrives in `kernel_main` as its first argument.

## `no_std`: Rust without an operating system

Ordinary Rust programs link the standard library, which assumes an OS underneath: files, threads, a heap. A kernel *is* the OS, so it is built with `#![no_std]` and `#![no_main]`: only `core` — the part of Rust that needs nothing but a CPU. No `Vec`, no `String`, no `println!` yet. The target is `x86_64-unknown-none`: "x86_64, no operating system".

Two things have to be supplied by hand: the entry point above, and a **panic handler** — what to do when Rust code hits an impossible state. Mine prints the message and location and halts.

## A way to speak: the serial port

Before the kernel can draw, it can talk over COM1, the first serial port: a 16550 UART at I/O port `0x3F8`. It is the simplest output device there is — wait until the "transmit empty" bit is set, write a byte — and QEMU, VMware and VirtualBox can all connect it to a terminal or a file. Every test in this project reads the kernel's serial output.

On top of it sits `kprintln!`, a macro that formats like `println!`. Rust's formatting machinery only needs one function — `write_str` — implemented for a type; the rest (`{}`, `{:#x}`, padding) comes free with the `fmt::Write` trait.

## The contract with the bootloader, checked by the compiler

`TaajBootInfo` is defined in C, in the bootloader's header. The kernel mirrors it in Rust with `#[repr(C)]`, which makes Rust lay the fields out exactly as C would. The risk with two definitions of one structure is that they drift apart — and then the kernel reads the wrong field and nothing complains.

So the Rust side ends with **compile-time checks**: assertions on the size of the structure and the offset of its fields. If someone changes one side and not the other, the kernel does not build. A wrong layout becomes a build error instead of a mystery at boot.

The kernel also validates what it was given at run time — a magic number and a version — before trusting any of it.

## Architecture-neutral from the first day

Everything that knows it is on x86_64 — the entry stub, port I/O, the serial driver — lives under `arch/x86_64/`. The rest of the kernel calls `arch::console_write` and does not know what is behind it. Entry #1 committed to AArch64 and RISC-V later; the cheapest time to draw that line is before there is any code on the wrong side of it.

## What it does when it boots

It takes its stack, starts the serial console, validates the boot info, prints the memory map, the ACPI pointer and the framebuffer's description — and paints the screen, to prove the framebuffer details (size, pitch, where each colour channel sits in a pixel) are right, not merely present.

## Tests from the first commit

`make boot64-test` boots the ISO headless in QEMU with the serial port going to a file, and checks that file for the lines a good boot prints. It is a crude test. It is also the reason every later step could be changed without fear: from the first commit, "does it still boot?" was one command.

## What I learned

- **The interesting bugs at this stage are not in code, they are in agreements**: where the kernel is linked versus where it is loaded, which register holds the boot info, the layout of one structure. Making those agreements checkable — by the linker script, by `repr(C)` plus compile-time assertions — is most of the work.
- **`unsafe` is smaller than I expected.** Port I/O and the entry stub need it. Parsing the boot info, formatting, the logic — none of it does.

---

## Testing note

```
make boot64-test
```

It should end with `boot64-test passed`. To watch it yourself:

```
make run64
```

In the terminal (the serial log), near the top, you should see:

| Line | Meaning |
|---|---|
| `Hello from TheTaaJ!` | The kernel reached Rust code and the serial console works |
| `loader         TheTaaJBoot (BIOS)` | The boot info was valid, and says how the machine was booted |
| `memory map     7 regions:` and ranges marked `usable` or `reserved` | The loader's memory map arrived intact |
| `framebuffer    1024x768, 32 bpp, …` | The screen description arrived |

Today the kernel goes on to do much more after these lines; this entry's work is everything up to them.
