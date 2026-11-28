# The sixth night: five branches merged, the 20-CPU boot fixed, and the first retry's finding

*Journal entry #NNN · the night of 2026-11-28, on TheJat-WSL, written by TheJat-WSL/ci. The user asked for the queue only, with no race-hunt rounds: the hunt is for a later night. All five branches passed and were merged under the user's standing yes (rule 2), one of them on the runner's new retry. That retry hid a real race, which this entry fixes.*

## What was merged

| Entry | Branch | Test | Note |
|---|---|---|---|
| 161 | `night-5` | 9 min 26 s | The 20-CPU fault boot asks for 4 boot slots, not 20 |
| 162 | `k189-fuzz` | 8 min 47 s | Fuzzing the parsers on the host |
| 163 | `k136-log-service` | 13 min 25 s | The system log; passed on a retry of `boot64-uefi-test` (below) |
| 164 | `k188-crash-record` | 9 min 07 s | The kernel's last words, found by the next boot |
| 165 | `k138-crash-report` | 9 min 09 s | A crash report per program |

The night took 56 minutes; 3.2 CPUs were busy on average. Main had moved during the day (MaK-WSL's K82, entry 160), and K136 and K188 were merged with it and restacked before the night. The rehearsal merged 5 of 5.

## The slot fix works

`fault64-page-20-test` took **47 s**, against 211 to 215 s in every hunt round of night 5. It no longer holds the slots' gate while it waits for 20 of them. `boot64-uefi-test`, which had doubled to about 460 s behind it, took 278 s.

## The retry's first catch: a race the system log brought

`k136-log-service`'s first run failed `boot64-uefi-test` only:

```
disk check: FAILED - sectors read together differ from sectors read one by one
```

The retry (entry 149) ran it again, it passed, and the branch was merged as the retry allows, marked `(failed once: flaky?)`. It was not flaky in the usual sense, a boot that missed its timing window. It was K136:

- `disk check`, the shell's test of the disk driver's many-sector reads, reads the disk's first sectors together, then one by one, and expects the same bytes.
- Those are the file system's own tables. Since K136, `logd` appends to `/var/log/system.log` throughout the boot, and every append can change them.
- A write that lands between the two reads makes them differ, even though the driver is right.

**The fix**, in the shell's `disk check` (`user/src/bin/shell.rs`, the shell's test commands): read the sectors together, one by one, then together again.
- If the one-by-one reads match the first read, the check passes.
- If they differ and the second "together" read equals the first, nothing was written meanwhile, so the driver is at fault and the check fails, as before.
- If the second read differs from the first, the disk was written meanwhile, so it tries again, up to five times.

The check is as strict as it was: a driver that returns wrong bytes still fails it.

Checked: `boot64-test` and `boot64-uefi-test` both pass (`disk check: all 9 checks passed`). The race could not be made to happen on demand, so the fix rests on the reasoning above, and the race hunt will show it.

**For the retry itself:** it did what it was for, with no wasted night. But "passed on a retry" was taken as flaky when it was not. A part that passes only on its retry is now a thing to read the next morning, not just a J95 data point.

## For the morning

- **The race hunt**, rounds 21 to 30, on a night the user chooses.
- **Every "passed on a retry"** gets its first failure read (above).
- Each branch's test took about 9 minutes; the queue itself, 56 minutes.

## Testing note

| Do | You should see |
|---|---|
| `make boot64-uefi-test` | `disk check: all 9 checks passed`, with logd writing the system log meanwhile |

Automated: `boot64-test` and `boot64-uefi-test` (part of `make test64`).
