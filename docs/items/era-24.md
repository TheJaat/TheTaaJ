# Era 24 · Protocols and the ABI: one IDL, versions, stability

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: every protocol between programs and services is written once in an interface description, and its encoding and decoding in Rust (and C for libc) are generated from it; every protocol and the system calls carry a version, and a program built for an older version still runs; a change that breaks the ABI fails a check before it can be merged. Whose it is, and when, is for the user to say. The vision says to "plan an IDL for IPC protocols"; this is it.

What is right: one `abi` crate (about 2,800 lines) shared by the kernel, the services, the Rust runtime and libc, so they can't disagree about a number; every value little-endian at an explicit offset; most protocols with host tests that encode and decode; the boot protocol (TaajBoot) append-only with a size field - the model the rest could follow.

What is missing: **every protocol is written by hand**, request by request - `abi::init`, `display`, `block`, `nic`, `net`, `power`, and outside `abi` the file protocol (`fileserver::protocol`) and the window protocol (`gui::protocol`); each new service repeats the work, and C has to mirror it again. **No versions**: not on the system calls, not on any protocol; a change to a message's layout breaks every program built before it, and nothing says so. **No check** that a change keeps the ABI; Rule "the ABI stays in step" is kept by care alone.

| # | Item | Size | Notes |
|---|---|---|---|
| K151 | **An interface description language**: a small one, ours (or FIDL-like), describing messages, their fields, handles carried, and replies; a generator for Rust (`no_std`) and C | L | First. Small: what TheTaaJ's protocols use, not more |
| K152 | **The protocols moved onto it** one at a time: the file protocol first (it changes in Era 14 anyway), then net, display, block, nic, init, power, the window protocol | L | After K151. With Era 14's J61 |
| K153 | **Versions**: each protocol says its version when a channel is opened; a service serves older versions it still knows; the system calls get a version a program states at start | M | After K151 |
| K154 | **An ABI check before merging**: the generated layouts compared with the last release's; a change that breaks one fails unless the version changes with it | S | After K153. A new line in "Before merging" |
| K155 | **The protocols documented from the IDL**: a page per protocol, generated, for the website and for whoever writes a service | S | |
