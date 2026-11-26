# Open files as handles: a descriptor is the file, not its name

*Journal entry #NNN · J60 and J61 of Era 14 (with J36): the first step of a VFS with the kernel left out. Written on TheJat-VM/files, on the branch `j60-file-handles` in TheTaaJ and TheKernel.*

## Why

Until now an open file in TheTaaJ was three things kept by libc in the program's own memory: a path, an offset and the flags. The file service remembered nothing between requests, and every request named the whole path from the root: "512 bytes of /docs/notes from 1024". It was simple, and the service could be restarted without losing anything, but a descriptor was tied to the **name**:

- rename a file a program has open, and its next read finds whatever has that name now, or nothing;
- remove it, and its next write makes a new file with the old name (J36);
- an open file can't be handed to another program, only its name can, and the name may mean something else by the time the child uses it.

Unix programs expect the opposite, and some depend on it: a compiler writes a temporary file and renames it into place while it is still open, a build tool removes a file another program is reading. Era 14 needs it as well, because a VFS with the kernel left out (entry 112) is built from open directories as handles.

## What it is

- **`OPEN`**, a new request in the file protocol (`fileserver::protocol`). It names a path and a set of flags (`READ`, `WRITE`, `DIRECTORY`, `CREATE`, `EXCLUSIVE`, `TRUNCATE`), and brings one end of a new channel. The service keeps that end: from then on, every request that arrives on it is about that file or directory.
- **One protocol on every channel** (J61). A file's channel takes the same requests as before (READ, WRITE, STAT, TRUNCATE, TOUCH, READ_INTO, WRITE_FROM, the streams) with no name in them. A directory's channel takes names, from that directory. The service's own two request channels are just the root directory, open to change and open to read. Whichever channel a request comes on, one function answers it.
- **The service knows where each open file is** (`fileserver/src/nodes.rs`):
  - a rename moves the paths of the open files it touches, including everything inside a renamed directory;
  - a removal of an open file only takes its name away. The file waits in a hidden directory, `.taaj-removed`, until its last channel closes;
  - a rename that replaces an open file sets the replaced one aside the same way.
- **`taaj::files::File`**: the client side of an open file, in the library both Rust programs and libc use. `Files::open` gives one. It reads, writes, truncates, stats, touches, lists and makes streams, and closes its channel when dropped.
- **libc's descriptors hold a `File`**:
  - `read`, `write`, `pread`, `pwrite`, `lseek(SEEK_END)`, `fstat`, `ftruncate` and `futimens` all go to the file's own channel;
  - `open` is one request, with its checks done by the service, so nothing can change between them;
  - a descriptor handed to a child by `posix_spawn` is the file's channel, not its name.
- **The kernel's limit on one wait** (`WAIT_HANDLES_MAX`) goes from 32 to 256. The file service now waits on every open file's channel, and the kernel itself is otherwise unchanged.

## How it works

### A node per channel

The service keeps a table of **nodes**, one per channel it serves:

```rust
pub struct Node {
    pub path: String,    // where it is now, from the root of the volume
    pub kind: Kind,      // file or directory
    pub access: Access,  // what its channel allows
    removed: bool,       // a file removed while open: it waits in REMOVED
    gone: bool,          // a directory that was removed
}
```

Slots 0 and 1 are the root, open to change (`ROOT`) and open to read (`ROOT_READ_ONLY`): the service's two request channels from init. Every request goes through `Nodes::resolve`, which turns the name it carries into a path from the root:

- **a file's channel**: the name must be empty, and the path is the node's;
- **a directory's channel**: the name is joined onto the node's path. It is a name *inside* the directory: `..` was never a valid name in the file protocol, so there is no way up and out of the directory a channel was given for. J63's sandboxes build on that.

The resolved request then goes to the same code that answered every request before. Nothing in the `Store`s (memory and TaajFS) changed: they still take whole paths, and the table is what makes a path follow a file.

### What a channel allows

A node's access is fixed when it is opened, and can only shrink:

| Opened on | With `WRITE` | Without |
|---|---|---|
| A channel that may change files | may change | read-only |
| A read-only channel | refused: `ReadOnly` | read-only |

