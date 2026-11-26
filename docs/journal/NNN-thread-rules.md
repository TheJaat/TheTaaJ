# Rules 1 and 3 of threads.rs, tested - and a panic that a 20-CPU machine swallowed

*Journal entry #NNN · a test change, and a kernel fix it found (J52). Written on TheJat-WSL/ci. Entry 046 broke each of the four rules of `threads.rs` on purpose: with rule 1 or rule 3 broken, every boot still passed - "held up by the argument in this entry, not by a test". Now each fails at once: rule 1 at the first switch of every boot, rule 3 in the threads self-test. Checking that brought out a kernel bug: on a machine with more CPUs than the kernel uses, a kernel panic was a triple fault, and its report never got out.*

## Why

The four rules at the top of `kernel/kernel64/src/threads.rs` exist because of the other CPUs:

1. **The lock is held across the switch.** Released before it, another CPU can take the thread just queued and run it on the stack this CPU is still saving into.
2. **Say you will sleep, then look.**
3. **A waiter stays in its queue until it leaves.** Waking does not take anyone out.
4. **A thread is gone when no CPU is on it.**

Entry 046's table of deliberate breakages ended: rule 1 broken, 12 boots, all passed; rule 3 broken - the bug that had hung one boot in three - 42 boots, all passed. Both bugs live in a gap a few instructions wide, and a test that waits for a race to land there rarely sees one. Entry 046 also tried to arrange rule 3's race with real threads, and that test passed with the bug in, so it was deleted.

## What it is

Not a test that waits for the race, but checks of what each mistake leaves behind - which is there every time:

- **Rule 1:** the thread a switch lands on is the one that releases the lock (`force_unlock`, in two places: after `context::switch`, and in `thread_entry` for a new thread). Both now first check that the lock *is* held:

  ```rust
  fn rule_1_holds() {
      assert!(THREADS.is_locked(), "rule 1 of threads.rs: the lock was released before the switch");
  }
  ```

  Broken, the lock is free when the first thread lands - and the first switches of a boot happen while the boot CPU is the only one switching, so nothing else can have taken the lock meanwhile. One atomic load per switch.

