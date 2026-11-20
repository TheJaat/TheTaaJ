# Waking the Other CPUs

*Journal entry #44 · the machine has had four CPUs all along and the kernel used one. Now it starts the others — each begins life as an 8086 and has to be walked up to 64-bit mode — has all of them count at the same time, and shows on every boot what a lock is for.*

```
[acpi] 4 CPU(s), APIC ids [0, 1, 2, 3]
...
[smp] CPU 1 awake: APIC id 1, stack below 0xffffd80000010000
[smp] CPU 2 awake: APIC id 2, stack below 0xffffd80000030000
[smp] CPU 3 awake: APIC id 3, stack below 0xffffd80000050000
[smp] 4 CPUs awake: CPU 0 runs everything; CPU 1, 2, 3 started through the page at 0x1000
[smp] counting together: 4 CPUs reached 400000 of 400000 with a lock; without one 398946, 1054 lost
```

Since entry 6 the kernel has read the firmware's list of CPUs and printed it with the words "this one runs; the rest wait for SMP". SMP — symmetric multiprocessing, several equal CPUs running one kernel — is step 5 of the roadmap, and it is too big for one entry. This is the first part: **the other CPUs wake up and run kernel code.** They do not run threads yet. After proving they work, they halt.

That sounds like little. It is the part with the most machinery per line, because of where a CPU starts.

---

## Where a CPU starts

When a PC is switched on, the firmware picks one CPU to run — the *bootstrap processor*, here "the boot CPU" — and leaves the rest powered but waiting. They are not running slowly or running an idle loop. They execute nothing, and wait for a message.

The message comes through the **local APIC**: the interrupt controller inside each CPU, which the kernel has used since entry 6 for its timer. Besides receiving interrupts, a local APIC can *send* them to another CPU's local APIC. These are called inter-processor interrupts (IPIs), and two kinds are special:

| Message | Meaning |
|---|---|
| **INIT** | "Reset yourself and wait." |
| **STARTUP** (SIPI) | "Start executing at this page." |

The trouble is in the second. A CPU that receives STARTUP begins exactly as the first CPU began when the machine was switched on: in **16-bit real mode**, as an Intel 8086 from 1978 would. It can reach 1 MiB of memory. It has no page tables and no stack. And the message has eight bits of room for an address: a page *number*, so the CPU starts at `page × 4096`, which must be below 1 MiB.

The boot CPU began there too, and a bootloader carried it to 64-bit mode. The other CPUs have no bootloader. The kernel is already running, in 64-bit mode, at an address near the top of memory that a real-mode CPU cannot even express.

So the kernel carries a tiny bootloader for them. It is called the **trampoline**: 264 bytes that take a CPU from real mode to the kernel's 64-bit Rust.

## The whole sequence

```
boot CPU                                 a CPU being started
--------                                 -------------------
copy the trampoline to a page
below 1 MiB; write into it the
page tables, stack and function
the new CPU is to use
send INIT; wait 10 ms                    resets, waits
send STARTUP(page), twice                starts at page:0000 in real mode
wait for it to say "awake"               trampoline: real mode ->
                                           protected mode -> long mode
                                         calls ap_main, in the kernel,
                                           on its own stack
                                         loads its GDT, TSS, the IDT
                                         says "awake"
next CPU
```

One CPU at a time, because the trampoline page holds *one* stack pointer. Two CPUs started together would take the same stack.

The sequence of messages is Intel's, from the manual (volume 3, "MP initialization protocol"): INIT, wait 10 ms for the reset to finish, then STARTUP twice, 200 µs apart. Twice because a CPU may miss the first. One that did not miss it is already running, and a running CPU ignores STARTUP.

(The firmware calls the other CPUs "application processors", AP. That is why the function is `ap_main`.)

## A page below 1 MiB

The kernel keeps the first megabyte out of its frame allocator entirely (entry 4): it holds the firmware's tables and whatever the bootloader left. Now I need one page of it.

`mm::init` chooses it, because that is where it is known what is down there that the kernel still reads: the boot info and the memory map. Everything else of the loader's is finished with. So: the lowest page that the firmware's map calls usable, that is not page 0, and does not overlap those two. On QEMU with either firmware this is `0x1000`.

