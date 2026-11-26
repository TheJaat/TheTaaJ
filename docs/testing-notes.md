# Testing notes, collected

The testing note from the end of each journal entry, from entry 044 on, one after another, so that a run of steps can be tested in one sitting. Each is copied from its entry word for word; the entry has the story. For entries before 044, see the entry itself.

Build once and everything below runs from the root of TheTaaJ. `make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from. `make test64` runs every automated test and should end `test64: all passed`.

---

## 044 · Waking the Other CPUs

[The entry](journal/044-waking-the-other-cpus.md)

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

---

## 045 · Each CPU Finds Itself: GS and `swapgs`

[The entry](journal/045-each-cpu-finds-itself.md)

```
make run64
```

Nothing new to type. In the kernel's log (the terminal `make run64` was started from):

| Look for | You should see |
|---|---|
| `[smp] CPU 1 awake` | The three `awake` lines as in entry 44, numbered 1, 2, 3 — each number now read by that CPU through its own GS |
| `[gs] system calls` | Twice: `[gs] system calls, timer interrupts, then a read through GS...`, each followed a few lines later by `[process] page fault at 0x… in user mode`, `user read at 0x5a5: page not present`, and `[process] N "gs" killed: page fault` |
| `[gs] FAILED` | Nothing. This line must not be there |
| `[selftest] user:` | `hello exited 42, spin and heapcheck exited 0, peek, cli, port, memro and gs were killed; 16 processes, nothing leaked` |

**In the window**

| Type | You should see |
|---|---|
| `primes`, `ps`, `cask` and a line, anything | Everything as before. Every key pressed is an interrupt and every command dozens of system calls, all through `swapgs` |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | The same `[gs]` lines and a working system: GS is used with one CPU too |

Automated: `make test64`

---

## 046 · Threads on Every CPU

[The entry](journal/046-threads-on-every-cpu.md)

```
make run64
```

**In the log** (the terminal `make run64` was started from)

| Look for | You should see |
|---|---|
| `[selftest] threads: two yielding` | `ran as "ababab"` — still, because this runs before the other CPUs join |
| `[smp] 4 CPUs run threads` | `[smp] 4 CPUs run threads: each has its own timer and idle thread, and all take from one queue` |
| `[selftest] smp:` | `threads ran on all 4 CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked` |
| `FAILED` | Nothing |

Start it five or six times. Every start should get to the prompt: the bug of this entry showed as one start in three stopping after the `[smp] 4 CPUs run threads` line.

**In the window** — the system is now on four CPUs; it should behave exactly as before

| Type | You should see |
|---|---|
| `primes` | The primes, as before |
| `ps` | The process list. Run it a few times |
| `forever &` four times, then `jobs` | Four jobs running. With one CPU the shell would now feel slow; with four it should not: type `echo still quick` and see it at once |
| `kill 1`, `kill 2`, `kill 3`, `kill 4` | Each `forever` stopped *(spinning programs being killed from another CPU; the automated test kills one at a time)* |
| `cat welcome \| wc` if you have a file, or `ls \| wc` | A count: a pipeline's two programs may now run on two CPUs |
| `cask`, a name, numbers, Ctrl+D | As in entry 43 |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | `[selftest] smp: 1 CPU; 40000 hand-offs between two threads, none lost; nothing leaked`, and a working system |
| `make run64 RUN_CPUS=2` | `[smp] 2 CPUs run threads…` and `threads ran on all 2 CPUs at once` |

**On the host**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched
```

should end `20 passed`.

Automated: `make test64`

---

## 047 · One CPU Taps Another on the Shoulder: the Reschedule Interrupt

[The entry](journal/047-one-cpu-taps-another.md)

```
make run64
```

**In the log** (the terminal `make run64` was started from)

| Look for | You should see |
|---|---|
| `[selftest] wake-ups:` | `a thread woken by a busy CPU ran on an idle one NNN us later (median of 40; slowest NNNN us; 40 interrupts between CPUs)`. The first number should be in the hundreds, sometimes a little over a thousand; 10 000 would be a whole tick. The last should be 40 or 41 |
| `FAILED` | Nothing |

**In the window** — nothing new to type; the system should feel the same or a little quicker

| Type | You should see |
|---|---|
| `ls \| wc` | A count, at once: the two programs of a pipeline hand every line to each other, and each hand-over used to be able to wait for a tick |
| `forever &` then `echo still quick` | The echo at once, with one CPU spinning |
| `kill 1` | The `forever` stopped |
| `primes`, `ps`, `cask` | As before |

**Other numbers of CPUs**

| Run | You should see |
|---|---|
| `make run64 RUN_CPUS=1` | `[selftest] wake-ups: 1 CPU, so nobody to tell`, and a working system |
| `make run64 RUN_CPUS=2` | The `wake-ups` line as with four |

**On the host**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched
```

should end `27 passed`.

Automated: `make test64`

---

## 048 · The Kernel Outgrows Its Megabyte

[The entry](journal/048-the-kernel-outgrows-its-megabyte.md)

```
make big64-test
```

It builds a second kernel (a minute the first time) and boots it twice. You should see, at the end:

| Look for | You should see |
|---|---|
| The last line | `big64-test passed` |

and in the two logs:

```
grep -a "big image\|ramdisk  " build/big64-bios.log build/big64-uefi.log
```

| Look for | You should see |
|---|---|
| `build/big64-bios.log` … `ramdisk` | `ramdisk        0x2000 bytes at 0x300000` — at 3 MiB, above the kernel |
| `build/big64-bios.log` … `big image` | `[test] big image: the kernel ends at 0x2c…, above 2 MiB; 768 KiB of it at 0x…-0x… is byte for byte what was built` |
| `build/big64-uefi.log` … `big image` | The same line (and `ramdisk        none`: the UEFI loader carries no ramdisk) |
| `FAILED` | Nothing |

**The ordinary system** — it is loaded by the changed loader too

```
make run64
```

| Type | You should see |
|---|---|
| nothing | The system starts as before, to the shell's prompt |
| `ls`, `primes`, `ps` | As before |

**To see the wall itself** (optional): in `kernel/kernel64/linker.ld` change `KERNEL_LOAD_LIMIT = 0x300000;` to `0x100000;` and run `make kernel64`. The build should stop with `the kernel image ends above 3 MiB, where the bootloader refuses to load it`. Change it back.

Automated: `make test64`

---

## 049 · The Milestone: A C Program on Four CPUs

[The entry](journal/049-the-milestone.md)

**1. Compile the C program on your machine and put it on the disk**

```
make cput C=cprimes
```

should end `cprimes is on build/taaj-disk.img: make run64, and type cprimes`.

**2. Run it, on four CPUs**

```
make run64
```

| Type | You should see |
|---|---|
| `cprimes 100` | `cprimes: 25 primes below 100` |
| `cprimes 12x` | `cprimes: not a limit between 0 and 1000000000: 12x` |
| `cprimes` | `usage: cprimes LIMIT` |
| `time cprimes 4000000` | `cprimes: 283146 primes below 4000000`, then `took 1.… s; the 4 CPUs worked for 1.… s in all` (mine: 1.05 and 1.08) — the two numbers about the same: one program, one CPU's work |
| `uptime` | `up … s (… ticks), on 4 CPUs` |
| `cprimes 60000000 &` and Enter, four times, quickly — each takes about a minute | `[1] cprimes 60000000` … `[4] cprimes 60000000` |
| `jobs` | Four jobs, running |
| `time wait` | Four lines `cprimes: 3562115 primes below 60000000`, then `took T s; the 4 CPUs worked for W s in all` with **W three to four times T** |

**3. The same on one CPU**

```
make run64 RUN_CPUS=1
```

| Type | You should see |
|---|---|
| `uptime` | `… on one CPU` |
| `cprimes 24000000 &` four times, then `time wait` | The same four answers (`1507122 primes below 24000000`), and `took T s; the one CPU worked for W s in all` with **W equal to T**, or a hair under. Typing will feel slow while they run - each key waits its turn behind four busy programs - so `time wait` may be read late and T may be much less than the minute the four really take. `uptime` before and after shows the real time. The ratio is the result. |

**4. Compare with your own machine** (optional)

```
build/c/host-cprimes 24000000
```

prints the line TheTaaJ printed. It is the same `libc/programs/cprimes.c`, compiled by `cc` against Linux's C library.

Automated: `make test64`

---

## 050 · First Words on AArch64

[The entry](journal/050-first-words-on-aarch64.md)

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

---

## 051 · The Device Tree: A Machine Describes Itself

[The entry](journal/051-the-device-tree.md)

**1. The ordinary machine**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `Hello from TheTaaJ!` and the `[cpu]` lines, as in entry 50 | the serial port now comes from the tree, and still works |
| `[tree] device tree at 0x48000000: 1048576 bytes, version 17, 56 nodes, the deepest 5 down` | the whole tree was walked |
| `[tree] machine     "linux,dummy-virt"` | QEMU's name for its machine |
| `[tree] memory      0x40000000 - 0x60000000 (512 MiB)` | |
| `[tree] cpus        1, with ids [0]` | |
| `[tree] serial      PL011 at 0x9000000, named as the console` | found through `/chosen` |
| `[tree]             its clock runs at 24000000 Hz` | found through a phandle |
| `[tree] interrupts  GIC version 2: shared part at 0x8000000, per-CPU part at 0x8010000` | |
| `[tree] timer       interrupts 29 (secure), 30 (physical), 27 (virtual), 26 (hypervisor)` | |
| `[tree] firmware    called with hvc` | |

**2. A different machine, the same kernel**

```
make run-aarch64 RUN_A64="-M virt,gic-version=3,virtualization=on -smp 4 -m 6G"
```

| Look for | Meaning |
|---|---|
| `…the loader entered at level 2` | |
| `[tree] memory      0x40000000 - 0x1c0000000 (6144 MiB)` | an end above 4 GiB |
| `[tree] cpus        4, with ids [0, 1, 2, 3]` | |
| `[tree] interrupts  GIC version 3: shared part at 0x8000000, per-CPU part at 0x80a0000` | |
| `[tree] firmware    called with smc` | |

**3. A tree with something missing**

```
make aarch64-test
make run-aarch64 RUN_A64="-dtb build/aarch64/clockless.dtb"
```

The first command runs every automated boot (it should end `aarch64-test: all passed`) and leaves the cut-down trees in `build/aarch64/`.

| Look for | Meaning |
|---|---|
| `[tree]             its clock is not given` | |
| `[tree] no serial port with a clock described: using the one QEMU's virt machine has, as a guess` | the fallback, saying that it is one |

and with `unnamed.dtb` in place of `clockless.dtb`: `[tree] serial      PL011 at 0x9000000, the first one found`.

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu devicetree
```

should end `test result: ok. 22 passed`. It takes a few seconds: one test reads eleven thousand damaged trees.

Automated: `make test64`

---

## 052 · Page Tables in Two Dialects

[The entry](journal/052-page-tables-in-two-dialects.md)

**1. The AArch64 kernel checks its page tables against the CPU**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[paging] the tables the CPU is using, read by the kernel: 5 lookups, all as the CPU does them` | the shared code reads AArch64 tables as the CPU does |
| `[paging] a page at 0x400b4000 mapped twice: written at 0xffffffffffe00000, read at 0xffffffffffe01000, where the CPU refuses a write` | what it writes means what was asked: one page, two addresses, two permissions |
| `[paging] a third address unmapped, then mapped to the page at 0x400b6000: the CPU reads the new page there` | unmapping, and making the CPU forget, work |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

