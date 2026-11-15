# Directories: a Tree of Names, Changed Without Ever Writing Over Anything

*Journal entry #26 · subdirectories in TaajFS, paths in the file service, `mkdir`, and a format version that keeps an old implementation's hands off*

Until now every file on a TheTaaJ disk lived in one list. Fifteen names in it already — nine of them programs — and `ls` was becoming a wall. Every later step on the roadmap also assumes a tree: a place for programs, a place for a user's files, a place for configuration. So this entry gives TaajFS directories inside directories, teaches the file service to take **paths**, and adds `mkdir`.

It is mostly a story about three decisions:

1. what a directory *is* on the disk,
2. how to change something deep in a tree when the rule is *never write over anything that is committed*, and
3. how to stop an older implementation of the format from destroying a disk it does not understand — which I found only because I was reading the host tools for a different reason.

---

## What was there

A TaajFS volume (entries #13–#15) is:

- two **superblock** slots, A and B; the valid one with the higher generation is live, and it names the root directory's first block;
- the **root directory**: a chain of directory blocks, each holding entries `{name, inode, size, kind}`;
- for each file an **inode** block listing its data blocks.

Every change is **copy-on-write**: new blocks are written into free space, and then one write — the other superblock slot — makes the new tree the live one. Power lost before that write: the old tree, untouched. After it: the new tree, complete. There is no "in between" on the disk at any moment.

Each entry already had a `kind` field. It had only ever held one value: `TYPE_FILE = 1`.

## Decision 1: what a directory is

```rust
pub const TYPE_FILE: u32 = 1;
/// A directory inside a directory. Its entry's `inode` is not an inode
/// but the first block of its own chain of directory blocks, and its
/// `size` is 0.
pub const TYPE_DIR: u32 = 2;
```

A subdirectory is an entry of kind 2 whose block number points **straight at a directory block** — the same kind of block, in the same kind of chain, as the root. There is no inode for a directory.

Unix file systems do give a directory an inode, because the inode is where owner, permissions and timestamps live, and because several names can refer to one inode (hard links). TaajFS has none of those yet. An inode block here would be 4096 bytes holding one number. When there is metadata to keep, the entry's block can become an inode; nothing today needs it.

The consequence I liked: **the root is not special.** "The entries of the directory starting at block N" is one function, and the root is just the N the superblock names:

```rust
fn directory_chain(&mut self, first: u32) -> Result<Vec<(u32, DirBlock)>>
fn entries(&mut self, first: u32) -> Result<Vec<DirEntry>>
```

Before this step those took no argument and read `self.sb.root_dir`. Most of the change to the volume code was adding that one parameter.

## Paths

A path is names separated by `/`, from the root down:

```rust
fn components(path: &str) -> Result<Vec<&str>> {
    let names: Vec<&str> = path.split('/').filter(|name| !name.is_empty()).collect();
    let bad = |name: &&str| name.len() >= NAME_MAX || name.contains('\0') || *name == "." || *name == "..";
    if names.len() > MAX_DEPTH + 1 || names.iter().any(bad) {
        return Err(Error::Name);
    }
    Ok(names)
}
```

Three rules are in those few lines:

- **Slashes at either end, and doubled ones, mean nothing.** `/a/`, `a` and `//a` are one path, and a path with no names is the root. (A `Vec<&str>` here is a list of *views* into the original string — nothing is copied.)
- **`.` and `..` are refused.** Nothing on the disk has those names. In Unix they are real entries in every directory; here "up" is something whoever *types* paths will resolve before the path reaches the file system. A file system that never sees `..` cannot be tricked by it.
- **At most 16 directories deep.** Not because 17 is unreasonable, but because *walking a tree read from a disk must have an end that does not depend on the disk.* More on that below.

Walking down is then a loop:

```rust
fn descend(&mut self, dirs: &[&str]) -> Result<Vec<u32>> {
    let mut firsts = vec![self.sb.root_dir];
    for name in dirs {
        let here = firsts[firsts.len() - 1];
        let entry = self.entries(here)?.into_iter().find(|e| e.is_named(name)).ok_or(Error::NotFound)?;
        if entry.kind != TYPE_DIR {
            return Err(Error::NotDirectory);
        }
        firsts.push(entry.inode);
    }
    Ok(firsts)
}
```

It returns the first block of **every** directory on the way, not just the last. The next section is why.

## Decision 2: changing something deep, without writing over anything

Suppose the disk holds `docs/notes/today` and I append a line to it. Copy-on-write says:

- the file's data gets a **new** block, so
- its inode (which lists the blocks) must be a **new** inode, so
- the entry for `today` in the `notes` directory must change (new inode number) — but that directory block is committed and may not be written over — so `notes` gets a **new** copy, which starts at a new block, so
- the entry for `notes` inside `docs` must change, so `docs` gets a **new** copy, so
- the entry for `docs` inside the root must change, so the root gets a **new** copy, and
- the superblock is written to point at the new root.

One changed byte, and a new copy of every directory between it and the top:

```
        before                         after (old blocks still there, unreferenced)

   superblock ─► root                 superblock ─► root'
                 ├─ top                             ├─ top        (same blocks as before)
                 └─ docs                            └─ docs'
                    ├─ plan                            ├─ plan    (same blocks as before)
                    └─ notes                           └─ notes'
                       └─ today                           └─ today'
```

Everything *off* the path — `top`, `plan`, and all their blocks — is shared between the old tree and the new one. Only the path is copied. And it is all still one commit: the single superblock write at the end. In code:

```rust
fn rewrite_path(&mut self, dirs: &[&str], leaf: &str, replacement: Option<DirEntry>) -> Result<u32> {
    let firsts = self.descend(dirs)?;
    let mut new = self.rewrite_directory(firsts[dirs.len()], leaf, replacement)?;
    for depth in (0..dirs.len()).rev() {
        let entry = DirEntry::new(dirs[depth], new, 0, TYPE_DIR);
        new = self.rewrite_directory(firsts[depth], dirs[depth], Some(entry))?;
    }
    Ok(new)   // the first block of a new root
}
```

Bottom directory first, then each parent with the child's new block number, up to the root. `create`, `put`, `write_at`, `delete` and the new `make_directory` all end the same way: `let root = v.rewrite_path(...)?; v.commit(root)`.

This is the design real copy-on-write file systems (ZFS, Btrfs) are built on; it is sometimes called "path copying". Its cost is visible — a change 16 deep writes 16 directories — and its gift is that **atomicity for nested changes came free.** I did not have to invent anything to make `write docs/notes/today` safe against a power cut; the property the flat file system had simply still holds.

### Removing a directory

```rust
if entry.kind == TYPE_DIR && !self.entries(entry.inode)?.is_empty() {
    return Err(Error::NotEmpty);
}
```

Only an empty directory can be removed. In this design removing a whole tree would be the *easiest* thing to write — drop one entry, commit, and everything beneath is unreachable and therefore free. That ease is exactly the reason not to offer it: one mistyped name should not cost more than one thing.

### Believing nothing the disk says

At mount, TaajFS works out which blocks are in use by walking the live tree (the free map is never stored — entry #13). With one directory that was a loop over one chain. With a tree, the walk's *shape* comes from the disk — and a damaged or malicious disk could contain a directory with an entry pointing back at the root. A walk that trusts it never ends.

```rust
let mut to_visit = vec![(self.sb.root_dir, 0usize)];
while let Some((first, depth)) = to_visit.pop() {
    if depth > MAX_DEPTH {
        return Err(Error::Corrupt(format!("directories nest deeper than {MAX_DEPTH}")));
    }
    for (number, dir) in self.directory_chain(first)? {
        if self.used.get(number as usize).copied().unwrap_or(false) {
            return Err(Error::Corrupt(format!("directory block {number} is reached twice")));
        }
        ...
```

Three protections:

- **A list of directories still to visit, not a function calling itself.** Recursion depth would be chosen by the disk, and this code runs inside the file service with a small stack.
- **A depth limit.**
- **A directory block reached twice is corruption.** A directory's blocks belong to it alone, so a second arrival means a loop, or two names for one directory — and either would make "remove this directory" free blocks another name still uses.

## Decision 3: the format version — the bug I nearly shipped

There are two implementations of TaajFS: the Rust crate the OS uses, and the original C tool, kept as a reference; `make tfscheck` holds them to each other, byte for byte. I was reading the C tool only to make sure my change did not alter the bytes of a *flat* volume. Then the obvious question arrived: **what does the C tool do with a volume that has a directory on it?**

It treats every entry as a file. So it would read the directory's block *as an inode*, take whatever numbers happen to be at the inode's offsets as "this file's data blocks", and build its free map from that. Blocks really in use would look free. Its next `put` could land on live data. Silent corruption, from a tool that reported success.

The superblock has a `version` field, checked by both implementations, that had always been 1. The fix seemed one line: directories mean version 2; the C tool mounts only version 1; done.

**That fix is wrong**, and seeing why was the most useful ten minutes of this step. A commit writes *one* superblock slot. After the first version-2 commit:

```
slot A: version 1, generation 7   ← the tree as it was BEFORE the directory
slot B: version 2, generation 8   ← live
```

The C tool ignores B — wrong version — and finds A perfectly valid. It mounts **the old tree**, whose free blocks are exactly where the new tree's data now lives. I would have turned "misreads a directory" into "confidently mounts a stale file system and overwrites the current one". Worse than doing nothing.

The fix has to make *both* slots say 2 **before** any directory exists:

```rust
fn allow_directories(&mut self) -> Result<()> {
    if !self.old_format {
        return Ok(());
    }
    self.commit_as(self.sb.root_dir, VERSION_DIRECTORIES)?;
    self.commit_as(self.sb.root_dir, VERSION_DIRECTORIES)?;
    self.old_format = false;
    Ok(())
}
```

The current tree — no directories in it yet — is committed twice, unchanged, as version 2: once into each slot. Only then does `make_directory` do its real work. Consider a power cut at each point:

| Cut after | Slots | An old reader sees |
|---|---|---|
| nothing | 1, 1 | the volume, correctly |
| first commit | 1 and 2, **same tree** | the same tree as the live one — safe to use |
| second commit | 2, 2 | nothing it will mount |
| the directory's commit | 2, 2 | nothing it will mount |

There is no row where a directory exists and a slot says 1. `old_format` is worked out at mount ("did any valid slot say 1?"), so a volume cut between the two commits simply finishes the job the next time a directory is made.

A volume that never gets a directory stays version 1 forever, and stays byte-identical to what the C tool writes — the cross-check still passes unchanged. Once a volume is version 2 it stays 2, even if every directory is removed again.

The general lesson has a name in real file systems — ext4 calls them *incompatible feature flags*: **a format change that an old reader would misinterpret must make the old reader refuse, and the switch itself must be atomic from the old reader's point of view too.**

## The file service: names become paths

The protocol (entry #12) barely changed shape. A request was `[op] [name length] [name] [number] [data]`; the name is now a path of up to 200 bytes. What changed:

| | Before | Now |
|---|---|---|
| `LIST` | lists everything | lists the directory the path names (`""` = root); each entry carries a **kind** byte |
| `STAT` | size | size, then kind |
| `MKDIR` (new, op 9) | — | makes an empty directory; counts as a *change*, so the read-only channel refuses it |
| statuses | | `NotDirectory`, `IsDirectory`, `NotEmpty`, `Exists` |

**The service still remembers nothing about its clients.** Every request carries a whole path from the root. There is no "current directory" in the file service, and there will not be: that is state about a *client*, and a service that held it would lose it on restart (entry #25's lesson — a restartable service is only as good as the state it does not keep). When `cd` arrives it will live in the shell.

The check that a channel may change things is still made once, before the path is even looked at:

```rust
let changes = matches!(
    request.op,
    protocol::WRITE | protocol::TRUNCATE | protocol::REMOVE | protocol::STALL | protocol::MKDIR
);
if changes && access == Access::ReadOnly {
    return Err(Status::ReadOnly);
}
```

Adding `MKDIR` to that list was the entire permission work — the pay-off for having put the check in one place (entry #22).

### The in-memory store: no tree at all

`RamStore` — what `fs` uses with no disk, and what the kernel's self-test runs against — had to grow directories too. It did not get a tree. It kept its one sorted map, keyed by the **whole path**:

```
"docs"              Directory
"docs/notes"        Directory
"docs/notes/today"  File(…)
"docs.txt"          File(…)
"top"               File(…)
```

What is in `docs`? The entries whose key starts with `docs/` and has no further slash. A `BTreeMap` keeps keys sorted, so those sit together and `range("docs/"..)` jumps straight to them. The rules a tree would enforce by its shape are checked instead: every prefix of a new path must be a `Directory`; a directory with any key under it cannot be removed.

Two structures, one behaviour — and the same test (`fscheck`) runs against both, which is what keeps them honest. Note `docs.txt` in the example: it sorts *between* `docs` and `docs/notes`, and one of the host tests is there to make sure it is not mistaken for something inside `docs`.

## The programs

- **`ls [DIR...]`** — no argument lists the root; directories show as `dir  name/`; several arguments each get a heading; a file argument shows that file's line.
- **`mkdir DIR...`** — new; a 33-line program. Each directory must go in one that exists (`mkdir a/b` needs `a`).
- **`cat`, `write`, `append`, `rm`** — unchanged source. They pass the name to the file service, and the name is now allowed to contain slashes. `rm` removes an empty directory.
- **The shell** grants `mkdir` the read-write channel like `write` and `rm`. And because the shell's "is this command a file?" lookup just asks the file service for that name, **a program in a directory runs by its path**: `tools/given` works with no change to the shell at all.

One thing worth being plain about: the shell's rule for *which programs get the files* is by exact name (`ls`, `cat`, `write`, `append`, `rm`, `mkdir`). `tools/given` is not on the list, so it runs with the screen only — and a copy of `rm` placed at `tools/rm` would get no files either. A name is a weak thing to hang a permission on; a proper answer (permissions recorded with the program, or granted by the user per run) is a later step.

## How I checked it

**On the host** (`make kernel64-test`, part of `make test64`):

- `taajfs`: 38 tests (10 new). Nested create/read/write/delete across a remount; same name in two directories; every refusal (`NotDirectory`, `IsDirectory`, `Exists`, `NotEmpty`, bad names), each followed by a check that nothing changed; a subdirectory spanning several blocks; nesting to exactly the limit; **a looped tree and a doubly-named directory refused at mount**; and after every sequence, the free-block count of the running volume must equal that of a fresh mount — no block leaked, none counted twice.
- **Power cuts, two new tests.** One runs four nested changes and cuts the power after 0, 1, 2, … writes until all complete; after each cut the volume must mount and show *exactly* the changes that had finished, each whole. The other does the same for the first `mkdir` on a version-1 volume and checks the rule from Decision 3 at every cut point: if a directory exists, no slot says version 1; and if any slot does, it points at the live tree.
- `fileserver`: 17 tests (3 new) — directories through the protocol on both stores.
- `tfstool`: the dump now walks the tree (indented), with a test for a loop.
- **Cross-check** (`make tfscheck`): the flat checks pass unchanged (still byte-identical). New: after `mkdir`, the C tool must fail `ls`, `get`, `put` and `rm`, and the image must be bit-for-bit what it was before it tried.

**In the OS**:

- The kernel's self-test runs `fscheck` at every boot; it gained a directories section (26 checks) against the in-memory store.
- The boot test's disk now has two directories made *on the host* (`docs` with a file, `tools` with a program). The test types 12 directory commands — listed in the testing note — and expects each line of output.
- After QEMU exits, the *host* tool reads the image: `docs/deep/note`, written by the OS two directories down, must contain `nested`, and the directory `work`, made and removed in the OS, must be gone. So both directions are covered: host-made directories read by the OS, OS-made ones read by the host.

`make test64` — host tests, BIOS boot, UEFI boot, seven fault boots — passed.

One more boot, by hand rather than in the suite: a disk made with **no** directories, so that the first `mkdir` happens inside the OS. `mkdir d`, `write d/f hi`, `ls d`, `ls` all printed what they should; afterwards the host tool's dump said `format version 2`, `d/f` read back as `hi`, and the C tool — which listed the disk happily before the boot — answered `mount: no valid superblock in either slot`.

**Not tested by hand:** I have not typed any of this into a `make run64` window myself; the testing note below is derived from the logs of automated boots. The one-off boot above is not part of `make test64`, so in the suite the version switch is covered by the host tests of the same code. Your `build/taaj-disk.img` is version 1, so your first `mkdir` will take exactly that path.

## What is still missing

- **No current directory.** Every path is from the root; there is no `cd`. That is the next step, and it belongs in the shell.
- **No rename or move**, and no copy program.
- **`rm` removes one thing**; there is no recursive remove, on purpose, yet.
- **No metadata** — no owner, permissions or times, for files or directories.
- **A deep change is slow** in proportion to its depth, and directories are searched linearly. Fine at this size.
- **Names are 27 bytes** per component on TaajFS.
- **The C tool is frozen at version 1.** It is now a reference for the flat format only.
- **Permission by program name** (above).

## What I learned

- **A recursive structure wants the root to be ordinary.** Once "a directory" was a function of a block number, the root needed no special case, and the tree was mostly one new parameter.
- **Copy-on-write goes all the way up.** Changing a leaf means a new copy of every ancestor — and in exchange, atomic nested updates need no new mechanism.
- **A format has readers you are not thinking about.** The dangerous case was not my code reading new disks; it was *old* code reading them. And a version number only protects if no valid copy of the old number is left lying in the other slot.
- **Anything walked from a disk needs its own bound.** Depth limit, visited set, no recursion: the disk does not get to decide how long the mount takes.
- **Two implementations of one behaviour can have unrelated shapes** — a tree on disk, a flat sorted map in memory — as long as one test suite runs against both.
- **State about a client belongs to the client.** The file service takes whole paths so that it has nothing to forget.
- In Rust: `Vec<&str>` as borrowed views of one string; `BTreeMap::range` for "everything with this prefix"; struct update syntax (`..self.sb`) making the version one more named field in `commit_as`.

---

## Testing note

```
make run64
```

Your disk (`build/taaj-disk.img`) keeps its files between runs and has no directories yet.

**Making and using directories**

| Type | You should see |
|---|---|
| `ls` | The files as before, ending `10 files, N bytes` or similar — now including `mkdir` |
| `mkdir docs` | Nothing printed (just the prompt). *This is the first directory on your disk: the volume becomes format version 2 here* |
| `ls` | A line `     dir  docs/` among the files; the last line starts `1 directory, ` |
| `mkdir docs/notes` | Nothing |
| `write docs/notes/today hello` | `docs/notes/today: 6 bytes` |
| `append docs/notes/today again` | `docs/notes/today: 12 bytes` |
| `cat docs/notes/today` | `hello` then `again` |
| `cat /docs//notes/today` | The same — extra slashes mean nothing |
| `ls docs` | `     dir  notes/` and `1 directory, 0 files, 0 bytes` |
| `ls docs docs/notes` | `docs:` and its listing, then `docs/notes:` and `      12  today`, `1 file, 12 bytes` |
| `ls docs/notes/today` | `      12  docs/notes/today` |
| `mkdir empty` then `ls empty` | `empty` |

**What is refused**

| Type | You should see |
|---|---|
| `mkdir docs` | `mkdir: docs: there is already something with that name` |
| `mkdir a/b` | `mkdir: a/b: no such file or directory` |
| `cat docs` | `cat: docs: is a directory` |
| `write docs x` | `write: docs: is a directory` |
| `write welcome/x y` (if you have a file `welcome`; otherwise use any file's name) | `write: welcome/x: not a directory` |
| `rm docs` | `rm: docs: the directory is not empty` |
| `write docs/../x y` | `write: docs/../x: not a valid name` |
| `with readonly mkdir x` | `mkdir: x: the files are read-only here` |

**Removing, and surviving a reboot**

| Type | You should see |
|---|---|
| `rm empty` then `ls empty` | `ls: empty: no such file or directory` |
| Close QEMU, `make run64` again, `cat docs/notes/today` | `hello` and `again` — still there |
| `rm docs/notes/today`, `rm docs/notes`, `rm docs` | Nothing printed; `ls` shows no `docs/` |

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool ls   build/taaj-disk.img
tools/tfstool-rs/target/release/tfstool ls   build/taaj-disk.img docs
tools/tfstool-rs/target/release/tfstool dump build/taaj-disk.img
```

The dump should say `format version 2: may hold directories` and show `docs/` with its contents indented beneath it. And the old C tool should now refuse the disk (it prints an error instead of a listing):

```
build/tools/tfstool ls build/taaj-disk.img
```

**A program in a directory** — from the host, with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool mkdir build/taaj-disk.img tools
tools/tfstool-rs/target/release/tfstool put   build/taaj-disk.img tools/given user/target/x86_64-unknown-none/release/given
```

then `make run64` and type `tools/given a b`: `given: the screen, no files` and `given: arguments "a b"`.

Automated: `make test64` (and `make tfscheck` for the two host tools).