So a program can be handed one file to read, and that is all it can do with it, whoever it hands it on to. That is the capability model of the Goals: what you can do is what you were given.

### Following a rename

```
RENAME d -> e        nodes:  "d"   -> "e"
                             "d/c" -> "e/c"     (inside it)
```

After a successful `Store::rename`, every node whose path is `from`, or starts with `from/`, gets its path rewritten. Removed and gone nodes are left alone.

### Outliving a removal

Unix keeps a file that is unlinked while open: the name goes, and the file goes with its last descriptor. Here:

1. `REMOVE x` on a file some node has open makes `.taaj-removed` (if it is not there yet) and renames `x` to `.taaj-removed/N`. The nodes on `x` follow it and are marked removed.
2. The name `x` is free: a new file can be made there, and it is a different file.
3. When the last node on `.taaj-removed/N` closes, the file is removed, and `.taaj-removed` too once it is empty.

`.taaj-removed` is the service's own:
- a name sent by a client can't reach it (`resolve` answers NotFound);
- `LIST` of the root leaves it out, with the entry numbers counted without it, so a client paging through the root neither skips a name nor sees one twice;
- if the service stops, or the power does, while it holds files, `Nodes::new` empties it the next time the service starts, as a Unix file system empties its orphan list at mount. The host test `a_removed_open_file_left_by_a_stop_is_removed_at_the_start` mounts a volume again in the middle and checks the blocks come back.

A rename that replaces an open file is the same case: the replaced file is set aside first, then the rename happens. If the rename then fails, the file is put back.

### The service's loop

`user/src/bin/fs.rs` watched its two request channels and its streams. It now also watches each open file's channel:

```
watched = [requests, look, streams..., open files...]
```

A request on any of them goes to the node that channel stands for. An `OPEN` keeps the channel end that came with it as the new node's. When a client closes every handle to an open file, the service hears `PeerClosed`, and `Nodes::close` lets the node go, removing the file if it was the last on a removed one.

### libc

```rust
pub(crate) struct File {
    pub(crate) file: taaj::files::File,  // the channel; closed when the last dup goes
    pub(crate) path: String,             // the name it was opened by: fchdir, fdopendir
    pub(crate) offset: u64,
    pub(crate) flags: c_int,
}
```

The descriptors that `dup` makes share one `File` through an `Arc` (Rust's counted shared pointer). When the last `Arc` goes, the `taaj::files::File` inside is dropped, and its `Drop` closes the channel. That is Rust's way of saying "close when nobody uses it any more", which C does with an explicit reference count.

`open` maps C's flags to the service's (`open_flags`) and sends one `OPEN`. Before, it asked `stat`, then decided, then made or emptied the file, three requests with room for another program in between. Now the service does all of it in one request.

`posix_spawn` hands a file on as its channel: `abi::descriptors`' `Entry::File` now brings a handle, a copy made by `File::share`. A file given as standard input or output still becomes a stream, now opened on the file's own channel, so it is the file even if it was renamed.

## Design decisions

- **Kept by the service, not by the kernel.** A node is a row in a table in the file service. The kernel sees a channel and nothing else, and knows no files, paths or open-file objects: the Goals' pure microkernel. The only kernel-side change is a bigger number in `abi`.
- **Paths kept up to date, not inode numbers.** The `Store`s work by path, and TaajFS's copy-on-write commits a change by rewriting the directories above it, from a path. Keeping the open files' paths right as renames happen needed no change in either store, and no format change. Opening by inode would have meant a second way into every store. The cost: a rename walks the open-file table (at most 200 rows).
- **Removal by setting aside, not by keeping an unlinked inode.** TaajFS rebuilds its free map by walking the tree at mount (until J73), so an inode outside the tree would have its blocks reused. A file in `.taaj-removed` is still in the tree, safe from that, and found at the next start if the service stops. This is NFS's "silly rename", done by the service itself and hidden from clients.
- **One protocol, the root as a directory.** The two request channels init gives every program are the root open twice. A Rust program or a libc that knows only path requests works unchanged, and a directory handed to a program works the same way rooted somewhere else. That is J62's mount table and J63's sandboxes, without a second protocol.
- **`WAIT_HANDLES_MAX` raised from 32 to 256, not a port now.** The service waits on every open file in one `object_wait`, and the kernel's limit was 32. The right answer is a port, one wait queue that many channels report to, so the cost doesn't grow with the number of handles. That is kernel IPC work, the `ipc` lane's, so I added it there as `K?-port`. Until then the limit is 256, with the cost proportional to it (each wait joins every handle's queue). The file service keeps at most 200 open (`OPEN_MAX`) and answers `Busy` past that, which libc gives as `ENFILE`. A compile-time assertion in `fs.rs` checks that the root's two channels, the streams and the open files fit in one wait.
- **The offset stays in the client.** As before, a descriptor's offset is libc's, and every read says where. The service would otherwise need a place per channel, and a child given the file would share it in ways POSIX only half promises. A child gets its own copy of where the parent had got to, as before.
- **A file opened to read may still be stamped.** POSIX lets anyone who may write a file set its times to now, whatever the descriptor was opened for: `futimens(fd, NULL)` on an `O_RDONLY` descriptor works. So a node remembers one more thing, `may_touch`: whether the channel it was opened from could change files. If so, `TOUCH` is allowed on it even when it was opened to read. It is the only change such a node allows, and a file opened from a read-only channel can't even do that.
- **Directories may be opened `WRITE`**, meaning "may change what is in it". libc never asks that (C opens directories only to read), but J63 will hand a program a writable directory.

