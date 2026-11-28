# No flaky tests, part one: the q35's power test fixed, the TCP test's packets kept, and a race hunt night after night

*Journal entry #156 · J95, first part (with K50 and K52). Written on TheJat-WSL/ci, by day, with quick checks only. J95 asks for every part of `make test64` run 50 times, and each part that ever fails fixed, or its limits measured again. This entry fixes the one flaky part the nights have shown so far, makes the other two known ones (K50, K52) leave enough behind to be understood, and lets the night runner run the 50 rounds a few at a time after each night's queue.*

## The q35's power test: `poweroff` typed before init gave up on the AHCI driver

On night 2, `k114-desktop-bench` failed `power64-test` with "q35-off: ahci did not give up on a controller with no disk". The branch changes nothing there, and it passed on its second run.

The q35 machine's AHCI controller has only the CD on it, no disk. Its driver ends at once (code 5), and init restarts it; at the fifth end within 30 s, init gives up on it and fs goes on with files in memory. The test checks for that line. In a passing log:

```
[shell] ready - type a command (try help)
[init] ahci ended with code 5 - restarting it      (four more)
[init] ahci ended (code 5) 5 times in 30 s - giving up on it
[fs] no disk (no disk (or its driver stopped)): files in memory only
poweroff
```

The shell is ready before init gives up, and `qemu-boot.sh` types as soon as the keyboard driver is ready. When the host is busy, the driver's restarts come more slowly than the typing, and `poweroff` arrives first. init then stops the services, and the give-up line never comes. Night 2's log shows that failure alone, with nothing else wrong.

**The fix** is in `kernel/kernel64/scripts/power-test.sh`. The q35 boots start their keys with `see:` and the give-up line, so the typing waits up to 10 s for it, then goes on as before.

Checked: `power64-test` alone passes, with the give-up line before `poweroff`. With 24 busy loops on the host, the old script and the new each passed four times out of four. That load did not bring the failure back, so the fix rests on the log above. The race hunt (below) is where it will be seen again, or not.

## K52: the TCP test keeps its packets

K52: once, in a busy `test64`, `aarch64-tcp-test`'s `netinfo` counted 1 segment answered "no such connection", and the test expects 0. The likeliest cause is a late retransmission from the host, for a connection TheTaaJ had already closed. Correct TCP answers that with a reset, and the test would be wrong to fail it. But a counter cannot tell that apart from a real bug: a connection dropped while still in use.

- `tcp-test.sh` now has QEMU write every packet to `LOG.pcap` (`-object filter-dump`), for both architectures. That is about 3 MB.
- **`kernel/kernel64/scripts/tcp-resets.py`** reads that file: Ethernet, IPv4 and TCP, with no tools needed on the host. For each reset the machine sent, it prints the segment from the other side that the reset answered. When the `netinfo` check fails, the test runs it before saying FAIL.

Checked: `tcp64-test` and `aarch64-tcp-test` pass, each with about 5,190 segments in its file. The script prints nothing on them, because the machine sent no reset. On a copy with one of the machine's ACKs turned into a reset, it printed the reset and the ACK from `10.0.2.2:34992` that it answered.

When K52 happens again, the report says what the segment was: a late FIN or ACK, which is fine and the check should allow; or anything else, which is a bug for the `network` lane.

## K50: nothing to change

`tools/race-hunt.sh` has kept a failed round's files since entry 105. K50 (`bigmem64-test` once not finishing under KVM) needs only to happen again during the hunt.

## The race hunt, night after night

There are 66 parts, and a round of all of them takes about 8 minutes, so 50 rounds is about 7 hours. That is too long for one night after a queue. So the hunt is spread over several nights:

- **`tools/race-hunt.sh`**:
  - goes on from the round after the last one in `build/race-hunt/summary.txt`;
  - takes the test64 lock per round (rule 13), so a session's own `make test64-again` can come between two rounds;
  - gives make the parts longest first, by `build/test64-times`;
  - does not build `test64-shared` again each round. Before, `make test64-parts` ran its `.PHONY` builds again every round, relinking LLVM's tools (entry 117);
  - writes `build/race-hunt/tally.txt`: each part that ever failed, in how many of all the rounds;
  - takes `ONLY="A B"` for one part, 50 times.
- **`night-test.py --hunt ROUNDS`**: after the queue, in the night tree, it checks out main as the queue left it, builds `test64-shared`, and runs `ALL=1 tools/race-hunt.sh ROUNDS`. The report gets the night's rounds and the tally so far. `NIGHT_HUNT` stands in for the hunt when trying the runner.
- `prepare()` returned early for main and skipped the fresh TheBootloader build (entry 148). Now main gets it too. That matters for `--hunt` and for `--all`, which also test main.

Checked:
- `ONLY=power64-test` for one round, then `ONLY="power64-test tcp64-test"` for one more. The second went on as round 2, both passed, and nothing was relinked.
- With two failed rounds added by hand to the summary, the tally read `power64-test failed in 2 of 4 rounds`, `tcp64-test failed in 1 of 4 rounds`.
- `--hunt 1` with stand-ins in the rehearsal sandbox put the round and the tally in the report.

## What is left of J95

- **The 50 rounds.** About 10 a night after the queue (`--hunt 10`, about 80 minutes) is five nights.
- **Each part that fails:** fixed, or its limits measured again. Then J95 is done.
- **K52:** decided from its first packet capture.

## Testing note

| Do | You should see |
|---|---|
| `make power64-test` | `power64-test passed`; in `build/power64.log.q35-off`, the give-up line before `poweroff` |
| `make tcp64-test`, then `python3 kernel/kernel64/scripts/tcp-resets.py build/tcp64.log.pcap` | The test passes; the script prints nothing |
| `ONLY=power64-test tools/race-hunt.sh 2`, twice | Rounds 1-2, then 3-4; `build/race-hunt/tally.txt` says 4 rounds |
| `night-test.py --hunt 10` | After the queue, "The race hunt (J95)" in the report, with the night's rounds and the tally |

Automated: `power64-test`, `tcp64-test` and `aarch64-tcp-test` are parts of `make test64`; the hunt is not.
