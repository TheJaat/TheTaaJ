# A system-call fuzzer: random calls from programs with nothing

*Journal entry #NNN · a test (K104, first part). Written on TheJat-WSL/ci. The kernel must survive anything a program asks of it: the worst a program may do is end itself. Now `fuzz` makes sure of it - programs with no handles at all make system calls with random numbers and random arguments, a quarter of a million of them in each `make test64`, on both CPUs. The kernel took all of them. Every bug this found was in the fuzzer.*

## Why

A microkernel's system calls are its whole surface: everything a program can do to the kernel, it does through some 45 calls. Each was written with its checks, and each check was tested by hand-written cases - the ones I thought of. A fuzzer thinks of the rest: arguments nobody would write, in orders nobody would choose. The **secure** and **reliable** goals both rest on this surface, and Era 18 asks for a fuzzer before its merge (K104).

## What it is

```text
with programs fuzz [CHILDREN [CALLS [SEED|random [trace]]]]
```

`fuzz` starts CHILDREN copies of itself (100) and waits for each. A child gets **no handles at all** - no screen, no files, no services, the least a program can have: it asks init for an `IMAGE` run with flags 0, so init hands it nothing but its arguments. Each child makes CALLS calls (3000) from an xorshift generator seeded with SEED plus its number, so the same seed makes the same calls, and a crash found is a crash repeated. Then `fuzz` says how they ended:

```text
fuzz: 100 children, 3000 calls each, seeds 1 to 100
fuzz: 87 made all their calls, 12 were killed for breaking a rule, 0 were stopped after 2 s, 1 ended otherwise, 0 did not start
fuzz: the other codes: -2
fuzz: done - 100 programs with no handles, 261000 calls from those that finished, the kernel still here
```

A kernel bug shows as a panic - the machine stops - or a hang - this never finishes, and the shell does not answer after it. `fuzz-test.sh` checks for both, and that every child was accounted for. It is `fuzz64-test` and `aarch64-fuzz-test` in `make test64`, about 20 s each.

## How it works

**The arguments** lean towards what finds bugs. Each is one of: 0; 1; a small number (a handle, a count); -1; a power of two (a size); anything; an address in the kernel's code; one in the kernel's half; a low unmapped one; one inside the child's scratch area; one just before its end, so that a buffer runs across it.

**What a child must not do to itself.** A child that ends after a dozen calls finds nothing, and the first runs were all that:

| What the children did | How it showed | What changed |
|---|---|---|
| `exit`, the last thread's `thread_exit` | - | Never drawn |
| `signal_return` to a made-up context; a thread started at a random entry; a handler and an alarm; a signal to itself; a garbage signal stack; their memory or thread pointer taken away; their memory limit cut | 34 of 40 killed at once; `signal_return` alone killed 32 | Drawn one time in 4096: rare, but still made, so their checks are fuzzed too |
| `sleep`, `channel_receive`, the waits, `futex_wait`, a `channel_send` to a full queue | 32 of 40 stopped after 2 s | The same |
| `random(scratch_end - 2, 40 KiB)`: the kernel, as asked, wrote random bytes over the child's own state, after its scratch page | Children hung | The scratch area is a reservation of its own, not a static: past its end is unmapped, and the kernel refuses a buffer that runs on |
| `debug_write` from the scratch area | Random bytes in the log the tests read | Its pointer is never the scratch area: bad pointers are what test its checks |

Each child's memory limit is first lowered to 32 MiB: random reservations must not take the memory other programs need.

**Finding where a child stopped.** A child has no output, so `trace` makes each child write every call to the kernel's console (`debug_write` needs no handle) before making it. That is how the `random` call above was found: seed 3's last line was `call 1077: 39(0x40affe, 0xa000, ...)`.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Children with no handles | One program fuzzing itself | The least authority is the attack to survive; and a child that kills itself is replaced, not the run ended |
| A seed per child, the same in every test | Random each run | A failure in `make test64` must be repeatable; `random` is there for long runs |
| Calls that end the child made rarely, not left out | Leaving them out | Their argument checks are the kernel's too |
| A time limit per child | Leaving the waits out | A wait is a fine end; the parent's two seconds bound the run |
| The kernel's survival as the verdict | Checking each call's result | No list of right answers covers random arguments; the kernel must not stop, nor stop answering |

## Bugs

None in the kernel: some half a million calls, from 200 programs on two CPUs, and every test boot answered after them. That is a result, not a guarantee - the fuzzer knows nothing of what each call means, and a bug that needs two calls in a meaningful order is unlikely to be found by chance.

In the fuzzer, all of the table above - each found by asking how the children ended, then by the kernel's own log (`[process] N "fuzz" killed: returned from a signal to no context`, 32 times), then by `trace`.

## How I checked it

- `fuzz64-test` and `aarch64-fuzz-test` (100 programs of 3000 calls): 87 finished on each, the rest killed by the kernel for what they did; 17 and 22 s.
- 20 programs of 20000 calls: 6 finished, 11 killed, 3 stopped - why the default is shorter children, more of them.
- `make unsafe-check`, and `make test64`.

## What is still missing

- **`cargo fuzz` on the host** for the parsers of untrusted input - ELF, TaajFS, the network, USB descriptors, ACPI tables, the protocols - K104's second half, now `K?-host-fuzz`; it needs a nightly Rust.
- Calls in meaningful orders: create a channel, then send on it, then close it twice. Random numbers rarely make a sequence.
- Coverage: which of the kernel's checks the fuzzer reached (J96).
- A long run before an era's merge: `with programs fuzz 10000 3000 random`, by hand for now.

## What I learned

- **A fuzzer's first bugs are its own.** Every one I found was the fuzzer hurting itself with what the kernel rightly did.
- **Ask the kernel what happened.** Its log said `returned from a signal to no context` 32 times; I had been guessing.
- **Count what you can see.** A child with no handles cannot report how far it got; "the kernel took 800000 calls" was a claim, and a wrong one.

## Testing note

From the root of TheTaaJ, put `fuzz` on the disk `make run64` boots (it is not one of the disk's usual programs), then boot:

```
make run-disk
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img fuzz user/target/x86_64-unknown-none/release/fuzz
make run64
```

Then:

| Type | You should see |
|---|---|
| `with programs fuzz 20 2000` | `fuzz: 20 children, 2000 calls each, seeds 1 to 20`, then after some seconds `fuzz: 18 made all their calls, 2 were killed for breaking a rule, ...` (the same numbers every time with the same seed) and `fuzz: done - 20 programs with no handles, ... the kernel still here` |
| `echo still here` | `still here` |
| `with programs fuzz 5 2000 random` | Different seeds, and the same `done` line |
| `fuzz` | `fuzz: it starts programs: run it as `with programs fuzz`` |

Automated: `make test64` (`fuzz64-test`, `aarch64-fuzz-test`).
