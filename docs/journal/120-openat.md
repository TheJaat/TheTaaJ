# Names from a directory, and a program given one directory

*Journal entry #120 · J63 of Era 14: the `openat` family, and a sandbox made of capabilities. Written on TheJat-VM/files, on the branch `j63-openat` in TheTaaJ and TheKernel.*

## Why

POSIX added `openat` and its family (2008) for two reasons:

- **A directory that moves.** A program working in `/build/obj` names `/build/obj/a.o`, then `/build/obj/b.o`. If someone renames `/build` in between, the second name means something else, or nothing. With the directory held open, `openat(dir, "b.o")` names a file in *that* directory, wherever it has gone. Build tools, `find`, `rm -r` and `tar` use it so that a moved directory can't make them act on the wrong files.
- **A program that must stay inside one directory.** Capsicum on FreeBSD, `openat2(RESOLVE_BENEATH)` on Linux and WASI all build sandboxes from it: the program gets a directory descriptor and nothing else, and every name is taken from there.

TheTaaJ had `utimensat` alone, and only with `AT_FDCWD`, because until entry 115 there was no such thing as an open directory. Since then a directory is a channel that stays that directory, and the file service lets no name on it out of it. Entry 118 gave each program a namespace, which can be a single directory. Both reasons are now within reach, and in a microkernel the second costs nothing extra: a sandbox is just a program that was handed less.

## What it is

- **The `*at` calls**: `openat`, `fstatat` (with `AT_EMPTY_PATH`), `mkdirat`, `unlinkat` (with `AT_REMOVEDIR`), `renameat`, `faccessat`, `utimensat`, and for completeness `fchmodat`, `fchownat` and `readlinkat`. They live in libc, beside the calls they generalise. The old calls are now the new ones with `AT_FDCWD`: `stat(p)` is `fstatat(AT_FDCWD, p, 0)`, `rmdir(p)` is `unlinkat(AT_FDCWD, p, AT_REMOVEDIR)`. Each operation has one implementation.
- **Where a name goes** (`libc/src/at.rs`, `place`):
  - a name from the root, or `AT_FDCWD`: the namespace, as every other call (entry 118);
  - a plain name (`"f"`, `"sub/g"`): **on the directory's own channel**, so it is a name in that directory, wherever it is now;
  - a name that climbs (`".."` in it): by name, in the namespace, from where the directory was opened. A directory's channel takes no `..` (the service refuses it), and only the namespace knows what is above.
- **A directory descriptor can change what is in it**, if the program may. C opens directories `O_RDONLY`, always. On TheTaaJ that made the directory's channel read-only, so `openat(dir, "f", O_CREAT)` failed with `EROFS`. POSIX asks only whether the program may change the directory, never how the descriptor was opened. libc now opens a directory to change it when the program may, and to read when it may not, so the descriptor carries exactly the program's rights over that directory.
- **`posix_spawn_file_actions_addroot_np(actions, path)`**, in `spawn.h`. It is TheTaaJ's own, named as glibc's `addchdir_np` is glibc's. The new program's whole tree is the directory `path` names: `/` is that directory, and so is its current directory. It gets:
  - the directory's channel, mounted at `/`;
  - **a registry without `files` and `files.look`**, narrowed by the registry service, as the shell narrows every program's.

  So it holds no way to any file outside that directory. A program that ignored libc, or unmounted its `/`, would find nothing else to ask. The test does exactly that.

## How it works

### `place`

```rust
pub(crate) fn place(state: &mut State, dirfd: c_int, name: &str) -> Result<Place, c_int> {
    if dirfd == AT_FDCWD || name.starts_with('/')   -> the namespace, as before
    match the descriptor:  not there -> EBADF;  not a directory -> ENOTDIR
    if ".." in name                                   -> the namespace: "<opened as>/<name>"
    else                                              -> Files::new(the directory's channel), name
}
```

A `Files` made on a directory's channel is a whole client library rooted in that directory. Every call a `Files` has (stat, open, make a directory, remove, rename, touch, `read_only`) already worked on it, because since entry 115 a directory's channel speaks the same protocol as the root's. So the `*at` calls needed no new requests, only a choice of which `Files` to ask.

`Place` also keeps a clone of the descriptor's file (an `Arc`) while the request is in flight. If another thread closes the descriptor in the middle, the channel stays open until this request is done.

### `renameat`

Two names, two places. If both are the namespace, or both are the same directory's channel (`Place::on`, the channel they go on), it is one request there. Otherwise the rename goes by the two names in the namespace, where `Files::rename` says `EXDEV` if they are under different mounts, as before.

### The sandbox

```
parent:  addroot_np(&actions, "box")
         posix_spawn  ->  run: open "box" as a directory (to change, or to read)
                          ask_init:  mounts      [("/", box's channel)]       (not the parent's)
                                     registry    narrow(list() minus files, files.look)
                                     directory   "/"
child:   libc start: no "files" in its registry -> Files::new(0), and the mount at "/"
```

The child's libc start code needed nothing new. A program with mounts and no file service was already a case entry 118 handled, with handle 0 as its root, which every request fails on.

## Design decisions

