# The 20-CPU boot first: test64's parts in 573 s instead of 940

*Journal entry #108 · a test fix. Written on TheJat-WSL. `make test64` stood still for six minutes in the middle of every run: one boot waiting for 20 boot slots held the gate that every other boot goes through, and it was waiting for the longest boot of all. The two typed boots now wait for `smp64-test` (about 20 s), and its 20-CPU boot comes first. On TheJat-WSL the test parts took 940 s before and 573 s after.*

## Why

Timing each part of `make test64` on TheJat-WSL (make's `--trace` for when a part starts, its "passed" line for when it ends) showed:

```
  0- 20 s   about twenty boots start
 20 s       smp64-test's third boot, 20 CPUs, asks boot-slots.sh for 20 of 23 slots
 20-474 s   it waits holding the gate: no other boot starts, even a 1-CPU one;
            from 90 s to 474 s the log shows nothing at all
            it waits for boot64-test's typed boot (3291 keys, about 450 s) to give back 4 slots
474-940 s   the rest - boot64-uefi-test, as long as boot64-test, starts only now and ends last
```

So the two longest boots ran one after the other instead of side by side. Every machine has this: on MaK-WSL (15 slots) and TheJat-VM the 20-CPU boot asks for all of them.

## How it fits together

`Makefile`:
- `boot64-test` and `boot64-uefi-test` have `smp64-test` as an order-only prerequisite (`| smp64-test`): they start after it, whatever make's order. Until now `smp64-test` started before them only because their own prerequisites took 18 s to check.
- `smp64-test` boots with 20 CPUs first, then 2, then 1: the big request is made when the fewest slots are held.

`boot-slots.sh` is unchanged. Its gate is what stops a big boot from being starved by small ones; the fix is to ask for many slots at a moment when they are free.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| The 20-CPU boot first, and the typed boots after it | `SLOTS=4` for the 20-CPU boot | 20 emulated CPUs on 4 slots would overload a machine without KVM (TheJat-VM), the very thing the slots prevent |
| An order-only prerequisite | Relying on the order make starts parts in | That order already looked right, and the convoy still happened |
| `boot-slots.sh` as it is | Letting small boots past a waiting big one | A big boot could then wait for ever |

## Bugs

- **The suite stood still for six minutes** in every run. Evidence: the timed log above (`boot64-uefi-test` from 20 s to 940 s; nothing logged from 90 s to 474 s). Cause: the gate held by a boot waiting for 20 slots while the 450-second typed boot held 4. Fix: above. Proof: the same timed run after the change - `smp64-test`'s 20-CPU boot starts at 0.2 s, and the last part ends at 573 s.

## How I checked it

On TheJat-WSL (KVM, 24 CPUs, `TEST_JOBS=12`), `make test64-shared` then `make -j12 --trace test64-parts`, each line timestamped:

| | Before | After |
|---|---|---|
| All 58 parts | 940 s | 573 s |
| `smp64-test`'s 20-CPU boot starts | 20 s, then waits 450 s | 0.2 s |
| `boot64-test`, `boot64-uefi-test` | one after the other, ending 492 s and 940 s | side by side, ending 569 s and 573 s |

Every part passed. The whole `make test64` (with `test64-shared`) took 626 s; before, 994 s.

## What is still missing

- **The typed boots are now the whole critical path**, about 460 s each: 3291 keys and 237 commands typed one at a time, 50 ms apart, on BIOS and again on UEFI. Next: type a short part, and run the rest from a script on the test disk; and the full set once rather than on both firmwares.
- **The typed boots start at 105 s, not 55 s**: by the time `smp64-test` ends, make's 12 jobs are taken by other parts. Worth about 50 s.
- `aarch64-keys-test` (261 s, typed too) is the next longest.

## What I learned

- **A queue with a gate is only as fast as its biggest request.** The slots were right; the moment the big request was made was not.
- **Measure the parts, not the whole.** The total said "16 minutes"; only per-part times showed six of them were spent waiting.

## Testing note

From the root of TheTaaJ, with nothing else booting on the machine:

```
make test64
```

| Look for | Meaning |
|---|---|
| `test64: all passed`, in about 10 minutes under KVM (before: about 16) | The suite no longer stands still |
| `boot64-test passed` and `boot64-uefi-test passed` within seconds of each other, at the end | The two typed boots ran side by side |
