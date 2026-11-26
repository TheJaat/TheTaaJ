# TaajFS version 6: file times, volumes and files over 256 MiB, a rename that replaces, and programs found in /bin

*Journal entry #101 · The-Jat's step D (`docs/who-does-what.md`), on the branch `taajfs-6`. The work list called this step "TaajFS version 4", but versions 4 and 5 already happened: change stamps (entry 081), long names (J19, entry 094) and faster writes (J20, entry 093) came in Era 6. What was left of the step:
- **J17, file times**: when each file last changed, to the nanosecond, for `stat`, `make` and `ls -l`;
- **J18, files over 256 MiB**: really volumes over 256 MiB, which was the limit;
- **J35, a rename that replaces**, as Unix's does;
- **J55, `/bin`**: the shell looks there now, as libc's exec has since entry 100.*

## Why

LLVM 23 (step E) and rustc (step H) both run into the file system as it was:
- **No times.** `stat` gave every file its change stamp as its time: a counter that only grows, shown as that many seconds after 1970. That was enough for `make` to tell which file is newer. But it is not a time a person can read, and a file put on the disk by the host and one written on TheTaaJ could not be compared with anything outside the disk.
- **256 MiB was the most a volume could hold**, so no file could be bigger. A volume's superblock counted at most 65536 blocks of 4 KiB. LLVM's static libraries and rustc's 182 MB binary with its build products need more than that.
- **`rename` refused to replace.** If the new name existed, rename failed with EEXIST. Every program that writes a file safely writes `x.tmp` and then renames it over `x`: rustc and cargo, `make`'s targets, editors. Here each of them needed a `remove` first, and between the two calls there was no `x` at all.
- **Programs only in the root.** libc's exec has looked in `/` and then `/bin` since entry 100, but the shell only looked in the root.

## How it fits together

```
 TheTaaJ                                  the disk (TaajFS)                        the host
 ─────────────────────────────────        ─────────────────────────────────        ──────────────────────
 fs (user/src/bin/fs.rs)                  superblock: total blocks up to 2^20       tfstool
   mount -> allow_times()  ────────────►  version 6 in both slots, once             open -> set_clock(host
   set_clock(taaj::realtime_ns)           ("raise_version", as before)                     SystemTime)
                                                                                   (never raises a version:
 write/touch -> a new inode               timed inode, magic "JAFU":               the C tool's crosscheck
   time = max(clock, newest + 1)  ─────►  magic size kind count stamp time            stays byte for byte)
                                          then 1016 block numbers
 STAT reply: kind size stamp time         (stamped: 1020; old: 1022)
   (25 bytes; was 17)
        │
 libc stat: st_mtim = time if non-zero,
   else the stamp in seconds (old files),
   1 for a directory
 ls -l: the time as a UTC date
 rename(a, b): replaces b, in one commit
 shell: name -> /name, else /bin/name
```

**Times.** A version 6 volume writes every inode in the timed layout: a 32-byte header with the time, after the stamp, as nanoseconds since 1970. `Volume::time_now` takes the clock's time, but never less than one nanosecond after the newest time already on the volume (`newest`, found while the free map is rebuilt at mount). So a file written later always has the later time, even when TheTaaJ's clock and the host's disagree. TheTaaJ's clock starts from the real-time clock's whole seconds; the host's is NTP's. The volume never stores a time that goes backwards, which is the one thing `make` needs.

**Bigger volumes.** `MAX_BLOCKS` is now 2^20, so a volume can be 4 GiB. Code older than this refuses a superblock that counts more than 65536 blocks, because its `is_valid` checks the count against its own maximum. So a big volume needs no version number of its own: an old TheTaaJ will not mount it either way. A file can now be as big as its inode reaches: 1016 index blocks of 1024 block numbers of 4 KiB, just under 4 GiB. Offsets in the protocol and the volume are 32 bits, so that still fits.

**Rename.** `Volume::rename` now does what POSIX says:
- a file over a file: the old one's blocks are freed in the same commit;
- a directory over an empty directory;
- otherwise it is refused: EISDIR (a file onto a directory), ENOTDIR (a directory onto a file), ENOTEMPTY (onto a directory with things in it).

The commit is the one TaajFS always makes, a new superblock naming a new tree. After a crash the disk holds either the old names or the new ones, never neither. The in-memory store (`fileserver/src/ram.rs`) behaves the same way.