What the choices cost: an open is now one more channel per file, which holds kernel memory while the file is open (K103 will count it), and a file service with many open files waits on many queues (`K?-port`).

## Bugs met

- **A lock held across an assignment** (`libc/src/spawn.rs`). Making a stream from a file given as standard output took the file's lock, through a pattern that borrowed the table entry, and then replaced that same entry. Rust refused to compile it ("cannot assign to `*source` because it is borrowed"). The guard returned by `lock()` lives until the end of its block, and the borrow it holds keeps the entry borrowed. Putting the lock in a block of its own, so the guard is dropped first, made the order explicit: take what is needed, let go, then change the entry. In C this would have compiled, and been a bug only if the lock protected the entry.
- **`fstat` read its answer under the descriptor table's lock** in the first version. A request to the service can wait (`patience`), and every other thread wanting a descriptor would have waited with it (`descriptors.rs`' rule: nothing that may wait is done holding the table). Rewritten so the table only hands out the shared file, and the service is asked after it is let go.

- **`futimens` on a file opened to read** (`fileserver/src/nodes.rs`). The first full `make test64` failed in `tcc64-test`: `clib`, the C library's own checks compiled on TheTaaJ by tcc, printed `clib: FAILED: futimens`. It opens a file `O_RDONLY` and stamps it with `futimens(fd, NULL)`. Before this entry libc stamped the file by its name, on the root channel that may change files, so it worked. Now it goes through the file's own channel, opened to read, and the service refused it with `ReadOnly`. That is stricter than POSIX, which asks only that the caller may write the file, not that the descriptor was opened to write. The fix is `may_touch` (see Design decisions), with a host check both ways and a 45th check in `handles.c`. None of my own checks had stamped a file opened to read: the C library's older checks found what mine missed, and that is what running all of `make test64` is for.
- **A clock check under load.** The same first run also failed one check in `basics64-test`: "the time of day and the time since boot move together", which allows 2 ms between two pairs of clock readings 50 ms apart. It has nothing to do with files and passed when run again on its own. Under `make test64` many QEMU machines share TheJat-VM, which has no KVM, so a machine can be paused for more than 2 ms between two readings. I note it here and leave it alone. If it fails again, its margin should be widened.

## How I checked it

- **Host tests** (`cargo test` in `fileserver`), 48 in all, 5 of them new:
  - `open_files_follow_their_file`, run on both the memory store and a TaajFS volume, covers:
    - renames, into a directory and of the directory it is in;
    - a removal while open;
    - a replacing rename;
    - the hidden directory, which can't be listed or reached;
    - what a channel allows, and every open flag;
    - an empty directory removed while open;
  - removed-but-open files left by a stop, removed at the next start;
  - paging through the root's listing one entry at a time, with the hidden directory in the middle;
  - the 200-file limit, and a freed slot used again.
