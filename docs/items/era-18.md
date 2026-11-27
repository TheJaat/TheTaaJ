# Era 18 · Security: hardening, trust and people

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: a program given no grants cannot crash or starve the machine, whatever it calls; the fuzzers (system calls on TheTaaJ, every parser on the host) run for a set time with nothing found; a program gets its manifest's grants only if it and its manifest are signed; the machine boots verified (UEFI Secure Boot, measured into a TPM where there is one); and two users on one machine cannot reach each other's files. Whose it is, and when, is for the user to say. Items already elsewhere are pointed to, not moved: K25 (ASLR), K26 (stack protector), K28 (who may switch off), K29 (TCP sequence numbers) in Security above; K37 (TLS) and K56 in Era 12; K27 (IOMMU) in Era 15; K84, K94 and K85 in Era 16; K97 in Era 17; J63's sandboxes in Era 14; K66 in Era 13.

What is right, and is TheTaaJ's strongest point: **capabilities from the bottom up**. A process reaches a kernel object only through a handle, whose rights (SEND, RECEIVE, DUPLICATE, TRANSFER, READ, WRITE, KILL) can only shrink when passed on. No ambient authority in the kernel for what matters: no "kill process 42" (`kill` and signals need a handle with KILL), starting a program needs a launcher handle, the process list needs the system handle. The shell **narrows the registry for each program it runs** - `ls` gets the read-only file channel, `ping` only `net`, an unknown program nothing - and a redirect gives a channel, not the file service. init's namespace table is the system's policy in one place. Drivers hold only their own registers, DMA memory and interrupt. The kernel is Rust, with about 260 `unsafe`. Randomness from the CPU, jitter and the clock, through ChaCha.

What is missing:

- **Kernel memory a program can use up**: channels, queued messages, handles, processes and threads are kernel allocations not counted against their creator, and the kernel heap panics when it can't grow (Era 16). The one way a program with no grants can bring the machine down.
- **Defences around the core**: SMEP, SMAP, PAN, KPTI (Era 16), the IOMMU (Era 15), ASLR (K25), stack protector (K26), and no speculation (Spectre) mitigations at all.
- **Trust in what runs**: which services a program gets is a list of names hard-coded in the shell (`CHANGE_PROGRAMS`, `NET_PROGRAMS`, ...), so whoever can write `/edit` plants code that gets `windows` and `files`, and a new program means editing the shell; nothing is signed; nothing verifies the bootloader or the kernel.
- **People**: no users or login; no disk encryption.
- **Assurance**: system calls and parsers (ELF, TaajFS, the network, USB descriptors, ACPI) are tested but never fuzzed; `unsafe` is not audited as a whole; no Miri.
- **Smaller**: `DEBUG_WRITE` lets any program write the kernel console; no audit log of grants and powerful actions (which the assistant will need).

All of it keeps the kernel pure: the kernel counts, checks rights and enforces; who gets what - manifests, signatures, users, the audit log, encryption - is in user-space services.

| # | Item | Size | Notes |
|---|---|---|---|
| K103 | **Kernel objects counted against their creator**: each channel, queued message, handle, process and thread charged to an account (the creator's, or the sender's for a message in someone's inbox), refused at the limit | M | First: the era's milestone. With K85; makes K66 a bounded leak until it is fixed |
| K104 | **Fuzzing**: a system-call fuzzer run on TheTaaJ (random calls, arguments and handles from a program with no grants), and `cargo fuzz` on the host for every parser of untrusted input - `elf`, `taajfs`, `net`, `usb` descriptors, `acpi` tables, the file and network protocols | M | **Done (entry 123)** on TheTaaJ: `fuzz` (user/src/bin/fuzz.rs) - programs with no handles make random system calls from a seed; `fuzz64-test` and `aarch64-fuzz-test` in `make test64` (100 programs of 3000 calls, seed 1); `with programs fuzz N CALLS random` for longer runs. `cargo fuzz` on the host is K189 |
| K189 | **`cargo fuzz` on the host for every parser of untrusted input** - `elf`, `taajfs`, `net`, `usb` descriptors, `acpi` tables, the file and network protocols | M | K104's second half; needs a nightly Rust on each machine, as K187 |
| K105 | **A manifest per program**: the services it needs (`files.look`, `net`, `windows`...), read by the shell and init, which grant from it rather than from lists of names in the shell; a program without one gets nothing, as now | M | After J62, so a manifest can name directories too |
| K106 | **Signed programs and manifests**: a key the machine trusts, checked before a program gets its manifest's grants; unsigned programs run with none | M | After K105 |
| K107 | **Verified boot**: UEFI Secure Boot (the bootloader and kernel signed), and measured boot into a TPM where there is one | L | With MaK-WSL, who owns TheBootloader |
| K108 | **Users and login**: each user a set of capabilities - a home directory handle and the services allowed - given by a login service, with no user ids checked in the kernel | L | After J62 and J76 |
| K109 | **Disk encryption**: a block service between `fs` and the disk driver, its key from the login (and the TPM) | L | After K108 |
| K110 | **Speculation mitigations**: IBRS/IBPB or retpolines, buffers cleared on return to user mode, on the CPUs that need them; the AArch64 equivalents | M | With K94 |
| K111 | **`DEBUG_WRITE` behind a capability**: a log handle given to whoever may write the kernel console | S | Small |
| K112 | **Every `unsafe` with a written reason** (`// SAFETY:`), checked by a lint; Miri over the host tests of the kernel's crates | M | **Done (entry 121)** for the lint: `make unsafe-check`, in `make test64` - clippy's `undocumented_unsafe_blocks` and `missing_safety_doc` as errors, over every crate and CPU; the 68 places without a reason given one. Miri is K187 |
| K187 | **Miri over the host tests** of the kernel's crates and the logic crates: the tests run in Rust's interpreter, which catches undefined behaviour (an `unsafe` whose reason is wrong) | M | **Done (entry NNN)**: `make miri` - every crate with `unsafe`, under Miri (nightly-2026-11-25); undefined behaviour found and fixed in the `usb`, `nvme` and `ahci` test simulators (misaligned DMA words; a pointer made invalid by a later borrow), and two test helpers' leaks. `libc` and `user/` not yet |
| K113 | **An audit log**: grants, kills, power, spawns with powers, kept by a user-space service and readable only with a handle for it | M | Before the assistant era, whose "undo" and activity log build on it |