**The shell.** `program_path` turns a bare name into `/name` if that file exists, else `/bin/name` if that does. Otherwise it keeps the root's name, so the "unknown command" message names it.

## Design decisions

- **A sixth version, not a field in the old inodes.** The stamped inode had no room in its header for a time. Growing the header costs 4 block numbers per inode (1020 to 1016), which changes where every block number is. An older TheTaaJ reading a timed inode as a stamped one would read garbage, so the volume says version 6 and older code refuses it. That is the same rule as versions 2 to 5 (`raise_version`).
- **The volume is raised when TheTaaJ mounts it, the host's tool never raises it.** `fs` calls `allow_times` at mount, the way it did for stamps. `tfstool` writes whatever the volume already is. That keeps `crosscheck.sh` meaningful: on a fresh volume the Rust tool and the C tool still write identical bytes. Once TheTaaJ has raised a disk, the host's tool writes timed inodes with the host's clock.
- **Times never go backwards on one volume.** The alternative was to trust the clocks. The test disk shows why I didn't: the host's tool writes "big" at the host's time, then TheTaaJ writes files at its own, which can be a second behind (whole seconds from the RTC). `make` would then think a fresh object file older than its source. With `max(clock, newest + 1)` a skew shows up as a few nanoseconds' nudge, not a wrong order.
- **Files from before version 6 keep their stamps.** libc gives an untimed file its stamp as seconds after 1970, as before, so it is always older than any timed file, which is the truth. A directory gets 1. Directories have no inode, so they have no time.
- **Setting a time explicitly still fails** (`utimensat` with a time gives EPERM). TheTaaJ can't keep a time that goes back. "Now" works, as a touch.
- **J55 is only half done.** The disk recipes (`make run-disk`), `check-boot.sh` and most tests put programs in the root and look for them there. Moving all of them is a change to every test, and it doesn't need a new format. The search is the part programs depend on. The move stays on the work list.

## Bugs

