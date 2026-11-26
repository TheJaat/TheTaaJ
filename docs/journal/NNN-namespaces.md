# A namespace per process: a directory seen where a program chooses

*Journal entry #NNN · J62 of Era 14: the second step of a VFS with the kernel left out. Written on TheJat-VM/files, on the branch `j62-namespaces` in TheTaaJ and TheKernel.*

## Why

Every name a TheTaaJ program used, `/docs/notes` or `../plan`, went to one place: the root of the one file service, through the channel the registry gave it. That had two consequences:

- **One tree for everybody.** A second disk, a `/tmp` in memory, a `/dev`, a `/proc` (J64-J67) would each need a place in that one tree, and the only program that could make a place was the file service itself.
- **No way to give a program less.** A program could be handed the whole tree, read-only or not, and nothing in between. A sandbox, "this program sees only its own directory" (J63), had nothing to build on.

Unix solves the first with `mount`, which is the kernel's: a table of file systems, one for the whole machine (or, since Linux's mount namespaces, one per group of processes). A pure microkernel has no file systems to mount. But since entry 115 it has something better: a directory is a channel, a handle the program holds. A mount is then nothing more than **a handle and a path, in the program's own memory**, and Plan 9 showed that a per-process namespace built that way is enough for everything Unix uses mounts for.

## What it is

- **`taaj_mount(at, directory, flags)`** and **`taaj_unmount(at)`**, in the new header `taaj.h` (what TheTaaJ's C library has that POSIX doesn't). From the moment `taaj_mount` returns, the directory `directory` names is also seen at `at`, in place of whatever was there, which stays out of sight until `taaj_unmount(at)`. `TAAJ_MOUNT_READ_ONLY` makes everything under the mount read-only.
- **The table is in `taaj::files::Files`** (`user/src/files.rs`), the client library both libc and Rust programs use. Every request a `Files` sends now goes through `route`, which picks the channel and the name to send on it.
- **The rules a C program sees**:
  - the **deepest mount wins**: with `/a` and `/a/b` both mounted, `/a/b/c` goes to `/a/b`'s directory;
  - **`..` is worked out by name in the namespace**: `/m/..` is `/`, whatever is mounted at `/m`, so a program can go up past a mount and back down somewhere else, as on Unix;
  - **`rename` between two mounts fails with `EXDEV`**, as between two file systems on Unix (`mv` then copies and removes);
  - **a mount point can't be removed or renamed**: `EBUSY`;
  - **a listing shows a mount** in the directory it is in, as a directory, even if the directory underneath has nothing by that name. `at` need not exist.
