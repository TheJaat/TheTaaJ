# Tests at full speed: KVM for the test scripts, and the boot test's two typing races

*Journal entry #100 · MaK's step A, item K19 (the race hunt), first part. Written on MaK as entry 099; The-Jat's step B was merged first and took that number (rule 5), so this is 100 - the commit in TheKernel (0016b7f) still says 099. Until now the boot tests used KVM on MaK only through a wrapper outside the repository, and under KVM the two typed boot tests failed every time. Now the repository chooses the accelerator itself, the same way for `make run64` and for every test, and the typed boot tests pass under KVM and under emulation alike.*

## Why

K19 is "run `make test64` under KVM again and again, and fix every race found". The first run on the merged `main` (entry 097 in) had already found one: `boot64-test` and `boot64-uefi-test` failed under KVM, and only there. Before hunting further, the tests had to say which accelerator they run on, and those two had to pass on it. Otherwise every later run would fail on the same two lines and hide anything new.

Two things were wrong:
- **The tests did not know about KVM.** Entry 098 made `make run64` use KVM when `/dev/kvm` can be opened, and its Makefile comment said the test scripts read `TAAJ_ACCEL`, but no script did. On MaK, KVM reached the tests only through `~/.local/bin/qemu-system-x86_64`, a local wrapper adding `-accel kvm`. On any other machine with KVM, the tests would have been emulated, and nothing in the repository showed which one a run had used.
- **The typed boot test assumed a slow CPU** in two more places than entry 098 fixed.

## How it fits together

```
Makefile ── TAAJ_ACCEL := kvm | tcg   (worked out once, exported)
   │            │
   │            ├── RUN_ACCEL ?= $(TAAJ_ACCEL)        make run64 and friends
   │            └── qemu-boot.sh: -accel $TAAJ_ACCEL  every x86_64 test boot
   │
   └── CPRIMES_LIMIT_X86 = KVM ? CPRIMES_LIMIT_KVM (48000000)
                               : CPRIMES_LIMIT_TCG (24000000)
         boot64-test boot64-uefi-test: CPRIMES_LIMIT = $(CPRIMES_LIMIT_X86)
         C_KEYS types $(call spell-keys,$(CPRIMES_LIMIT)) → 4 8 0 0 0 0 0 0
         EXPECT_C expects "[4] cprimes $(CPRIMES_LIMIT)"
```

- **One accelerator for everything.** `TAAJ_ACCEL` is decided at the top of the Makefile: `kvm` if `/dev/kvm` can be read and written, else `tcg`. It is exported, so every recipe and every script sees it. `qemu-boot.sh`, which starts every x86_64 test boot, passes `-accel "${TAAJ_ACCEL:-tcg}"`; run by hand without it, it emulates as before. `make test64 TAAJ_ACCEL=tcg` emulates anyway, on any machine.
- **The cprimes limit is written once.** It used to be written out three times: `24000000` in the Makefile, `2 4 0 0 0 0 0 0` in the keys the test types, and `cprimes 24000000` in the expected output. Now the keys are spelled from the variable by a small function (`spell-keys`), and the expected output uses it too.
- **The limit depends on the accelerator, for the x86_64 boot tests only.** A target-specific variable gives `boot64-test` and `boot64-uefi-test` `CPRIMES_LIMIT_X86`: 48000000 under KVM, 24000000 emulated. `aarch64-keys-test` types the same keys but is emulated on every machine, so it keeps 24000000.
- **Background output is matched anywhere in a line.** The four `cfloat` lines in `EXPECT_C` are no longer anchored at the start of a line, like the cprimes answers since entry 098.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| The repository chooses KVM, for the tests as for `make run64` | A wrapper on each machine | A machine-local wrapper is invisible: a log could not say how it was made, and `TAAJ_ACCEL=tcg` could not undo it. The-Jat, with no `/dev/kvm`, gets `tcg` exactly as before |
| `TAAJ_ACCEL` decided once with `:=` | `RUN_ACCEL ?= $(or $(TAAJ_ACCEL),$(shell ...))` as entry 098 had it | One `$(shell)` for the whole run, one name the scripts read, and `RUN_ACCEL` simply follows it |
| A bigger limit under KVM, for x86_64 only | Shortening the typing, or measuring the work differently | The test's point is four programs running at once while the shell waits for them. The fix that keeps that meaning is making them last longer than the typing. AArch64 is emulated everywhere, so a bigger limit there would only make the slowest part of `test64` slower |
| 48000000, measured | A formula from the host's speed | The time cprimes takes grows faster than the limit (trial division: about N^1.5), and the typing time depends on the monitor, not the CPU. Measured: at 24000000 the CPUs worked 1.26 times the waited time (1.5 needed); at 48000000, 3.3 times |
| Unanchored lines only for background output | Unanchoring every expected line | The other lines are printed by programs run in the foreground, which never race the typing; their `^` still catches a line printed in the wrong place |

## Bugs