If there is no such page the kernel says so and stays with one CPU. It does not panic: one CPU is a working system.

## The trampoline: code that does not know where it is

The trampoline is assembled into the kernel, in `.rodata`, linked like everything else at `0xFFFFFFFF80…`. But it never runs there. It is *data* there. It is copied to the low page and runs from the copy.

So it cannot use its own labels as addresses: the linker's addresses are wrong for the copy. I first meant to have the boot CPU patch the right addresses into the copy. There is a better way. Real mode itself tells the code where it is: a CPU started at page `P` begins with `CS = P / 16`. So `CS × 16` is the address of the trampoline's first byte.

```asm
    xorl    %ebx, %ebx
    movw    %cs, %bx
    shll    $4, %ebx                # EBX = where this page is
```

EBX keeps that value from the third instruction to the last, through all three modes. Every label is then "EBX + how far the label is from the start", and that *distance* the assembler does know:

```asm
    movl    (ap_trampoline_cr3 - ap_trampoline_start)(%ebx), %eax
```

The trampoline would work copied to any page below 1 MiB, with nothing patched but the real parameters.

(This is AT&T syntax — source first, destination second, `%` before registers — the only assembly in the kernel that is. LLVM's assembler handles 16-bit code most reliably in it.)

## Three modes, one bit and one jump each

| Mode | What the trampoline does to leave it |
|---|---|
| **Real mode**, 16-bit | Loads a small GDT of its own. Sets `CR0.PE` (protection enable). Far-jumps to a 32-bit code segment. |
| **Protected mode**, 32-bit, no paging | Sets `CR4.PAE`. Loads `CR3` with page tables. Writes `EFER` (long mode enable, and no-execute). Sets `CR0.PG` (paging). Far-jumps to a 64-bit code segment. |
| **Long mode**, 64-bit | Loads the stack pointer, puts the argument in RDI, calls `ap_main`. |

Why a *jump* each time? Setting the bit changes what the CPU is capable of. But how instructions are decoded — 16, 32 or 64 bits — comes from the code segment in CS, and the only way to load CS is a far jump (or a far return). So each step is: flip the bit, then jump to a segment of the new kind.

Why can it not go straight from 16-bit to 64-bit? Because 64-bit mode exists only with paging on, and paging can only be turned on from protected mode.

Two details cost thought:

**The moment after paging goes on.** The instruction after `mov %eax, %cr0` is fetched *through the page tables*, at the same address as before: a low one, `0x10xx`. The kernel's page tables map nothing low; the lower half belongs to programs. So the CPU needs tables that map the trampoline page at its own address *and* the kernel. I make them the way a process's are made (`PageTables::new_user`: the kernel's half shared, the lower half empty) and add one page. The new CPU uses them for a few dozen instructions, `ap_main` switches to the kernel's own, and when all CPUs are up the boot CPU frees them.

**EFER before paging.** The kernel's tables use the no-execute bit. A CPU that has not been told about that bit (`EFER.NXE`) treats it as reserved, and a reserved bit in a page table is a fault — on a CPU with no IDT, a reset. So the trampoline is given the boot CPU's own `EFER` (less the one bit the CPU sets itself, "long mode active") and writes it before paging goes on.

## The first Rust on a new CPU

```rust
extern "C" fn ap_main(start: &'static Start) -> ! {
    unsafe {
        mmu::set_control(start.control);      // CR0 and CR4 as on the boot CPU
        mmu::activate(mm::kernel_root());     // off the borrowed page tables
        gdt::load(start.tables, start.double_fault_stack);
        idt::load();                          // from here a fault is a message
        syscall::init();
    }
    apic::enable_local(platform::SPURIOUS_VECTOR);
    kprintln!("[smp] CPU {} awake: ...");
    start.awake.store(true, Ordering::Release);
    count();
    super::halt_forever()
}
```

`Start` is everything the new CPU is handed, made by the boot CPU on the heap and never freed. The boot CPU allocates it all in advance — stacks, tables — so that the new CPU's first steps need nothing but what it is given. Until `idt::load()` any fault on it resets the machine, so the less it does before that the better.

`&'static Start`: a reference that lives for the rest of the program. `Box::leak` makes one: it gives up ownership of a heap allocation and returns a reference that can never dangle, because nothing will ever free it. The right tool for something that lasts as long as the machine is on.

What is per CPU and what is shared:

| | Per CPU | Why |
|---|---|---|
| Stack (60 KiB, guard page below) | yes | Two CPUs on one stack overwrite each other's calls. |
| Double-fault stack | yes | Same. |
| TSS | yes | It holds the stacks this CPU switches to. |
| GDT | yes | It is where a CPU finds its TSS: one slot, one address. |
| IDT | **shared** | An exception means the same thing on every CPU. But each CPU has its own *register* pointing at it, hence `idt::load()`. |
| Page tables | **shared** | One kernel. Each CPU has its own CR3 register. |
| Local APIC | yes, in hardware | All at the same address: a CPU reading it sees its own. That is how `apic::local_id()` answers "who am I" on any CPU. |

`gdt.rs` used to have three statics (`GDT`, `TSS_SEGMENT`, `LOADED`) and comments saying "once, on the boot CPU". They are now one struct, `CpuTables`. The boot CPU's is still a static, because it is needed before there is a heap; the others are boxed.

## Counting together

A started CPU prints a line. That proves it runs Rust and that the console's lock works between CPUs. It does not prove they run *at the same time*, or that locks hold when they do.

So every CPU, the boot CPU included, waits for a flag and then adds one, 100 000 times, to two numbers:

```rust
for _ in 0..ROUNDS {
    *LOCKED.lock() += 1;

    let seen = UNLOCKED.load(Ordering::Relaxed);
    UNLOCKED.store(seen + 1, Ordering::Relaxed);
}
```

`LOCKED` is behind the spin lock the kernel has had since entry 4. `UNLOCKED` is not behind anything; it is read, then written, as two separate steps. Between the read and the write another CPU may do both — and its addition is then written over.

```
CPU 1: reads 41
CPU 2: reads 41
CPU 1: writes 42
CPU 2: writes 42        <- two additions, the number went up by one
```

With four CPUs the locked number must be exactly 400 000. That is the test. The unlocked one is printed beside it, and it is short every time, by a different amount:

| CPUs | With a lock | Without | Lost |
|---|---|---|---|
| 2 | 200 000 | 160 239 | 39 761 |
| 4 (BIOS) | 400 000 | 398 946 | 1 054 |
| 4 (UEFI) | 400 000 | 396 340 | 3 660 |
| 16 | 1 600 000 | 1 586 360 | 13 640 |

(One run each. The numbers change on every boot.)

Since entry 6 the comment on the kernel's lock has said "the spin part still protects against other CPUs, once there are any". That was written on faith. This line is the first time the kernel has had two CPUs in the same code at once, and it puts the reason for every lock in the kernel into the boot log.

I do not understand why two CPUs lose so many more than four or sixteen. My guess is that with two, each spends less time waiting for the lock and so more of its time between the unlocked read and write; but I have not measured it and QEMU's emulation of CPUs is not real hardware's timing.

## What the kernel decides without hardware

`kernel64::smp` (in the library half, so tested on the host) has what needs no APIC:

- **`plan`**: which CPUs to use, from the firmware's list. The boot CPU is number 0 whether or not the firmware lists it. CPUs marked not enabled are skipped. A CPU listed twice is started once — starting a CPU that is already running would reset it under the kernel's feet. At most `MAX_CPUS` (16); the rest are counted and left asleep.
- **`init_message`, `startup_message(page)`, `destination(id)`**: the bit patterns. `startup_message` returns `None` for an address a CPU cannot start at (not a whole page, not below 1 MiB, or page 0).

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What started CPUs do | Prove they work, then halt with interrupts off | *Run threads at once.* The scheduler, the system call entry and the TSS code all assume one CPU. Each of those is its own step. A CPU that is awake and idle harms nothing. |
| How the trampoline finds itself | From CS, at run time | *Patch absolute addresses in when copying.* Works, but the copying code must then know the trampoline's insides. |
| Page tables for the climb | A throwaway address space, like a process's | *Map the low page in the kernel's own tables.* The lower half is the programs'; and the three tables it would take would stay forever. |
| Which low page | Chosen from the memory map at boot | *A fixed address like 0x8000.* The usual choice, and a guess about what the bootloader left there. |
| Started | One at a time | *All at once with a STARTUP broadcast.* Each would need to find its own stack before it has one; possible (an atomic counter in the trampoline) but more assembly for 30 ms saved. |
| Waiting 10 ms | A loop on the time-stamp counter | *The timer and `sleep`.* There are no threads yet when the CPUs are started, on purpose: nothing else is running to be disturbed. |
| A CPU that does not answer | Left out, with a message; the rest still start | *Panic.* Fewer CPUs is a slower system, not a broken one. |
| Tests | Every boot test now runs with 4 CPUs | *One special SMP boot.* From now on one CPU is the special case. |

## Bugs, and one thing I believed that was false

It booted the first time I ran it: three CPUs said they were awake. I did not expect that, and I think the reason is that I disassembled the trampoline in each of its three modes before running it, and compared the offsets by hand with the labels. A mistake in a trampoline has no error message. The machine resets.

What I got wrong was a belief. I wrote in `gdt.rs` that two CPUs *cannot* share a GDT and TSS, because the CPU marks a TSS "busy" when it loads it and refuses a busy one. To see the refusal I gave CPUs 1, 2 and 3 the same tables on purpose. All three started and counted to 400 000.

The CPU does refuse a busy TSS. But my `load` writes the TSS's descriptor into the GDT afresh every time it runs, and a fresh descriptor is not busy. So nothing stops the sharing. The damage would come later: the first time two of those CPUs took an interrupt at the same moment, onto the same stack. The comment now says that, and says how I know.

It is the kind of bug this stage is full of: nothing fails when the mistake is made.

## How I checked it

**Automated** (`make test64`):

- 9 host tests of `kernel64::smp`: the plan (order, a boot CPU that is not id 0 or not listed, disabled CPUs, duplicates, the limit, no holes in the numbering) and the messages, compared with the values in Intel's manual.
- The two full boot tests (BIOS and UEFI) now run with 4 CPUs. `check-boot.sh` requires a line from each of CPUs 1–3, `4 CPUs awake`, and `reached 400000 of 400000 with a lock`. Everything those tests did before — a couple of hundred shell commands, files, the C programs — runs with three more CPUs awake.
- A new target, `smp64-test`, boots with 1 CPU (`1 CPU: no others to start`), 2 CPUs, and 20 (16 used, `4 more CPU(s) than the 16 this kernel uses: left asleep`), and runs the same checks on each.
- The seven fault boots also run with 4 CPUs.

**By hand, once each:**

- Made CPU 2 read address `0x10` in `ap_main`. The log had `CPU EXCEPTION 14: page fault`, with `rsp` inside CPU 2's own stack; then `CPU 2 (APIC id 2) did not answer: left out`, CPU 3 started, and three CPUs counted to 300 000. So a fault on a new CPU is reported, and the "did not answer" path works.
- The shared-tables experiment above.
- Disassembled the trampoline with `objdump` as 16-, 32- and 64-bit code.

**Not tested:**

- **Real hardware.** Everything here ran in QEMU, without KVM (this machine has none), so the CPUs are emulated by host threads. Real CPUs differ most in timing: the 10 ms and 200 µs waits are the manual's, not measured.
- A machine with **no free page below 1 MiB**, or with page tables above 4 GiB: the messages for those exist and have never been printed.
- A CPU that is merely **slow** to answer (more than half a second). It would be reported as silent and then wake up anyway. The page tables are deliberately not freed in that case, but nothing would count it.
- **APIC ids above 255** (machines with x2APIC and very many CPUs). The firmware table entry for them is not parsed; those CPUs are not seen at all.

## What is still missing

SMP is not done; the CPUs are only awake. In order:

1. **Per-CPU data.** The system call entry keeps the kernel stack in one global (`KERNEL_RSP`), with a comment from entry 8: "One CPU, so one global; with SMP this becomes per-CPU data reached through GS". `set_kernel_stack` and the I/O bitmap write to the boot CPU's TSS. Each CPU needs a block of its own and a way to find it.
2. **The scheduler on every CPU.** One "current thread" per CPU, an idle thread per CPU, a timer per CPU, run queues. And the hard part: every place that says "interrupts are off, so nothing else can be running" is now wrong. Entry 17 closed the kill and sleep races with exactly that argument.
3. **CPUs talking to each other.** When one CPU unmaps a page, the others still have the old translation cached (a "TLB shootdown" is how they are told). When the kernel panics, the others must stop — today a fault on one CPU halts that CPU and the rest carry on, as the experiment above showed.
4. **The milestone**: a C program, compiled on the host, running with all four.

Also: the kernel never uses the started CPUs for anything after the count, so today `make run64` with four CPUs is exactly as fast as with one.

## What I learned

- **Every CPU starts as an 8086.** Not only the first, and not only at power-on. The history of the PC is replayed, in 264 bytes, each time a CPU starts.
- **A mode switch is a bit and a jump.** The bit changes what the CPU can do; the far jump loads CS, and CS is what says how instructions are read.
- **Code that will be copied must not know its address.** The trick is to find one true address at run time (here from CS) and express everything else as a distance from it. Distances survive copying.
- **"Per CPU" has three meanings.** A register each CPU has (CR3, the IDT pointer, the APIC at one shared address). A table each CPU needs a separate copy of (GDT, TSS, stacks). And a table they share (the IDT, the page tables). Sorting things into those three is most of the design.
- **`Box::leak`** turns "allocated once and never freed" from a leak into a type: `&'static T`.
- **`Ordering::Release` and `Acquire`.** The new CPU stores `awake = true` with `Release`; the boot CPU loads it with `Acquire`. Together they promise: whoever sees the `true` also sees everything the other CPU wrote before it. On one CPU I had used `Relaxed` everywhere and it could not matter.
- **A race is a number.** I have read the explanation of lost updates many times. `1054 lost` in my own boot log is different.
- **An experiment that does not fail is a result.** I expected a fault from the shared TSS and got none, and learned more than the fault would have taught.

## Testing note

```
make run64
```

Nothing new to type: what changed is in the kernel's log, in the terminal `make run64` was started from. Scroll up to just after the `[selftest] timer` line.

**In the log**

| Look for | You should see |
|---|---|
| `[acpi] 4 CPU(s)` | `[acpi] 4 CPU(s), APIC ids [0, 1, 2, 3]` |
| `[smp] CPU 1 awake` | Three lines, for CPU 1, 2 and 3, in that order, each with its APIC id and a different stack address |
| `[smp] 4 CPUs awake` | `[smp] 4 CPUs awake: CPU 0 runs everything; CPU 1, 2, 3 started through the page at 0x1000` |
| `[smp] counting together` | `4 CPUs reached 400000 of 400000 with a lock; without one NNNNNN, NNNN lost`. The first two numbers must be 400000. The last two differ on every boot; "0 lost" is possible but I have not seen it |

**In the window**

| Type | You should see |
|---|---|
| `primes`, or any command you like | Everything works as before: the system still runs on CPU 0 alone |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | `[smp] 1 CPU: no others to start`, and a working system |
| `make run64 RUN_CPUS=2` | One `awake` line, `2 CPUs awake`, `reached 200000 of 200000 with a lock` |
| `make run64 RUN_CPUS=20` | `[smp] 4 more CPU(s) than the 16 this kernel uses: left asleep`, fifteen `awake` lines, `reached 1600000 of 1600000` *(slow to start in QEMU: twenty emulated CPUs on a few real ones)* |
| `make run64-uefi` | The same four lines as `make run64` |

**On the host**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu smp
```

should end `9 passed`.

```
make smp64-test
```

boots with 1, 2 and 20 CPUs and should end `smp64-test passed`.

Automated: `make test64`
