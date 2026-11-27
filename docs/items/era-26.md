# Era 26 · Packages and updates

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: a program is installed, updated and removed on TheTaaJ with one command, from a signed package, its files and its manifest placed by a package service; the system itself (kernel, libc, services) is updated the same way, with the old version kept until the new one has booted (Era 21's A/B kernels); and an update that fails leaves the system as it was. Whose it is, and when, is for the user to say. The assistant's draft (`docs/ai-assistant.md`) asks for a package manager for programs and models; this is it. Pointed to: K106 and K105 (Era 18), K132 and K134 (Era 21), J46.

What is right: TaajFS commits atomically (copy-on-write), so an update could be made all-or-nothing; programs already run with only what is granted to them, which a package's manifest can declare; the disk is built reproducibly from recipes on the host (`tools/tfstool-rs`, `put-*.sh`).

What is missing: **everything on the disk is put there from the host**: there is no package, no package format, no way to install or remove a program on TheTaaJ but copying files; no update of the system from TheTaaJ; no record of which file belongs to which program; no versions; no signatures.

| # | Item | Size | Notes |
|---|---|---|---|
| J98 | **A package format**: an archive of files, a manifest (Era 18's, with the services the program needs), a version, its dependencies, a signature | M | First |
| J99 | **A package service** that installs, updates and removes packages, keeps what file belongs to which, and makes each change one TaajFS commit (all or nothing); `pkg install/remove/list` | M | After J98 and J62 |
| J100 | **Packages built on the host** from the same recipes that build the disk today, so the disk is made of packages | M | After J98 |
| J101 | **A repository**: a signed index of packages, fetched over the network (after K37, TLS) or from a USB stick | M | After J99 |
| J102 | **System updates**: the kernel (through Era 21's A/B), libc and the services as packages; the previous version kept until the new one has run | L | After K132 |
