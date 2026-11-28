# The self-test's slow kill: hunted

*Journal entry #NNN · a step, MaK-WSL/sched's K?-slow-kill. Written on MaK-WSL/sched. The kernel's self-test kills a program five ways and requires each kill to end it within 3 ticks, measured up to three times. Twice now a boot has failed that: entry 153's bug 10 (x86, one CPU, "forever spin took 5 ticks", during a race hunt) and entry 160's bug 6 (emulated AArch64, "forever sleep took 12 ticks", during `make test64`). This entry is the hunt for why.*

## Why

A kill that is slow every time would be a scheduler bug: a killed thread's wake-up lost, or the thread left behind others. A kill that only looks slow is the host: ticks follow the CPU's counter, and a host that does not run the virtual machine for a moment makes that moment count (entry 105 found it under KVM, and is why each kill is measured up to three times). The panic could not tell the two apart.

## How it works

The panic now says every try's ticks, not only the last: "forever sleep took 12 ticks to end, three times over (the tries: [a, b, 12] ticks)". One slow try beside fast ones is the host; three slow by about as much is this kernel.

## The hunt

| Hunt | Rounds | Parts | Failed |
|---|---|---|---|
| `ALL=1 tools/race-hunt.sh 4`, at main 184ce96f (with entry 159's split parts) | 4, 767-796 s each | 78 | none |

The failing run of entry 160 was more loaded than these: before entry 159 split them, `fault64-test` and `e1000-64-test` took about 640 s each in it, beside everything else.

## How I checked it

- The hunt above. RESULT of the longer hunt.

## What is still missing

- A cause. RESULT.

## Testing note

```
ALL=1 tools/race-hunt.sh 20
```

| Look for | You should see |
|---|---|
| `build/race-hunt/summary.txt` | `failed: none` in every round; a failure of `aarch64-thread-test` or a boot's self-test has every try's ticks in its panic |

Automated: `make test64`
