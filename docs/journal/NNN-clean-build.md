# A clean build from a fresh clone, in a container, with one command

*Journal entry #NNN · a tool (J92, and with it J54). Written on TheJat-WSL/ci. Our three machines have drifted from a new one: packages installed by hand over months, Rust toolchains, LLVM 18.1.3 unpacked in a home directory. Whether TheTaaJ builds and passes on a machine that has none of that, nobody knew (J54: "never tried"). Now `sh tools/clean-build.sh` makes a fresh Ubuntu 24.04, runs the setup script in it, clones TheTaaJ, and runs `make test64` - and the first run passed.*

## Why

A project that builds only where it was built is not reproducible: a dependency installed once and forgotten breaks every new contributor, and every new machine. J92 asks for the build from a fresh clone, "in a container, with one command", checking `tools/setup-wsl.sh` on the way; J93, our own CI, is built on it.

## What it is

```text
sh tools/clean-build.sh [DIR [REF]]
```

1. **Ubuntu's own minimal 24.04** (`ubuntu-base`): the newest point release, its `SHA256SUMS` checked against `SHA256SUMS.gpg` with the archive keyring every Ubuntu has, then the image against its checksum.
2. **A fresh container**, unpacked for the run and deleted after it, entered with `unshare` - no Docker, no root:

   ```text
   unshare --map-auto --map-root-user --mount --pid --fork
            |               |              |       '- its own processes, and a /proc of them
            |               |              '- its own mounts: /dev and /sys of the host, the apt cache
            |               '- root inside is me outside
            '- uids 1 to 65535 inside are my /etc/subuid range (newuidmap, from `uidmap`): apt and dpkg
               give files to other users, and with only root mapped they cannot
   ```

   `/dev` is the host's, so `/dev/kvm` works and the tests run under KVM.
3. **Inside**: `setup-wsl.sh`, fetched from GitHub at REF, as a new user would - it installs the packages and Rust, clones TheTaaJ **over HTTPS** with its submodules, builds the i686 cross compiler and the ISO. Then REF is checked out and `make test64` runs, **holding the machine's test64 lock** (bind-mounted in: the container's `/tmp` is its own).

DIR (`~/.cache/taaj-clean-build`) keeps the download, the packages apt fetched (a second run fetches nothing again) and each run's log.

## What it found

The first run passed - all of `make test64` on a clean Ubuntu 24.04, from a fresh clone, in 35 minutes (the ports' first build is most of it):

```text
   no GitHub SSH key: cloning read-only over HTTPS
   QEMU:       QEMU emulator version 8.2.2 (Debian 1:8.2.2+ds-0ubuntu1.18) (-accel kvm)
   LLVM:       18.1.3
   Rust:       rustc 1.100.0 (0ef1bc386 2026-11-09)
...
test64: all passed
```

`setup-wsl.sh` needed one change: as root, in a container, there is no `sudo` - it now runs its commands without it when it is root. Ubuntu 24.04's LLVM is the reference 18.1.3, so the byte-for-byte tests needed nothing of what TheJat-WSL's Ubuntu 26.04 needs (an unpacked 18.1.3, a `cstdint` wrapper; journal entry 109 and before).

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| `unshare` and `uidmap` | Docker | Nothing to run as a service, nothing as root; `uidmap` is one small package, on every machine |
| `ubuntu-base`, checked | A Docker image | Ubuntu's own, signed; the same key `apt` trusts |
| `setup-wsl.sh` from GitHub, a clone over HTTPS | Copying this tree in | What a new machine has is the repository on the server, not my tree |
| Ubuntu 24.04 | 26.04 | The reference: the byte-for-byte tests compare with its LLVM |
| The machine's test64 lock taken inside | Not | Rule 13: one `make test64` per machine, container or not |
| A fresh root file system every run, packages kept | A prepared image | The setup script is part of what is checked; only the downloads are saved |

## Bugs

- **The package cache's `partial/` could not be read or deleted from outside**: apt's own user (uid 42 inside, one of my subuids outside) owns it. The script cleans it up inside the user namespace, as it does the container.
- **A test of this tool's first version could only be of a pushed branch**: `setup-wsl.sh` comes from GitHub at REF. That is the point - it is what a new machine gets - so a branch is pushed before its clean build.

## How I checked it

- `sh tools/clean-build.sh ~/.cache/taaj-clean-build j92-clean-build`: passed, 2128 s, with the log lines above.
- With `main` merged in, again: see the merge.

## What is still missing

- **J93, our own CI**: this, run for every pushed branch, the result posted where the branches are.
- **AArch64 hosts and other distributions**: only an x86_64 Ubuntu 24.04 is made.
- The run takes about 35 minutes; the ports' build is most of it.

## What I learned

- **A clean machine is the only proof a build needs nothing it doesn't say.** It passed first time - which nobody knew before.
- **A container needs no daemon.** A user namespace with a uid range, a mount namespace and `chroot` is all `apt` needs.

## Testing note

From any directory (it needs `uidmap`, `curl`, `gpgv`, and a range in `/etc/subuid`; about 35 minutes):

| Do | You should see |
|---|---|
| `sh tools/clean-build.sh` | `ubuntu-base-24.04.5-base-amd64.tar.gz: signature and checksum good`, then after about 35 minutes `test64: all passed` and `a fresh clone of main builds and passes on a clean Ubuntu 24.04` |
| `ls ~/.cache/taaj-clean-build/logs` | The run's log; `grep "LLVM:\|QEMU:" it` shows `18.1.3` and `-accel kvm` |

Automated: not part of `make test64` - it runs `make test64` itself, in the clean machine.
