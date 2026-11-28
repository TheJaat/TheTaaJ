# The crash record: a kernel's last words, found by the next boot

*Journal entry #164 · K188, K137's second half (Era 22). Written on TheJat-WSL/ci, by day, with quick checks only; it stands on K136, the system log.*

## The problem

When the kernel stops, it says why on the serial line and on the screen, and halts. It stops on a panic, a CPU exception it cannot recover from, or init ending. Since entry 122, what it says includes a backtrace with the kernel's own symbols. But unless someone was reading at that moment, it is gone at the next start. K136's system log does not help: it is written by `logd`, through `fs`, through a disk driver, all of them programs, and nothing runs any more. The kernel has no disk driver of its own, and must not have one just for this.

What a reset leaves alone is memory. So, like Linux's `pstore`/`ramoops`, the kernel also writes its last words into an area of memory that it never hands out, at the same place every boot, and the next kernel looks there first.

## How

- **`kernel64::crashrecord`** (the kernel's library, host-tested): the area's format, with a 24-byte header holding the magic, the length, an FNV-1a checksum and the time the kernel began to stop, then the text.
  - Every write brings the checksum, then the length, up to date. The record is whole at every moment: if the kernel stops writing halfway (a second fault), what was written is still found.
  - A record that does not add up is not one. That covers memory firmware wrote over, or memory that held something else after a power cycle.
  - `place()` picks the area: the top 64 KiB of the highest usable region below 4 GiB on a PC, or of memory on AArch64. It keeps clear of what is in use: below 1 MiB, the ramdisk, the boot info and its map, the screen, the kernel, regions that are not usable, and on AArch64 the device tree's reserved ranges (firmware's) and the tree itself. Given the same machine, it is the same place every boot.
- **`main.rs`**: before memory management starts, the area becomes one more region, *reserved*. Where regions overlap, the one that is not usable wins, so the allocator never hands it out. `mm.rs` did not change. On AArch64 it is also put in the direct map; on a PC, everything below 4 GiB already is.
- **`logtap.rs`** (K136's):
  - At `start()`, before the self-tests, it reads the area through the direct map. If there is a record, it goes into the system log's ring as a new kind of record, `abi::log::kind::CRASH`, and the serial line says `[kernel] the boot before this one stopped: "<its first line>" - its last words (N bytes) are in the system log (log -s crash)`. Then the area is cleared.
  - `last_words()`: while the kernel is stopping, `console.rs` hands it what it writes, in place of the ring. It takes no lock, because a lock may never be let go. On its first call, it starts the record with the time.
- **Every way of stopping is caught the same way.** The panic handler, the CPU exceptions on both architectures and init's end (`stop_the_system`) all call `console::break_lock()` first, which is what puts the console into its "stopping" state. No halt path needed changing.
- **`syslog`** and **`logd`**: records of the new kind are lines whose writer is `crash`, every one `crit`, with the time the first kernel began to stop. The panic's blank lines are left out. So `log -s crash` shows the last boot's last words, and `log -l crit` shows them among anything else that bad.

## The test: `crash64-test`

A kernel built with a new test feature, `fault-panic` (one of the `fault-*` features: never in a normal build):

1. The first boot finds no record. Right after starting init, it panics on purpose (`a deliberate panic, for the crash record (fault-panic)`), says so with its backtrace, and resets the machine through `0xCF9`. QEMU is told to reset instead of quitting (`-action reboot=reset`), so the memory stays.
2. The second boot, in the same QEMU, finds the record and says so. It goes on as usual: init, `logd`, the shell.
3. `log -s crash` shows the panic line, its message, the thread, the backtrace, down to `[test] resetting...`. Every line is `crit`, with the first boot's time.

It boots with one CPU, so init cannot get as far as the keyboard before the first boot panics. What the second boot's log showed:

```
2026-11-28 07:07:19.877 crash      crit  *** KERNEL PANIC: panicked at src/main.rs:950:9:
2026-11-28 07:07:19.877 crash      crit  a deliberate panic, for the crash record (fault-panic)
2026-11-28 07:07:19.877 crash      crit      in thread "boot" (1)
2026-11-28 07:07:19.877 crash      crit      backtrace:
2026-11-28 07:07:19.877 crash      crit      #0  0xffffffff8014b5dc  core::panicking::panic_fmt+0x2c
2026-11-28 07:07:19.877 crash      crit      #1  0xffffffff8012f54a  kernel64::run_fault_test+0x4a
...
```

## Found on the way

- **`aarch64-test` checks the highest free frame exactly.** It expects the top of memory, and the crash area now takes the top 64 KiB. The test's own expectation moved down by 64 KiB.
- **The first `place()` looked only at usable memory.** That failure showed the area could have landed on what firmware keeps at the top of some AArch64 boards (the test's `KEPT` megabytes), or on a ramdisk a loader put at the top. Now `place()` is given everything in use and goes below it. Two host tests cover that.
- **K136's `boot64-test` expectations.** The boot test lists the disk's root, and K136 added `log` and `logd`'s `/var`. Fixed on `k136-log-service`, with a note in its entry.

## What it does not do

- **A power cycle clears memory**, and then there is nothing to find. The record is for a warm reboot, which is what someone does after a panic, and what a watchdog would do.
- **Firmware may still write there.** UEFI firmware allocates from the top of memory during the next boot, before the kernel runs, so under UEFI a record may be written over. The checksum means a damaged record is ignored, never shown wrong. On QEMU's PC with its BIOS the area came through a reset untouched (the test); on AArch64 the record is made and read the same way, but no test resets that machine yet. A place in the firmware's own persistent store (UEFI variables) would survive both; that is for later.
- **Nothing resets the machine after a panic yet**, except the test feature. A "reset N seconds after a panic" setting is K188's natural next step, once there is a kernel command line.

## Checked

- **Host tests:**
  - `crashrecord`, five tests: found at every moment, the first words kept when full, records that do not add up, the place at the top, the place clear of what is in use;
  - `syslog`'s crash lines.
- **`kernel64-test`, `coverage`, `unsafe-check`:** `kernel/kernel64` is at 91.9%, against its floor of 91.
- **Boot tests:**
  - `crash64-test` (new);
  - `boot64-test`: memory setup changed, and K136's listing fix;
  - `aarch64-test`: the region and the direct map on AArch64;
  - `log64-test`;
  - `fault64-page-test`: a fault part still reports and stops as before.
- The full `make test64` is for the night.

## Testing note

| Do | You should see |
|---|---|
| `make crash64-test` | `crash64-test passed`; in `build/crash64.log`, the panic, `[test] resetting...`, then `[kernel] the boot before this one stopped: ...` and the `crash` lines from `log -s crash` |
| After any panic, a warm reset (QEMU: `system_reset` in the monitor), then `log -s crash` | The last boot's last words, `crit` |

Automated: `make crash64-test` (part of `make test64`).
