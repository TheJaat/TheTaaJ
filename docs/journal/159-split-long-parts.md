# The two longest parts split into twelve, and the race hunt measured

*Journal entry #159 · three fixes from the fourth night's notes. Written on TheJat-WSL/ci, by day, with quick checks only.*

## `fault64-test` and `e1000-64-test`, one part per boot

With the parts started longest first (entry 149), two parts set how long each `make test64` and each race-hunt round took. The parts run 12 at a time, but each of these two did its boots one after another:

| Part | Seconds (night 4) | What it did, in turn |
|---|---|---|
| `e1000-64-test` | 377 | four boots: e1000 and e1000e (82540EM, 82574L), IP, TCP |
| `fault64-test` | 354 | eight kernels, each built with its own `fault-*` feature and booted |

Each boot is independent of the others, so each is now a part of its own:

- **`FAULT64_PARTS`**: `fault64-page-test`, `fault64-page-20-test`, `fault64-double-test`, `fault64-rodata-test`, `fault64-nx-test`, `fault64-stack-test`, `fault64-thread-stack-test` and `fault64-init-test`, from `FAULT64_CASES`.
  - `fault-test.sh` takes a test's CPU count after a last `-` as well as after `@` (`page-20`, as its logs were already named).
  - Each test now has its own cargo target directory. `page` and `page-20` had shared `target/fault-page`, and now run at once, while `fill-symbols.py` writes into the kernel it is given.
- **`E1000_64_PARTS`**: `e1000-64-82540em-test`, `e1000-64-82574l-test`, `e1000-64-ip-test` and `e1000-64-tcp-test`.
  - The test disk is made once (`e1000-64-disk`), and each boot gets a copy of its own, because two QEMUs may not share a disk.
  - The logs keep their names.
- `make fault64-test` and `make e1000-64-test` still run them all. `TEST64_PARTS` names the new parts in the old parts' places.

Checked: `make -j12 fault64-test e1000-64-test` ran all twelve in **52 s**, and all passed. They had taken 377 s and 354 s as two parts. In a full `make test64`, the other parts share the machine, so the gain there is for the next night to measure. The new parts have no times in `build/test64-times` yet, so the first run starts them first.

## The race hunt's rounds, part by part

On night 4, round 1 took 447 s and rounds 2 to 10 about 527 s, with the same parts in the same order, and nothing said which part took longer. Each round now records its parts' times with `tools/part-time.sh`, as `make test64` does: `build/race-hunt/round-N.times`. The summary names each round's five longest parts:

```
round 1: exit 0, 23 s, failed: none
    longest: e1000-64-82540em-test 23 fault64-nx-test 4
```

Checked: one round of two parts (`ONLY=`), shown above.

## The hunt's CPU time in the report

The night 4 report said "0.5 CPUs busy on average while testing": the hunt's 87 minutes counted as testing, but its CPU time did not. `hunt()` now measures it with `/usr/bin/time`, as `test()` measures `make test64`, and adds it to the night's CPU. The hunt's line says it too: `- 86 min 54 s (CPU ...); tonight's rounds:`.

Checked: in the rehearsal sandbox, with a stand-in hunt that kept one CPU busy for 3 s: `- 3 s (CPU 3 s)`, and "1.0 CPUs busy on average".

## Testing note

| Do | You should see |
|---|---|
| `make -j12 fault64-test e1000-64-test` | Eight `fault-...: ok` lines and four `e1000-64-...-test passed`, then `e1000-64-test passed`, in about a minute |
| `make test64` | The twelve new parts among the others; `build/test64-times` has their seconds |
| `ONLY=fault64-nx-test tools/race-hunt.sh 1` | `build/race-hunt/round-N.times`; a `longest:` line in the summary |

Automated: the twelve parts are parts of `make test64`.
