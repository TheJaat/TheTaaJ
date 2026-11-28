# The seventh night: five branches merged, and logd's writes caught by a leak check

*Journal entry #NNN · the night of 2026-11-28, on TheJat-WSL, written by TheJat-WSL/ci. The user asked for the queue only, with no race-hunt rounds. All five branches passed and were merged under the user's standing yes (rule 2), one on the runner's retry. As agreed after night 6, its first failure was read before anything else: it was a race between the system log's writes and a boot test's leak check, and this entry fixes it.*

## What was merged

| Entry | Branch | Test | Note |
|---|---|---|---|
| 166 | `night-6` | 8 min 56 s | The disk check's race with logd (night 6) |
| 167 | `k141-top` | 8 min 12 s | `top` |
| 168 | `k139-tracing` | 9 min 19 s | Tracing, stacked on K141 |
| 169 | `k140-profiler` | 11 min 31 s | The profiler, stacked on K139; passed on a retry of `aarch64-keys-test` (below) |
| 170 | `k142-debugger` | 9 min 21 s | The debugger, stacked on K140 |

The night took 54 minutes; 3.5 CPUs were busy on average. The stack of four (K141, K139, K140, K142), which had had quick checks only, held.

## The retry's catch: logd at work, counted as a leak

`k140-profiler`'s first run failed `aarch64-keys-test` only:

```
FAIL: running programs left processes, threads or channels behind
```

The typed session runs `ps` before its programs and again after them. `check-boot.sh` wants the same processes, threads and channels both times: a channel more means a handle the shell or init forgot. The two lines were:

```
10 processes, 15 threads, 31 channels
10 processes, 15 threads, 33 channels
```

Two channels more, nothing else. It was not K140, which changes nothing in the shell, init or channels; it was K136's `logd`:

- **Every request to the file service makes a channel for its answer** (`taaj::files`, `channel_create` per call). `logd` appends to `/var/log/system.log` through it.
- **`logd` wrote whenever it read something new**, every 0.2 s. A `ps` is logged a moment before it counts ("[shell] line: \"ps\""), so `logd` was often writing just as `ps` counted, and its answer channels were in the count.
- On AArch64 (emulated) a write takes long enough to be caught often. On x86_64 it is rarer, but the race is the same.

The retry passed it, and K140 was merged, rightly: the failure was not its own.

**The fix, in `logd`:** it now writes once the log has been quiet for 300 ms, or at the latest 2 s after the oldest line waiting, if the log never is quiet. It reads every 50 ms. A command and its counts come within milliseconds of its line, so `logd` waits until they are over. The leak check stays as strict as it was.

**And the boot's backlog at once.** The first try of the fix delayed everything, and `log64-test` failed four times out of four: right after the boot, the log is busy, and the first `log -s hello` found nothing written. What `logd` finds when it starts is now written at once: everything from before it, the self-tests and the boot's crash reports. The shell is not running yet, so nothing is counting.

Checked:
- `aarch64-keys-test` and `log64-test`, four rounds each (`ONLY=... tools/race-hunt.sh 4`): every round passed. The first try of the fix had failed `log64-test` in all four; the keys test had passed.
- `crash64-test`, `boot64-test`, `boot64-uefi-test`, `debug64-test`.

## For the morning

- **The race hunt**, rounds 21 to 30, when the user says.
- **Two catches in two nights by the retry**, both K136's: the disk check's reads (night 6) and the leak check's counts (night 7). Both were races between `logd`'s writes and a test that assumed nothing writes meanwhile. Other tests may assume the same; the hunt is where they will show.
- **A request to the file service could reuse one answer channel**, instead of making one each time. That is the file service's lane's decision; it would make `logd` cheaper too.

## Testing note

| Do | You should see |
|---|---|
| `ps`, then programs that end, then `ps` | The same processes, threads and channels, with logd writing the log meanwhile |

Automated: `aarch64-keys-test`, `boot64-test` (part of `make test64`).
