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
