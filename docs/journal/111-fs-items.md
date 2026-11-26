# The TaajFS items: speed, scale and checking

*Journal entry #111 · a work-list change, not a feature. Written on TheJat-VM, on the branch `fs-items`. The user asked how good TaajFS is, and then for what was found to become work items, on a branch of their own. Ten J70-J79 items in `docs/who-does-what.md`, under "TaajFS: speed, scale and checking", numbered at merge (rule 5). Entry 112, merged with this one, made them half of Era 14 after all - the user asked for every area as an era.*

## Why

A look at TaajFS (`taajfs/src/format.rs`, `volume.rs`, `cache.rs`, and the file service `user/src/bin/fs.rs`) to answer "how good is our file system?" found it strong where it matters most and a starter everywhere else:

- **Strong**: copy-on-write - a block the live superblock reaches is never overwritten, a change is committed by writing one superblock into the slot that isn't live, so a power cut leaves the old tree or the new one, never a mix. Each request is on the disk before its reply. Six format versions, each upgraded in place. One crate shared with `tfstool`, so the format can't drift. 79 unit tests, some with a disk that pretends to lose power.
- **Starter**:
  - every request is its own synchronous commit, rewriting the inode, every directory up to the root and the superblock;
  - a name is found by reading its directory from the start (`DIR_ENTRIES` per block, a chain of blocks);
  - there is no free map on disk: `mount` walks the whole tree to rebuild it (`volume.rs` says so at its top);
  - one request at a time, and one disk request in flight (K10);
  - no checksums: a block the disk returns wrong is read back as if right;
  - `MAX_BLOCKS` is 2^20 blocks of 4 KiB, 4 GiB, and a file's size is 32 bits;
  - no owners, permission bits, links, or times besides the modification time;
  - `mmap` of a file is a copy written back (entry 103), not shared pages.

## The items

| Item | What | Format change |
|---|---|---|
| J70 | The benchmark: small files, a big file, a big directory, mount time, blocks written per request | No |
| J71 | Group commits | No |
| J72 | Directories found by hash or B-tree | Version 7 |
| J73 | The free map on the disk | Version 7 |
| J74 | Checksums, kept with the pointer to the block | Version 7 |
| J75 | 64-bit sizes and block numbers | Version 7 |
| J76 | Owners, permission bits, access and change times | Version 7 |
| J77 | Symbolic and hard links | Version 7 |
| J78 | More than one request at a time | No |
| J79 | A page cache shared with programs | No |

E1 (`fsck`), E4 (a redirected file in one commit) and J36 (descriptors that follow the file) already exist; the section points to them rather than repeating them. J71 covers E4.

## Design decisions

- **Not a new era.** The user asked whether this would be one. Eras in TheTaaJ are milestones someone can see - rustc running, the real PC booting, sound playing - and every one of these items serves an era that exists: the speed items serve Era 9's builds, the size and permission items Era 7's real PC and K12's installer. So they are a section of The-Jat's list, like "Memory, also needed for big builds", and not an era of their own. If the user wants a storage milestone anyway, the section can become one without changing the items.
- **One format change, not seven.** Six of the items change what is on the disk. Done one by one they would be six format versions and six in-place upgrades; done together they are TaajFS version 7, designed once and upgraded once. Group commits, more requests at a time and the page cache don't change the format and can come before it.
- **Measure first**, as with the IPC items: the benchmark comes before any change.
- **Checksums with the pointer**, not in the block: a block that holds its own checksum can be wrong and still agree with itself (a write that went to the wrong place). Kept where the block is pointed to from, the tree checks itself from the superblock down.
- **Crash safety is not on the list.** It is already right, and every item must keep it: the power-loss tests run against each one.
- **In The-Jat's list.** TheJat-VM wrote TaajFS versions 4 to 6 (entries 093, 094, 101). Not put in any machine's decided order: that is for the user to say.

## How I checked it

- Each claim against the code: `MAX_BLOCKS = 1 << 20` and `BLOCK_SIZE = 4096` in `format.rs`; the directory entry's 32-bit `size` (`DirEntry::new(name, inode, size: u32, kind)`); "there is no free map on disk; it is rebuilt from the tree" at the top of `volume.rs`, and `mount` picking the newest superblock and rebuilding the free map; `NAME_MAX` 28 plus `MORE_NAME` 36, 64 bytes.
- Nothing was built or run: this changes only documents.

## What is still missing

- The items themselves, version 7's design, and a decision on when they are done.

## What I learned

- **Crash safety and corruption are different problems.** Copy-on-write guarantees that what was committed is consistent; it says nothing about whether the disk gives back what was written. ZFS and Btrfs pair copy-on-write with checksums for exactly this reason.
- **Format changes are cheapest in batches.** Every version is an upgrade path that must be kept and tested forever, so it pays to gather the changes that need one.

## Testing note

Nothing to boot: this changes only `docs/`. To see the items:

```
grep -n '^| J70 \|^| J79 ' docs/who-does-what.md
```

| Look for | You should see |
|---|---|
| `J70` ... `J79` | ten rows under "TaajFS version 7", in Era 14 (entry 112 moved them there) |
