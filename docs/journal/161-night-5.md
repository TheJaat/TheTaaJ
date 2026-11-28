# The fifth night: the split parts merged, a 20-CPU boot that held up the others, and race-hunt rounds 11 to 20

*Journal entry #161 · the night of 2026-11-28, on TheJat-WSL, written by TheJat-WSL/ci. Two branches were queued; both passed and were merged under the user's standing yes (rule 2). The race hunt's rounds 11 to 20 all passed. But the night was slower, not faster: the first part-by-part times of the hunt showed why, and the fix is in this entry.*

## What was merged

| Entry | Branch | Test |
|---|---|---|
| 158 | `night-4` | 8 min 59 s |
| 159 | `split-long-parts` | 12 min 26 s |

`split-long-parts`'s own test was 3.5 minutes slower than the nights before. Its twelve new parts had no times yet, so they were started first, with all eight fault kernels and the e1000 boots at once.

## The race hunt: rounds 11 to 20

All 77 parts passed in all ten rounds, so 20 of J95's 50 rounds have passed with no part ever failing. The hunt took 104 minutes with 269 CPU minutes; the CPU time is now measured.

| Round | Seconds |
|---|---|
| 11 | 494 |
| 12-15, 17, 19 | 666 to 673 |
| 16, 18, 20 | 558 to 562 |

Night 4's rounds took about 527 s, so most rounds got slower. Each round's part times (`round-N.times`, new last night) said where the time went:

| Part | Night 4's `test64-times` | Rounds 11-20 |
|---|---|---|
| `fault64-page-20-test` | (inside `fault64-test`) | 211 to 215 s, every round |
| `fault64-page-test` | (inside `fault64-test`) | 4 to 13 s |
| `boot64-uefi-test` | 215 s | 446 to 473 s |
| `boot64-test` | 216 s | 227 or 437 s |

The same fault boot took 4 s with 4 CPUs and 212 s with 20. In rounds 12 and 13, many unrelated parts (`aarch64-times-test`, `make64-test`, `llvm64-test`) each took about 210 s longer than in round 11.

## The cause: one boot asking for 20 of the 23 boot slots

Every boot takes "boot slots" before it starts (`boot-slots.sh`): one per emulated CPU. Without them, emulators short of host CPUs fail timed checks. To avoid deadlocks, a boot takes its slots while holding one lock, the gate. `fault64-page-20-test` boots 20 CPUs, so it asked for 20 of the machine's 23 slots. Holding the gate, it waited until 20 were free, and every other boot started in that time waited at the gate behind it. That is the problem `smp64-test` had (entry 117), and why `make test64` runs `smp64-test` alone first.

The 20-CPU boot was there before the split, inside `fault64-test`: it was a reason that part took 354 s. Split off and started early, it now waits at the busiest moment, and holds up the most boots.

**The fix**, in `fault-test.sh`: every fault boot asks for at most 4 slots (`SLOTS=4`). The fault boots' kernel stops at its first fault, right after the CPUs start. Its CPUs sit idle, and the four it does not use, the firmware keeps parked, so 20 host CPUs were never needed.

Checked: `fault64-page-20-test` alone, between two hunt rounds, passed in 4.85 s.

## For the morning

- The next night shows the fix: `fault64-page-20-test` should take seconds, not 212 s, and `boot64-uefi-test` should be back near 215 s.
- `boot64-test` was 227 s in some rounds and 437 s in others, apparently depending on whether it reached the gate before the 20-CPU boot. With the fix, it should stay near 227 s.
- Other big boots can hold the gate the same way. A boot asking for more than half the slots could be run alone, as `smp64-test` is, or the gate could let a small boot take free slots while a big one waits. That second way needs care: the big boot could then wait for ever.

## Testing note

| Do | You should see |
|---|---|
| `make fault64-page-20-test` | `fault-page-20: ok` in a few seconds |
| A race-hunt round | `fault64-page-20-test` a few seconds in `round-N.times` |

Automated: the fault parts are parts of `make test64`.
