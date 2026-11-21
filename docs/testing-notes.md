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