(The two page addresses may differ if the kernel's size changes.)

**2. Two rules broken on purpose**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-rodata
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-nx
```

The first command should end `aarch64-test: all passed`, and leaves the two kernels in `build/aarch64/`.

| Kernel | Look for |
|---|---|
| `Image-fault-rodata` | `[test] writing to read-only data...` then `*** CPU EXCEPTION 0x25: data abort` and `kernel write at 0xffffffffffe01000: protection violation (at level 3)` |
| `Image-fault-nx` | `[test] executing data...` then `*** CPU EXCEPTION 0x21: instruction abort` and `kernel instruction fetch at 0xffffffffffe00000: protection violation (at level 3)` |

Each ends with a register dump and nothing after it.

**3. The PC, whose page tables the same code now writes**

```
make run64
```

| Type | You should see |
|---|---|
| `mem` | the memory summary, as before |
| `ps` | the list of processes - each one an address space made by the changed code |
| `cprimes 100` | `cprimes: 25 primes below 100` |

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu paging
```

should end `test result: ok. 20 passed`.

Automated: `make test64`

---

## 053 · One Memory Manager, Two CPUs

[The entry](journal/053-one-memory-manager-two-cpus.md)

**1. The AArch64 kernel starts its memory manager**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[memory] 130737 frames free of 393216 (510 MiB), bitmap 48 KiB at 0x40000000` | all of the 512 MiB found, less the kernel, the tree and the tables |
| `[memory] own page tables at 0x4000c000: direct map 0xffff800000000000, NX on, write protection on` | the switch happened - and the console survived it |
| `[memory] heap at 0xffffc00000000000, 1024 KiB to start` | `Box`, `Vec` and `String` now work |
| `[tree] read again through the direct map: the same 56 nodes, in memory kept out of the allocator` | the device tree is still readable, where it now is |
| `[selftest] top of memory: the highest free frame, at 0x5ffff000, written and read back` | the last page of memory is really there |
| `[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped` | each part of the kernel has only the permissions it needs |
| `[selftest] memory: all passed` | the same line the PC prints |
| `[paging] the low half is off: address 0 and 0x40080d9c, where this code really is, translate to nothing` | a null pointer now faults |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

(Frame counts and the small addresses may differ by a little if the kernel's size changes.)

**2. More memory than the kernel can reach at first**

```
make run-aarch64 RUN_A64="-m 6G"
```

| Look for | Meaning |
|---|---|
| `[tree] memory      0x40000000 - 0x1c0000000 (6144 MiB)` | six GiB described |
| `[memory] 1572480 frames free of 1835008 (6142 MiB), bitmap 224 KiB at 0x40000000` | all six in the allocator - five arrived in the second go |
| `[selftest] top of memory: the highest free frame, at 0x1bffff000, written and read back` | a page above 4 GiB, reached through the direct map |

**3. Four rules broken on purpose**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-null
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-rodata
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-nx
```

The first command should end `aarch64-test: all passed`, and leaves the kernels in `build/aarch64/`. Leave each QEMU with **Ctrl+A, then X**.

| Kernel | Look for |
|---|---|
| `Image-fault-null` | `[test] reading through a null pointer...` then `*** CPU EXCEPTION 0x25: data abort` and `kernel read at 0x0: not mapped (the level 0 table has no entry)` |
| `Image-fault-rodata` | `[test] writing to read-only data...` then `kernel write at 0xffffffff800911a8: protection violation (at level 3)` |
| `Image-fault-nx` | `[test] executing data...` then `*** CPU EXCEPTION 0x21: instruction abort` and `kernel instruction fetch at 0xffffffff80094000: protection violation (at level 3)` |

The two addresses are now inside the kernel itself (`0xffffffff800…`), not a test page; their last digits may differ.

**4. A machine that keeps memory back**

`make aarch64-test` leaves `build/aarch64/reserved.dtb`, a device tree that reserves the first 64 KiB and the top 16 MiB:

```
make run-aarch64 RUN_A64="-dtb build/aarch64/reserved.dtb"
```

| Look for | Meaning |
|---|---|
| `[tree] reserved    0x40000000 - 0x40010000` and `[tree] reserved    0x5f000000 - 0x60000000` | both reservations read |
| `bitmap 48 KiB at 0x40010000` | the bitmap moved off the reserved 64 KiB |
| `(493 MiB)` in the `frames free` line | sixteen fewer than before |
| `the highest free frame, at 0x5efff000` | just below the reserved top |

**5. The PC, on the same memory manager**

```
make run64
```

In the terminal it was started from, before the prompt:

| Look for | Meaning |
|---|---|
| `[selftest] frames: allocate, free, reuse; the kernel's own are not free` | the new check, on the PC |
| `[selftest] top of memory: the highest free frame, at 0x…, written and read back` | the new check, on the PC |
| `[selftest] memory: all passed` | as before |

| Type | You should see |
|---|---|
| `mem` | the memory summary, as before |
| `ps` | the list of processes |
| `cprimes 100` | `cprimes: 25 primes below 100` |

**6. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu
```

should end `test result: ok. 157 passed`.

Automated: `make test64`

---

## 054 · Interrupts and a Tick on AArch64

[The entry](journal/054-interrupts-and-a-tick-on-aarch64.md)

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

---

## 055 · Threads on AArch64

[The entry](journal/055-threads-on-aarch64.md)

**1. Threads on AArch64**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[selftest] timer: 10 ticks in 99 ms (expected 100)` | the shared timer test, now on this CPU too (98 to about 130 is normal) |
| `[selftest] context switch: there and back with a known value in each of 11 registers, none changed` | the switch keeps every register it must |
| `[selftest] threads: two yielding threads ran as "ababab"` | threads take turns |
| `[selftest] threads: a thread that never yields was preempted` | the tick takes the CPU back |
| `[selftest] threads: sleep(5) took 5 ticks; 200 threads spawned and reaped, nothing leaked` | sleeping works; stacks are recycled (5, 6 or 7 ticks) |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

**2. Stacks that run out**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-stack
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-thread-stack
```

The first command should end `aarch64-test: all passed` and leaves the kernels in `build/aarch64/`. Leave each QEMU with **Ctrl+A, then X**.

| Kernel | Look for |
|---|---|
| `Image-fault-stack` | `[test] overflowing the boot stack...` then `*** KERNEL STACK OVERFLOW: the boot stack ran out` and `kernel write at 0xffffffff800c0f90: the page kept unmapped below the stack` |
| `Image-fault-thread-stack` | `[test] overflowing a thread's stack...` then `*** KERNEL STACK OVERFLOW: a thread's stack ran out` and `kernel write at 0xffffd80000020e40: the page kept unmapped below the stack` |

Each is followed by a register dump and nothing after it. The addresses may differ in their middle digits; the last four are always below `1000` - the lowest page of a slot.

**3. The PC, with the shared tests**

```
make run64
```

In the terminal it was started from, before the prompt:

| Look for | Meaning |
|---|---|
| `[selftest] timer: 10 ticks in … ms (expected 100)` | as before, now timed through `arch::clock()` |
| `[selftest] threads: sleep(5) took … ticks; 200 threads spawned and reaped, nothing leaked` | as before |

| Type | You should see |
|---|---|
| `ps` | the list of processes |
| `uptime` | the time since boot |

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu sched
```

should end `test result: ok.` with no failures - the scheduler's policy, the same code both CPUs now run.

Automated: `make test64`

---

## 056 · Programs on AArch64

[The entry](journal/056-programs-on-aarch64.md)

**1. Programs on AArch64**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[hello] hello from user mode! I am process 1, running at exception level 0` | a program runs, and its system calls work |
| `[hello] the kernel refused all 5 bad pointers` | bad pointers are refused, not followed |
| `[process]   user read at 0xffffffff80100000: protection violation (at level 3)` then `"peek" killed: data abort in a program` | a program cannot read the kernel |
| `[process] … "cli" killed: trapped system register access` | a program cannot mask interrupts |
| `[process]   user write at 0x100000000000: protection violation (at level 3)` then `"memro" killed` | read-only means read-only |
| `[regs] at the start: 29 registers zero, the stack pointer a multiple of 16` | nothing of the kernel's leaks into a new program |
| `[regs] 26 registers and the stack pointer, through system calls and timer interrupts for 5 ticks: none changed` | a program gets its registers back |
| `[selftest] user: hello exited 42, spin, heapcheck and regs exited 0, peek, cli and memro were killed; 14 processes, nothing leaked` | all seven, twice, every frame returned |
| `[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left` | channels and handles between programs |
| `[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left` | the file service and its heap |
| `[selftest] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked` | ending a program from outside |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

The lines of different programs are interleaved - they run at once - and process numbers above 1 can differ.

**2. The same on other machines**

```
make run-aarch64 RUN_A64="-M virt,virtualization=on,gic-version=3"
make run-aarch64 RUN_A64="-cpu cortex-a53 -m 6G"
```

The same lines as in 1.

**3. The PC, with the tests from their new file**

```
make run64
```

In the terminal it was started from, before the prompt:

| Look for | Meaning |
|---|---|
| `[hello] hello from user mode! I am process 1, running in ring 3` | the same program, the other CPU's word for it |
| `[selftest] user: hello exited 42, spin and heapcheck exited 0, peek, cli, port, memro and gs were killed; 16 processes, nothing leaked` | unchanged |
| `[selftest] ipc: …`, `[selftest] files: …`, `[selftest] kill: …` | unchanged |

| Type | You should see |
|---|---|
| `run hello` | `[hello] hello from user mode! …` and `the kernel refused all 5 bad pointers` |
| `ps` | the list of processes |

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu elf
readelf -h user/target/aarch64-unknown-none-softfloat/release/hello | grep Machine
readelf -h user/target/x86_64-unknown-none/release/hello | grep Machine
```

The first should end `test result: ok.`; the other two (from the top directory, after `make user-aarch64 user64`) should say `AArch64` and `Advanced Micro Devices X86-64`.

Automated: `make test64`

---

## 057 · C on AArch64

[The entry](journal/057-c-on-aarch64.md)

**1. C programs on AArch64**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**. After the `[selftest] kill:` line:

| Look for | Meaning |
|---|---|
| `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two words` | a C program started, with its arguments |
| `7.0 / 2 * 4 = 14` | arithmetic with fractions, done without floating-point registers |
| `[process] … "chello" exited with code 3` | what `main` returned |
| `[process] … "chello" exited with code 4` | what was given to `exit()` |
| `strings: 28 checks, all passed` and `memory: 15 checks, all passed` | the library's string functions and `malloc` |
| `apple fig pear` | `cmem sort pear apple fig` |
| `printf: 42 -7 ff 0X2A [   hi] [hi   ] [00042] x 100%` | `printf` |
| `cprimes: 25 primes below 100` | a loop that only computes |
| `[selftest] c: 8 runs of 4 C programs, twice - each ended with the code its main returned; nothing leaked` | every exit code as expected, twice, nothing left behind |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

Each program's lines appear twice (the test runs two rounds).

**2. The files themselves**

```
make cprogs-aarch64
readelf -h build/c-aarch64/chello | grep Machine
readelf -lW build/c-aarch64/chello | grep LOAD
ls -l build/c-aarch64/chello build/c/chello
```

`Machine` should be `AArch64`; the three `LOAD` lines should end in `0x1000` (4 KiB alignment); the two files should be of similar size (about 50 KB and 54 KB), not 111 KB.

**3. The PC, unchanged**

```
make run64
```

| Type | You should see |
|---|---|
| `chello one "two words"` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two words` |
| `chello float` | `7.0 / 2 * 4 = 14` |
| `cmem` | `strings: 28 checks, all passed` and `memory: 15 checks, all passed` |

**4. Host checks**

```
cd libc && cargo clippy --release --target aarch64-unknown-none-softfloat
cd libc && cargo clippy --release
```

Both should print no warnings.

Automated: `make test64`

---

## 058 · Devices without a bus

[The entry](journal/058-devices-without-a-bus.md)

**1. No devices: thirty-two empty places**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[virtio] 32 places at 0xa000000 - 0xa004000: 0 with a device, 32 empty` | every place the tree lists was looked at; none is occupied |

**2. A disk and a second device**

```
head -c 3145216 /dev/zero > /tmp/taaj-a64-disk.img
make run-aarch64 RUN_A64="-drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -device virtio-rng-device"
```

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003e00: block device (legacy interface), 6143 sectors (3071 KiB), interrupt 79 (edge)` | the disk, in the last place, with its real size |
| `[virtio] 0xa003c00: source of random numbers (legacy interface), interrupt 78 (edge)` | the second device, one place down |
| `[virtio] 32 places at 0xa000000 - 0xa004000: 2 with a device, 30 empty` | the count |

**3. The newer interface**

```
make run-aarch64 RUN_A64="-global virtio-mmio.force-legacy=false -drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003e00: block device (modern interface), 6143 sectors (3071 KiB), interrupt 79 (edge)` | the same disk, version 2 of the registers |

**4. What the tree itself says** (needs `dtc`: `sudo apt install device-tree-compiler`)

```
qemu-system-aarch64 -M virt -cpu cortex-a72 -machine dumpdtb=/tmp/virt.dtb
dtc -I dtb -O dts /tmp/virt.dtb | grep -c 'compatible = "virtio,mmio"'
```

should print `32`.

**5. Host checks**

```
cd virtio && cargo test
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu devicetree
```

Both should end `test result: ok.` - 7 tests and 26 tests.

**6. The PC is unchanged**

```
make run64
```

| Type | You should see |
|---|---|
| `ls` | the files on the disk, as before |

Automated: `make test64`

---

## 059 · One driver, two machines

[The entry](journal/059-one-driver-two-machines.md)

**1. A disk, driven by a program**

```
python3 kernel/kernel64/scripts/disk-pattern.py make /tmp/taaj-a64-disk.img 6143
make run-aarch64 RUN_A64="-drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**. Near the end:

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003e00 block device: registers in the page at 0xa003000, 64 KiB DMA at 0x…, interrupt 79, off until its driver waits - ready for a driver` | the kernel made the three capabilities |
| `[blk] virtio disk at 0xa003e00: 6143 sectors (3071 KiB), queue of 1024, interrupts` | the driver, a program, set the device up |
| `[blkcheck] 348 checks, all passed: 6143 sectors, 33 written at 100 and the last one` (twice) | every request and refusal behaved |
| `[process] … "blk" exited with code 0` (twice) | the driver ended when its clients left |
| `[selftest] disk: blk and blkcheck, twice on one device - every check passed; 338 interrupts passed to the driver; nothing leaked` | interrupts were used; no memory or channel left behind (the number may differ a little) |

Then, after leaving QEMU, the disk itself:

```
python3 kernel/kernel64/scripts/disk-pattern.py check /tmp/taaj-a64-disk.img 6143 written
```

should print `the disk image, read from outside: 34 sectors written, 6109 as they were`.

**2. The other interrupt controller**

```
python3 kernel/kernel64/scripts/disk-pattern.py make /tmp/taaj-a64-disk.img 6143
make run-aarch64 RUN_A64="-M virt,gic-version=3 -drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Look for | Meaning |
|---|---|
| `[irq] GIC version 3: 288 interrupt lines, all off but the timer's` | version 3 |
| the same `[blk]`, `[blkcheck]` and `[selftest] disk:` lines as in 1 | the interrupt was routed there too |

**3. A read-only disk**

```
python3 kernel/kernel64/scripts/disk-pattern.py make /tmp/taaj-a64-disk.img 6143
make run-aarch64 RUN_A64="-drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0,readonly=on -device virtio-blk-device,drive=d0"
python3 kernel/kernel64/scripts/disk-pattern.py check /tmp/taaj-a64-disk.img 6143 untouched
```

| Look for | Meaning |
|---|---|
| `[blk] virtio disk at 0xa003e00: 6143 sectors (3071 KiB), read-only, queue of 1024, interrupts` | the driver noticed |
| `[blkcheck] 340 checks, all passed: 6143 sectors, read-only - every write refused` | no write got through |
| `the disk image, read from outside: 0 sectors written, 6143 as they were` | and the file agrees |

**4. No disk, and a disk with no driver**

```
make run-aarch64
make run-aarch64 RUN_A64="-global virtio-mmio.force-legacy=false -drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Look for | Meaning |
|---|---|
| `[selftest] disk: no block device a driver exists for - not run` | both |
| `[virtio] 0xa003e00 block device: the modern interface - no driver speaks it yet` | the second only |

**5. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu gic
cd kernel/abi && cargo test
cd virtio && cargo test
```

Each should end `test result: ok.` - 12, 7 and 8 tests.

**6. The PC's disk, through the same driver**

```
make run64
```

| Type | You should see |
|---|---|
| `ls` | the files on the disk, as before |
| `disk` | `disk: … sectors of 512 bytes (… KiB)`, as before |

Automated: `make test64`

---

## 060 · The same system on the other CPU

[The entry](journal/060-the-same-system-on-the-other-cpu.md)

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

---

## 061 · Every CPU, and a loader of my own

[The entry](journal/061-every-cpu-and-a-loader-of-my-own.md)

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

---

## 062 · A network card

[The entry](journal/062-a-network-card.md)

**1. The card, on the PC**

```
make run64
```

`make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from. The machine now has a network card, on QEMU's own small network.

In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:04.0 virtio network card: ports 0x…, 160 KiB DMA at 0x…, MSI-X - handed to init` | the kernel found the card and gave it to init |
| `[init] started: registry display fs blk nic shell kbd; supervising` | `nic` is one of the services |
| `[nic] virtio network card at 00:04.0: address 52:54:00:12:34:56, link up, queues of 256 and 256, MSI-X interrupts` | the driver is running |

In the QEMU window:

| Type | You should see |
|---|---|
| `services` | `keys display init system files files.look disk.look nic` |
| `niccheck` | `[niccheck] nobody answers on this network: the card was asked to send one frame, and nothing more`, then `niccheck exited with code 77` - right: on this network nobody speaks the test's language |
| `niccheck now` | `usage: niccheck, niccheck restart` |

**2. A machine without a card**

```
make run64 RUN_NET=
```

| Look for / Type | You should see |
|---|---|
| in the log | no `[nic]` line; `[init] started: registry display fs blk shell kbd; supervising` |
| `niccheck` | `[niccheck] FAILED: no network card (or its driver stopped)` |

**3. With someone on the other end of the wire**

In one terminal, start the peer; it writes its two port numbers to a file:

```
kernel/kernel64/scripts/wire.py /tmp/wire.ports /tmp/wire.verdict &
cat /tmp/wire.ports
```

That prints two numbers, for example `46011 53872`. Put the first after `udp=127.0.0.1:` and the second after `localaddr=127.0.0.1:`:

```
make run64 RUN_NET="-netdev socket,id=n0,udp=127.0.0.1:46011,localaddr=127.0.0.1:53872 -device virtio-net-pci,netdev=n0"
```

| Type | You should see |
|---|---|
| `niccheck` | `[niccheck] 233 checks, all passed: card 52:54:00:12:34:56, 37 frames out, 176 in (101 passed on, 2 for others, 73 dropped)` (a few seconds; the log also shows `[nic] asked to stall: doing nothing for 50 ticks` and `[nic] working again` - that is check 9 keeping the driver busy) |
| `niccheck restart` | in the log: `[nic] asked to stall: doing nothing for 2000 ticks`, then after two seconds `[init] nic has not answered for 2 s - stopping it`, `[init] nic ended with code -1001 - restarting it`, a second `[nic] virtio network card at …` line, and `[niccheck] restart: 9 checks, all passed: the driver was stopped, another started, and the card works again` |

Close QEMU, then ask the peer what it saw:

```
kill %1; sleep 1; cat /tmp/wire.verdict
```

| Look for | Meaning |
|---|---|
| `wire: ok: 39 frames from 52:54:00:12:34:56: 34 to echo, 4 asking for a burst, 1 for frames to others` | every frame was at least 60 bytes, all from one address, none that should have been refused |

**4. The same on AArch64**

```
make run-aarch64
```

The terminal is the machine's keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003c00: network card (legacy interface), interrupt 78 (edge)` | found in the device tree's list of places |
| `[virtio] 0xa003c00 network card: handed to init` | given to init |
| `[nic] virtio network card at 0xa003c00: address 52:54:00:12:34:56, link up, queues of 1024 and 1024, interrupts` | the same driver, its registers in memory this time |

| Type | You should see |
|---|---|
| `niccheck` | `[niccheck] nobody answers on this network: …` and `niccheck exited with code 77` |

With the peer: start `wire.py` again as in step 3 (it was stopped there), and use the two numbers it writes this time:

```
make run-aarch64 RUN_A64="-netdev socket,id=n1,udp=127.0.0.1:46011,localaddr=127.0.0.1:53872 -device virtio-net-device,netdev=n1" RUN_A64_DEVICES="-drive file=build/aarch64/taaj-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Type | You should see |
|---|---|
| `niccheck` | `[niccheck] 233 checks, all passed: card 52:54:00:12:34:56, 37 frames out, 176 in (101 passed on, 2 for others, 73 dropped)` |

**5. The tests by themselves**

```
make net64-test
make aarch64-net-test
```

Each ends with a `wire: ok: 39 frames from …` line and `net64-test passed` / `aarch64-net-test passed`. The logs are `build/net64.log` and `build/aarch64/net.log`; the peer's verdicts are beside them (`.wire`).

**6. Host checks**

```
cd kernel/abi && cargo test nic
cd virtio && cargo test
```

`8 passed` and `11 passed`.

Automated: `make test64`

---

## 063 · On the network

[The entry](journal/063-on-the-network.md)

**1. The machine on its network (PC)**

```
make run64
```

A QEMU window opens to type in; the kernel's log appears in the terminal.

In the terminal's log:

| Look for | Meaning |
|---|---|
| `[registry] net may ask for: nic` | the network service may use the card's driver, and nothing else |
| `[init] started: registry display fs blk nic net shell kbd; supervising` | `net` is a service |
| `[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2` | it is running |

| Type | You should see |
|---|---|
| `netinfo` | `address  10.0.2.15/24`, `router   10.0.2.2`, `card     52:54:00:12:34:56`, then four lines of counters, all 0 on a fresh boot |
| `ping 10.0.2.2` | four lines `56 bytes from 10.0.2.2: seq=1 ttl=255 time …` (seq 1 to 4, a second apart), then `4 sent to 10.0.2.2, 4 answered` |
| `ping 10.0.2.2 1 1000` | `1000 bytes from 10.0.2.2: seq=1 ttl=255 time …` |
| `ping 10.0.2.99 1` | after a second: `10.0.2.99 cannot be reached: seq=1, nobody on this network has that address`, `1 sent to 10.0.2.99, 0 answered`, `ping exited with code 1` |
| `ping` | `usage: ping ADDRESS [COUNT [BYTES]] - an address like 10.0.2.2, at most 1472 bytes` |
| `netinfo` | `frames` now counts some in and out |

(10.0.2.2 is QEMU's own router, inside the emulator. Please do not ping addresses outside 10.0.2.x in a test: QEMU would pass those on to the real network.)

**2. Datagrams between the machine and the host**

Start the machine with one of the host's ports passed on to the machine's port 7:

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=udp:127.0.0.1:5007-:7 -device virtio-net-pci,netdev=n0"
```

| Type (in QEMU) | You should see |
|---|---|
| `udpecho &` | `[1] udpecho`, and `udpecho: listening on port 7` |

Then in another terminal on the host:

```
echo hello | nc -u -w1 127.0.0.1 5007
```

| Look for | Meaning |
|---|---|
| `hello` printed by `nc` | the datagram went in, and its echo came back |
| in QEMU: `udpecho: 6 bytes from 10.0.2.2:…` | the machine saw it (6: `hello` and the newline) |

And the other way. On the host:

```
nc -u -l 127.0.0.1 5008
```

| Type (in QEMU) | You should see |
|---|---|
| `udpsend 10.0.2.2 5008 hello from taaj` | `udpsend: 15 bytes from port 49152 to 10.0.2.2:5008`, and on the host `nc` prints `hello from taaj`. Two seconds later the machine says `udpsend: no answer` (nobody typed one) |
| `udpecho 7 1` | `udpecho: port 7: another program has that port` |

**3. A machine without a card**

```
make run64 RUN_NET=
```

| Type | You should see |
|---|---|
| `ping 10.0.2.2 1` | `ping: no network (no card, or its service stopped)` |

**4. The same on AArch64**

```
make run-aarch64
```

The terminal is the keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2` | the same service |

| Type | You should see |
|---|---|
| `ping 10.0.2.2 2` | two lines `56 bytes from 10.0.2.2: seq=… ttl=255 time …`, then `2 sent to 10.0.2.2, 2 answered` |
| `ping 10.0.2.99 1` | `10.0.2.99 cannot be reached: seq=1, nobody on this network has that address` |

**5. The tests by themselves**

```
make ip64-test
make aarch64-ip-test
```

Each prints a line from each of the two other sides, and ends `ip64-test passed` / `aarch64-ip-test passed`:

| Look for | Meaning |
|---|---|
| `wire: ok: 15 checks; … frames from 52:54:00:12:34:56; answered 0 questions for 10.0.2.2, 3 pings and 1 datagrams; … questions about addresses nobody has` | the router made by hand found nothing wrong |
| `host: ok: 5 checks; 1 datagrams from the machine answered` | over QEMU's network, datagrams got through both ways |

The logs are `build/ip64.log` (hand-made router) and `build/ip64.log.user` (QEMU's network), and the same under `build/aarch64/ip.log`.

**6. Host checks**

```
cd net && cargo test
cd kernel/abi && cargo test
```

`30 passed` and `21 passed`.

Automated: `make test64`

---

## 064 · Connections

[The entry](journal/064-connections.md)

**1. Connections out of the machine (PC)**

On the host, in one terminal, something to connect to - a listener that prints what it gets:

```
nc -l 127.0.0.1 5009
```

In another terminal:

```
make run64
```

| Type (in QEMU) | You should see |
|---|---|
| `tcpsend 10.0.2.2 5009 hello from taaj` | `tcpsend: connected to 10.0.2.2:5009 from port 49152`, `tcpsend: 16 bytes sent, sum 335d05b2`; on the host, `nc` prints `hello from taaj` and ends - this `nc` ends as soon as the other side says it has no more to send - so the machine reads to the end at once: `tcpsend: 0 bytes back, sum 00000000, in … ms` |
| `tcpsend 10.0.2.2 5009` | `tcpsend: 10.0.2.2:5009: nothing is listening on that port there` (the `nc` has ended), and `tcpsend exited with code 1` |
| `tcpsend 10.0.2.99 80` | after three seconds: `tcpsend: 10.0.2.99:80: nobody on this network has that address` |
| `netinfo` | a last line `tcp      … segments in, … sent again, 0 answered "no such connection"` |

**2. Connections into the machine, and C**

Start the machine with two of the host's ports passed on to the machine's ports 7 and 8000:

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=tcp:127.0.0.1:5007-:7,hostfwd=tcp:127.0.0.1:5080-:8000 -device virtio-net-pci,netdev=n0"
```

| Type (in QEMU) | You should see |
|---|---|
| `tcpecho &` | `[1] tcpecho`, and `tcpecho: listening on port 7` |

On the host:

```
echo hello there | nc -N 127.0.0.1 5007
```

| Look for | Meaning |
|---|---|
| `hello there` printed by `nc` | the line went in and came back |
| in QEMU: `tcpecho: 12 bytes echoed to 10.0.2.2:…` | the machine served the connection to its end |

| Type (in QEMU) | You should see |
|---|---|
| `cnet serve 7 1` | `cnet: port 7: another program has that port` |
| `cnet serve 8000 1` | `cnet: listening on port 8000`, and it waits |

On the host:

```
echo hi from the host | nc -N 127.0.0.1 5080
```

| Look for | Meaning |
|---|---|
| `you sent 17 bytes: HI FROM THE HOST` printed by `nc` | a C program accepted the connection, read to the end and answered |
| in QEMU: `cnet: 17 bytes from 10.0.2.2:…, answered` | |

And C going out. On the host: `nc -l 127.0.0.1 5009`, then:

| Type (in QEMU) | You should see |
|---|---|
| `cnet send 10.0.2.2 5009 hello from c` | `cnet: connected to 10.0.2.2:5009 from port …`; `nc` prints `hello from c` and ends; then `cnet: 0 bytes back, and the other side is done` |

**3. The same on AArch64**

On the host: `nc -l 127.0.0.1 5009`. Then:

```
make run-aarch64
```

The terminal is the keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Type | You should see |
|---|---|
| `tcpsend 10.0.2.2 5009 hello from arm` | `tcpsend: connected to 10.0.2.2:5009 from port 49152`, `tcpsend: 15 bytes sent, sum …`; `nc` prints `hello from arm` and ends; `tcpsend: 0 bytes back, sum 00000000, in … ms` |
| `tcpsend 10.0.2.2 5009` | `tcpsend: 10.0.2.2:5009: nothing is listening on that port there` |

**4. The tests by themselves**

```
make tcp64-test
make aarch64-tcp-test
```

Each prints the host's verdict and ends `tcp64-test passed` / `aarch64-tcp-test passed`:

| Look for | Meaning |
|---|---|
| `host: ok: 5 checks; 3 connections from the machine served, 300029 bytes` | every connection into the machine came back whole, and the three out of it arrived |

Host checks:

```
cd net && cargo test           # 72 tests: the protocols, TCP and the stack
cd kernel/abi && cargo test    # 22 tests
make libc-test                 # errno, and inet.c against this machine's own
```

Automated: `make test64`

---

## 065 · A page from TheTaaJ

[The entry](journal/065-a-page-from-taaj.md)

**1. The page, from the PC**

Start the machine with the host's port 8080 passed on to the machine's port 80. (`make run64` puts the site on its disk as `www`, beside the programs.)

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 -device virtio-net-pci,netdev=n0"
```

| Type (in QEMU) | You should see |
|---|---|
| `httpd &` | `[1] httpd`, and `httpd: serving /www on port 80` |

On the host, in another terminal:

| Type (on the host) | You should see |
|---|---|
| `curl -i http://127.0.0.1:8080/` | `HTTP/1.1 200 OK`, `Server: TheTaaJ`, `Content-Type: text/html; charset=utf-8`, `Content-Length: 1974`, `Connection: close`, an empty line, and the page; in QEMU: `httpd: 10.0.2.2 "GET /" 200 1974 bytes` |
| `curl -s http://127.0.0.1:8080/ \| cmp - user/www/index.html && echo same` (in the TheTaaJ directory) | `same` |
| `curl -s http://127.0.0.1:8080/style.css \| cmp - user/www/style.css && echo same` | `same` |
| `curl -I http://127.0.0.1:8080/about/` | `HTTP/1.1 200 OK` and `Content-Length: 1549`, no page (HEAD); in QEMU: `"HEAD /about/" 200 0 bytes` |
| `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/nothing` | `404` |
| `curl --path-as-is -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/../welcome` | `403`; in QEMU: `"GET /../welcome" 403 182 bytes` |
| `curl -i -X POST -d x=1 http://127.0.0.1:8080/` | `HTTP/1.1 405 Method Not Allowed` and `Allow: GET, HEAD` |
| `printf 'GET / HTTP/1.0\r\n\r\n' \| nc -N 127.0.0.1 8080 \| head -1` | `HTTP/1.1 200 OK` |

| Look for | Meaning |
|---|---|
| without `--path-as-is`, the `..` request is a `404`, and QEMU logs `"GET /welcome"` | curl removed the `..` itself before sending; the server never saw it |

And in a browser on the host: open `http://127.0.0.1:8080/`.

| Look for | Meaning |
|---|---|
| the page "TheTaaJ", in a serif font with a diagram in a grey box; "About this server" leads to a page with a table | the page and its style sheet both came from the machine |
| in QEMU: `"GET /" 200 1974 bytes` and `"GET /style.css" 200 1374 bytes` (a browser may also ask for `/favicon.ico`: `404`) | |

(I have not looked at the page in a browser myself: this row is what it should show.)

**2. The same on AArch64**

```
make run-aarch64 RUN_A64_DEVICES="-drive file=build/aarch64/taaj-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 -device virtio-net-device,netdev=n0"
```

The terminal is the keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Type | You should see |
|---|---|
| `httpd &` | `[1] httpd`, `httpd: serving /www on port 80` |

| Type (on the host, another terminal, in the TheTaaJ directory) | You should see |
|---|---|
| `curl -s http://127.0.0.1:8080/ \| head -5` | `<!doctype html>`, `<html lang="en">`, `<head>`, ... |
| `curl -s http://127.0.0.1:8080/ \| cmp - user/www/index.html && echo same` | `same`; in QEMU: `httpd: 10.0.2.2 "GET /" 200 1974 bytes` |

**3. The tests by themselves**

```
make http64-test
make aarch64-http-test
```

Each prints the host's verdict and ends `http64-test passed` / `aarch64-http-test passed`:

| Look for | Meaning |
|---|---|
| `host: ok: 17 checks` | every one of the nineteen connections was answered as it should be, the 1 MiB file byte for byte |

The machine's side is in `build/http64.log` (and `build/aarch64/http.log`): `grep -a 'httpd:' build/http64.log` shows one line per connection, ending with `httpd: 10.0.2.2 sent no request`.

Host checks:

```
cd net && cargo test           # 78 tests: the protocols, TCP, the stack and HTTP
```

Automated: `make test64`

---

## 066 · A screen and a pointer

[The entry](journal/066-a-screen-and-a-pointer.md)

**1. The PC: the arrow and clicks**

```
make run64
```

Click inside the QEMU window: QEMU then sends the host's mouse to the machine as a PS/2 mouse. **Ctrl+Alt+G** gives the mouse back to the host.

| Type / do | You should see |
|---|---|
| (at boot, in the terminal) | `[irq] PS/2 mouse on GSI 12, sharing the keyboard's vector` and `[kbd] PS/2 keyboard ready (driver in ring 3, process N); the mouse too` |
| move the mouse | a white arrow with a black outline moves on the screen |
| click left, right, middle | in the terminal: `[display] left button at (X, Y)`, `right …`, `middle …`, with where the arrow's tip was |
| move far past any edge | the arrow stops at the edge; a click there says `(0, …)`, `(1023, …)`, `(…, 0)` or `(…, 767)` |
| `echo hi` | `hi` on the screen; where the text is drawn under the arrow, the arrow stays on top and nothing is left behind when it moves away |

(I drove these with the monitor's `mouse_move` and `mouse_button`, not a hand on a mouse: these rows are what a real mouse should give.)

**2. AArch64: a screen, a keyboard and a tablet**

```
make run-aarch64-screen
```

QEMU opens a window; the kernel's log stays in the terminal. Close the window to leave.

| Type / do | You should see |
|---|---|
| (at boot, in the terminal) | `[screen] ramfb: 1024x768, 32 bits a pixel, 3072 KiB at …`, `[hid] virtio tablet at …: "QEMU Virtio Tablet"`, `[hid] virtio keyboard at …: "QEMU Virtio Keyboard"` |
| (in the window) | the amber band, the gradient, and the shell's prompt, as on the PC |
| `echo hi` (typed in the window) | `hi`, on the screen |
| move the host's mouse over the window | the arrow follows it with nothing to grab: the tablet sends where the host's pointer is |
| click near the window's top-left, then bottom-right corner | `[display] left button at (X, Y)` close to `(0, 0)`, then close to `(1023, 767)` |

`make run-aarch64` is unchanged: the serial terminal, no window.

(I started this without a window, `RUN_A64_DISPLAY="-display none"`, and saw the boot lines and the shell; the window rows are what it should show.)

**3. The tests by themselves**

```
make screen64-test
make aarch64-screen-test
```

| Look for | Meaning |
|---|---|
| `clicks: left (512, 384);right (612, 434);middle (1023, 767);left (783, 527)` (PC) | each click at the place worked out by hand, and the pointer stopped at the corner |
| `clicks: left (512, 384);right (612, 434);middle (511, 191);right (0, 0);left (1023, 767);left (783, 527)` (AArch64) | the mouse's steps and the tablet's jumps both moved the one pointer |
| `check-pointer: the arrow is at (783, 527), 114 pixels of it on the screen, and nowhere else` | the screenshot has the arrow where the pointer is, and none left behind |
| `screen64-test passed` / `aarch64-screen-test passed` | |

The screenshots are `build/screen64.log.ppm` and `build/aarch64/screen.log.ppm`, and they open in an image viewer.

Host checks:

```
cd gfx && cargo test           # 8 tests: clipping, copy, text, the arrow, save-under
cd input && cargo test         # 41 tests: keys, PS/2 packets, evdev events, virtio keys
cd virtio && cargo test        # 12 tests, the input device's configuration among them
cd kernel/abi && cargo test    # 23 tests, the POINTER message among them
```

Automated: `make test64`

---

## 067 · Windows

[The entry](journal/067-windows.md)

**1. The PC: windows**

```
make run64
```

Click inside the QEMU window so that it takes the mouse (**Ctrl+Alt+G** gives it back to the host).

| Type / do | You should see |
|---|---|
| (at boot) | the amber band, and below it a window with the title bar "terminal", grey-blue text on navy, the shell's prompt in it; in the terminal you started QEMU from: `[display] window 1 "terminal" opened, 984x632 at (19, 80)` |
| `counter &` | `[1] counter`; a small window "counter" opens over the terminal, at (200, 150), with an amber title bar and a red close box, `clicks: 0` and two buttons, `+1` and `quit`; the terminal's title bar turns grey-blue: the new window has the focus |
| click `+1` | the button looks pushed in while held; `clicks: 1` when let go |
| press on `+1`, move off it, let go | nothing counts |
| type `+` | `clicks: 2`: keys go to the window with the focus. In the log: `[counter] key +` |
| type `a` | nothing on the screen; in the log: `[counter] key a`, and the shell did not get it |
| click anywhere on the terminal | the terminal comes to the front and its title bar turns amber; counter's turns grey-blue (it is behind the terminal now) |
| `echo back` | `back`: the keys are the shell's again |
| drag the terminal by its title bar | the terminal follows the pointer; where it was, the desktop shows again. It stops with part of its title bar still on the screen, however far you drag. In the log, when you let go: `[display] window 1 moved to (x, y)` |
| click counter's title bar (drag the terminal aside first if it covers it) | counter comes to the front |
| click counter's close box | `[display] window 2: its program is asked to close it`, `[counter] closed by its close box`; the window goes, and the terminal has the focus again |
| `counter &`, then click `quit` | `[counter] quit`, and the window goes |

**2. AArch64: the same, in a window of QEMU's**

```
make run-aarch64-screen
```

The arrow follows the host's pointer over QEMU's window with nothing to grab (a tablet). Every row of 1 applies, from `counter &` on.

(I drove these through QEMU's monitor and QMP, as the automated tests do, and looked at the screenshots; not with a hand on a mouse in a window.)

**3. The tests by themselves**

```
make gui64-test
make aarch64-gui-test
```

| Look for | Meaning |
|---|---|
| `check-pixels: 3 pixels as expected` | the terminal is drawn where it was dragged, and the desktop where it and counter were |
| `check-pointer: the arrow is at (430, 162), 114 pixels of it on the screen, and nowhere else` | the arrow is on what was counter's close box |
| `gui64-test passed` / `aarch64-gui-test passed` | every step: the window opened, clicked, typed into, the focus given back, the terminal dragged, the window closed |

The log of one is the story of the test: `grep -a 'display\]\|counter\]\|shell\] line' build/gui64.log`.

Host checks:

```
cd gui && cargo test           # 22 tests: the protocol, the desktop's rules, the toolkit
cd gfx && cargo test           # 10 tests, Rect::union and Surface::region among them
```

Automated: `make test64`

---

## 068 · A text editor

[The entry](journal/068-text-editor.md)

**1. The PC: write a file**

```
make run64
```

| Type | You should see |
|---|---|
| `edit hello.md` | a window "edit hello.md" over the terminal, with a block cursor at the top left and a status line ` hello.md  line 1/1, column 1  a new file`; in the terminal you started QEMU from: `[edit] new file hello.md` |
| `# Hello`, Enter, `written on TheTaaJ` | the text as typed; the status line shows `line 2/2, column 19  [modified]` |
| Backspace a few times, then retype | characters go and come back; Backspace at the start of line 2 joins it onto line 1, Enter splits it again |
| the arrows, Home, End | the cursor moves; Up and Down from the end of a long line through a short one come back to the same column |
| click anywhere in the text | the cursor jumps there (or to the end of that line) |
| Ctrl+S | `saved, N bytes` in the status line, `[modified]` gone; in the log: `[edit] saved hello.md: N bytes, 2 lines` |
| type something, then Ctrl+Q | `unsaved changes: ctrl+q again to drop them` |
| Ctrl+Q | the window closes, and the shell has the keys again |
| `cat hello.md` | what was saved, without the last change |

**2. A long file: scrolling through a journal entry**

Put the previous entry on the disk first, with QEMU closed (the disk exists after the first `make run64`):

```
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img journal.md docs/journal/067-windows.md
make run64
```

| Type | You should see |
|---|---|
| `edit journal.md` | `# Windows` at the top; `[edit] opened journal.md: 325 lines, …`. Its dashes and arrows are drawn as `?`: the font has only ASCII |
| Page Down | the view moves 23 lines, and the old last line is now the first; `[edit] view from line 24` |
| hold Down | the view scrolls a line at a time once the cursor reaches the bottom |
| End on a line longer than 76 characters | the view scrolls sideways |
| the close box with unsaved changes | the status line warns; a second press closes |

**3. Write a journal entry, and copy it to the host**

```
make run64
```

In TheTaaJ: `edit entry.md`, write, Ctrl+S, Ctrl+Q. Close QEMU, then on the host:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img entry.md > entry.md
```

`entry.md` holds exactly what was saved.

**4. AArch64**

```
make run-aarch64-screen
```

The same as 1 to 3, in QEMU's window, with `build/aarch64/taaj-disk.img` as the disk. It is the disk `make run-aarch64` uses, so close any other AArch64 QEMU first.

(I drove these through QEMU's monitor and QMP, as the automated tests do, and looked at a screenshot; not with my own hands on the keys.)

**5. The tests by themselves**

```
make edit64-test
make aarch64-edit-test
```

| Look for | Meaning |
|---|---|
| `check-pixels: 3 pixels as expected` | the cursor, the status line and the text drawn where they should be |
| `edit64-test passed` / `aarch64-edit-test passed` | every step of the three editing sessions, and the files on the disk exactly as typed, the unsaved changes not in them |

The editor's side of the story: `grep -a 'edit\]' build/edit64.log`. The files it saved, as read back: `build/edit64.log.note.got` and `build/edit64.log.long.got`.

Host checks:

```
cd gui && cargo test           # 38 tests, 16 of them the editor's (gui::text)
```

Automated: `make test64`

---

## 069 · A C compiler on TheTaaJ

[The entry](journal/069-tcc.md)

**1. The PC: compile a C program on TheTaaJ**

`make run64` now puts tcc, its headers and libc.a on the run disk. Put a C source on it first, with QEMU closed (the disk exists after the first `make run64`):

```
make run-disk
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img chello.c libc/programs/chello.c
make run64
```

| Type | You should see |
|---|---|
| `ls lib` | `libc.a` (about 208,000 bytes) and `tcc/` |
| `ls lib/tcc` | `include/` and `libtcc1.a` |
| `tcc chello.c -o mine` | nothing printed; in the log `[registry] shell gave tcc: files` and `"tcc" exited with code 0` |
| `mine one two` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two` |
| `mine float` | its lines, then `7.0 / 2 * 4 = 14` |
| `ls` | `mine`, a new file of about 110 KB |
| `write bad.c oops` | `bad.c: 5 bytes` (`oops` and a newline) |
| `tcc bad.c` | `bad.c:2: warning: type defaults to int`, `bad.c:2: error: ';' expected (got '<eof>')`, and `tcc exited with code 1` |

**2. Write a program in the editor and compile it**

```
make run64
```

| Type | You should see |
|---|---|
| `edit hi.c` | the editor's window |
| `#include <stdio.h>`, Enter, `int main(void) { printf("%.3f\n", 1.0 / 8); return 0; }`, Ctrl+S, Ctrl+Q | saved, and the shell again |
| `tcc hi.c -o hi` | nothing printed |
| `hi` | `0.125` |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64-disk
tools/tfstool-rs/target/release/tfstool put build/aarch64/taaj-disk.img chello.c libc/programs/chello.c
make run-aarch64
```

The same as part 1, typed at the terminal; `ls lib` shows libc.a at about 239,000 bytes, and `mine` is about 125 KB.

**4. On the host**

```
make libc-test
build/tcc/x86_64/cross/taaj-tcc libc/programs/chello.c -o build/mine
```

| Look for | Meaning |
|---|---|
| `format: 2127 checks, all the same as the host's printf` | `%f %e %g` agree with glibc |
| `test result: ok. 3 passed` (third block) | strtod's helpers |
| `build/mine`, about 110 KB | the cross compiler: put it on a disk and it runs, the same file tcc on TheTaaJ writes |

**5. The tests by themselves**

```
make tcc64-test
make aarch64-tcc-test
make boot64-test
```

| Look for | Meaning |
|---|---|
| `tcc on TheTaaJ wrote hello, chello2, clib.o and clib2 byte for byte as tcc on the host does` | tcc ran on TheTaaJ, and its output is exactly right |
| `tcc64-test passed`, `aarch64-tcc-test passed` | all of it, on each machine |
| `library: 53 checks, all passed` in `build/serial64.log` | the new library parts, built by clang |
| `cfloat 1: 30000000 rounds, 0 wrong` (and 2, 3, 4) | four programs doing floating point at once, each keeping its own registers |

Automated: make test64

---

## 070 · TheTaaJ builds its C library and C programs

[The entry](journal/070-building-on-taaj.md)

**1. The PC: build the C library and the programs on TheTaaJ**

`make run64` puts tcc, the sources in `/src` and the script `/src/build` on the run disk.

```
make run64
```

| Type | You should see |
|---|---|
| `ls /src/libc/c` | the C library's `.c` files and `format.h` |
| `time source /src/build` | each command after a `+`, no errors, `built: the C library and 11 programs`, then `took` about 20 to 40 s |
| `ls ..` | the prompt is `taaj:/src/libc/programs>` now: the script's `cd` stays done. `libc.a` (about 225,000 bytes), `c/`, `programs/` |
| `./cprimes 100` | `cprimes: 25 primes below 100` |
| `./chello built here` | `Hello from C`, `argc 3`, `argv[1] built`, `argv[2] here` |
| `cd /` | the prompt is `taaj:/>` again |
| `write broken tcc nofile.c` | `broken: 13 bytes` |
| `source broken` | `+ tcc nofile.c`, `tcc: error: file 'nofile.c' not found`, `tcc exited with code 1`, `source: broken: stopped at line 1` |

**2. Change a program on TheTaaJ, and rebuild it there**

```
make run64
```

| Type | You should see |
|---|---|
| `edit /src/libc/programs/chello.c` | the editor, the file's first lines |
| Page Down twice, then Down seven times | the status line says `line 54/…`, on `print("Hello from C\n");` |
| End, Left five times, Backspace, `TheTaaJ` | the line now reads `print("Hello from TheTaaJ\n");` |
| Ctrl+S, Ctrl+Q | saved, and the shell again |
| `cd /src/libc/programs` | the prompt says `taaj:/src/libc/programs>` |
| `tcc -L.. chello.c -o chello` | nothing printed |
| `./chello` | `Hello from TheTaaJ` |
| `chello` | `Hello from C`: a bare name is always the program in `/`, wherever the shell is |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64
```

The same as part 1, typed at the terminal. The build takes about 45 s, and this `libc.a` is about 276,000 bytes.

**4. The tests by themselves**

```
make tcc64-test
make aarch64-tcc-test
```

| Look for | Meaning |
|---|---|
| `took … s; the 2 CPUs worked for … s in all` in `build/tcc64.log` | how long the build took on TheTaaJ |
| `source: broken: stopped at line 3` in the log | a script stops at the command that fails |
| `tcc on TheTaaJ wrote all 28 files byte for byte as tcc on the host does: …` | everything built on TheTaaJ is exactly what the cross compiler builds |
| `tcc64-test passed`, `aarch64-tcc-test passed` | all of it, both boots, on each machine |

Automated: make test64

---

## 071 · TCC builds itself on TheTaaJ

[The entry](journal/071-tcc-builds-itself.md)

**1. The PC: TCC builds itself**

```
make run64
```

| Type | You should see |
|---|---|
| `ls /src/tcc` | `build`, `config.h`, `tcc.c`, `tccgen.c` and the rest: 24 files, `lib/`, about 1,277,000 bytes |
| `time source /src/tcc/build` | the six commands, each after a `+`, then `built: tcc by tcc, tcc by itself, and its library`, then `took` about 13 to 20 s |
| `ls ..` | the prompt is `taaj:/src/tcc/lib>`; `tcc2` and `tcc3` are both `502425` bytes: the same size, because they are the same file |
| `cd /` | the prompt is `taaj:/>` |

**2. Use the compiler TCC compiled, then put the old one back**

| Type | You should see |
|---|---|
| `mv tcc tcc.clang` | nothing printed: the compiler clang built is set aside |
| `cp src/tcc/tcc3 tcc` | `tcc: 502425 bytes` |
| `tcc src/libc/programs/cprimes.c -o p` | nothing printed, after a second or so |
| `p 1000` | `cprimes: 168 primes below 1000` |
| `rm tcc` | nothing printed |
| `mv tcc.clang tcc` | nothing printed: `/tcc` is the one from the host again |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64
```

The same as parts 1 and 2, typed at the terminal. The build takes about 26 s. The sources are `arm64-*` instead of `x86_64-*` and `i386-*`, and tcc2 and tcc3 are both `598669` bytes.

**4. The tests by themselves**

```
make tcc64-test
make aarch64-tcc-test
```

| Look for | Meaning |
|---|---|
| `built: tcc by tcc, tcc by itself, and its library` in `build/tcc64.log` | all three generations were built on TheTaaJ |
| `tcc on TheTaaJ wrote all 38 files byte for byte as tcc on the host does: …` (36 on AArch64) | tcc2 and tcc3 are the same file and the cross compiler's, and libtcc1.a is the one TheTaaJ was given |
| `tcc64-test passed`, `aarch64-tcc-test passed` | all of it, on each machine |

Automated: make test64

---

## 072 · C++ on TheTaaJ

[The entry](journal/072-cxx-on-taaj.md)

**1. The PC: C++ programs**

```
make run64
```

The first `make run64` after this step builds `libc++.a` for the PC, which takes a few minutes.

| Type | You should see |
|---|---|
| `cxxhello pear apple pear fig` | `Hello from C++`, `4 words, 3 different, in order: apple fig pear pear`, `pear 2, apple 1, fig 1`, `the longest: apple (5 letters)` |
| `cxxhello` | `Hello from C++`, `give me some words: cxxhello pear apple pear fig` |
| `cxxhello antidisestablishmentarianism` | `1 word, 1 different, …`, `the longest: antidisestablishmentarianism (28 letters)`, then `caught: no word may be longer than 12 letters, and "antidisestablishmentarianism" is 28` |
| `cxxlib` | after a moment, `c++: 56 checks, all passed`, then `c++: the objects outside main were destroyed` |
| `cxxlib throw` | `libc++abi: terminating due to uncaught exception of type std::runtime_error: nobody catches this`, then `cxxlib exited with code 134` |

**2. The C library's new parts**

`clib` reads `welcome` and `docs/`, which the test disks have and the run disk does not; make them first (skip the first two lines if they are already there):

| Type | You should see |
|---|---|
| `mkdir docs` | nothing printed |
| `echo Hello from the host > welcome` | nothing printed |
| `with files clib` | `library: 70 checks, all passed`, then `library: atexit ran its handlers, newest first` |
| `ls` | `cxxhello` (about 427,000 bytes), `cxxlib` (about 751,000) and `clib` among the programs |

**3. AArch64**

Close any AArch64 QEMU first: `build/aarch64/taaj-disk.img` is the disk `make run-aarch64` uses.

```
make run-aarch64
```

The same as parts 1 and 2, typed at the terminal. `cxxhello` is about 410,000 bytes and `cxxlib` about 714,000.

**4. The tests by themselves**

```
make libc-test
make cxx64-test
make aarch64-cxx-test
```

| Look for | Meaning |
|---|---|
| `math: 150135396 checks against this machine's, 0 failed` | math.c gives glibc's answers, bit for bit |
| `scan: 200461 checks, all the same as the host's sscanf` | sscanf reads what glibc's does |
| `C++ on TheTaaJ (x86_64): cxxlib's 56 checks passed, the destructors ran after main, an uncaught exception ended in std::terminate, cxxhello sorted, counted and caught` | all of part 1 on the PC (`aarch64` for the other) |
| `cxx64-test passed`, `aarch64-cxx-test passed` | each machine |

Automated: make test64

---

## 073 · LLVM on TheTaaJ

[The entry](journal/073-llvm-on-taaj.md)

**1. The PC: llvm-tblgen**

```
make llvm-put
make run64
```

The first `make llvm-put` builds LLVM's Support and TableGen for the PC, about 230 files, which takes a few minutes. It puts `llvm-tblgen`, the 25 `.td` files it reads (under `src/llvm`) and a broken `bad.td` on the run disk. Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly llvm-tblgen --version` | `TheTaaJ LLVM version 18.1.3`, `  Optimized build.` |
| `time with files llvm-tblgen -gen-vt -I src/llvm/include src/llvm/include/llvm/CodeGen/ValueTypes.td -o vt.inc` | `took` about 2 s |
| `time with files llvm-tblgen -gen-intrinsic-enums -I src/llvm/include src/llvm/include/llvm/IR/Intrinsics.td -o enums.inc` | `took` about 9 s |
| `with files llvm-tblgen bad.td` | `bad.td:2:17: error: Couldn't find class 'Nothing'`, the line `def something : Nothing;` with a `^` under `Nothing`, then `llvm-tblgen exited with code 1` |
| `ls` | `vt.inc` 18018, `enums.inc` 32736, `llvm-tblgen` 3634824, `bad.td` 38, and `src/` |

**2. The C library's new parts**

`clib` reads `welcome` and `docs/`. Make them first if they are not there yet:

| Type | You should see |
|---|---|
| `mkdir docs` | nothing printed |
| `echo Hello from the host > welcome` | nothing printed |
| `with files clib` | `library: 104 checks, all passed`, then `library: atexit ran its handlers, newest first` |
| `cxxlib` | `c++: 62 checks, all passed`, then `c++: the objects outside main were destroyed` |

**3. AArch64**

Close any AArch64 QEMU first.

```
make llvm-put-aarch64
make run-aarch64
```

The same as parts 1 and 2, typed at the terminal. Its disk already has `welcome`. The `-gen-vt` job takes about 3.5 s, and the intrinsics job about 19 s. `llvm-tblgen` is 3171856 bytes.

**4. The tests by themselves**

```
make libc-test
make llvm64-test
make aarch64-llvm-test
```

| Look for | Meaning |
|---|---|
| `llvm-tblgen on TheTaaJ (x86_64) read 25 .td files and wrote, byte for byte as the host's: vt.inc (18018 bytes) enums.inc (32736 bytes) impl.inc (3203861 bytes); took …` | all three jobs, and their outputs equal to the host's (`aarch64` for the other) |
| `llvm64-test passed`, `aarch64-llvm-test passed` | each machine |

Automated: make test64

---

## 074 · LLVM code generation on TheTaaJ

[The entry](journal/074-llvm-code-generation.md)

**1. The PC: llc and llvm-mc**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

The first `make llvm-put` builds LLVM's code generators for the PC: about 1,500 files, which takes a long time. It makes a new 128 MiB run disk, `build/llvm-disk.img`, with everything the usual run disk has, plus:
- `llvm-tblgen`, `llc`, `llvm-mc`;
- `cwords.c` in LLVM's language for each machine (`x86_64.ll`, `aarch64.ll`) and in AArch64 assembly (`aarch64.s`);
- a broken `bad.ll` and `bad.s`.

Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly llc --version` | `TheTaaJ LLVM version 18.1.3`, `Default target: x86_64-unknown-taaj`, and under `Registered Targets:` `aarch64`, `arm64`, `x86` and `x86-64` among others |
| `time with files llc -O2 x86_64.ll -o x86_64.s` | `took` about 6 s |
| `time with files llvm-mc -triple=x86_64-unknown-none-elf -filetype=obj x86_64.s -o x86_64.o` | `took` about 2 s |
| `time with files llc -O2 -filetype=obj aarch64.ll -o aarch64.o` | `took` about 7 s: AArch64 code, made on the PC |
| `with files llvm-mc -triple=aarch64-unknown-none-elf -filetype=obj aarch64.s -o mc-aarch64.o` | nothing printed |
| `with files llc bad.ll -o bad.out` | `program: error: program: bad.ll:2:7: error: value doesn't match function result type 'i32'`, the line `  ret i64 0` with a `^` under `i64`, then `llc exited with code 1` |
| `with files llvm-mc -triple=x86_64-unknown-none-elf -filetype=obj bad.s -o bad.o` | `bad.s:2:2: error: invalid instruction mnemonic 'notaninstruction'`, the line with `^~~~~~~~~~~~~~~~` under it, then `llvm-mc exited with code 1` |
| `ls` | `x86_64.s` 15906, `x86_64.o` 5848, `aarch64.o` 6712, `mc-aarch64.o` 6712, `llc` 34185400, `llvm-mc` 11067720 |

**2. The same files from the host's LLVM**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img aarch64.o > /tmp/taaj-aarch64.o
/usr/lib/llvm-18/bin/llc -O2 -filetype=obj build/llvm64.log.work/aarch64.ll -o /tmp/host-aarch64.o
cmp /tmp/taaj-aarch64.o /tmp/host-aarch64.o && echo same
```

| Look for | Meaning |
|---|---|
| `same` | the object TheTaaJ wrote is the host's, byte for byte (`build/llvm64.log.work` is there once `make llvm64-test` has run) |

**3. AArch64**

Close any AArch64 QEMU first.

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1, typed at the terminal. `llc --version` says `Default target: aarch64-unknown-taaj`. The two `llc` steps take about 9 s each, and the `llvm-mc` steps about 3 s. `llc` is 28930096 bytes and `llvm-mc` 10395896. The four files it writes are the same sizes as on the PC, and the same bytes.

**4. The tests by themselves**

```
make libc-test
make llvm64-test
make aarch64-llvm-test
```

| Look for | Meaning |
|---|---|
| `libm: 45602992 checks against this machine's long double, 43614069 the nearest double, 0 too far off` | musl's functions on the host, against glibc's |
| `llc and llvm-mc on TheTaaJ (x86_64) wrote, byte for byte as the host's: x86_64.s (15906 bytes) aarch64.o (6712 bytes) x86_64.o (5848 bytes) mc-aarch64.o (6712 bytes)` | llc and llvm-mc's outputs equal to the host's (`aarch64` for the other machine) |
| `llvm64-test passed`, `aarch64-llvm-test passed` | each machine |

Automated: make test64

---

## 075 · Clang on TheTaaJ

[The entry](journal/075-clang-on-taaj.md)

**1. The PC: clang**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

The first `make llvm-put` builds Clang for the PC: about 620 files more than E2, which takes a while. It makes a new 192 MiB run disk, `build/llvm-disk.img`, with what E2's had, plus:
- `clang`, its headers in `/lib/clang/18/include`, and the C library's headers in `/include`;
- TheTaaJ's C programs in `/src` (`src/cwords.c`, `src/chello.c`, ...);
- `cflags`: the options TheTaaJ's build compiles them with, as a response file;
- a broken `bad.c`.

Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly clang --version` | `TheTaaJ clang version 18.1.3`, `Target: x86_64-unknown-taaj`, `Thread model: posix`, `InstalledDir: /` |
| `with readonly clang -print-resource-dir` | `/lib/clang/18` |
| `time with files clang @cflags -fno-ident -c src/cwords.c` | `took` about 20 s, nothing else printed |
| `time with files clang @cflags -c src/chello.c src/cmem.c src/cprint.c` | `took` about 35 s: three objects, from one clang |
| `with files clang @cflags -S src/chello.c -o chello.s` | nothing printed |
| `with files clang -c bad.c` | `bad.c:2:12: error: use of undeclared identifier 'x'`, the line `    2 \|     return x;` with a `^` under the `x`, `1 error generated.`, then `clang exited with code 1` |
| `with files clang -c nothere.c` | `clang: error: no such file or directory: 'nothere.c'`, `clang: error: no input files`, then `clang exited with code 1` |
| `ls` | `cwords.o` 5808, `chello.o` 2976, `cmem.o` 12864, `cprint.o` 2936, `chello.s` 7997, `clang` 82217968 (the last four were made without `-fno-ident`, so they carry `TheTaaJ clang version 18.1.3` in `.comment`) |

**2. The same object from the host's clang**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img cwords.o > /tmp/taaj-cwords.o
mkdir -p /tmp/e3/src && cp libc/programs/cwords.c /tmp/e3/src/
cd /tmp/e3
clang-18 --target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding \
    -fno-stack-protector -fno-pic -mno-red-zone -nostdlibinc -isystem ~/Workspace/TheTaaJ/libc/include \
    -fno-ident -c src/cwords.c -o /tmp/host-cwords.o
cmp /tmp/taaj-cwords.o /tmp/host-cwords.o && echo same
cd -
```

The host compiles from `src/cwords.c` too, in a directory of its own: an object holds the name of the file it was compiled from.

| Look for | Meaning |
|---|---|
| `same` | the object TheTaaJ wrote is the host's, byte for byte |

**3. AArch64**

Close any AArch64 QEMU first.

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1, typed at the terminal. `clang --version` says `Target: aarch64-unknown-taaj`. The `cwords.c` step takes about 40 s and the three-file step about 55 s. `clang` is 67332776 bytes, `cwords.o` (with `-fno-ident`) 6648, `chello.o` 3336, `cmem.o` 13720, `cprint.o` 3432.

**4. The tests by themselves**

```
make clang64-test
make aarch64-clang-test
make llvm64-test
```

| Look for | Meaning |
|---|---|
| `clang on TheTaaJ (x86_64) wrote, byte for byte as the host's: chello.o cmem.o cprint.o cfile.o cstdio.o cwords.o cask.o cprimes.o cnet.o cfloat.o clib.o other.o cwords.s cstdio.i` | every output equal to the host's (`aarch64` for the other machine) |
| `clang's headers not on the disk (names over 27 bytes): avx512vlvp2intersectintrin.h ...` | the ten headers TaajFS cannot name yet |
| `clang64-test passed`, `aarch64-clang-test passed`, `llvm64-test passed` | each machine; llvm64-test now checks `llc: error: llc: bad.ll` |

Automated: make test64

---

## 076 · lld on TheTaaJ: TheTaaJ builds its C programs

[The entry](journal/076-lld-on-taaj.md)

**1. The PC: compile, link, run**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

`make llvm-put` now makes a 256 MiB disk with E3's files and `ld.lld`, `lib/libc.a`, `user.ld` and `ldflags` (the build's linker options); the C programs are in `libc/programs`. Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly ld.lld --version` | `TheTaaJ LLD 18.1.3 (compatible with GNU linkers)` |
| `time with files clang @cflags -c libc/programs/chello.c` | `took` about 17 s |
| `time with files ld.lld @ldflags chello.o lib/libc.a -o chello2` | `took` about 8 s (once 30 s: see "What is still missing"), nothing else printed |
| `chello2 one two` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two`, then the lines `chello` always prints |
| `with files ld.lld chello.o -o x` | `ld.lld: error: undefined symbol: write` (and others), each with `>>> referenced by chello.c`, then `ld.lld exited with code 1` |
| `ls` | `chello2` 62456, the same as `chello` |

**2. The same program as the build's**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img chello2 > /tmp/chello2
cmp /tmp/chello2 build/c/chello && echo same
```

| Look for | Meaning |
|---|---|
| `same` | the program TheTaaJ compiled and linked is the one the build made |

**3. AArch64**

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1, but slow: the `clang` step takes about 30 s and the `ld.lld` step a minute or more (see "What is still missing"). `chello2` is 66568 bytes. Compare with `build/c-aarch64/chello`.

**4. The tests by themselves**

```
make clang64-test
make aarch64-clang-test
```

| Look for | Meaning |
|---|---|
| `ld.lld on TheTaaJ (x86_64) linked, byte for byte as the build's own programs: chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib` | all eleven programs equal to the build's (`aarch64` likewise) |
| `clang64-test passed`, `aarch64-clang-test passed` | each machine |

Automated: make test64

---

## 077 · A program starts a program: `clang x.c -o x` on TheTaaJ

[The entry](journal/077-a-program-starts-a-program.md)

**1. The PC: compile and link in one**

```
make llvm-put
make run64 RUN_DISK=build/llvm-disk.img
```

`make llvm-put` now also puts `/tmp` and `/lflags` on the disk, and makes the disk again if it is one from before E4 (smaller than 256 MiB). Close QEMU before running it.

| Type | You should see |
|---|---|
| `time with programs clang @cflags @lflags libc/programs/chello.c lib/libc.a -o chello3` | `[init] started /ld.lld for a program, from a file`, then `took` about 48 s, no errors |
| `chello3 one two` | `Hello from C`, `argc 3`, `argv[1] one`, `argv[2] two` |
| `with files clang @cflags @lflags libc/programs/chello.c lib/libc.a -o x` | `clang: error: unable to execute command: posix_spawn failed: not permitted`, `clang exited with code 1` |
| `with programs clang @cflags -nostdlib libc/programs/cwords.c -o y` | ld.lld's `undefined symbol` errors, then `clang: error: ld.lld command failed with exit code 1` |
| `ls tmp` | `empty`: clang removed the objects |

**2. The same program as the build's**

Close QEMU first. On the host:

```
tools/tfstool-rs/target/release/tfstool get build/llvm-disk.img chello3 > /tmp/chello3
cmp /tmp/chello3 build/c/chello && echo same
```

| Look for | Meaning |
|---|---|
| `same` | clang on TheTaaJ, starting ld.lld itself, made the build's program |

**3. AArch64**

```
make llvm-put-aarch64
make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
```

The same as part 1; the first step takes about 80 s. Compare with `build/c-aarch64/chello`.

**4. The tests by themselves**

```
make clang64-test
make aarch64-clang-test
```

| Look for | Meaning |
|---|---|
| `clang on TheTaaJ (x86_64) compiled and linked chello in one, starting ld.lld itself: the build's chello, byte for byte` | the one-step build, compared (`aarch64` likewise) |
| `clang64-test passed`, `aarch64-clang-test passed` | each machine |

Automated: make test64

---

## 078 · make on TheTaaJ: files that know when they changed

[The entry](journal/078-make-on-taaj.md)

**1. The PC: make builds the C programs**

```
make run-disk
make run64
```

`make run-disk` now puts `/make` on the disk, and `/src/libc/programs/Makefile` with the programs' sources. Close QEMU before running it. The disk must be one TheTaaJ mounts, for its files to get stamps: any run disk is.

| Type | You should see |
|---|---|
| `cd /src/libc/programs` | the prompt says `taaj:/src/libc/programs>` |
| `time with programs make` | `tcc chello.c -o chello` and ten more lines like it, then `took` about 25 s |
| `with programs make` | `make: nothing to be done for all` |
| `with files append cprimes.c /* changed */` | `cprimes.c: 2507 bytes` |
| `with programs make` | `tcc cprimes.c -o cprimes`, and nothing else |
| `./cprimes 100` | `cprimes: 25 primes below 100` (the one just built; a bare `cprimes` is the root's) |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1; the first make takes about 43 s.

**3. The tests by themselves**

```
make make64-test
make aarch64-make-test
```

| Look for | Meaning |
|---|---|
| `make on TheTaaJ (x86_64) built prog with tcc, then only what each change put out of date; -n, -t, a failure, a refusal; main.o, util.o and prog byte for byte the host's tcc's` | every step, and the comparison (`aarch64` likewise) |
| `make64-test passed`, `aarch64-make-test passed` | each machine |

Automated: make test64

---

## 079 · Pipes and redirections: a program's input and output, chosen by the program that starts it

[The entry](journal/079-pipes-and-redirections.md)

**1. The PC: a recipe with a redirection**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. The Makefile on the disk has a new target, `primes.txt`.

| Type | You should see |
|---|---|
| `cd /src/libc/programs` | the prompt says `taaj:/src/libc/programs>` |
| `with programs make primes.txt` | `tcc cprimes.c -o cprimes` (if it was not built yet), then `./cprimes 100 > primes.txt` |
| `with files cat primes.txt` | `cprimes: 25 primes below 100`: what cprimes printed, in the file |
| `with programs make primes.txt` | `make: 'primes.txt' is up to date` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make pipe64-test
make aarch64-pipe-test
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 58 checks of pipes, file actions, system, popen and the environment passed; make ran recipes with <, >, \| and 2>&1` | every check, and make's recipes (`aarch64` likewise) |
| `pipe64-test passed`, `aarch64-pipe-test passed` | each machine |

Automated: make test64

---

## 080 · Fast tests: TaajFS stops reading the whole disk at every write, programs load in big reads, and the long boots run side by side

[The entry](journal/080-fast-tests.md)

**1. The PC: a build and a big program**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `time source /src/build` | the C library and 11 programs built; `took` well under two minutes (it was over ten) |
| `time with readonly ls` | `took` a fraction of a second |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `test64: all passed`, in 14 to 16 minutes | every part |

Automated: make test64

---

## 081 · A small shell: `sh -c`, so that system, popen and make run commands as POSIX says

[The entry](journal/081-a-small-shell.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `with programs sh -c 'echo one && echo two; false \|\| echo three'` | `one`, `two`, `three` |
| `cd /src/libc/programs` | |
| `with programs sh -c 'echo c*.c'` | `cask.c cfile.c ... cwords.c`, the C files that start with c |
| `with programs sh -c 'N=$(cprimes 100); echo "said: $N"'` | `said: cprimes: 25 primes below 100` |
| `with programs sh -c 'cd /nowhere \|\| echo no such place'` | `sh: cd: /nowhere: no such directory`, then `no such place` |
| `with programs sh -c 'for f in *; do echo $f; done'` | `sh: for is not in this shell yet`, `sh exited with code 2` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 74 checks ... passed; sh ran a list and a script` | the shell, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64

---

## 082 · Control flow in sh: if, loops, case, functions, `${X%.c}` and `$((...))`

[The entry](journal/082-control-flow-in-sh.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `cd /src/libc/programs` | |
| `with programs sh -c 'for f in c*.c; do echo "C: $f"; done'` | `C: cask.c` ... `C: cwords.c`, one line for each |
| `with programs sh -c 'n=0; while [ $n -lt 3 ]; do n=$((n + 1)); echo "round $n"; done'` | `round 1`, `round 2`, `round 3` |
| `with programs sh -c 'f() { echo "f got $# arguments: $*"; }; f a "b c"'` | `f got 2 arguments: a b c` |
| `with programs sh -c 'for f in cask.c notes; do case $f in *.c) echo "$f: C";; *) echo "$f: other";; esac; done'` | `cask.c: C`, `notes: other` |
| `with programs sh -c 'f=/src/libc/programs/cask.c; echo ${f##*/} ${f%.c}'` | `cask.c /src/libc/programs/cask` |
| `with programs sh -c 'cprimes 20 \| while read a b c; do echo "[$c] [$a]"; done'` | `[primes below 20] [cprimes:]` |
| `with programs sh -c 'cat << END'` | `sh: a here-document, <<, is not in this shell yet`, `sh exited with code 2` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 82 checks ... passed; sh ran a list and a script with if, for, while read, case and a function` | the shell, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64

---

## 083 · The C library by make: TheTaaJ's C library and its programs built by make on TheTaaJ

[The entry](journal/083-the-c-library-by-make.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `cd /src/libc/c` | |
| `time with programs make` | each command (`cd . && tcc -c format.c`, ... `cd ../programs && tcc -L.. sh.c -o sh`), then `took ... s` (about 80 s) |
| `with programs make` | `make: nothing to be done for all` |
| `with programs ../programs/sh -c 'for p in ../programs/c*o; do echo "made: ${p##*/}"; done; echo "$((6 * 7))"'` | `made: chello`, `made: cstdio`, `42` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1 (the make takes about 130 s).

**3. The tests by themselves**

```
make libmake64-test aarch64-libmake-test
make test64
```

| Look for | Meaning |
|---|---|
| `make on TheTaaJ (x86_64) built the C library and its 12 programs ... all 76 files byte for byte the host's tcc's` | the library and programs, by make, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make libmake64-test aarch64-libmake-test

---

## 084 · SATA disks: an AHCI driver, so TheTaaJ's disk can be the kind every PC has

[The entry](journal/084-sata-disks.md)

**1. The PC, with the disk on SATA**

```
make run64 RUN_DISK_BUS=sata RUN_DISK=build/sata-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:03.0 AHCI controller: registers 0x...+0x1000, 132 KiB DMA at 0x..., MSI - handed to init` | the kernel found the controller and set up its MSI |
| `[init] started: registry display fs ahci nic net shell kbd; supervising` | init started `ahci`, not `blk` |
| `[ahci] AHCI 1.0 controller at 00:03.0, 6 ports; disk on port 0: QEMU HARDDISK, 32768 sectors (16 MiB), MSI interrupts` | the disk's IDENTIFY answer |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` |
| `write sata hello` | `sata: 6 bytes` |
| `cat sata` | `hello` |
| `wc sata` | `sata: 1 line, 1 word, 6 bytes` |
| `disk stall 3` | `disk: the driver will answer nothing for 3 s`; in the log, `[init] ahci has not answered for 2 s - stopping it`, `[init] ahci ended with code -1001 - restarting it`, and the `[ahci] AHCI 1.0 controller ...` line again |
| `disk read 0` | `0000  4a 41 46 53 04 00 00 00 ...  JAFS............` |
| `cat sata` | `hello` |

Close QEMU, then on the host:

| Type | You should see |
|---|---|
| `tools/tfstool-rs/target/release/tfstool get build/sata-disk.img sata` | `hello` |

**2. A q35: the chipset's own AHCI controller, under UEFI**

```
qemu-system-x86_64 -machine q35 -m 512M -bios /usr/share/ovmf/OVMF.fd -cdrom build/image64.iso -serial stdio \
    -drive file=build/sata-disk.img,format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=ide.3
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[pci] 00:1f.2 AHCI controller: ...` and `[ahci] AHCI 1.0 controller at 00:1f.2, 6 ports; disk on port 3: QEMU HARDDISK, 32768 sectors (16 MiB), MSI interrupts` (the boot CD is on port 2) |
| `cat sata` | `hello`, written in part 1 |
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |

**3. VMware (not tested by me)**

```
qemu-img convert -O vmdk build/sata-disk.img build/sata-disk.vmdk
```

A new VMware VM: firmware **UEFI** (the BIOS loader cannot read a SATA CD, see "What is still missing"), `build/image64.iso` as its CD, and `sata-disk.vmdk` as an existing disk on its **SATA** controller. Then the same steps as part 1. Its log goes to the VM's serial port, if one is added and sent to a file.

**4. The tests by themselves**

```
make ahci64-test
make test64
```

| Look for | Meaning |
|---|---|
| `ahci drove the disk on a PC's added AHCI controller (port 2, past a CD drive) and on a q35's own (port 3, past the boot CD): ...` | both machines |
| `test64: all passed` | every part |

Automated: make ahci64-test

---

## 085 · NVMe disks: a driver for the other disk a modern PC has

[The entry](journal/085-nvme-disks.md)

**1. The PC, with the disk on NVMe**

```
make run64 RUN_DISK_BUS=nvme RUN_DISK=build/nvme-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:03.0 NVMe controller: registers 0x...+0x2000, 148 KiB DMA at 0x..., MSI-X - handed to init` | the kernel found the controller; +0x2000 is BAR 0 cut short of its MSI-X table |
| `[init] started: registry display fs nvme nic net shell kbd; supervising` | init started `nvme`, not `blk` |
| `[nvme] NVMe 1.4 controller at 00:03.0: QEMU NVMe Ctrl; namespace 1, 32768 sectors (16 MiB), 128 KiB a command, MSI-X interrupts` | the controller's and the namespace's IDENTIFY answers |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` |
| `write nvme hello` | `nvme: 6 bytes` |
| `cat nvme` | `hello` |
| `wc nvme` | `nvme: 1 line, 1 word, 6 bytes` |
| `disk stall 3` | `disk: the driver will answer nothing for 3 s`; in the log, `[init] nvme has not answered for 2 s - stopping it`, `[init] nvme ended with code -1001 - restarting it`, and the `[nvme] NVMe 1.4 controller ...` line again |
| `disk read 0` | `0000  4a 41 46 53 04 00 00 00 00 10 00 00 00 10 00 00  JAFS............` |
| `cat nvme` | `hello` |

Close QEMU, then on the host:

| Type | You should see |
|---|---|
| `tools/tfstool-rs/target/release/tfstool get build/nvme-disk.img nvme` | `hello` |

**2. A q35 under UEFI, with a controller that moves 16 KiB a command**

```
qemu-system-x86_64 -machine q35 -m 512M -bios /usr/share/ovmf/OVMF.fd -cdrom build/image64.iso -serial stdio \
    -drive file=build/nvme-disk.img,format=raw,if=none,id=n0 -device nvme,serial=taaj0002,drive=n0,mdts=2
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[pci] 00:03.0 NVMe controller: registers 0xc000000000+0x2000, ...` (a 64-bit BAR above 4 GiB) and `[nvme] NVMe 1.4 controller at 00:03.0: QEMU NVMe Ctrl; namespace 1, 32768 sectors (16 MiB), 16 KiB a command, MSI-X interrupts` |
| `cat nvme` | `hello`, written in part 1 |
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` (each 256-sector read is eight commands) |

**3. The tests by themselves**

```
make nvme64-test
make test64
```

| Look for | Meaning |
|---|---|
| `nvme drove the disk on a PC's added NVMe controller (a request a command) and on a q35's (16 KiB a command): ...` | both machines |
| `test64: all passed` | every part |

Automated: make nvme64-test

---

## 086 · Power-off and reboot: ACPI in user space, and an orderly shutdown

[The entry](journal/086-poweroff-and-reboot.md)

**1. Switch off, with a file written just before**

```
make run64 RUN_DISK=build/power-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[acpi] power: PM1a control 0x604, reset 0xcf9 <- 0x6 (the PC's; the FADT names none), DSDT ... bytes at 0x... - handed to init` | the kernel read the FADT and handed on the controls |
| `[init] started: registry display fs blk nic net shell kbd power; supervising` | init started `power` |
| `[power] ACPI: off is sleep type 0/0 (\_S5); reset is 0x6 into port 0xcf9; ready` | `power` read `\_S5` from the DSDT |

In the QEMU window:

| Type | You should see |
|---|---|
| `write note before-off` | `note: 11 bytes` |
| `cat note` | `before-off` |
| `poweroff` | `Switching off...`; in the log, `[init] switching off: stopping the services`, then `[init] stopped shell`, `kbd`, `net`, `nic`, `fs`, `blk`, `display`, `registry`, and last `[power] switching off: sleep type 0/0 into PM1 control (ACPI S5)`. The QEMU window closes by itself, and `make` returns |

Then on the host:

| Type | You should see |
|---|---|
| `tools/tfstool-rs/target/release/tfstool get build/power-disk.img note` | `before-off` |

**2. Reboot**

```
make run64 RUN_DISK=build/power-disk.img
```

| Type | You should see |
|---|---|
| `reboot` | `Resetting...`; in the log, `[init] resetting: stopping the services`, the services stopped, `[power] resetting: 0x6 into port 0xcf9`, then `Hello from TheTaaJ!` again: the machine starts over, in the same window |
| `cat note` (once the shell is back) | `before-off` |

**3. A q35 under UEFI: the FADT's own reset register**

```
qemu-system-x86_64 -machine q35 -m 512M -bios /usr/share/ovmf/OVMF.fd -cdrom build/image64.iso -serial stdio \
    -drive file=build/power-disk.img,format=raw,if=none,id=d0 -device virtio-blk-pci,drive=d0
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[acpi] power: PM1a control 0x604, reset 0xcf9 <- 0xf, DSDT ... - handed to init` (no "the PC's": this FADT names its reset register) |
| `cat note` | `before-off` |
| `poweroff` | the services stopped, `[power] switching off: ...`, and QEMU ends by itself |

**4. AArch64: PSCI**

```
make run-aarch64 RUN_A64_DISK=build/aarch64/power-disk.img
```

(A new disk of its own; your `build/aarch64/taaj-disk.img` is not touched.)

| Look for / Type | You should see |
|---|---|
| (the log) | `[psci] power: SYSTEM_OFF and SYSTEM_RESET - handed to init` and `[power] PSCI: SYSTEM_OFF and SYSTEM_RESET; ready` |
| `write note arm` | `note: 4 bytes` |
| `poweroff` | `Switching off...`, the services stopped, `[power] switching off (PSCI)`, `[psci] SYSTEM_OFF`, and QEMU ends by itself (no Ctrl+A X needed) |

**5. The tests by themselves**

```
make power64-test
make aarch64-power-test
make test64
```

| Look for | Meaning |
|---|---|
| `poweroff and reboot worked on a PC and a q35: ...` | the five x86 machines |
| `poweroff and reboot worked on AArch64: ...` | the two AArch64 machines |
| `test64: all passed` | every part |

Automated: make power64-test

---

## 087 · Intel Ethernet: an e1000 driver, beside nic

[The entry](journal/087-e1000-intel-ethernet.md)

**1. The 82540EM (QEMU's `e1000`): polled**

```
make run64 RUN_NIC=e1000 RUN_DISK=build/e1000-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:04.0 Intel Ethernet card: registers 0xfeb80000+0x20000, 104 KiB DMA at 0x..., no interrupt (polled) - handed to init` | the kernel found the card and handed it on, without an interrupt (it has no MSI) |
| `[init] started: registry display fs blk e1000 net shell kbd power; supervising` | init chose `e1000`, not `nic` |
| `[e1000] Intel 82540EM at 00:04.0: address 52:54:00:12:34:56, link up, rings of 32 and 16, polled` | the driver reset the card and read its address |
| `[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2` | the network service asked the new driver who it is, as it asks `nic` |

In the QEMU window:

| Type | You should see |
|---|---|
| `netinfo` | `address  10.0.2.15/24`, `router   10.0.2.2`, `card     52:54:00:12:34:56` |
| `ping 10.0.2.2 3` | three lines `56 bytes from 10.0.2.2: seq=N ttl=255 time under 10 ms`, then `3 sent to 10.0.2.2, 3 answered` |
| `poweroff` | `Switching off...`; in the log `[init] stopped net`, then `[init] stopped e1000` (the card's driver, in nic's place), `fs`, `blk`, ..., and QEMU ends by itself |

**2. The 82574L (QEMU's `e1000e`): MSI**

```
make run64 RUN_NIC=e1000e RUN_DISK=build/e1000-disk.img
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[pci] 00:04.0 Intel Ethernet card: registers 0xfeb80000+0x20000, 104 KiB DMA at 0x..., MSI - handed to init` |
| (the log) | `[e1000] Intel 82574L at 00:04.0: address 52:54:00:12:34:56, link up, rings of 32 and 16, MSI interrupts` |
| `ping 10.0.2.2 3` | `3 sent to 10.0.2.2, 3 answered` |

**3. The virtio card, as before**

```
make run64 RUN_DISK=build/e1000-disk.img
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[nic] virtio network card at 00:04.0: ...` and `nic` (not e1000) in init's started line |
| `ping 10.0.2.2 3` | `3 sent to 10.0.2.2, 3 answered` |

**4. The tests by themselves**

```
make e1000-64-test
make net64-test
make test64
```

| Look for | Meaning |
|---|---|
| `wire: ok: 39 frames from 52:54:00:12:34:56: ...` (twice) | niccheck over the 82540EM and the 82574L, counted by the host's end of the wire |
| `host: ok: 5 checks; 3 connections from the machine served, 300029 bytes` | TCP over the 82574L |
| `e1000-64-test passed` | all four |
| `test64: all passed` | every part |

Automated: make e1000-64-test

---

## 088 · USB: an xHCI driver, with keyboards and mice

[The entry](journal/088-usb-xhci-keyboard-mouse.md)

**1. A USB keyboard and mouse**

```
make run64 RUN_USB=yes RUN_DISK=build/usb-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:05.0 USB controller: registers 0xfeb90000+0x3000, 276 KiB DMA at 0x..., MSI-X - handed to init` | the kernel found QEMU's xHCI controller and handed it on, cut short at its MSI-X table |
| `[init] started: registry display fs blk nic net shell kbd usb power; supervising` | init started `usb` |
| `[usb] xHCI 1.0 controller at 00:05.0: 8 ports, 0 scratchpad pages, MSI-X interrupts` | the driver reset and started the controller |
| `[usb] port 5: keyboard (0627:0001, high speed) ready` | the keyboard was enumerated and switched to the boot protocol |
| `[usb] port 6: mouse (0627:0001, high speed) ready` | the mouse too |

In the QEMU window (QEMU sends what you type to the keyboard that came last: the USB one):

| Do | You should see |
|---|---|
| type `echo hello from usb` and Enter | `hello from usb` |
| hold a letter key down for a second | the letter once, then repeating after half a second |
| click in the window (QEMU then gives it the mouse; Ctrl+Alt+G gives it back), move the mouse | the pointer moves |
| `poweroff` | in the log, `[init] stopped shell`, then `[init] stopped usb`, `kbd`, ..., and QEMU ends by itself |

**2. Plugging in and pulling out**

In the same machine, open QEMU's monitor with Ctrl+Alt+2 (Ctrl+Alt+1 goes back):

| Type in the monitor | In the log |
|---|---|
| `device_add usb-kbd,id=k2,bus=xhci.0` | `[usb] port 7: keyboard (0627:0001, high speed) ready` |
| `device_del k2` | `[usb] port 7: unplugged` |

**3. The tests by themselves**

```
make usb64-test
make test64
```

| Look for | Meaning |
|---|---|
| `USB worked: a keyboard typed (MSI-X, MSI under UEFI, polled), a held key repeated, a mouse clicked where it was, a disk was named, and a keyboard came and went` | the four machines of `usb64-test` |
| `test64: all passed` | every part |

---

## 089 · USB sticks: mass storage, and a stick as the system's disk

[The entry](journal/089-usb-sticks.md)

**1. A USB stick as the system's disk**

```
make run64 RUN_DISK_BUS=usb RUN_USB=yes RUN_DISK=build/usb-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched); here it is plugged in as a USB stick, with a USB keyboard and mouse beside it. In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:04.0 USB controller: registers 0xfebb0000+0x3000, 408 KiB DMA at 0x..., MSI-X - handed to init` | the controller, with room for the stick's buffers (408 KiB) |
| `[usb] port 1: disk "QEMU QEMU HARDDISK" (46f4:0001, SuperSpeed), 32768 sectors (16 MiB) ready` | the stick answered INQUIRY, TEST UNIT READY, READ CAPACITY |
| `[fs] ready: TaajFS on the disk, ... blocks free` | fs found the volume on the stick, through `usb` |
| no `[blk]`, `[ahci]` or `[nvme]` lines | there is no other disk |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` |
| `write hello on the stick`, then `cat hello` | `on the stick` |
| `disk stall 3` | `[init] usb has not answered for 2 s - stopping it`, a second `[usb] xHCI ...` line and the stick `ready` again; then `cat hello` still works |
| `poweroff` | `[init] stopped shell`, `kbd`, ..., `[init] stopped fs`, then `[init] stopped usb` (after fs: it is the disk's driver) |

After QEMU ends, on the host:

```
tools/tfstool-rs/target/release/tfstool get build/usb-disk.img hello
```

prints `on the stick`.

**2. No stick, no disk**

```
make run64 RUN_DISK_BUS=usb RUN_USB=yes RUN_DISK=build/usb-disk.img RUN_DISK_ARGS="-device qemu-xhci,id=xhci"
```

(`RUN_DISK` still names the disk the programs are copied to; `RUN_DISK_ARGS` leaves it out of the machine.) The log shows `[fs] no disk (nothing in the drive): files in memory only`.

**3. A stick beside another disk**

```
make run64 RUN_USB=yes RUN_USB_ARGS="-device qemu-xhci,id=xhci -drive if=none,id=s,format=raw,file=build/usb-disk.img -device usb-storage,bus=xhci.0,drive=s" RUN_DISK=build/other-disk.img
```

(`build/usb-disk.img` from 1 is the stick; a second new disk, `build/other-disk.img`, is the system's.) The stick is started (`[usb] port ...: disk "QEMU QEMU HARDDISK" ... ready`), but `[blk]` serves the system's disk.

2 and 3 are the `polled` and `pc` machines of `usb64-test`.

**4. The tests by themselves**

```
cd usb && cargo test
make usb64-test
make test64
```

| Look for | Meaning |
|---|---|
| `test result: ok. 24 passed` | the controller, enumeration and the stick against the simulator |
| `USB worked: ... and a stick was the system's disk - across a restart of its driver` | the five machines of `usb64-test` |
| `test64: all passed` | every part |

---

## 090 · Booting from a USB stick

[The entry](journal/090-booting-from-a-usb-stick.md)

**1. Booting from a USB stick, in QEMU**

```
make run64-stick
```

This makes `build/stick64.img`: the boot image, plus a 16 MiB TaajFS partition from `build/stick-disk.img` (made the first time, with the programs on it; your `build/taaj-disk.img` is not touched). It then boots it as a USB stick, with no CD and no other disk, and a USB keyboard and mouse. In the terminal's log:

| Look for | Meaning |
|---|---|
| `  loader         TheTaaJBoot (BIOS)` | SeaBIOS ran the stick's MBR, which loaded stage1, and stage2 read the kernel through int 13h |
| `[usb] port 1: disk "QEMU QEMU HARDDISK" (46f4:0001, SuperSpeed), 53848 sectors (26 MiB) ready` | the kernel's usb driver found the same stick |
| `[fs] TaajFS partition: sectors 20480 to 53247` | fs found partition 3 in the stick's MBR |
| `[fs] ready: TaajFS on the disk, ... blocks free` | and mounted it |

In the QEMU window (typing goes to the USB keyboard):

| Type | You should see |
|---|---|
| `ls` | the programs, `welcome`, ... |
| `write hello from the stick`, then `cat hello` | `from the stick` |
| `poweroff` | `[init] stopped fs`, then `[init] stopped usb` |

Then, on the host:

```
tools/tfstool-rs/target/release/tfstool get build/stick64.img hello
```

prints `from the stick`: tfstool finds the TaajFS partition in the image too.

**2. The same stick under UEFI**

```
make run64-stick-uefi
```

The log shows `  loader         TheTaaJBoot (UEFI)`, and the rest as in 1. (`make run64-stick-uefi` makes the image again from `build/stick-disk.img`; see What is still missing.)

**3. On a real USB stick** (all data on the stick is lost; check the device name twice)

```
make stick64
lsblk                                   # find the stick, say /dev/sdX
sudo dd if=build/stick64.img of=/dev/sdX bs=4M conv=fsync
```

Then boot the PC from the stick (its boot menu: often F12, F11 or Esc). This is R7. Nothing here has run on real hardware yet.

**4. The tests by themselves**

```
cd taajfs && cargo test
make stick64-test
make test64
```

| Look for | Meaning |
|---|---|
| `test result: ok. 64 passed` | TaajFS, including the partition table |
| `Booting from a USB stick worked: the MBR, stage1 and stage2 through int 13h (BIOS), the EFI system partition (UEFI), TaajFS in the stick's partition; and the loader read a SATA CD` | the three machines of `stick64-test` |
| `test64: all passed` | every part |

---

## 091 · First boot outside QEMU: VMware

[The entry](journal/091-first-boot-in-vmware.md)

**1. In VMware Workstation** (or any VMware)

```
make stick64
qemu-img convert -f raw -O vmdk build/stick64.img build/stick64.vmdk
```

Copy `build/stick64.vmdk` to the host. If VMware's shared folder is set up: `sudo vmhgfs-fuse .host:/ /mnt/hgfs -o allow_other`, then `cp build/stick64.vmdk /mnt/hgfs/<share>/`.

Make a new VM: "Other 64-bit", 1–2 GB of memory, an existing disk (`stick64.vmdk`) attached as **SATA** (not SCSI: TheTaaJ has no driver for VMware's SCSI controllers), BIOS firmware. To see the log, add a serial port that writes to a file.

| Look for | Meaning |
|---|---|
| `framebuffer    1024x768, 32 bpp, ...` in the boot info | the loader found the video BIOS's mode list (bug 1) |
| `[irq] APIC timer at 100 Hz (bus clock 4 MHz), TSC ... MHz: stated by the hypervisor (the PIT measured ...)` | the clock frequencies came from CPUID leaf 0x40000010 (bug 2) |
| `[selftest] timer: 10 ticks in ... ms (expected 100)` | about 100 on boots 3 and 4; 200–220 on others (see "Not explained"). The test passes up to 300 |
| `[ahci] ... disk on port 0: VMware Virtual SATA Hard Drive, 53848 sectors` | VMware's AHCI controller works with our driver |
| `[fs] TaajFS partition: sectors 20480 to 53247` | the stick's TaajFS partition |

On the screen, a terminal window opens. Type `ls`: 39 files and 4 directories.

**2. The same VM under UEFI, with USB**

Power the VM off, then:
- Settings → Options → Advanced: firmware **UEFI**, Secure Boot off;
- Settings → Add… → USB Controller → **USB 3.2** (1.1 and 2.0 are UHCI and EHCI, which TheTaaJ does not drive);
- in the VM's `.vmx` file: `keyboard.vusb.enable = "TRUE"` and `mouse.vusb.enable = "TRUE"`.

| Look for | Meaning |
|---|---|
| `  loader         TheTaaJBoot (UEFI)` | VMware's UEFI ran our loader from the EFI system partition |
| `[usb] xHCI 1.20 controller at 03:00.0: 8 ports, 0 scratchpad pages, MSI interrupts` | VMware's xHCI controller |
| `[usb] port 6: keyboard (0e0f:0006, full speed) ready` | VMware's USB keyboard: typing goes through it |
| `[usb] port 5: 0e0f:0003, ..., an input device without the boot protocol (class 0x03) - not driven` | the absolute mouse, which needs a report parser - **once**, not three times |
| `[usb] port 7: 0e0f:0002, ..., a hub (class 0x09) - not driven` (and port 8) | VMware's hubs, once each |

Type `ls`: the keys come through USB now.

**3. Four CPUs and power off**

Settings → Processors: 4 (or 2). Boot, then type `write hello kept across a reboot`, `cat hello`, `poweroff`.

| Look for | Meaning |
|---|---|
| `[smp] 4 CPUs awake: CPU 1, 2, 3 started through the page at 0x2000` | INIT/SIPI woke VMware's CPUs |
| `[selftest] wake-ups: ... ran on an idle one ... us later` | interrupts between CPUs; about 60 µs in VMware |
| `[init] stopped shell`, ..., `[init] stopped fs`, `[init] stopped ahci`, ... | services stopped in order, fs before the disk's driver |
| `[power] switching off: sleep type 0/0 into PM1 control (ACPI S5)` | and the VM switches itself off |

Boot again and type `cat hello`: `kept across a reboot`.

**4. In QEMU**

```
make run64
```

The log says `TSC ... MHz: measured against the PIT` (QEMU without KVM gives no timing leaf), `10 ticks in 99 ms` or `100 ms`, and the screen is 1024x768 as before.

**5. Undriven USB devices, in QEMU**

```
make run64 RUN_USB=yes RUN_USB_ARGS="-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0 -device usb-hub,bus=xhci.0"
```

The log names the tablet (`an input device without the boot protocol`) and the hub once each, and the USB keyboard types.

**6. The tests by themselves**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu hypervisor
make boot64-test stick64-test usb64-test
make test64
```

| Look for | Meaning |
|---|---|
| `test result: ok. 3 passed` | CPUID answers decoded: VMware believed, Hyper-V and nonsense refused |
| `boot64-test passed`, `stick64-test passed` | the VESA change still finds QEMU's mode |
| `usb64-test passed` | among the rest, the tablet and the hub are named once each |
| `test64: all passed` | every part |

---

## 092 · Descriptors for a child: inheritance, close-on-exec, here-documents and `exec`

[The entry](journal/092-descriptors-for-a-child.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. Type each line in TheTaaJ's shell, exactly as written (the `\$` in the first line is meant).

| Type | You should see |
|---|---|
| `with programs sh -c 'echo "cat <<END" > h.sh; echo "six sevens: \$((6 * 7))" >> h.sh; echo END >> h.sh'` | nothing; `sh` exits with code 0. h.sh now holds a here-document |
| `with programs sh h.sh` | `six sevens: 42` |
| `with programs sh -c 'exec 3< h.sh; read -r a <&3; read -r b <&3; echo "[$a] [$b]"'` | `[cat <<END] [six sevens: $((6 * 7))]`: two lines read through descriptor 3 |
| `with programs sh -c '{ echo first; cprimes 10; echo last; } > g.txt; cat g.txt'` | `first`, `cprimes: 4 primes below 10`, `last`, in that order |
| `with programs sh -c 'n=0; while read l; do n=$((n + 1)); done < h.sh; echo "$n lines"'` | `3 lines`: the loop's variable kept |
| `with programs sh -c 'exec cprimes 10; echo never'` | `cprimes: 4 primes below 10`, and no `never` |
| `with programs sh -c 'a &'` | `sh: running a command in the background, &, is not in this shell yet`, `sh exited with code 2` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 102 checks ... passed; sh ran a list and a script with if, for, while read, case, a function, exec 3< and a here-document` | inheritance and the shell, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64

---

## 093 · Faster writes: a commit per megabyte, not per kilobyte

[The entry](journal/093-faster-writes.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. `src/tcc/tccgen.c` (273,655 bytes) is one of the TCC sources the disk carries.

| Type | You should see |
|---|---|
| `time with files cp src/tcc/tccgen.c copy.c` | `copy.c: 273655 bytes`, then `took 0.xx s`: well under a second (0.17 s in my scripted boot) |
| `with files cat src/tcc/tccgen.c > cat.c` | nothing: cat's output, a stream, went into cat.c, gathered into a few commits. (Not under `time`: TheTaaJ's shell sends time's "took" line into the redirected file too) |
| `with files wc copy.c` and `with files wc cat.c` | `9197 lines, 32363 words, 273655 bytes` for each, the same as `with files wc src/tcc/tccgen.c` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 108 checks ... passed` | big writes, stdio and streams, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64

---

## 094 · Long names: up to 64 bytes on TaajFS

[The entry](journal/094-long-names.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. The first long name makes your disk version 5. The Rust `tfstool`, which `make` uses, reads it; the C one never read a disk with directories anyway.

| Type | You should see |
|---|---|
| `with files mkdir a-directory-whose-name-is-longer-than-27` | nothing: the directory's name is 40 bytes |
| `with files cp src/tcc/tccgen.c a-directory-whose-name-is-longer-than-27/the-code-generator-of-tcc-under-a-long-name.c` | `...under-a-long-name.c: 273655 bytes` |
| `with files ls a-directory-whose-name-is-longer-than-27` | `273655  the-code-generator-of-tcc-under-a-long-name.c`, the whole name, then `1 file, 273655 bytes` |
| `with files wc a-directory-whose-name-is-longer-than-27/the-code-generator-of-tcc-under-a-long-name.c` | `9197 lines, 32363 words, 273655 bytes` |
| `with files write this-name-is-sixty-five-bytes-long-which-is-one-more-than-allowed x` | `write: this-name-...-than-allowed: not a valid name`: 65 bytes is one too many |
| `with files mv a-directory-whose-name-is-longer-than-27/the-code-generator-of-tcc-under-a-long-name.c short.c` | nothing; then `with files ls a-directory-whose-name-is-longer-than-27` says `empty` |

After closing QEMU, on the host: `tools/tfstool-rs/target/release/tfstool dump build/taaj-disk.img | grep version` says `format version 5`.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 119 checks ... names of up to 64 bytes on the disk` | long names from C, from sh, and read back by the host's tool; the same for aarch64 |
| `clang64-test passed`, with no "headers not on the disk" line | every clang header is on its disk |
| `test64: all passed` | every part |

Automated: make test64

---

## 095 · Threads in a program: pthreads for real, on every CPU

[The entry](journal/095-threads.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `threads` and `threadcheck` on the disk.

| Type | You should see |
|---|---|
| `with files threads` | after a few seconds, `threads: 30 checks, all passed` |
| `threads speed` | `threads: the same work: 1 thread 0.xx s, 4 threads 0.xx s (N times as fast)`, with N around 3 to 4 on 4 CPUs |
| `threads exit` | `threads: exit(7) from a thread, with others waiting and one spinning`, then `threads exited with code 7` |
| `threadcheck` | `threadcheck: 4 threads, 20000 additions and as many blocks, all passed` |
| `cxxlib` | `c++: 66 checks, all passed` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make thread64-test
make aarch64-thread-test
make test64
```

| Look for | Meaning |
|---|---|
| `threads on TheTaaJ (x86_64): 30 checks of pthreads on 4 CPUs passed; ...` | every check, exit from a thread, the last thread, Rust threads; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make thread64-test, make aarch64-thread-test, make test64

---

## 096 · A bigger machine: memory taken as it is touched, a limit for each program, and rustc measured

[The entry](journal/096-bigger-machine.md)

**1. The PC**

```
make run-disk
make run64 RUN_MEMORY=4G
```

Close QEMU before `make run-disk`. It puts `bigmem` on the disk. `RUN_MEMORY` is new; it defaults to 512M as before.

| Type | You should see |
|---|---|
| `with files bigmem` | `bigmem: limit 3836 MiB, 0 MiB used at the start`, then `bigmem: 60 threads, 480 MiB of stacks, took N KiB` (a few hundred) and `bigmem: 23 checks, all passed` |
| `bigmem big` | after a few seconds, `bigmem: touched 3072 MiB in N s; it took 3072 MiB; every page kept its word: yes; after munmap: 0 MiB` |
| `with programs bigmem limit` | `bigmem: limit lowered to 64 MiB, raising it refused`, `bigmem: my limit is 64 MiB`, `bigmem: 256 MiB mapped: yes; touching it all`, then `bigmem was killed by the kernel` and, on the serial console, `killed: out of memory - 64 MiB used, its limit is 64 MiB` |
| `bigmem overflow` | `bigmem: a thread with 1 MiB of stack goes 2 MiB deep`, then `bigmem was killed by the kernel` (a page fault under its stack) |
| `bigmem shoot` | `bigmem: unmapping the page another thread is reading`, then `bigmem was killed by the kernel` at once |
| `bigmem report` | `bigmem: my limit is 3836 MiB` |
| `threads speed` | as in entry 095: threads still work, now on 8 MiB stacks |

With `make run64` (512 MB), `bigmem` says a limit of 476 MiB, and `bigmem big` touches what the limit allows.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64 RUN_MEMORY=4G
```

The same as part 1, the limit 3837 MiB.

**3. The tests by themselves**

```
make bigmem64-test
make aarch64-bigmem-test
make test64
```

| Look for | Meaning |
|---|---|
| `a bigger machine (x86_64, 4 GB): 23 checks of reserved memory, mmap and big stacks passed; ...` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

**4. R0, on the host (optional, takes about 20 minutes and 15 GB)**: `build/r0/` holds the rustc source and the scripts, outside git. With cmake and ninja on the PATH, `./x.py build --stage 1 compiler/rustc` in `build/r0/rustc-1.100.0-src`; then `build/r0/measure-all.sh` builds our crates with it and writes each crate's memory and time to `build/r0/measure-*.log`.

Automated: make bigmem64-test, make aarch64-bigmem-test, make test64

---

## 097 · The core basics: the date, a finer clock, signals, and writes that reach the disk

[The entry](journal/097-core-basics.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `basics` on the disk.

| Type | You should see |
|---|---|
| (at boot, in the terminal) | `[clock] the CMOS clock says <today's date and time, UTC>; time is counted from here at N Hz` |
| `basics date` | today's date and time in UTC, e.g. `Wednesday 25 November 2026, 15:32:46 UTC; up 4.333 s` |
| `with programs basics` | `basics: the date is ...`, a few seconds of alarms, then `basics: 33 checks, all passed`. In the terminal, four children: `ended by signal 15 (SIGTERM)`, `exited with code 7`, `ended by signal 6 (SIGABRT)`, `killed: page fault` |
| `basics catch`, then Ctrl+C | `basics: waiting for a line (Ctrl+C to interrupt)`, then `basics: caught SIGINT while waiting for a line - tidying up` |
| `forever`, then Ctrl+C | `forever was stopped`; in the terminal `ended by signal 2 (SIGINT)` |
| `with files basics speed` | `basics: 4 MiB in 64 writes took N ms` (a few hundred) |
| `clib assert` | `libc/programs/clib.c:461: main: assertion failed: number == 2`, then `clib was ended by signal 6 (SIGABRT)` (it was `exited with code 134`) |

In the terminal, `[blk] virtio disk at ...: ..., a write cache (flushed on commit), ...`.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1. At boot: `[clock] the PL031 says ...`, at 62500000 Hz.

**3. The tests by themselves**

```
make basics64-test
make aarch64-basics-test
make test64
```

| Look for | Meaning |
|---|---|
| `the core basics (x86_64): the date read at boot is the host's (N s apart); 33 checks of the clock, signals and fsync passed; Ctrl+C reached a waiting program's handler; 4 MiB written with flushes` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

Automated: make basics64-test, make aarch64-basics-test, make test64

---

## 098 · Two machines: The-Jat and MaK, and what the second one found at once

[The entry](journal/098-two-machines.md)

On The-Jat (no KVM):

```
make run64
```

| Look for | You should see |
|---|---|
| The QEMU command printed by make | `qemu-system-x86_64 -accel tcg ...` |
| The system | It boots as before |

On MaK (KVM), the same command prints `-accel kvm`, and the boot to the shell takes a second or two instead of several. To force emulation there, use `make run64 RUN_ACCEL=tcg`.

```
make libc-test
```

| Look for | Meaning |
|---|---|
| `scan: N checks, all the same as the host's sscanf (a bare 0x: as C says)` | `sscanf` agrees with the host everywhere, and with C on a bare `0x` |

```
make test64
```

| Look for | Meaning |
|---|---|
| `test64: all passed` | Every part, with the bootloader and `check-boot.sh` fixes |

The work split itself is in `docs/who-does-what.md`.

Automated: make libc-test, make test64

---

## 099 · Std's ground, part 1: random numbers, real thread-local storage, signal stacks, SIGCHLD

[The entry](journal/099-std-ground-1.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `ground` on the disk.

| Type | You should see |
|---|---|
| (at boot, in the terminal) | `[random] seeded from 256 timings (no random instruction on this CPU)` |
| `ground random` | `ground: random ` and 32 hex digits, different every time |
| `with programs ground` | after a few seconds `ground: caught the stack overflow on the signal stack`, then `ground: 51 checks, all passed`; in the terminal, its children `exited with code 5`, `exited with code 0`, `ended by signal 15 (SIGTERM)`, `exited with code 3` and `killed: page fault`, then `"ground" exited with code 0` |
| `with files threads` | `threads: 30 checks, all passed`, as before, now with real thread-local storage |

To see the CPU's random instruction used, start QEMU yourself with `-cpu max` added to the `make run64` command line. The kernel then says `[random] seeded from 8 numbers from RDSEED and 256 timings`.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1. With `-cpu max` instead of `cortex-a72`, the kernel says `... from RNDR ...`.

**3. The tests by themselves**

```
make ground64-test
make aarch64-ground-test
make test64
```

| Look for | Meaning |
|---|---|
| `std's ground (x86_64): the kernel's random numbers seeded with and without a random instruction, different on two boots; 51 checks of getrandom, thread-local storage, signal stacks and SIGCHLD passed; a stack overflow caught on the signal stack` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

Automated: make ground64-test, make aarch64-ground-test, make test64

---

## 100 · Std's ground, part 2: poll and select, dl_iterate_phdr, pthread attributes, exec with PATH, directory descriptors, zlib

[The entry](journal/100-std-ground-2.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `ground2` on the disk.

| Type | You should see |
|---|---|
| `with programs ground2` | after a few seconds `ground2: 87 checks, all passed`; in the terminal, its children `exited with code 7` (twice: the program exec started, then the one that exec'd), `ended by signal 15 (SIGTERM)` then `exited with code -1115` (the go-between passing the signal's code on), `exited with code 42` twice, `exited with code 3` twice, `exited with code 4`, and at the end `"ground2" exited with code 0` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make ground2-64-test
make aarch64-ground2-test
make test64
```

| Look for | Meaning |
|---|---|
| `std's ground, part 2 (x86_64): 87 checks of poll, select, dl_iterate_phdr, getauxval, dlfcn, pthread attributes, exec with PATH, descriptors through exec, argv[0], realpath and zlib passed` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

Automated: make ground2-64-test, make aarch64-ground2-test, make test64

---

## 101 · TaajFS version 6: file times, volumes and files over 256 MiB, a rename that replaces, and programs found in /bin

[The entry](journal/101-taajfs-6.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `write t one` then `ls -l` | `t` with its size and today's date and time (UTC) |
| `write u two` then `ls -l` | `u` a little later than `t` |
| `mv u t` then `ls` | only `t`: the rename replaced it; `cat t` says `two` |
| `mkdir d` then `mv d t` | `mv: d to t: not a directory`: a directory does not replace a file |
| `mv t d` then `ls d` | `t` went into `d` (a directory as TO means "into it", as before) |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make times64-test
make aarch64-times-test
make test64
```

---

## 102 · Memory for big programs, part 1: mprotect for real, memory given back, a heap that returns it, and copies that cannot race an unmap

[The entry](journal/102-pages.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `pages` on the disk.

| Type | You should see |
|---|---|
| `with programs pages` | after some seconds `pages: N copies went in, M were refused` (the two add up to 20000) and `pages: 54 checks, all passed`, then `"pages" exited with code 0` |
| `mem` before and after | the memory in use back where it was: what pages took, it gave back |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make pages64-test
make aarch64-pages-test
make test64
```

---

## 103 · Memory for big programs, part 2: shared file mappings that are written back, pread and pwrite, and why programs are not loaded page by page yet

[The entry](journal/103-shared-mappings.md)

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts the new `pages` on the disk.

| Type | You should see |
|---|---|
| `with programs pages` | `pages: N copies went in, M were refused`, then `pages: 77 checks, all passed`; between them a second `pages` started "for a program" exits with code 0 (the child that writes through a mapping and exits) |
| `ls` afterwards | no `p.file` or `p.exit`: the checks clean up after themselves |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make pages64-test
make aarch64-pages-test
make test64
```

---

## 104 · Tests at full speed: KVM for the test scripts, and the boot test's two typing races

[The entry](journal/104-tests-at-full-speed.md)

On MaK (KVM):

```
make boot64-test
```

| Look for | Meaning |
|---|---|
| `four copies of cprimes at once: took 8.33 s; the 4 CPUs worked for 27.78 s in all` (times vary) | The four copies ran at once under KVM; worked is well over 1.5 times took |
| `boot64-test passed` | Every expected line was found, the cfloat lines wherever they were |

On either machine:

```
make --eval='show-accel: ; @echo $$TAAJ_ACCEL' show-accel
make test64
make test64 TAAJ_ACCEL=tcg
```

| Look for | Meaning |
|---|---|
| `kvm` on MaK, `tcg` on The-Jat | The accelerator the tests use |
| `test64: all passed`, twice | Every part, at full speed and emulated |

---

## 105 · The race hunt: twenty rounds under KVM, a SIGCHLD that came late, and tests that timed the host

[The entry](journal/105-the-race-hunt.md)

On MaK (KVM):

```
make ground64-test
```

| Look for | Meaning |
|---|---|
| `ground: 52 checks, all passed` | Every SIGCHLD of the hundred children was handled before its `waitid` returned |

On either machine:

```
make test64
make test64 TAAJ_ACCEL=tcg
```

| Look for | Meaning |
|---|---|
| `test64: all passed`, twice | Every part, at full speed and emulated |

To hunt for races (run `make test64-shared` first; each round takes about seven minutes on MaK):

```
tools/race-hunt.sh 20
```

| Look for in `build/race-hunt/summary.txt` | Meaning |
|---|---|
| `round N: exit 0, ... failed: none` | A clean round |
| `failed: <part>` | That part failed; `build/race-hunt/round-N/` holds every file the round wrote |

---

## 106 · How we work: pull requests, one reviewer, and the one who merges

[The entry](journal/106-how-we-work.md)

With the reviewer running (in WSL on the laptop):

```
gh pr list --repo TheJaat/TheTaaJ
```

| Look for | Meaning |
|---|---|
| A pull request labelled `review:pass` or `review:changes`, with a comment headed **Review** | The reviewer has looked at it |
| After `merge-ok`: the pull request merged, its branch gone, its journal entry numbered in `docs/journal/README.md` | The reviewer merged it as `how-we-work.md` says |

---

## 107 · Back to branches: no pull requests, no reviewer

[The entry](journal/107-back-to-branches.md)

From the root of TheTaaJ:

```
ls docs/how-we-work.md docs/review.md
grep -c "pull request\|reviewer" docs/who-does-what.md
```

| Look for | Meaning |
|---|---|
| `No such file or directory`, twice | The pull-request flow's files are gone |
| `0` | `who-does-what.md`'s rules no longer mention pull requests or a reviewer |
