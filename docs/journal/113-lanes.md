# Fourteen lanes: the eras worked in parallel, and a brief for every new session

*Journal entry #113 · a working step, not a feature. Written on TheJat-VM, on the branch `era-plan`. After entry 112 gave every part of TheTaaJ an era, the user asked to "make the new era's this way that we can rapidly develop all of them within a week using different sessions and with the TheJat-WSL, TheJat-VM and MaK-WSL, with different sessions of inside of them. Put full context that we don't have to explain it to the new sessions."*

## Why

Entry 112 left about 270 open items in Eras 12 to 29, written as one list per era. Two things stood in the way of working on them at once:

- **Nothing said who takes which item.** The order was per machine (TheJat-VM's steps A-J, MaK-WSL's A-I). That fits one session per machine, not several.
- **Every new session had to be told the same things again.** That meant the goals, the rules, how a branch is merged, which repository is whose, what not to touch, and how the user wants the journal written. Most of it was in `who-does-what.md`. The rest was only in what the user had said to earlier sessions: write as the user, explain Rust, do all the work, never kill someone else's QEMU, never touch the user's disk images.

## What it is

- **`docs/start-here.md`**: everything a new session needs, in eight parts:
  1. who you are (machine and lane);
  2. what TheTaaJ is, with a map of the repositories and directories;
  3. the user's wishes;
  4. setting up;
  5. doing an item, from picking it to committing;
  6. merging;
  7. what never to do;
  8. what to do when something is unclear.
- **`docs/lanes.md`**: fourteen lanes, five on TheJat-WSL, four on MaK-WSL and five on TheJat-VM. Each lane has its own files, its order in waves, and what it waits for from other lanes. It also lists what is not in a lane this week.
- **`tools/new-session.sh LANE`**: makes a session's own tree, `TheTaaJ-LANE` next to the checkout:
  - a worktree at `origin/main`;
  - its submodules;
  - the machine's i686 cross compiler linked in.
- **`tools/lanes-check.py`**: checks that every open item of `who-does-what.md` is in exactly one lane, or named as not in one, and that no lane holds a done item.
- **`docs/who-does-what.md`**:
  - a pointer to both files at the top;
  - rule 13 names the script, the test lock, and that the user's checkout is not built in;
  - rule 15: when a moved `main` needs a new `make test64`;
  - rule 16: `abi` numbers are given at merge;
  - each session gets a row in its machine's "Now" table;
  - "The decided order" says where each old step's items went.

A session starts with one line typed by the user: `You are TheJat-WSL/ipc. Read docs/start-here.md.`

## How the lanes were cut

A lane is **a set of files**, not an era. Two sessions conflict when they change the same lines, so each lane owns the files its items change:

| Lane | Machine | Owns | Eras |
|---|---|---|---|
| `ipc` | TheJat-WSL | `channel.rs`, `ipc.rs`, `cap.rs`, `services.rs` | 13 (IPC), 24 |
| `memory` | TheJat-WSL | `memory/`, `mm.rs`, `memory_object.rs`, `paging.rs` | 16, parts of 9 and 18 |
| `ci` | TheJat-WSL | the test rules, the boot-test scripts, `tools/` | 25, 22 |
| `network` | TheJat-WSL | `net/`, the network service, `sockets.rs` | 12 |
| `services` | TheJat-WSL | `init`, the registry | 23, parts of 18 |
| `sched` | MaK-WSL | `sched.rs`, `smp.rs`, `futex.rs`, `alarms.rs` | 13 (threads), 17 |
| `boot` | MaK-WSL | TheBootloader, `bootinfo.rs`, `elf.rs` | 21 |
| `devices` | MaK-WSL | `pci.rs`, `devices.rs`, `interrupts.rs`, `acpi.rs`, the drivers | 15, the real PC, ACPI, 10 |
| `desktop` | MaK-WSL | `gui/`, `gfx/`, `input/`, `display`, the terminal | 19 |
| `files` | TheJat-VM | `descriptors.rs`, `fileserver/`, the file protocol | 14 (VFS) |
| `taajfs` | TheJat-VM | `taajfs/`, `tfstool-rs` | 14 (TaajFS 7) |
| `libc` | TheJat-VM | the rest of `libc/`, the shell | 20 |
| `crypto` | TheJat-VM | new crates | 29, 26 |
| `arch` | TheJat-VM | `arch/`, the linker scripts | 28 |

