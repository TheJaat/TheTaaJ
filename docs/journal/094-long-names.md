# Long names: up to 64 bytes on TaajFS

*Journal entry #94 · Era 6, last planned step. A file or directory on TaajFS can now have a name of up to 64 bytes. Before, the limit was 27. A name that doesn't fit a directory slot takes a second slot right after it, and the volume becomes version 5 before the first such name is written. A directory with only short names is byte for byte what it was, so the C tool still reads every old volume. C programs get `NAME_MAX` (64) in limits.h and `ENAMETOOLONG` for a longer name. clang's headers now all go on the disk: 11 of them had names too long before.*

## Why

The limit kept getting in the way of the self-hosting era:
- **clang's headers.** Eleven of them have names of 28 to 36 bytes: CUDA's, HIP's, Hexagon's, and `avx512vlvp2intersectintrin.h`, which `<immintrin.h>` includes. The clang test and `make llvm-put` left them off the disk.
- **LLVM's sources.** Some of AArch64's `.td` files have 28-byte names.
- **libc++.** 42 of its 1,017 headers have names over 27 bytes (the longest is 43), so C++ couldn't be compiled on TheTaaJ.

The file service itself always allowed 64 bytes (`COMPONENT_MAX`), and its memory store took them. Only the disk said no.

## How it fits together

A directory block is 12 bytes of header and then 102 slots of 40 bytes. A slot holds 28 bytes of name (NUL-terminated, so 27 usable), then the inode, the size and the type. Version 5 adds one more kind of slot:

```
 short name (1..27 bytes): one slot, exactly as since version 1
   [ name, NUL-padded (28)      ][ inode ][ size ][ type 1|2 ]

 long name (28..64 bytes): two slots, side by side in one block
   [ name 0..28 (28)            ][ inode ][ size ][ type 1|2 ]
   [ name 28..64, NUL-padded (36)                 ][ type 3   ]   <- TYPE_MORE
```

The second slot has no inode or size; its type field says `TYPE_MORE` (3), and everything before it is the name's tail.

