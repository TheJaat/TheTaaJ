# The third night: four branches, four passes, and the first parts ordered by their times

*Journal entry #155 · the night of 2026-11-28, on TheJat-WSL, written by TheJat-WSL/ci. Four branches were queued, and all four passed their whole `make test64` on the first try and were merged under the user's standing yes (rule 2). Nothing failed and nothing was waited for. The night took 41 minutes.*

## What was merged

| Entry | Branch | Session | Test | Note |
|---|---|---|---|---|
| 149 | `night-speed` | TheJat-WSL/ci | 9 min 24 s | The four speed fixes; first, so the branches after it had them |
| 150 | `night-2` | TheJat-WSL/ci | 8 min 48 s | The second night's entry; the first run with its parts ordered by their times |
| 151 | `k42-key-events` | MaK-WSL/desktop | 9 min 33 s | MaK-WSL's host test of `key_message()`: kernel/abi 87.2% (floor 87; night 2 had 86.6%) |
| 152 | `k46-wheel` | MaK-WSL/desktop | 8 min 47 s | Stacked on `k42-key-events`, tested with it merged |

Wall time was 40 min 59 s: 36 min 32 s testing and the rest fetching and merging. CPU time was 115 min, so 3.2 CPUs were busy on average.

`k46-wheel` was tested on main with `k42-key-events` merged, which is exactly what main is now (merge-item.py only numbers and adds the README row), so main was not tested again.

Before the night: `restack.py --check` reported the stack, and `merge-rehearsal.sh` merged 4 of 4 in the sandbox. The rehearsal showed that putting `night-speed` first gave `night-2` the next number, which `night-speed`'s entry had used for it. That entry now names `night-2`'s entry instead of numbering it.

## The part times

The first measured run (`night-speed`, in the Makefile's order) gave each part's time, waiting for boot slots included. From `night-2` on, each run started its parts longest first. The longest, by the last run:

| Part | Seconds |
|---|---|
| fault64-test | 341 |
| e1000-64-test | 319 |
| boot64-test | 216 |
| boot64-uefi-test | 216 |
| aarch64-test | 211 |

The shortest are a few seconds (`aarch64-pages-test` 5, `aarch64-ground2-test` 6). In all, 66 parts.

With the longest first, the runs were 8 min 47 s to 9 min 33 s, against about 10 minutes before. Three runs are too few to say how much of that is the order. The busy CPUs barely moved (3.0 to 3.2), so the parts mostly wait rather than compute.

## For the morning

- **`fault64-test` and `e1000-64-test` now set the night's length.** At 5.5 to 6 minutes, they are most of the parts' 7.5 minutes. What in them waits is where to look next: their own sleeps and timeouts, or boot slots.
- **The time counts waiting for boot slots.** A part that waits for others' boots looks long. Recording the wait separately (`boot-slots.sh` knows it) would tell a slow part from a starved one.
- **`TEST_JOBS` above 12** is still to try: with 3 CPUs busy of 24, there is room, if the timing tests stay steady under the load (rule 13).
- **TheBootloader's main** is at `d898aab` now; MaK-WSL fast-forwarded it.
- **The retry** was not needed tonight: nothing failed.

## Testing note

| Do | You should see |
|---|---|
| `cat ../TheTaaJ-night/build/test64-times` | Each part's seconds in the last run, longest first |

Automated: not part of `make test64`.