Where an item's files belong to another lane, the item goes to that lane. Some examples:
- K69, per-CPU kernel heaps, is Era 13 but changes `mm.rs`, so it is `memory`'s.
- K103, counting kernel objects, is Era 18's milestone, and it is `memory`'s too.
- K25, ASLR, is `boot`'s, since the kernel's half needs a relocatable kernel.

The machines follow from what each is good at:
- **TheJat-WSL** has KVM and the most CPUs, so it gets the kernel's hot paths (`ipc`, `memory`), whose benchmarks need real speed, and the network, whose tests need real time. It also gets `ci`: Era 25 already named it as the CI machine.
- **MaK-WSL** keeps the scheduler, where its step A (K20) is in progress, and the bootloader, which only it changes (rule 7). It also keeps the real PC's devices and the desktop.
- **TheJat-VM** is emulated, so it gets what boots least: the VFS, TaajFS, libc, and the crypto crates, tested on the host. Its `arch` lane makes new CPUs, which are emulated everywhere anyway.

MaK-WSL's old step D, the network, moved to TheJat-WSL. A laptop with four sessions would otherwise have the most work and the least memory.

## Design decisions

- **The status stays in one place.** `lanes.md` holds each lane's order, never whether an item is done. "Done (entry NNN)" stays in `who-does-what.md`, so the two files can't disagree. `lanes-check.py` catches an item that is in no lane, or in two.
- **The cross-lane waits are written down** ("Across lanes"): which item waits for which other lane's. A session whose next item isn't ready takes its lane's next ready one. It never does the other lane's item in its own branch.
- **The 3 MiB wall first.** The kernel image has about 680 KiB left, and every lane adds tests that go into it. Two first-wave items end that limit, J51 (`ci`) and K130 (`boot`), so they are early in both lanes.
- **One `make test64` per machine, by a lock.** Rule 13 already said only one per machine, because the timing tests fail when two compete for the CPUs. Five sessions can't keep that by agreement. `flock /tmp/taaj-test64.lock make test64` waits for the lock instead. `/tmp` is per machine, so the three machines don't wait for each other.
- **Rule 15: a moved `main` doesn't always need `make test64` again.** Rule 2 asked for it again whenever new commits change anything outside `docs/`. With fourteen sessions merging, `main` moves during almost every run, and a branch could wait all day. The full run is needed only when the new commits change a file the branch changes, or `kernel/abi`. Otherwise the item's own tests are run again. The risk, written in the rule: two changes tested apart can break together. `main`'s own test run (J93) is what finds that, so J93 is in `ci`'s first wave. **This loosens the user's rule 2**, so I asked first: the user agreed to it when this entry was merged ("yes merge it with rule 15").
- **Rule 16: `abi` numbers given at merge**, like journal numbers. Two lanes adding a system call on the same day would both take number 45. The second to merge takes the next free one. Everything outside `abi` uses the name (`abi::call::NAME`), so that change is one line.
- **New test parts go beside their area, not at the end of `TEST64_PARTS`.** Git sees two lines added at the same place as a conflict, even when they are different lines. Each lane adding its parts at the end would conflict every time.
- **Sessions never build in the machine's first checkout.** It holds the user's own disk images (`build/taaj-disk.img`, `build/aarch64/taaj-disk.img`), which `make run64` and `make run-aarch64` write. A session works in its own tree, where those images are its own.
- **One tree per session, kept for the whole lane.** The first `make test64` in a new tree builds the ports: LLVM, Clang, lld and libc++, about 40 minutes. A tree per item would pay that every time. I thought about copying another tree's `build/`, but `tools/build-llvm.py` rebuilds by file times, and a fresh checkout's sources are all newer than any copied object. It would rebuild anyway, unless the copied files' times were touched, and that would hide a real change. Paying once per session is simpler and always right.
- **The approval of `lanes.md` is the go-ahead.** A session starts its lane's next item without asking, and asks only before merging (rule 2). Without that, fourteen sessions would wait on the user at the start of every item as well as at its end.
- **A week can't finish them all, and the plan says so.** About 270 items, many L-sized, are more than fourteen sessions can merge in a week. The week's aim is every lane started, each benchmark first, and each lane's first wave merged. The waves after that continue on the same lanes.