- **A child gets its parent's mounts.** `posix_spawn` sends each mount, with a copy of its directory's channel, on the channel that already carries descriptors 3 and up (`abi::descriptors`' new `MOUNT` entry). libc's start reads them into the child's own table. After that the two tables are separate: a mount made by the child is not the parent's, and one made by the parent later is not the child's.
- **No kernel change, no file-service change.** The kernel sees channels. The file service sees requests on a directory's channel, with names inside that directory, as it did since entry 115. Only `abi` gained a message kind.

## How it works

### Routing a name

```rust
fn route(&self, name: &str) -> (Handle, String, bool) {
    let whole = self.path(name);                 // "../m/f" from "/x" -> "m/f"
    let deepest = self.mounts.iter()
        .filter_map(|mount| under(&mount.at, &whole).map(|rest| (mount, rest)))
        .max_by_key(|(mount, _)| mount.at.len());
    match deepest {
        Some((mount, rest)) => (mount.directory.channel(), rest.into(), rest.is_empty()),
        None => (self.requests, whole, false),   // the service's root, as before
    }
}
```

1. The name is resolved, `.` and `..` included, against the current directory, by name (`fileserver::path`), as before. This is where `..` crosses mounts: by the time a name is routed it is a path from the root of the namespace, with no `..` left in it.
2. The longest mount path the result is under wins.
3. What goes on the wire is the **rest** of the path, a name inside the mounted directory. The service's `Nodes::resolve` (entry 115) joins it onto the directory's own path, and never lets it out of that directory: a name on a directory's channel can't contain `..`. So a mount can't be escaped through the service, only through the namespace, which is the program's own.

`list`, `rename`, `remove` and `make_directory` need one more thing from `route`: whether the name *is* a mount (the rest is empty). That is what gives `EBUSY`, and what keeps `mkdir` from making a directory under a mount point.

### The table in memory

```rust
pub struct Files {
    requests: Handle,       // the root: the service's channel, as before
    directory: String,      // the current directory, a path in the namespace
    mounts: Vec<Mount>,     // in the order they were made
    ..
}
pub struct Mount { pub at: String, pub directory: Arc<File> }
```

A `Files` is never changed in place. `mounted`, `unmounted` and `moved_to` (chdir) each return a new one, sharing the directories through `Arc`. libc keeps the current one in an `Arc<Files>` and swaps it. So a thread in the middle of a request finishes it with the namespace it started with, and needs no lock beyond the one around the swap. A directory's channel is closed when the last `Files` that has it lets it go: that is `Arc` again, counting for us, and `File`'s `Drop` (entry 115) closing it.

### Handed down

```
parent:  taaj_mount("/m", "ns/real", 0)
         posix_spawn(...)  ->  descriptors channel:
                                 [0][MOUNT]"/m"   + a copy of ns/real's channel
                                 [3][FILE]...     + ...
child:   libc start: Files { root: from the registry, mounts: [/m -> ns/real] }
```

The child's mounts are made when it starts, before `main`, not on first use. So the directories are the child's from the beginning, whatever the parent does next.

## Design decisions

- **In the client, not in a server.** A "mount service" that every name goes through would be one more hop per request, and one more program everything depends on. Here a name goes straight to the service that has the file. What the namespace costs is a few string comparisons per request, in the program's own memory, so no other program can see or change a program's namespace.
- **The table in `taaj::files`, the call in libc.** J62 asks for it "in libc", but libc's file calls are `taaj::files` calls, so the table belongs there. Rust programs already have it, and only lack a way to receive their parent's mounts (see "What is still missing").
- **`taaj_mount(at, directory, flags)`, not Linux's `mount(source, target, type, flags, data)`.** Linux's `mount` names a device and a file-system type for the kernel to start. Here there is nothing to start: the directory is already served. A name of our own also avoids pretending to be Linux's call and behaving differently. A program ported from Linux that calls `mount` would need it as root, which TheTaaJ has no notion of yet (K108).
- **The mounted directory is opened once, at mount time.** A mount follows the directory, not its name. If `ns/real` is renamed after it is mounted at `/m`, `/m` is still that directory (entry 115's nodes follow it). That is a bind mount on Linux.
- **Mounts first, then descriptors, on one channel.** A new startup handle (`abi::startup`) would have meant a new flag every starter of programs must know about. The descriptors channel is libc's own, and a mount is libc's state just as a descriptor is. The order of the messages keeps the order of the mounts, which matters when one is under another.
- **`at` need not exist.** Unix wants a directory to mount on. Plan 9 doesn't, and a sandbox that mounts a directory at `/data` shouldn't have to make `/data` first in a tree it can't write. The listing shows the mount anyway.
- **Mounting at `/`** replaces the whole tree, root included (`under("", path)` matches everything). That is the sandbox J63 needs, and costs nothing extra here.

## Bugs met

- **Which error comes first.** `rename("/m", "/elsewhere")`, with `/m` a mount, is two things wrong at once: it renames a mount point, and to a place under another mount (the root). The first boot answered `EXDEV`, because `rename` checked the mounts before the mount point. Linux answers `EBUSY`: a mount point can't be renamed anywhere, so that is the more useful error. The check that caught it is the 38th in `namespace.c`. The order of the two checks in `Files::rename` is now Linux's.

## How I checked it

- **`abi`'s host tests**, 35 in all, one new: `a_mount_goes_with_the_descriptors` covers:
  - the message, there and back;
  - that it carries a handle and is for no descriptor;
  - that a mount on descriptor 3, or a path not from the root, is refused;
  - that it doesn't fit in too small a buffer;
  - mounting at `/`.
- **The `files` lane's boot test**, `handles64-test` and `aarch64-handles-test`, now runs `namespace.c` in the same boot as `handles.c`, so one boot covers the lane. `namespace.c` has 38 checks:
  - a directory mounted at a path that doesn't exist: read, `stat`, listed in `/`, written through, a rename inside the mount;
  - `EXDEV` for a rename out of it, `EBUSY` for `rmdir` and `rename` of the mount point;
  - `chdir` into a mount, `getcwd`, `..` up out of it and back;
  - a mount over a directory that has files, which hides them, and a mount under a mount (the deepest wins); `taaj_unmount` shows them again;
  - a read-only mount: reads, but `EROFS` for writing and unlinking, and the directory unchanged;
  - `ENOTDIR`, `ENOENT` and `EINVAL` from `taaj_mount`;
  - a child started with the mounts reads and writes under them, can't write under the read-only one, and a child started after `taaj_unmount` doesn't have it.
- `make test64`: see the testing note's last line.
- **Not tested**:
  - a Rust program started from C with mounts (it ignores them: see below);
  - a mount of a directory on another file service, because there is only one file service until J64-J67. The routing is the same code either way, a channel and a name.

## What is still missing

- **Rust programs don't receive mounts.** `taaj::file_service()` reads the registry's `files` and the starting directory, but not the descriptors channel, which only libc reads. Rust programs started by the shell (also Rust) have no mounts to receive anyway. When the shell gets a `mount` command, `taaj::file_service()` should read the MOUNT entries too.
- **No `mount` command** in the shell, and no mounts at boot. init could give each program a namespace (J63, K105's manifests).
- **Symbolic links** across a mount: TheTaaJ has none until `taajfs`' J77. When it does, a link's target has to be resolved here, in the namespace, as `..` is.
- **`fchdir` and `fdopendir`** still go by the name the directory was opened by: libc's current directory is a path. In a namespace that is now consistent, since the name is the namespace's, but a directory renamed after `open` is still found by its old name.

## What I learned

- **A mount table is a routing table.** Longest-prefix match, the same as an IP router choosing a route, is all that `mount` does with a path. The rest of Unix's `mount` is about starting a file system, which in TheTaaJ is a service started like any other.
- **Plan 9 made the namespace per process in 1992**, and every process inheriting its parent's by default is what makes it usable: a program sees what its parent sees unless someone chose otherwise. Linux added mount namespaces in 2002, and containers are built on them. In a microkernel where a directory is a handle, the same idea needs no kernel at all.
- **Immutable values shared through `Arc` are a simple way to update shared state.** Instead of locking the table while a request is in flight, a mount builds a new table and swaps a pointer. Readers keep the old one until they are done. In C this would be read-copy-update, with the freeing done by hand. In Rust, `Arc` frees the old table when the last reader drops it.

## Testing note

Start TheTaaJ:

```
make run64
```

| Type | You should see |
|---|---|
| `with programs namespace` | `namespace: 38 checks, all passed` |
| `with programs handles` | `handles: 45 checks, all passed`: entry 115's checks, unchanged |
| `ls` | no `ns`, `shadow`, `m` or `ro` left behind |

The same on AArch64 with `make run-aarch64`: `with programs namespace` gives the same line.

On the host:

```
cd kernel/abi && cargo test
```

| Look for | You should see |
|---|---|
| `tests::a_mount_goes_with_the_descriptors` | `ok` |
| the last line | `test result: ok. 35 passed` |

Automated: `make handles64-test aarch64-handles-test`, and `make test64`.