**The format** (`taajfs/src/format.rs`):
- `DirEntry.name` is 64 bytes now.
- `slots()` says how many slots an entry takes: 2 when its name is 28 bytes or more (it doesn't fit the first slot with a NUL after it).
- `write_to` writes one or two slots.
- `DirBlock::parse` joins a `TYPE_MORE` slot to the entry before it. A `TYPE_MORE` slot with no entry before it is nothing a volume writes, so it is read as an unused slot, not trusted as a file.
- The block's `count` is the number of slots used, as before, so the C tool walks the same bytes.

**The volume** (`taajfs/src/volume.rs`):
- `components()` allows names of up to `LONG_NAME_MAX` (64).
- `pack()` cuts a directory's entries into blocks by slots, so a long name never straddles two blocks. With only short names it cuts exactly as `chunks(102)` did, which is how C cuts them.
- `allow_name(name)` raises the volume to version 5 if the name is long. `create`, `make_directory`, `put` and `rename` call it after their checks and before their change. It is never called inside the change, because raising the version is itself two commits (see the decisions).

**The file service** (`fileserver/src/disk.rs`): the disk store's name check uses `LONG_NAME_MAX` instead of the slot's 28. That was the only line between programs and long names.

**The C library**: `limits.h` has `NAME_MAX` 64. libc's `text()`, which every function taking a path uses to read it, gives `ENAMETOOLONG` for a name over 64 bytes or a path of `PATH_MAX` or more. Before, the file service's "bad name" became `EINVAL`, which said nothing useful.

**The tools**: `tfstool dump` says what version 5 may hold. `clang-test.sh` and `put-llvm.sh` put every header on the disk now; they still leave out and list any name over 64.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where the rest of the name goes | A second slot right after the first, in the same block | Wider slots for every name (say 72 bytes): every directory on every disk would change, the C tool would read none of them, and short names, which are most names, would waste the room. A separate name block per directory: one more block to read, write and commit for every lookup and every change |
| The longest name | 64, the file service's `COMPONENT_MAX` | 255 like Linux: up to eight slots per name, for names nobody here has. 64 covers every header of clang and libc++, and every LLVM source |
| Telling a second slot apart | Its type field, at the same offset as every slot's type | A flag in the name bytes: a name may hold any byte but NUL and `/`, so nothing in them is safe to use |
| Old readers | Version 5, raised in both superblock slots before the first long name is written | Version 4 plus a hope: older code would take the second slot for a file, whose "inode" is four letters of the name |
| When to raise | Only when a long name is about to be written | On mount, as `allow_stamps` is: a volume with no long names would stop being readable by older code for nothing |
| A stray second slot | Read as unused | Refusing the whole block: a damaged slot would hide every other name in it |

## Bugs

- **`mount` capped "the lowest version" at 4.** It started from `VERSION_STAMPS` and took the minimum over both slots, so a version-5 volume looked like version 4 after every mount. `raise_version(5)` would then commit the tree twice more before every long name, forever. Nothing would have broken; every long name would just have cost three commits instead of one. I found it while reading `mount`, before any test ran. `long_names_survive_a_remount_and_make_the_volume_version_5` now checks that a long name written after a remount is exactly one commit, and putting the cap back fails it.
- **Three of my tests were wrong, not the code:**
  - the packing test expected a long name plus 101 short ones to leave 1 entry for the next block (it leaves 2: the long name and 100 short ones fill 102 slots);
  - `refusals_change_nothing` counted free blocks after leaving my new 64-byte file on the disk;
  - pipes.c's "50-byte" name was 51 bytes.
- **tfstool's dump wording.** I changed "N entries" to "N slots" in tfstool-rs's dump, then saw that the C tool prints the same line and `crosscheck.sh` compares the two dumps. I put it back. (`count` was always a slot count; with only short names the two are the same.)

## How I checked it

**Host tests:**
- `cargo test` in `taajfs`: 64 → 72.
  - `a_long_name_takes_a_second_slot`: names of 5, 28, 56, 64 and 27 bytes in one block, 8 slots, the second slot's bytes and type where they belong, and back. The first 28 bytes alone are not the name.
  - `short_names_are_laid_out_as_before`: a full block of short names, byte for byte the old layout.
  - `a_second_slot_alone_is_an_unused_one`.
  - `long_names_survive_a_remount_and_make_the_volume_version_5`: 27 bytes keeps version 1; a 40-byte file, a 64-byte file, a 28-byte directory and a file inside it; both slots say 5; all listed and read after a remount; then one long name is one commit.
  - `a_long_name_can_be_renamed_to_and_from_and_deleted`: a rename into a long name raises the version; then moved to another directory, renamed short, deleted.
  - `a_long_name_is_never_split_between_blocks`: `pack()` with 101 short names and then a long one, a long one first, only short names (cut as C cuts them), and none.
  - `a_directory_of_long_and_short_names_fills_its_blocks`: 160 names, two thirds long, some deleted and renamed. After a remount, every block holds at most 102 slots, and every block but the last at least 101.
  - `the_first_long_name_locks_old_readers_out_before_it_exists`: a power cut after every possible number of writes. Whenever the long name is there, no valid slot says less than 5.
- `fileserver`: still 40 tests. The disk store takes 64 bytes, refuses 65, and renames a directory to a 40-byte name and back.
- `make tfscheck`: the C and Rust tools still agree on version-1 volumes.
- tfstool put, list, get and dump of a 54-byte name on the host.

**On TheTaaJ:**
- `pipes.c`: 108 → 119 checks, in `long_names()`:
  - `NAME_MAX` is 64;
  - a 50-byte file is made, written, read back and listed;
  - it is renamed to 64 bytes; `stat` finds the new name, and `ENOENT` for the old;
  - `open` and `mkdir` of a 65-byte name give `ENAMETOOLONG` and make nothing;
  - a 40-byte directory, and sh writing a 50-byte file in it (`pipes emit inside > dir/file`), read back and listed.

  pipe-test.sh then reads both files off the disk with the host's tfstool and checks that the dump says version 5.
- `make pipe64-test` and `make aarch64-pipe-test` passed.
- `make clang64-test` passed (10 min). It no longer prints "clang's headers not on the disk", and `avx512vlvp2intersectintrin.h` reads back off its disk.

**Mutation experiments**, each undone:

| What I broke | What happened |
|---|---|
| `mount` caps the lowest version at 4 again | 1 test failed: `long_names_survive_a_remount…` (3 commits, not 1) |
| `pack` counts entries, not slots | 2 failed: `a_long_name_is_never_split…`, `a_directory_of_long_and_short_names…` (a block overfilled) |
| `rename` doesn't raise the version | 1 failed: `a_long_name_can_be_renamed…` |
| `put` doesn't raise the version | 1 failed: `the_first_long_name_locks_old_readers_out…` (version 1 slots, with a long name in the tree) |
| `DirBlock::parse` doesn't join a second slot | 5 failed |
| A stray second slot is read as an entry | 1 failed: `a_second_slot_alone_is_an_unused_one` |
| libc's `text()` doesn't check name lengths (pipe64-test) | 2 checks failed: "65 bytes: ENAMETOOLONG", "for mkdir too" (errno was 22, EINVAL) |

`make test64` passed, in 21 min 0 s. That's longer than entry 093's 13 min 26 s, because test64 runs the clang tests, and they now put 11 more headers on their disks. I didn't measure how much of the difference that is; the machine was not otherwise idle either.

I checked the testing note with a scripted boot, on a scratch disk made by `make run-disk RUN_DISK=build/n94.img`. Every line said what the note says. On the host afterwards:
- `tfstool dump` said version 5;
- `short.c`, copied through the two long names and moved back, was byte for byte `tccgen.c`.

## What is still missing

- **The C tool reads only version 1**, so it can't read a disk with a long name. It already couldn't read one with a directory, so nothing changes for the disks TheTaaJ uses.
- **Names over 64 bytes.** Nothing on TheTaaJ or in the ports needs them; the file service's protocol would have to change too.
- **C++ on TheTaaJ's clang.** All of libc++'s header names fit now (the longest is 43), but putting them on the disk and compiling C++ there is its own step.
- **`<immintrin.h>` with TheTaaJ's clang**: its last missing header is on the disk now, but no test includes it yet.
- **Renaming a short name to a long one in a full directory block** moves entries between blocks, so the directory's blocks after it are all rewritten. That's correct, just not minimal; every change rewrites the directory anyway.

## What I learned

- **Leave room in a format, then use it.** Every slot already had a type field, and only 1 and 2 were taken. A third value, read by new code and refused by old code through the version number, was enough to make names longer without moving a single existing byte.
- **A version raise must happen before the change, not inside it.** Raising commits the unchanged tree into both superblock slots. Doing it inside a change would commit half a change. Doing it after would leave a moment where an old slot points at a tree with a long name in it. The power-cut test tries every moment.
- **Reading the code finds what the tests don't think to ask.** The version cap in `mount` would have passed every test I had written. It showed up only because I read `mount` to see where `lowest` came from.

**Rust, as used here:**
- **Destructuring assignment**: `(start, used) = (i, 0);` sets two existing variables at once, without `let`. It's been in Rust since 1.59.
- **Closures that borrow**: in `DirBlock::parse`, `let slot = |i| &block[...]` and `let is_more = |i| ...` capture `block` by reference, so the loop reads like a description: "if the next slot is a second slot, join it". Each closure has its own unnamed type, which is why they are bound with `let` and not written as a type.
- **`iter().map(DirEntry::slots).sum::<usize>()`**: a method can be passed by its path where a closure is expected; `DirEntry::slots` is the function `fn(&DirEntry) -> usize`. `sum` needs to be told what to add up into, hence the `::<usize>` ("turbofish").
- **Slices of one Vec as blocks**: `pack` returns `Vec<&[DirEntry]>`, views into the entries, not copies. The borrow checker makes sure the entries outlive the views.
- **`pub use`** re-exports: `taaj::files` now re-exports `COMPONENT_MAX`, so libc gets the file service's limit through the crate it already depends on, and can't drift from it.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. The first long name makes your disk version 5. The Rust `tfstool`, which `make` uses, reads it; the C one never read a disk with directories anyway.

| Type | You should see |
|---|---|
| `with files mkdir a-directory-whose-name-is-longer-than-27` | nothing: the directory's name is 40 bytes |
| `with files cp src/tcc/tccgen.c a-directory-whose-name-is-longer-than-27/the-code-generator-of-tcc-under-a-long-name.c` | `...under-a-long-name.c: 273655 bytes` |
| `with files ls a-directory-whose-name-is-longer-than-27` | `273655  the-code-generator-of-tcc-under-a-long-name.c`, the whole name, then `1 file, 273655 bytes` |
| `with files wc a-directory-whose-name-is-longer-than-27/the-code-generator-of-tcc-under-a-long-name.c` | `9197 lines, 32363 words, 273655 bytes` |
| `with files write this-name-is-sixty-five-bytes-long-which-is-one-more-than-allowed x` | `write: this-name-...-than-allowed: not a valid name`: 65 bytes is one too many |
| `with files mv a-directory-whose-name-is-longer-than-27/the-code-generator-of-tcc-under-a-long-name.c short.c` | nothing; then `with files ls a-directory-whose-name-is-longer-than-27` says `empty` |

After closing QEMU, on the host: `tools/tfstool-rs/target/release/tfstool dump build/taaj-disk.img | grep version` says `format version 5`.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 119 checks ... names of up to 64 bytes on the disk` | long names from C, from sh, and read back by the host's tool; the same for aarch64 |
| `clang64-test passed`, with no "headers not on the disk" line | every clang header is on its disk |
| `test64: all passed` | every part |

Automated: make test64