## How I checked it

- `python3 tools/lanes-check.py` found what my first draft missed. E2 and E3 (done by K148 and K145), J54 (done by J92) and K51 (done by K131) were open but in no lane. So were Era 9's J14-J25, now named as not in a lane while step E is paused. The final run gave: "270 open items; 258 rows in 14 lanes; 12 named as not in a lane", then "every open item is in one lane".
- `tools/new-session.sh`:
  - `sh -n` and `dash -n` passed; `--help` and an unknown lane (which lists the fourteen) both work.
  - A real run for `crypto` made `TheTaaJ-crypto` at `origin/main`, with TheKernel at `b544ca5`, TheBootloader at `73d962c` and `toolchain/i686` linked. A second run said "already there".
  - The first run left `git status` showing the link as untracked: `toolchain/.gitignore` says `i686/`, which matches a directory but not a link to one. The script now adds `/i686` to the toolchain repository's `info/exclude`. After running it again, `git status` in the tree and in `toolchain` is empty.
- The `make` targets that `start-here.md` names (`iso64`, `test64-shared`, `run64-uefi`, `run-aarch64`, `kernel64-test`) exist in the `Makefile`, and `flock` is on the machine.
- **Not run**: `make test64`, since this changes only `docs/` and two tools that nothing in the build calls. The new tree's first build was not run either.

## What is still missing

- The "Now" tables get a row per session only as each session starts: each machine edits its own (rule 6).
- `tools/new-session.sh` makes trees only beside the checkout it is run from, which is what all three machines have.
- Once sessions have run for a day or two, the waves should be measured against what they really merge, and the next week's plan sized from that.
- Rule 15 needs `main`'s own test run (J93) to be safe. Until J93 runs, a combined break is found by the next session's `make test64`.

## What I learned

How finely work is split is decided by the files more than the topics. An era is the right unit for asking "what is done", but a lane, the files one session changes, is the right unit for working in parallel. The two only line up where an era's code is in one place. Era 18, security, touches nearly every file, so it is spread over five lanes. A check that runs, `lanes-check.py`, kept the two views in step better than reading did: it found five items my reading missed.

## Testing note

Nothing to boot: this changes `docs/` and adds two tools.

```
python3 tools/lanes-check.py
sh tools/new-session.sh --help
sh tools/new-session.sh nope
sh tools/new-session.sh crypto
git -C ../TheTaaJ-crypto status --short
grep -n '^15\. \|^16\. ' docs/who-does-what.md
```

| Do | You should see |
|---|---|
| `python3 tools/lanes-check.py` | `270 open items; 258 rows in 14 lanes; 12 named as not in a lane`, then `lanes-check: every open item is in one lane` (the counts change as items are done or added) |
| `sh tools/new-session.sh --help` | the script's comment: what it makes and why |
| `sh tools/new-session.sh nope` | `no lane 'nope'`, then the fourteen lanes, `ipc` to `arch` |
| `sh tools/new-session.sh crypto` | `Lane 'crypto' has its tree: .../TheTaaJ-crypto` and the next steps, or `already there` |
| `git -C ../TheTaaJ-crypto status --short` | nothing |
| the `grep` for rules 15 and 16 | "When `main` moved after `make test64` passed" and "Numbers in code are given at merge too" |
| `docs/start-here.md` | eight parts, from "Who you are" to "When something is unclear" |
| `docs/lanes.md` | "Starting a session", the machines and their lanes, "Across lanes", then fourteen lanes, each with a table of items and waves |

Automated: none (`make test64` doesn't read these files).