- `kernel/abi`'s 33 tests, one of them changed (a file's descriptor entry now has a handle).
- **A boot test of its own**, `handles64-test` and `aarch64-handles-test`, which runs `handles.c` on TheTaaJ in one boot: 45 checks. They cover:
  - writes after a rename;
  - `fstat`, `lseek(SEEK_END)`, `ftruncate` and `pread` after the file and then its directory were renamed;
  - a removed file still read, written and stat'ed, with `.taaj-removed` not listed and a new file at the old name;
  - a reader keeping the old file through a replacing rename;
  - a child given the file as descriptor 3 *after* it was renamed, writing into it;
  - and what `open` refuses (`EBADF` both ways, `EEXIST`, `ENOTDIR`, `EISDIR` twice, `ENOENT`), with `fdopendir` on a directory descriptor.

  Both passed on the first run, on TheJat-VM, while the LLVM port was still compiling in the background.
- `make test64`: see the testing note's last line. This entry's merge waits for it.
- **Not tested**:
  - more than a few open files at once on TheTaaJ (the 200 limit only on the host);
  - a crash of the `fs` service while a removed file is open, on TheTaaJ (only on the host, by mounting again);
  - the speed. J70, the file-system benchmark (the `taajfs` lane's first item), will measure what an open costs.

## What is still missing

- **A port** (`K?-port`, `ipc` lane): until then, 200 open files per service.
- **Streams follow names**: a shell's `> file` opens a stream by path, and the stream keeps the path. A file renamed while a program writes to it through `>` is not followed (descriptors are).
- **libc's `Open` is still a closed list** of kinds (file, socket, pipe ends, the three standard ones). J62 makes "a handle that speaks the file protocol" the general case, with the namespace per process.
- **`fchdir` and `fdopendir` still use the name** the directory was opened by: libc's current directory is a path (J62 changes that).
- **`isatty`, `/dev`, `/tmp`, `/proc`**: J64-J66, on this protocol.
- **Rust programs** (`ls`, `cp`, the shell) still use path requests. They work as before, and gain nothing from this until they open files.

## What I learned

- **A name and a file are different things**, and Unix's file descriptors are what makes that visible: everything about rename-while-open and unlink-while-open follows from "the descriptor holds the file". A stateless protocol, which TheTaaJ had, is simpler and survives restarts, but it can only offer names. The cost of offering files is state in the server, and with it questions the old design never had to answer: what happens to that state on a rename, on a removal, on a crash.
- **The orphan problem is old.** Unix file systems keep an orphan list for files unlinked while open, so a crash doesn't leak their blocks. NFS, which is stateless like TheTaaJ was, renames the file to `.nfsXXXX` instead. TheTaaJ now does what NFS does, inside the server, and cleans up at start, as an orphan list does.
- **Rust's `Drop` and `Arc` give "close when the last user is done" for free.** In C, `dup` means a reference count kept by hand, and getting it wrong is a leaked or double-closed handle. Here the type system keeps the count.

## Testing note

Start TheTaaJ:

```
make run64
```

| Type | You should see |
|---|---|
| `with programs handles` | `handles: 45 checks, all passed` |
| `ls` | no `.taaj-removed`, and none of the test's files (`a`, `b`, `d`, `doomed`, `x`, `moved`, `f`, `g`) left |
| `echo hello > note` then `cat note` | `hello`: the shell's streams work as before |
| `mv note memo` then `cat memo` | `hello` |
| `rm memo` then `ls` | `memo` is gone, and there is no `.taaj-removed` |

The same on AArch64 with `make run-aarch64`: `with programs handles` gives the same line.

On the host:

```
cd fileserver && cargo test
```

| Look for | You should see |
|---|---|
| `nodes::tests::open_files_follow_their_file_in_memory` | `ok` |
| `nodes::tests::open_files_follow_their_file_on_disk` | `ok` |
| `nodes::tests::a_removed_open_file_left_by_a_stop_is_removed_at_the_start` | `ok` |
| `nodes::tests::listing_the_root_pages_past_the_hidden_directory` | `ok` |
| `nodes::tests::open_files_are_counted` | `ok` |
| the last line | `test result: ok. 48 passed` |

Automated: `make handles64-test aarch64-handles-test`, and `make test64`.
