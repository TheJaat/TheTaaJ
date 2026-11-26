# Every unsafe with a reason, checked

*Journal entry #121 · a check, and the reasons it asked for (K112). Written on TheJat-WSL/ci. Rust's `unsafe` marks code whose soundness the compiler cannot check: a person must, and says why in a `// SAFETY:` comment, or for an `unsafe fn` in a `# Safety` section saying what its caller must promise. Most of TheTaaJ's do; 68 places did not, or not where a tool can find them. Now `make unsafe-check` - part of `make test64` - fails, naming the place, when one is missing.*

## Why

Of the three goals `unsafe` touches most - **secure**, **reliable**, **maintainable** - the reason is what makes the code reviewable: an `unsafe` block without one asks the next reader to rediscover why it is sound, or to trust that it is. The project's rule was to write one, and nothing checked it.

## What it is

`tools/unsafe-check.sh`, run by `make unsafe-check`, runs clippy (Rust's linter) with two of its lints made errors, and every other lint off:

| Lint | Asks for |
|---|---|
| `undocumented_unsafe_blocks` | a `// SAFETY:` comment just above every `unsafe` block and `unsafe impl` |
| `missing_safety_doc` | a `# Safety` section on every public `unsafe fn` |

over every crate, for every CPU it is built for: the kernel, the programs (`user/`, for AArch64 only the programs the `Makefile` builds there) and the C library, each for x86_64 and AArch64, and the 16 crates with logic tested on the host. All with the Rust the kernel pins (`rust-toolchain.toml`), so the check means the same on every machine: the host crates otherwise get the machine's default Rust, which differs between machines and may have no clippy.

It is a part of `make test64` (`unsafe-check`, beside `kernel64-test`), and takes a few seconds once the crates are built.

## The 68

What the lints found, and what they were:

| Where | How many | What was missing |
|---|---|---|
| `libc/src/threads.rs` | 38 | 37 `pthread_*` functions (and `__emutls_get_address`) with no `# Safety` section; one AArch64 `asm!` block that shared the x86_64 one's comment |
| `libc/src/spawn.rs` | 7 | one `// SAFETY:` above a loop, for three `unsafe` uses in it |
| `libc/src/memory.rs` | 3 | `unsafe {}` inside an `unsafe fn` whose `# Safety` said it all, but the block didn't point to it |
| `user/` | 14 | "SAFETY (each port access below):" - a reason, in a form the lint does not read; comments a line too high; the user heap's `unsafe impl GlobalAlloc`, and `Send` for `Mutex`, with none |
| `kernel/kernel64/` | 6 | the same forms; the kernel heap's `unsafe impl GlobalAlloc` with none |

Most had a reason already. The ones that truly had none:

- **The two heaps' `unsafe impl GlobalAlloc`** (kernel and user): `alloc` returns null or a block of at least the layout's size and alignment, given to no one else until `dealloc` returns it (the heap crate, tested on the host); `dealloc` gets only what `alloc` returned, with its layout - which is GlobalAlloc's own contract on its caller; the lock keeps two CPUs, or two threads, out of the heap at once.
- **`Send` for `SpinLock` and `Mutex`**: the lock owns its value; moving the lock moves the value, sound whenever the value may move between threads.
- **The pthread functions' contracts**, written as `string.rs` writes its own: for example `pthread_cond_wait` - "`cond` must point to a condition from `PTHREAD_COND_INITIALIZER` or `pthread_cond_init`; `mutex` must point to a mutex from `PTHREAD_MUTEX_INITIALIZER` or `pthread_mutex_init`, not destroyed, and locked by this thread."

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Clippy's own lints | A script that searches for `SAFETY` near `unsafe` | Mine found 68 too - but different ones: it flagged `heap` and `string.rs`, whose reasons clippy reads fine, and missed others. The compiler knows where an `unsafe` block is |
| Only these two lints, as errors | All of clippy | The rest is style, and would bury the two that matter under hundreds of warnings |
| One Rust for every crate, the pinned one | Each crate's default | My first run linted the host crates not at all: they used the machine's 1.97.0, which has no clippy, and my loop hid the error |
| A `make` target, given `USER_A64_BINS` | A list in the script | The AArch64 program list lives in the `Makefile` once |
| Miri left for an item of its own | In this one | It needs a nightly Rust, a new toolchain for every machine |

## Bugs

- **The first count was of the wrong thing.** My own search said 68 places as well, mostly in `heap` and `string.rs` - both clean by clippy's reading. Counting with a regex is not knowing where an `unsafe` block is.
- **The host crates were not linted at first** (above): the error was there, and `2>/dev/null` hid it. The script now pins the Rust and prints what fails.
- **`user/` for AArch64 does not build whole**: `kbd` and `port` are PC programs. The check builds what the `Makefile` builds.

## How I checked it

- `make unsafe-check`: passes on the branch, with `main` merged in (its new code included).
- A planted `unsafe { *p }` without a comment in `heap/src/lib.rs`: the check failed with `unsafe block missing a safety comment heap/src/lib.rs:431:5`, and passed again once it was taken out.
- Each new reason read against its code. The pthread contracts were checked against the parameter names and what each function reads and writes.
- `make test64`: see the merge.

## What is still missing

- **Miri over the host tests** (K112's second half, now its own item): runs the tests in an interpreter that catches undefined behaviour - a reason that is wrong - not only a reason that is missing.
- A reason can be present and wrong; nothing but review checks what it says.

## What I learned

- **A rule nobody checks drifts.** Of some 900 places that use `unsafe`, 68 had no reason the lint could find.
- **Check that a check runs.** The host crates "passed" because they never ran.

## Testing note

From the root of TheTaaJ:

| Do | You should see |
|---|---|
| `make unsafe-check` | `unsafe-check: every unsafe block, impl and public unsafe fn says why` |
| Add `pub fn f(p: *const u8) -> u8 { unsafe { *p } }` to `heap/src/lib.rs`, then `make unsafe-check` | `error: unsafe block missing a safety comment` and the place, and `make` fails. Take it out again |

Automated: `make test64` (its `unsafe-check` part).