- **Rule 3:** the harm comes from a wake-up landing between a waiter joining a queue and saying it waits. Nothing can put a wake-up there on purpose, but what it leaves behind can be looked at. `threads::rule_3_holds`, run by the threads self-test, does what `wait_any_until` does - join a queue - and is woken there before it says it waits:

  ```rust
  pub fn rule_3_holds() -> bool {
      let queue = WaitQueue::new();
      let me = current();
      queue.add(me);
      queue.wake_all();          // the wake-up in the gap
      let held = queue.holds(me);
      queue.remove(me);
      held
  }
  ```

  Waking a running thread does nothing else (the policy's `wake` answers false for it), so the check leaves no trace. It lives in `threads.rs`, so the queue's `add` and `remove` stay private.

## The bug it found

To see the checks fail, I broke each rule and booted. On 1, 2, 4, 8 and 16 CPUs each broken rule ended in its own panic. On **20** CPUs - `smp64-test`'s third boot, 4 more than the kernel uses - the log stopped after the threads self-test's first line, with no panic, and QEMU's record ended in `Triple fault`. The CPU that faulted had `EFER=0`: not in long mode.

| | |
|---|---|
| What failed | Any kernel panic on a machine with more than `MAX_CPUS` (16) CPUs: the report was lost and the machine reset |
| Evidence | Twice in two boots with 20 CPUs; never with 16 or fewer. `Triple fault` in the QEMU log, on a CPU with `EFER=0` |
| Cause | A panic stops the other CPUs with an NMI, sent with the APIC's shorthand "everyone but me". That includes the CPUs the kernel left asleep, which the firmware parked in real mode. Their interrupt table is the firmware's, in low memory the kernel has used since: an NMI there faults, and faults again - a triple fault, which resets the machine before the panic has printed |
| Fix | The NMI goes to each CPU the kernel started, one by one, by the APIC ids it keeps (`arch/x86_64/smp.rs`, `nmi_others`), counting those as they answer - so a panic halfway through starting them reaches the ones already running. `kernel64::smp::nmi_message` is an NMI for one CPU; the broadcast is gone |
| Proof | A new fault test, `page@20`: the page-fault kernel booted with 20 CPUs must say "kernel read at 0x4000000000: page not present", as `page` does with 4. With main's four files it fails; with the fix it passes. Rule 3 broken, 20 CPUs, three boots: each printed the panic, none a triple fault |

`fault-test.sh` now takes `FEATURE@CPUS`; a test without the `@` boots with qemu-boot.sh's 4, as before.

One more, in how sessions work rather than in the kernel: my first full `make test64` failed `handles64-test` and `aarch64-handles-test` ("parameter not set" in `handles-test.sh`). I had branched TheKernel from its own `main`, as `start-here.md` said, and that `main` already held another session's next entry (118, a namespace per process), whose TheTaaJ half - the `Makefile` its new script needs - was not merged yet. Rule 12 merges TheKernel first, so between the two halves TheKernel's `main` is ahead of what TheTaaJ points to. A branch should start from the pair TheTaaJ's `main` names: `git submodule update && git -C kernel switch -c NAME`. `start-here.md` says so now, and I rebased my TheKernel work onto it.

And a flaky check, met in the full run after `main` was merged in: an AArch64 boot (`aarch64-test`'s `reserved`) stopped with `KERNEL PANIC ... the clock counted 9 ticks in ten interrupts`. Five more runs of `aarch64-test` passed, and showed why: of some 270 timer self-tests, 208 counted exactly 10 ticks - the check's limit. It asked for 10 ticks *since boot* after ten interrupts, counting on the interrupt it lined up with being past the first tick; when it was the very first, ten periods rounded down to whole ticks can give 9. Now it measures the ticks *across* the ten interrupts, and asks for 9 - what ten periods always hold, with one lost to rounding (`selftest.rs`, `timer`).

## Design decisions

| Decision | Instead of | Why (and the goals) |
|---|---|---|
| Check what a broken rule leaves behind | A stress test that waits for the race | Deterministic, every boot (Stable). Entry 046 showed the race almost never lands |
| `assert!` on every switch | `debug_assert!`, or a test-only feature | The kernel is built in release mode, where `debug_assert!` is gone; one atomic load is nothing beside a switch (Performant) |
| Rule 3's check in `threads.rs`, called by the self-test | `add` and `remove` made public | The queue's insides stay private (Maintainable) |
| An NMI per started CPU | Keeping the broadcast, and parking spare CPUs ourselves | The kernel doesn't touch CPUs it doesn't use; up to 15 messages on the way to a halt cost nothing |

## How I checked it

- **Each rule broken on purpose**, then restored (the file compared afterwards):

  | Change | Result |
  |---|---|
  | As written | `smp64-test` (20, 2, 1 CPUs) and `aarch64-test` passed |
  | Rule 1 broken: the guard dropped before the switch | `KERNEL PANIC ... rule 1 of threads.rs: the lock was released before the switch`, at the first switch |
  | Rule 3 broken: `wake_all` empties the queue (entry 046's bug) | `KERNEL PANIC ... rule 3 of threads.rs: a wake-up took a waiter out of its queue`, in the threads self-test, on 1 to 20 CPUs |

- `page@20` with and without the NMI fix, as above.
- `cargo test --lib smp` (the host tests of the APIC messages): 9 passed.
- `make test64`: see the result below.

Not tested: rules 2 and 4 got no new check (entry 046 found rule 2's breakage in three boots, and rule 4's in the fault tests); the AArch64 kernel's way of stopping CPUs (PSCI, a software interrupt per CPU) was not changed or looked into for spare CPUs; real hardware.

## What is still missing

- A deterministic check for rule 2 ("say you will sleep, then look").
- Whether AArch64 machines with more CPUs than `MAX_CPUS` panic cleanly.

## What I learned

- **Test the state a race leaves, not the race.** The gap is a few instructions; the damage - a waiter in no queue, a lock already free - is there to see every time.
- **Break the thing to see the test fail - on more than one machine shape.** Rule 3's check passed its first trial on 1 CPU; it was the 20-CPU boot that showed the panic itself was broken.
- **A panic must not depend on what it stops.** "Everyone but me" counted CPUs that were never ours.

## Testing note

`make run64` boots as before; the checks say nothing while they hold. To see them:

| Do | You should see |
|---|---|
| `make test64` | `fault-page@20: ok (kernel read at 0x4000000000: page not present)` among the fault tests, and `test64: all passed` |
| In `kernel/kernel64/src/threads.rs`, change `core::mem::forget(guard);` in `schedule` to `drop(guard);`, then `make smp64-test` | `KERNEL PANIC: panicked at src/threads.rs:...: rule 1 of threads.rs: the lock was released before the switch` in `build/smp64-20.log`. Change it back |
| In `WaitQueue::wake_all`, take the waiters out (`core::mem::take(&mut *self.waiters.lock())`), then `make smp64-test` | `rule 3 of threads.rs: a wake-up took a waiter out of its queue` in the log - with 20 CPUs too. Change it back |

Automated: `make test64`.