- **One implementation per operation.** The old calls had been written separately, each with its own copy of the same few lines. Making them the `*at` calls with `AT_FDCWD` means `stat` and `fstatat` can't drift apart. It also moved most requests out of the descriptor table's lock: `place` takes the lock only to look at the table, and the request to the service happens after it is let go. Before, `stat`, `mkdir`, `rmdir`, `unlink`, `rename` and `access` each waited for the service while holding the table, so every other thread's `read` waited with them. `openat` still asks with the table held, because the free number it found must still be free when the file is put there. That one can be fixed by reserving the slot first.
- **A plain name on the directory's channel, `..` by name.** A plain name is where `openat` matters most: it's what build tools use, and the directory's channel makes it exact. A name with `..` can't go on that channel, and the namespace only knows the directory by the name it was opened by. If the directory was renamed after it was opened, `openat(dir, "../x")` uses the old name. Linux would follow the real parent. That needs the file service to know a directory's parent and to say what it is, a request it doesn't have. It belongs with `taajfs`' J77 (symbolic links), which has to solve the same question.
- **The sandbox removes `files` from the registry, not just from the namespace.** A namespace is libc's, in the child's own memory, and a program can change it (`taaj_unmount("/")`) or never use libc at all. What a program holds is the only boundary in a capability system. So the registry the child gets has no `files`, and the root directory's channel is the only file channel it holds. Its other services stay. A program in a box that may use the network still may: that is a separate decision, and the shell's grants already make it per program.
- **`_np`, not a new standard name.** It isn't POSIX, and the suffix says so, as glibc's and macOS's non-portable additions do. A program that wants a sandbox on Linux uses something else entirely (namespaces, seccomp), so nothing portable is lost.
- **A directory descriptor gets the program's rights, not "read-only because it was opened `O_RDONLY`".** The alternative, a separate flag to open a directory for changing, exists nowhere else, and every ported program would fail on it.

## Bugs met

- **A directory named like the program.** The first boot of `at.c` failed everything but the sandbox. It made a directory `at` in the root, where the program `at` itself was: `open("at", O_DIRECTORY)` found the program, a file, and said `ENOTDIR`, correctly. The test's directory is now `atdir`.
- **`EROFS` through a directory descriptor.** With that fixed, every change made through the directory failed with `EROFS`: C opens directories `O_RDONLY`, and the node remembered it. See "A directory descriptor can change what is in it" above. `handles.c`'s check that `open(dir, O_WRONLY | O_DIRECTORY)` is `EISDIR` still holds: asking to *write* a directory is still refused, only naming things in it is allowed.

## How I checked it

- **The `files` lane's boot test**, `handles64-test` and `aarch64-handles-test`, now runs `at.c` after `handles.c` and `namespace.c`, still one boot per architecture: 45 + 38 + 30 checks. `at.c` checks:
  - each `*at` call on a directory descriptor: made, seen, renamed, touched, removed;
  - `AT_EMPTY_PATH`, and `ENOENT` for an empty name without it;
  - `..` from a directory descriptor, and a name from the root that ignores it;
  - **the directory renamed while open**: `faccessat`, `openat` and a new file through the descriptor still reach it, under its new name;
  - `EISDIR`, `ENOTDIR`, `EBADF` where POSIX says;
  - **a child in a tree of its own**:
    - it starts in `/`, sees its directory's file at `/inside`, and doesn't see `/outside`;
    - `..` can't climb out, from a name or from a descriptor for `/`;
    - what it writes lands in `box`;
    - with its `/` unmounted, nothing at all is reachable;
  - `addroot_np` on a file: `ENOTDIR` from `posix_spawn`.
- `make test64`: see the testing note's last line.
- **Not tested**:
  - a sandboxed child that starts a program itself. It would need `init`, and could pass on only what it has: its one directory;
  - `renameat` between two different directory descriptors (it goes by name, which `rename`'s checks cover).

## What is still missing

- `..` from a directory descriptor follows the name the directory was opened by, not where it is now (see Design decisions).
- No shell command runs a program in a box yet. `with box DIR program` in the shell would be the obvious one, and the shell's grants are where it belongs.
- `linkat` and `symlinkat` don't exist: there are no links (J76, J77).
- `O_PATH` (Linux's "open a directory only to name things in it") isn't needed: every directory descriptor already is that.

## What I learned

- **`openat` exists because names are not stable and handles are.** It is the same lesson as entry 115, one level up: a path is a question ("what is called this, now?"), a descriptor is an answer that stays true. Every race of the form "checked one file, used another" (TOCTOU) is a program asking the question twice.
- **A sandbox in a capability system is subtraction.** Unix-like systems build sandboxes by adding rules: seccomp filters, AppArmor profiles, chroot plus dropped privileges. Each rule is a thing that can be wrong. Here the child is handed one channel to one directory and a registry without the file service. There is no rule to get wrong, only things it doesn't have. The test checks the strongest form: with libc's namespace removed, the child reaches nothing.
- **Rust's `Arc` in `Place` makes a race impossible that C would have to document.** Holding a clone of the descriptor's file for the length of the request means a `close` on another thread can't free the channel underneath it. In C this is the classic "use after close" on a descriptor number that another thread has already reused.

## Testing note

Start TheTaaJ:

```
make run64
```

| Type | You should see |
|---|---|
| `with programs at` | `at: 30 checks, all passed` |
| `with programs namespace` | `namespace: 38 checks, all passed` (entry 118's) |
| `with programs handles` | `handles: 45 checks, all passed` (entry 115's) |
| `ls` | no `atdir`, `moved`, `box` or `outside` left behind |

The same on AArch64 with `make run-aarch64`: `with programs at` gives the same line.

Automated: `make handles64-test aarch64-handles-test`, and `make test64`.