**1. The tests used KVM only through a wrapper outside the repository.** Found by reading `qemu-boot.sh` after entry 098: it never passed `-accel`, although the Makefile comment said the test scripts read `TAAJ_ACCEL`. On MaK the tests had run under KVM since 2026-11-25 only because `~/.local/bin/qemu-system-x86_64` added `-accel kvm -cpu host`. Fixed by the exported `TAAJ_ACCEL` and `-accel` in `qemu-boot.sh`; MaK's wrapper is retired (moved aside, not needed).

**2. `cfloat`'s lines were expected at the start of a line.** The run of `make test64` on the merged `main`, under KVM, failed both typed boot tests with

```
FAIL: no output matching "^cfloat 1: 30000000 rounds, 0 wrong"
```

The four `cfloat` copies run in the background (`cfloat 1 &` ...) while the test goes on typing. Under KVM one of them ends while the next command is being echoed, and the line in the log reads `cflcfloat 3: 30000000 rounds, 0 wrong` (seen in `build/serial64.log` on 2026-11-25). Every copy printed the right thing; the expectation was wrong. The earlier KVM runs had stopped at the cprimes check before reaching this one. Fixed by matching those four lines anywhere in a line.

**3. The cprimes copies ended before `time wait` was typed.** After entry 098's fix to the counting, the next check failed under KVM:

```
FAIL: four programs did not run at once: took 1.40 s; the 4 CPUs worked for 1.76 s in all
```

`check-boot.sh` asks that, while the shell waited (`time wait`), the CPUs did at least 1.5 times the waited time in work: proof that the copies ran at once. Emulated, a copy at 24000000 runs long past the typing (took 14.66 s, worked 52.61 s: 3.6). Under KVM it runs several times faster and most of its work is done while the four `cfloat` lines and `time wait` are still being typed, so the wait sees only the end of it (1.76 / 1.40 = 1.26). Fixed by a bigger limit under KVM; measured at 48000000: took 8.33 s, worked 27.78 s, 3.3.

**4. The testing notes would have left out every entry from 100 on.** `tools/testing-notes.py` collected `docs/journal/0[0-9][0-9]-*.md`: three digits, the first a 0. Found when this entry, renumbered from 099 to 100 at merge time, was missing from the rebuilt `docs/testing-notes.md`. Fixed by collecting `[0-9][0-9][0-9]-*.md`; entries 044 to 099 come out the same, and 100 follows 099.

## How I checked it

- `make -n` on each test: under KVM `boot64-test` gets `CPRIMES_LIMIT=48000000` and types `4 8 0 0 0 0 0 0` four times; `aarch64-keys-test` keeps `24000000` and `2 4 0 0 0 0 0 0`; with `TAAJ_ACCEL=tcg`, `boot64-test` is back at `24000000`.
- A recipe sees `TAAJ_ACCEL=kvm` and `RUN_ACCEL=kvm` on MaK, and `tcg` with `TAAJ_ACCEL=tcg`.
- `boot64-test` under KVM, with MaK's local wrapper moved aside (so KVM came from the repository alone): passed, `four copies of cprimes at once: took 8.33 s; the 4 CPUs worked for 27.78 s in all`.
- `make test64` under KVM and `make test64 TAAJ_ACCEL=tcg` (emulated, as The-Jat runs it): see the testing note; both must end `test64: all passed`.

## What is still missing

- **The rest of K19:** `make test64` under KVM again and again (20 runs), and whatever those runs find. This entry fixes the races the first runs showed.
- **Under KVM the CPU model is QEMU's default**, not `-cpu host` as MaK's wrapper had it. Nothing in TheTaaJ needs more yet; the AI engine's AVX2 will (it needs XSAVE in the kernel first).
- **The typing itself is slow**: `qemu-boot.sh` sends one key at a time through the monitor, so a typed boot test takes about six and a half minutes whether the CPU is fast or not.
- `make run` (the 32-bit system) and the AArch64 scripts don't use `TAAJ_ACCEL`; AArch64 can't use KVM on a PC anyway.

## What I learned

- **A test that types and checks output is a test of timing too.** Background programs print whenever they finish; on a fast machine that is in the middle of the next line typed. A real terminal does the same, so the test must accept it.
- **A local fix that makes tests pass can hide what the repository really does.** MaK's wrapper made the tests use KVM, but the repository never said so. Moving the choice into the Makefile made it visible and reversible (`TAAJ_ACCEL=tcg`).
- **Measure, don't scale.** The host said a cprimes copy takes 21.7 s at 24000000 (while `test64` was running beside it), which would have suggested the copies were already long enough. They weren't: the typing takes longer still.

## Testing note

On MaK (KVM):

```
make boot64-test
```

| Look for | Meaning |
|---|---|
| `four copies of cprimes at once: took 8.33 s; the 4 CPUs worked for 27.78 s in all` (times vary) | The four copies ran at once under KVM; worked is well over 1.5 times took |
| `boot64-test passed` | Every expected line was found, the cfloat lines wherever they were |

On either machine:

```
make --eval='show-accel: ; @echo $$TAAJ_ACCEL' show-accel
make test64
make test64 TAAJ_ACCEL=tcg
```

| Look for | Meaning |
|---|---|
| `kvm` on MaK, `tcg` on The-Jat | The accelerator the tests use |
| `test64: all passed`, twice | Every part, at full speed and emulated |
