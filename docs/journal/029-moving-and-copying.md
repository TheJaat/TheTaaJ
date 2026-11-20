# Moving and Copying: One Is a Single Commit, the Other Cannot Be

*Journal entry #29 · rename in TaajFS, a `RENAME` request in the file service, `mv` and `cp` — and why the two commands, which look like twins, are built in opposite ways*

With a tree (entry #26) and a place to stand in it (entry #28), the next thing I reached for was the most ordinary one: I had put a file in the wrong directory and wanted to move it. There was no way. TaajFS could create, write and delete; a file's name was fixed the moment it was made. The only "move" available was `cat` it, `write` it somewhere else by hand, and `rm` the original.

This entry adds:

- **rename** to TaajFS — a new name, a new directory, or both, for a file or a whole directory, in one commit;
- a **`RENAME`** request to the file protocol, in both stores;
- the programs **`mv`** and **`cp`**;
- `tfstool mv` on the host.

`mv` and `cp` look like the same command with one difference. Underneath they are opposites, and the entry is mostly about why.

---

## What "move" has to mean

Think about what could go wrong with the hand-made move — copy, then delete:

- Power fails after the copy and before the delete: **the file is in both places.** Annoying.
- Do it the other way round to avoid that — delete, then write: power fails in between and **the file is in neither place.** Gone.
- Move a directory with a thousand files that way and there are a thousand moments like these.

A real move must have no such moment. At every instant, including the instant the power dies, the thing is **wholly in the old place or wholly in the new one.** That is *atomicity*, and TaajFS already has the tool for it: every change builds new blocks off to the side and becomes real with one write of the superblock (entry #13). The work here was to make "take it out of one directory and put it into another" fit inside **one** such commit.

And it should not copy anything. A directory entry is 40 bytes: a name, a block number, a size, a kind. The file's contents hang off the block number. Moving the file means *moving the entry* — the megabyte of data it points at never needs to be touched.

## Rename in TaajFS

### The easy half: same directory

`rewrite_directory` (entry #26) writes a new copy of a directory with one entry replaced, found by name. Nothing says the replacement must have the *same* name:

```rust
let moved = DirEntry::new(to_name, entry.inode, entry.size, entry.kind);
...
// One directory changes: the entry is replaced, where
// it stands, by itself under the new name.
v.rewrite_path(&from_dirs, from_name, Some(moved))?
```

Same block number, same size, same kind, new name. Then, as for every change since entry #26, each directory above gets a new copy up to a new root, and one commit publishes it.

### The real half: two directories

`mv docs/notes/today archive/today` changes two directories: `docs/notes` loses an entry, `archive` gains one. Until now every operation changed exactly one directory and copied the one path above it. This one has **two paths**:

```
            root                          root''
           /    \                        /     \
        docs    archive      →       docs'     archive'
          |                            |           |
        notes                        notes'      today   ← the same entry, the same blocks
          |
        today
```

I thought about a clever version: find where the two paths part, rewrite each branch below that point, then rewrite the shared part once with both changes. It needs a function that changes two entries of one directory at once, and careful handling of the cases where one path is a prefix of the other (`mv a/b/c a/c`). Possible, and every one of those cases is a place for a bug.

The version I wrote instead is two lines:

```rust
let without = v.rewrite_path(&from_dirs, from_name, None)?;
v.rewrite_path_from(without, &to_dirs, to_name, Some(moved))?
```

1. Remove the entry from the old directory, exactly as `rm` would. That produces a new root, `without` — **written to the disk, but not committed**. The superblock still points at the old tree; nobody can see this one.
2. Add the entry to the new directory — *starting from `without`*, not from the committed root. That produces a second new root that has both changes.
3. Commit the second root.

The only new thing needed was a version of the walk-down and of `rewrite_path` that starts from a root I name instead of the committed one:

```rust
fn descend(&mut self, dirs: &[&str]) -> Result<Vec<u32>> {
    self.descend_from(self.sb.root_dir, dirs)
}

/// The same, down a tree whose root directory starts at `root` -
/// which need not be the committed one: a change made of two steps
/// walks, for its second, the tree its first step wrote.
fn descend_from(&mut self, root: u32, dirs: &[&str]) -> Result<Vec<u32>> {
```

Why is this safe? Because of what the copy-on-write rule has guaranteed all along: **blocks that are not reachable from the committed superblock do not exist.** The intermediate tree `without` — the one where the file is in *neither* place — is on the disk for a moment, but nothing points at it. If the power fails there, the next mount walks from the old superblock, never sees it, and counts its blocks as free. The dangerous state is real, but it is unreachable, and unreachable is the same as never written.

So a tree I am building may go through any number of intermediate shapes. Only the root I finally commit matters. That is a more general thing than rename: it is how any operation needing several changes at once can be made atomic here.

**The cost.** The shared part of the two paths is written twice (once in each step), and the first copy is thrown away. A move near the root wastes a block or two; a move between two directories 16 deep wastes about 16. They are free again as soon as the commit lands. But they must *exist* for a moment, so a move on a completely full disk can fail with `the file system is full` even though it keeps no space at all. There is a test for exactly that, checking that the failed move gives every block back.

### What a move must refuse

```rust
pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
    let (from_dirs, from_name) = split_leaf(from)?;
    let (to_dirs, to_name) = split_leaf(to)?;
    let entry = self.find(&from_dirs, from_name)?.ok_or(Error::NotFound)?;
    let same_directory = from_dirs == to_dirs;
    if same_directory && from_name == to_name {
        return Ok(()); // it is there already
    }
    if entry.kind == TYPE_DIR {
        let through = from_dirs.len() + 1;
        if to_dirs.len() >= through && to_dirs[..through - 1] == from_dirs[..] && to_dirs[through - 1] == from_name {
            return Err(Error::IntoItself);
        }
        if to_dirs.len() + 1 + self.height(entry.inode)? > MAX_DEPTH {
            return Err(Error::Name);
        }
    }
    if self.find(&to_dirs, to_name)?.is_some() {
        return Err(Error::Exists);
    }
    ...
```

Three refusals, each with a reason worth writing down.

**1. Nothing is replaced.** If `to` already names something, the move is refused: `there is already something with that name`. Unix's `rename()` does the opposite — it atomically replaces the destination, and a lot of software depends on that (write a new version to a temporary file, rename it over the old one). I may want that one day, as a separate, explicitly-named operation. As the *default* behaviour of `mv` it means one mistyped name silently destroys a file. Entry #26 refused to remove a non-empty directory for the same reason: *one mistyped name should not cost more than one thing.* Here it costs nothing.

**2. A directory cannot move into itself.** `mv docs docs/old`. Follow the two steps: remove `docs` from the root; then add it… inside `docs`, which is no longer reachable from the root. The result would be a directory containing itself, attached to nothing — every file in it lost, and (if it *were* reachable) a loop that entry #26's mount check exists to refuse. The test is on the names alone: is the path to the new place a path *through* the old one?

```
from      = d            from_dirs = [],      from_name = d
to        = d/inner/x    to_dirs   = [d, inner]
                                      ^ the way there begins with "d": refused
```

(`to_dirs[..through - 1] == from_dirs[..]` compares two *slices* — Rust compares them element by element, so this is "do the first few names match" in one expression.) The comparison is by whole names, so `dd` is not inside `d`; there is a test that moves `d` into `dd` to make sure.

**3. A move may not bury directories too deep.** Paths are at most 16 directories deep (entry #26), and the mount refuses a volume that nests deeper — that limit is what guarantees walking the tree ends. `mkdir` checks it. But a move can break it without making anything: take a directory that itself contains three levels, and move it to depth 15. Nothing was created, and now there are directories at depth 18 that no path can name and that **make the volume unmountable**.

I only saw this because I was writing down the list of invariants `mkdir` protects and asking, for each, "can `rename` break this without calling `mkdir`?" So before a directory moves, its *height* is measured — how many levels of directories are under it — and `depth of the new place + height` must fit:

```rust
fn height(&mut self, first: u32) -> Result<usize> {
    let mut highest = 0;
    let mut to_visit = vec![(first, 0usize)];
    while let Some((first, depth)) = to_visit.pop() {
        if depth > MAX_DEPTH {
            return Err(Error::Corrupt(format!("directories nest deeper than {MAX_DEPTH}")));
        }
        highest = highest.max(depth);
        for entry in self.entries(first)? {
            if entry.kind == TYPE_DIR {
                to_visit.push((entry.inode, depth + 1));
            }
        }
    }
    Ok(highest)
}
```

The same shape as the mount's walk: a list of directories still to visit rather than a function calling itself, and its own depth limit, because it is reading a disk. This is the one place where moving a directory costs something proportional to its size — the directories in it are read (not the files). A file moves in constant time.

### The format does not change

A rename writes directory blocks, and nothing in them is new: an entry with a different name. So the format version is untouched. A volume with no directories stays version 1 after a rename, and the old C tool — which has no rename of its own — reads the result correctly. `make tfscheck` now checks exactly that: the Rust tool renames a file on a flat volume, the C tool reads it back under the new name, and both tools' dumps of the volume agree.

## The protocol: a request with two paths

Every request so far named one path:

```
[op] [path length] [path] [number] [data]
```

A rename needs two. I did not change the layout. The second path travels in the **data** field — the part that carries a `WRITE`'s bytes:

```rust
/// Gives what the path names a new path instead: the data is the
/// new path. ...
pub const RENAME: u8 = 10;
```

```rust
protocol::RENAME => {
    // The second path travels where a WRITE's data does.
    let to = core::str::from_utf8(request.data).map_err(|_| Status::BadRequest)?;
    store.rename(name, to).map(|()| 0)
}
```

`from_utf8` is the one new check: data is bytes, a path must be text, and bytes from a client are not to be trusted to be text. Then the same path rules as everywhere else apply to it.

Two more small things in the protocol:

- `RENAME` joined the list of operations that *change* something, so the read-only channel refuses it before either path is looked at — one word added to the one `matches!` (entry #22's design paying off again).
- A new status, `IntoItself = 12`: *a directory cannot be moved into itself.* I could have reused `not a valid name`, but the name is perfectly valid; the message should say what is actually wrong.

### The in-memory store, and a bug its test caught

`RamStore` has no tree (entry #26): one sorted map from whole paths to nodes. Renaming a file is taking one key out and putting another in. Renaming a **directory** means every key under it changes:

```
"docs"              →  "papers"
"docs/notes"        →  "papers/notes"
"docs/notes/today"  →  "papers/notes/today"
```

My first version found them like this: start at the key `docs`, walk forward in sorted order, and take keys while they are `docs` itself or start with `docs/`. The host test failed:

```
assertion `left == right` failed
  left: (NotFound, [])
 right: (Ok, ["bottom", "today"])
```

After renaming `docs` to `papers`, `papers/notes` did not exist. The directory had moved and **left its contents behind**. The test happened to have a file called `docs.txt` in the root, and the sorted order is:

```
"docs"
"docs.txt"          ← '.' is 0x2E, '/' is 0x2F: this sorts BEFORE "docs/..."
"docs/notes"
"docs/notes/today"
```

The walk took `docs`, met `docs.txt` — not under `docs` — and stopped. Everything after it was never reached.

What makes this one sting: **I wrote about exactly this trap in entry #26**, about listing a directory, and wrote a test for it there. Then I made the same mistake in new code five days later. The reason the test caught it is that I had copied the `docs.txt` neighbour into the new test out of habit. The fix is to start the search at `docs/`, where the contents begin, not at `docs`:

```rust
let mut moving = alloc::vec![old.clone()];
if directory {
    moving.extend(
        self.nodes
            .range::<String, _>(&inside..)          // inside = "docs/"
            .take_while(|(path, _)| path.starts_with(&inside))
            .map(|(path, _)| path.clone()),
    );
}
```

On a real disk this could not have happened — there the contents hang off a block number, not off a spelling. It is a bug that belongs to the "paths as keys" design, and it is the second time that design has shown the same sharp edge. If it shows it a third time, `RamStore` gets a tree.

The collect-then-change shape (`moving` is built first, then the map is modified in a second loop) is forced by Rust: I cannot remove from a map while an iterator is borrowing it. The borrow checker turns "modifying a collection while walking it" from a subtle runtime bug into a compile error.

## `mv`

```rust
let to = target::target(&files, from, to);
match files.rename(from, &to) {
    Ok(()) => 0,
    Err(error) => {
        println!("mv: {from} to {to}: {}", error.describe());
        1
    }
}
```

The whole program is: work out the target, ask for one rename, report. Being one request, it inherits everything above: atomic, instant for a file of any size, and it moves a directory as easily as a file.

`target` is the one convenience, shared with `cp`:

```rust
pub fn target(files: &Files, from: &str, to: &str) -> String {
    match files.stat(to) {
        Ok((Kind::Directory, _)) => {
            let whole = files.resolve(from);
            let name = whole.rsplit('/').next().unwrap_or("");
            format!("{}/{name}", to.trim_end_matches('/'))
        }
        _ => String::from(to),
    }
}
```

If the destination is a directory, "move to it" means "move *into* it, keeping the name": `mv notes docs` makes `docs/notes`. The name comes from the *resolved* path, so that `mv ../x .` knows the thing is called `x`. (`rsplit('/')` walks the pieces from the right; its first item is the last name.)

This rule lives in the program, not in the file service. The service's `RENAME` means exactly what it says — this path becomes that path — and "into a directory" is a habit of the command line. It is also why the error message names both paths (`mv: docs to docs/docs: …`): the user typed `mv docs docs`, and should see what that was taken to mean.

Relative paths needed nothing: `Files::rename` resolves both names against the directory the program was started in (entry #28), so `mv house ../cottage` works from inside `docs` with no code in `mv` about it.

## `cp` — and why it is not like `mv`

`mv` is one request because moving is a change of *names*. Copying is a change of *data*: there must be twice as many blocks afterwards as before. Could that be one atomic request too?

A copy-on-write file system *can* do it: point a second entry at the **same** blocks and let the two files drift apart as each is written — a "reflink" (Btrfs and XFS offer this). Since TaajFS never writes over a committed block, the idea fits it. But it would end something every part of TaajFS has assumed so far without my ever stating it: **a block belongs to exactly one file.** Two implementations and a dump tool were written with that in mind, and I have not gone through them asking what a shared block does to each. Adding block sharing to the format as a side effect of wanting `cp` would be the wrong order. It is a format decision and deserves its own entry.

So `cp` is an ordinary client program, and does what any client can:

```rust
let mut copied = files.write(&to, &[]);          // an empty file first
...
let read = files.read(from, |piece| {
    if copied.is_ok() {
        copied = files.append(&to, piece);
    }
});
```

It reads the source a message's worth at a time and appends each piece to the destination. It never holds more than one message (about a kilobyte) in memory, so it copies a file of any size in constant memory.

The closure passed to `read` *captures* `copied` and `files` from the function around it and changes `copied` each time it is called — that is what makes it an `FnMut` rather than an `Fn`. A closure cannot `return` from the outer function or stop the read early, so after a failed append it simply stops appending and lets the rest of the file be read and dropped.

**What `cp` is not: atomic.** Every append is its own commit. Being honest about what that means:

| If… | Then… |
|---|---|
| the copy finishes | the copy is complete and on the disk |
| an append fails (disk full) | `cp` removes the part it made: the copy or nothing |
| the power fails partway | **a short copy is left behind**, looking like a real file |
| another program creates the destination between `cp`'s check and its first write | it is overwritten |

The last two are real gaps. The usual cure for the third is the one I said above I am not offering yet: write to a temporary name, then rename over the destination. The fourth needs a "create only if it does not exist" request. Both are listed under what is missing.

`cp` also refuses to replace an existing file (the same rule as `mv`), refuses a directory (`cp: docs: is a directory` — there is no copying of trees yet), and prints the size of what it made, as `write` does.

It is also slow for big files in a way `mv` is not: a 100 KB file is about a hundred requests, each one a commit that rewrites the path to the root. Fine for what is on the disk today.

## Who may run them

`mv` and `cp` joined `write`, `append`, `rm` and `mkdir` on the shell's list of programs that get the read-write channel. With the read-only channel they are refused by the service, not by any check of their own:

```
taaj:/> with readonly mv rooted x
mv: rooted to x: the files are read-only here
```

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How a move across directories stays atomic | Two rewrites, the second on top of the first's uncommitted root; one commit | Finding the fork and patching both branches — fewer blocks written, many more cases |
| Destination exists | Refused | Replacing it (Unix `rename`) — useful, but a typo destroys a file |
| A directory into itself | Refused, by comparing names | — (it would orphan the directory) |
| Depth after a move | Checked: place + height ≤ 16 | Not checking — the volume would stop mounting |
| The second path in the protocol | In the data field | A new request layout |
| "Into a directory" | In `mv`/`cp`, by a `stat` first | In the service — it is a command-line habit, not a file-system fact |
| `cp` | A client loop of read and append | Shared blocks (a reflink) — a format change that needs its own thinking |

## How I checked it

**On the host** (`make kernel64-test`, part of `make test64`):

- `taajfs`, 7 new tests (45 in all):
  - a rename in place: one commit, the same free-block count before, after, and after a remount; the entry keeps its position; a flat volume stays format version 1;
  - moves down, up and sideways between directories;
  - a directory moved with everything in it, and then a directory moved into what used to be its own child's place;
  - every refusal — missing source, missing or non-directory destination parent, existing destination of either kind, into itself at three depths, bad names — followed by a check that **no commit happened** (the generation number is unchanged) and no block was taken; and that `d` *can* move into `dd`;
  - the depth rule, at the limit, and a remount afterwards;
  - a move on a nearly full volume failing with `Full` and returning every block;
  - **a power cut at every write** during a file move across the tree and a directory move: after each cut the volume must mount, and each thing must be in exactly one of its two places with its contents intact.
- `fileserver`, 3 new tests (30 in all): `RENAME` through the protocol on both stores, including the read-only refusal, a second path that is not text, and the `docs.txt` neighbour that caught the bug above.
- `make tfscheck`: the C tool reads a file the Rust tool renamed; both dumps agree.

**In the OS, at every boot:** `fscheck` gained 18 checks of renaming against the in-memory store, including a rename by relative names from inside a directory.

**In the boot test** (BIOS and UEFI), 19 new lines are typed — the testing note below. After QEMU exits the host tool reads the disk and must find: `cottage` in the root holding what `home` held (it was renamed to `house`, moved into `docs`, and moved out again by `../cottage`); no `home` and no `docs/house`; `docs/welcome` equal to the original; `docs/long2` — a 4692-byte copy, made by five appends — **byte for byte** the file the host put on the disk; and `docs/deep/note` still there after its directory was moved to `tools` and back.

`make test64` and `make tfscheck` passed.

One boot test run failed on the way, and it was my own check: entry #28's test verified a file called `home` in the root, and this entry's test renames it. The OS was right; the check was stale. I changed the check to look for `cottage` instead, which proves the same thing.

**Not tested:**

- I have not typed any of this into a `make run64` window myself. The testing note uses its own file names (`box`, `shelf`, `crate`) so that it works on any disk; the automated boot does the same *kinds* of step with other names (`docs`, `tools`), and the expected lines are taken from its serial log with the names changed. Steps the automated boot has no counterpart for are marked in the note, and three more are unmarked small ones: `cat home` after the rename, `ls box` showing `empty`, and moving a directory *and* renaming it in one step (`mv shelf/box crate`; the boot moves a directory keeping its name, the host tests do both).
- `cp` cleaning up after a failed append (a full disk) is written but has never run — nothing in the tests fills the disk during a copy.
- A power cut in the middle of a `cp` is not tested; I know what it leaves (a short file) from reading the code, not from seeing it.
- Moving the directory the shell is standing in. By entry #28's reasoning the shell is left holding a name that no longer exists, and `cd` gets out. Not tried.
- How long `cp` takes for a large file. Not measured.

## What is still missing

- **`cp` is not atomic** and cannot be, without either shared blocks or a replace-by-rename.
- **No replacing rename.** Useful for "write new, swap in"; to be added as its own request when something needs it.
- **No "create only if new"**, so `cp`'s check-then-write has a gap.
- **No copying of directories** (`cp -r`), and no recursive remove.
- **`mv` and `cp` take exactly two names** — no `mv a b c dir`.
- **A move writes the shared part of the two paths twice.**
- **`RamStore` still keys by path**, and has now tripped on sort order twice.
- The earlier list stands: no metadata, 27-byte names, permission by program name, no `/bin`.

## What I learned

- **Atomic is about what is reachable, not about what is written.** The tree where the file is in neither place really is on the disk for a moment. It does not matter, because nothing points at it. Once I saw that, a two-directory change stopped being a special problem.
- **A new operation has to be checked against every invariant the old ones protect.** `mkdir` enforced the depth limit, so I had stopped thinking of it as something that could be broken. A move breaks it without creating anything.
- **Refuse by default; replace on request.** The dangerous version of an operation can be added later under its own name. It cannot be taken away once things depend on it.
- **Moving is about names and copying is about data** — which is why one is a single small request and the other is a loop, however alike the commands look.
- **Writing a lesson down is not the same as having learned it.** The `docs.txt` trap was in my own journal. What actually saved me was the habit of putting the awkward neighbour in the test.
- **A convenience belongs in the program that offers it.** "`mv x dir` means into `dir`" is not a fact about files, and the service is simpler for not knowing it.
- In Rust: comparing slices with `==`; `rsplit(..).next()` for the last piece; closures that capture and mutate (`FnMut`); `str::from_utf8` as the gate between untrusted bytes and text; and the borrow checker forcing "collect, then modify" on a map.

---

## Testing note

```
make run64
```

`make run64` puts the new programs `mv` and `cp` on your disk. The steps below make their own files, so they work on any disk. Start in the root (`cd`).

**Renaming and moving a file**

| Type | You should see |
|---|---|
| `write home yes` | `home: 4 bytes` |
| `mv home house` | Nothing printed |
| `cat house` | `yes` |
| `cat home` | `cat: home: no such file or directory` |
| `mkdir box` | Nothing |
| `mv house box` | Nothing — it goes *into* the directory |
| `ls box/house` | `       4  box/house` |
| `cd box` then `mv house ../cottage` then `cd` | Nothing; prompt back to `taaj:/> ` |
| `cat cottage` | `yes` |
| `ls box` | `empty` |

**Moving a directory**

| Type | You should see |
|---|---|
| `mkdir box/inner` then `write box/inner/note nested` | `box/inner/note: 7 bytes` |
| `mkdir shelf` then `mv box shelf` | Nothing |
| `cat shelf/box/inner/note` | `nested` — the directory moved with everything in it |
| `mv shelf/box crate` | Nothing — moved and renamed in one step |
| `ls crate` | `     dir  inner/` and `1 directory, 0 files, 0 bytes` |

**Copying**

| Type | You should see |
|---|---|
| `cp cottage copy` | `copy: 4 bytes` |
| `cat copy` | `yes` |
| `cp cottage crate` | `crate/cottage: 4 bytes` |
| `cp ls crate/ls2` | `crate/ls2: N bytes` — the same N that `ls ls` shows. (This one takes a moment: it is many requests.) |
| `crate/ls2` | A listing of the root — the copy of the program runs. *(Not in the automated test.)* |

**What is refused**

| Type | You should see |
|---|---|
| `mv crate crate` | `mv: crate to crate/crate: a directory cannot be moved into itself` |
| `mv crate crate/inner/x` | `mv: crate to crate/inner/x: a directory cannot be moved into itself` *(not in the automated test)* |
| `mv copy cottage` | `mv: copy to cottage: there is already something with that name` |
| `cp copy cottage` | `cp: cottage: there is already something with that name` |
| `mv none x` | `mv: none to x: no such file or directory` |
| `mv copy nowhere/x` | `mv: copy to nowhere/x: no such file or directory` *(not in the automated test)* |
| `cp crate x` | `cp: crate: is a directory` |
| `mv one` | `usage: mv FROM TO` |
| `with readonly mv copy x` | `mv: copy to x: the files are read-only here` |
| `cat copy` | `yes` — none of the refusals changed anything |

A refused command is also followed by a line such as `mv exited with code 1`.

**Surviving a reboot** — close QEMU, `make run64` again:

| Type | You should see |
|---|---|
| `cat cottage` | `yes` |
| `cat crate/inner/note` | `nested` |

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool ls  build/taaj-disk.img crate
tools/tfstool-rs/target/release/tfstool mv  build/taaj-disk.img copy copy2
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img copy2
```

The first lists `cottage`, `inner/` and `ls2`; the second prints `mv: ok`; the third prints `yes`.

**Cleaning up**, back in the OS: `rm copy2 cottage crate/cottage crate/ls2 crate/inner/note`, then `rm crate/inner`, `rm crate`, `rm shelf`.

Automated: `make test64` (and `make tfscheck` for the two host tools).
