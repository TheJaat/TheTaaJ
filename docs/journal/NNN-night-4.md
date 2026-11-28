# The fourth night: two branches merged, and the race hunt's first ten rounds, all passed

*Journal entry #NNN · the night of 2026-11-28, on TheJat-WSL, written by TheJat-WSL/ci. Two branches were queued; both passed the first time and were merged under the user's standing yes (rule 2). Then the runner's new `--hunt 10` ran the first 10 of J95's 50 rounds on main: 67 parts each, and none failed.*

## What was merged

| Entry | Branch | Test | Note |
|---|---|---|---|
| 155 | `night-3` | 8 min 40 s | The third night's entry |
| 156 | `j95-no-flaky` | 9 min 02 s | J95's first part: the q35 power test's fix, the TCP test's packets, the hunt over several nights |

Before the night, the rehearsal merged 2 of 2. MaK-WSL merged `k193-cpu-locks` (153) and `bootloader-deps` (154) during the day; both were merged into `j95-no-flaky` before the night. MaK-WSL also merged `k116-real-keys` (157) while the hunt ran.

## The race hunt: rounds 1 to 10

`night-test.py --merge --wait 60 --hunt 10`: after the queue, main (`b9eff91a`, with both merged), its shared builds, then `ALL=1 tools/race-hunt.sh 10`.

| Round | Seconds | Failed |
|---|---|---|
| 1 | 447 | none |
| 2-10 | 524 to 531 | none |

The hunt took 86 min 54 s, and the whole night 107 min 27 s. Every part passed in every round, including the three J95 is after:
- `power64-test`, with its q35 fix;
- `bigmem64-test` (K50);
- `aarch64-tcp-test` (K52), whose packets are now kept.

Ten clean rounds do not prove that these parts never fail. K50 was seen once in 20 rounds (entry 105), so the 40 rounds still to run are needed.

## For the morning

- **Round 1 took 447 s and the others about 527 s.** Every round runs the same parts in the same order (by `build/test64-times`), so the extra 80 s is not the order. Each round's log has no time stamps, so it cannot say which part grew. The next nights could stamp the round logs as the runner does its own.
- **The report's CPU line is wrong for a night with a hunt.** It says "0.5 CPUs busy on average", because the hunt's 87 minutes count as testing but its CPU time is not measured. The hunt should be measured with `/usr/bin/time`, as `test()` is.
- **The rest of J95:** 40 rounds, at 10 a night (`--hunt 10`), is four more nights. Each part that fails gets fixed, with its round's files in `build/race-hunt/round-N/`.
- **The longest parts by the last run:** `e1000-64-test` 377 s, `fault64-test` 354 s, `clang64-test` 312 s. Splitting `e1000-64-test`'s four boots and `fault64-test`'s cases into parts of their own is still the next speed-up.

## Testing note

| Do | You should see |
|---|---|
| `cat ../TheTaaJ-night/build/race-hunt/tally.txt` | `race-hunt: 10 rounds in all`, and no part failed |

Automated: not part of `make test64`.
