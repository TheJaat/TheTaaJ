# A gdb stub: gdb debugging the kernel through a serial line

*Journal entry #NNN · K198, K142's third part (Era 22). Written on TheJat-WSL/ci, by day, with quick checks only. QEMU has its own gdb server, so the kernel could always be debugged in QEMU. A real PC has nothing of the kind: when the kernel goes wrong there, its log is all there is. Now a kernel built for it speaks gdb's own protocol on the PC's second serial port.*

## A session

From `gdb64-test`: the real gdb, in batch mode, with the kernel's ELF file, connected through QEMU's COM2, which the test makes a TCP port:

```
rip            0xffffffff8013e0c6  0xffffffff8013e0c6 <kernel64::arch::x86_64::gdb::wait_at_boot+294>
rsp            0xffffffff802ae5f0  0xffffffff802ae5f0 <kernel64::arch::x86_64::entry::BOOT_STACK+54768>
#0  kernel64::arch::x86_64::gdb::wait_at_boot () at src/console.rs:237
#1  0xffffffff8014085a in kernel64::kernel_main (boot_info_address=77824) at src/main.rs:120
#2  0xffffffff8010000e in _start ()
0xffffffff802ae5f0 <...BOOT_STACK+54768>:	0xffff800000013000	0x0000000100000000
Breakpoint 1 at 0xffffffff80129d70: file src/selftest.rs, line 29.

Breakpoint 1, kernel64::selftest::run () at src/selftest.rs:29
29	pub fn run() {
rip            0xffffffff80129d70  0xffffffff80129d70 <kernel64::selftest::run>
#0  kernel64::selftest::run () at src/selftest.rs:29
#1  0xffffffff8014085f in kernel64::kernel_main (boot_info_address=77824) at src/main.rs:121
#2  0xffffffff8010000e in _start ()
[Inferior 1 (Remote target) detached]
```

Then the kernel goes on, and its boot finishes as if nobody had looked.

## How

- **`kernel64::gdbstub`** (the kernel's library, host-tested): the protocol.
  - Packets: `$data#checksum`, with `+` and `-` answers, and Ctrl+C.
  - Registers in gdb's x86-64 layout: the 16 general registers, rip, eflags, and cs ss ds es fs gs; `g`, `G`, `p`.
  - Memory: `m` and `M`, with `E14` where there is none.
  - Software breakpoints, `Z0` and `z0`, with what they hid shown in their place.
  - `c`, `s`, `D` and `k`, and the queries gdb sends when it connects. Anything else gets the empty answer, "not supported", which gdb lives without.
- **`arch/x86_64/gdb.rs`**, only in a kernel built with the `gdb-stub` feature:
  - **COM2**, at `0x2F8`, polled, as COM1 is for the log.
  - **Memory by walking the page tables by hand**, from CR3, with no locks: an address with nothing there answers `E14` instead of faulting inside the stub. A breakpoint goes into the kernel's code, which is read-only where it runs, through the direct map.
  - **`wait_at_boot`**, before the self-tests, says `[gdb] waiting for gdb on COM2` and stops in an `int3` until gdb lets it go.
  - **The exception handler** gives the kernel's own breakpoints and single steps (#DB, after `s` sets the trap flag) to the stub while gdb is attached. At a breakpoint of the stub's, rip is put back on it, as gdb expects. After `D`, they are the kernel's again: the self-test's own breakpoint is passed over, as always.
  - **A panic** with gdb attached stops for gdb after printing its report.
- **Normal builds are unchanged:** the stub is not even compiled into them.

## In other lanes' files

A line or a few each:
- `arch/x86_64/idt.rs`: the stub's breakpoints and steps;
- `arch/x86_64/mod.rs`, `arch/mod.rs`: the module;
- `main.rs`: the wait at boot, and the panic;
- `Cargo.toml`: the feature.

## Checked

- **`gdb64-test`, new:**
  - it builds the kernel with `gdb-stub` and boots it with QEMU's COM2 on a free TCP port;
  - gdb connects with the kernel's ELF, reads rip and rsp, walks the stack to `kernel_main`, reads stack memory;
  - it sets a breakpoint at `selftest::run` (its address from `nm`), continues, stops there with rip on it, and detaches;
  - the boot then reaches `[kernel] idle`.
- **Host tests:** `gdbstub`, five:
  - packets framed and received, a damaged one, Ctrl+C;
  - registers out and back in gdb's layout, one by one too;
  - memory read and written, with `E14` where there is none;
  - a breakpoint hidden under its byte, put back on detach, and rip moved back onto it;
  - what gdb asks first, and the empty answer for the rest.
- **Quick checks:**
  - `kernel64-test`, `coverage` (`kernel/kernel64` went from 91.9% to 92.2%) and `unsafe-check`;
  - clippy's unsafe check on the `gdb-stub` build;
  - `boot64-test`, `aarch64-test`, `crash64-test`, `debug64-test`, `fault64-page-test`.
- The full `make test64` is for the night.

## Not yet

- **Real hardware is not tested here.** No real PC with a cable has run it. Under QEMU, COM2 is the same 16550 UART a PC has.
- **One CPU stops; the others run on.** gdb sees one thread.
- **No hardware breakpoints or watchpoints** (`Z1` to `Z4`), and Ctrl+C reaches the kernel only when it is stopped: the stub polls.
- **AArch64** has no stub.

## Testing note

| Do | You should see |
|---|---|
| `make gdb64-test` | `gdb64-test passed`; gdb's session in `build/gdb64.log.gdb` |
| A `gdb-stub` kernel in QEMU with `-serial tcp:127.0.0.1:1234,server=on,wait=off` as the second serial port; `gdb kernel64 -ex 'target remote :1234'` | Stopped in `wait_at_boot`; `bt`, `break`, `continue`, `detach` |

Automated: `make gdb64-test` (part of `make test64`).