- **A test gave a big inode two index blocks where one was enough.** It worked out the index count from the old 1020 slots instead of the timed 1016. I fixed the test, and the arithmetic now lives in one place (`format::slots_for`).
- **A borrow error:** `v.time(&v.lookup(""))` borrows `v` mutably twice. `lookup` returns a value that borrows nothing, so a separate `let` does it.
- **The first boot of the new test:** all 43 checks passed, but `ls -l` was "unknown command". The test disk had only `times` on it. Now `ls` goes in `/bin` too, which also checks the shell's search a second time.
- **The first `test64` failed in two places, both tests written for the old rules:**
  - **boot64 and its UEFI twin** type `mv rooted welcome` and expected "there is already something with that name". Now the rename replaced `welcome`, and the check that `rooted` was still in the root failed. That step now types `mv docs rooted`, a directory onto a file, which is still refused ("not a directory").
  - **clib**, built by clang and by tcc on TheTaaJ, compared `st_mtime`, whole seconds: "the file written later has the later st_mtime". With stamps every change was a new "second". With real times, two changes a millisecond apart are in the same second. clib now compares the whole `st_mtim`, as `make` does (pdpmake's `timespec_le`, entry 100). Any program that compares only seconds would see such files as the same age, which is what Linux shows too.
- **Then two of my own.** My clib change named a variable `before`, and the same function already has a `before` further down: clang refused it, and the second run stopped after 10 seconds. My first cstdio change renamed the moved file back afterwards, so the cleanup's `remove` of both names found one missing ("the C program's scratch files were left behind"). The check is now one `check` again, so cstdio's count stays 52, which the boot test expects exactly. The file is made again, empty, for the cleanup.
- **The pipe tests (x86_64 and AArch64) wanted "format version 5"** in the dump, as proof that long names were allowed. The disk is version 6 now. They accept 5 or later: what they check is that long names are allowed, and every later version allows them.
- **The host's `get` complained "Broken pipe"** when `dd` stopped reading after 18 bytes. That is harmless; the test sends it to `/dev/null`.

## How I checked it

- `cargo test` in `taajfs` (78 tests), `fileserver` (43), `tfstool-rs`, with new tests:
  - a timed inode round trip;
  - a superblock over 256 MiB that only new code accepts;
  - times from the clock that never go back;
  - a timed volume holding big files with four fewer slots;
  - rename replacing a file, an empty directory, and refusing the rest, on a volume, in memory and on a disk;
  - a disk saying when a file changed.
- `crosscheck.sh`: the C and Rust tools still agree byte for byte.
- `times.c` with `times-test.sh`, on x86_64 and AArch64 (`times64-test`, `aarch64-times-test`, now in `test64`):
  - a 320 MiB volume (81920 blocks) with a 300 MiB file the host put on it;
  - TheTaaJ reads it at seven places either side of 256 MiB, writes past 290 MiB, and appends a MiB;
  - times are real and ordered to the nanosecond, a touch makes a file newer, a rename keeps the time;
  - rename replaces and refuses as above;
  - `times` and `ls` are only in `/bin`;
  - `ls -l` shows `315621376  2026-11-25 22:36:16  big`;
  - afterwards the host reads TheTaaJ's bytes past 256 MiB and its added MiB, `dump` says version 6, and a file the host's tool writes then is the newest on the disk.
- fscheck's rename section and `cstdio.c` changed to the new behaviour (both run in `test64`).
- Mutations, each put back afterwards:
  - `time_now` without the `max`: `times_come_from_the_clock_and_never_go_back` fails.
  - Timed inodes never written (`timing()` false): that test and `a_timed_volume_holds_big_files...` fail.
  - `MAX_BLOCKS` back to 65536: the superblock test and the layout test fail.
  - The shell not looking in `/bin`: `unknown command: times` and `unknown command: ls`, and the boot test fails.
  - libc ignoring the time: exactly three checks fail ("its time is a time of day", "by TheTaaJ's clock", "a time of day for it"). The ordering checks still pass on stamps alone, which is the fallback working as intended.
- `make test64`: all passed in 14 min 58 s, on the fifth run. The first four found the old-rule tests and my two slips listed under Bugs. The boot64 "did not run all 237 commands" warning is the soft one from entry 100; boot64 itself passed.

## What is still missing

- **Programs in `/bin` on the run disks** (J55's other half): the disk recipes and the tests still put them in the root.
- **Times you set yourself** (`utimensat` with a time, `touch -d`) and access times. There is one time, the last change.
- **Directory times.** A directory has no inode, so its entries carry no time.
- **Volumes over 4 GiB and files over 4 GiB**: 32-bit block numbers and offsets. That is enough for rustc; the next limit is a format change.
- **`ls -l` shows files only**: a directory's line has no time, since it has none to show.

## What I learned

- **Order matters more than accuracy for a build tool.** `make` only asks "is this older than that". A clock that is a second off is harmless; a clock that runs backwards is not. Keeping the newest time per volume and nudging past it makes the disk itself the source of order, the way the stamps already were. The stamps remain the fallback.
- **A bigger limit that old code already refuses needs no version.** The version number is for things old code would misread. A superblock with too many blocks is something old code already turns away.
- **Howard Hinnant's days-to-civil algorithm** (in `ls`): start the year in March, so February, the month whose length varies, comes last, and count in 400-year eras of exactly 146 097 days. Then a date is a few divisions with no tables or loops. I checked it against Python's `datetime` on 100 000 random times.
- **Rust:** `fn() -> u64` as a field (`clock: Option<fn() -> u64>`). A plain function pointer, not a closure, carries no state and no lifetime, so `Volume` stays free of generics for it. The host passes a function reading `SystemTime`, TheTaaJ passes `taaj::realtime_ns`, the tests pass a fixed one.
- **Rust:** `is_some_and` (`arguments.first().is_some_and(|first| first == "-l")`) tests what an `Option` holds without unwrapping it. `usize::from(bool)` then turns the answer into how many arguments to skip.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`.

| Type | You should see |
|---|---|
| `write t one` then `ls -l` | `t` with its size and today's date and time (UTC) |
| `write u two` then `ls -l` | `u` a little later than `t` |
| `mv u t` then `ls` | only `t`: the rename replaced it; `cat t` says `two` |
| `mkdir d` then `mv d t` | `mv: d to t: not a directory`: a directory does not replace a file |
| `mv t d` then `ls d` | `t` went into `d` (a directory as TO means "into it", as before) |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make times64-test
make aarch64-times-test
make test64
```
