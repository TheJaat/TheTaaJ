# Using TheTaaJ: a page for the people who run it

*Journal entry #145 · a small step, E8 (either machine's list), done by MaK-WSL. Written on MaK-WSL. Everything written about TheTaaJ until now was for the people building it: the journal, the testing notes, the work plan. `docs/using-taaj.md` is for someone who has just typed `make run64`: what to type next, and what each part does.*

## Why

The journal explains how every piece works, a hundred and thirty entries deep; the README says how to build and start it. Between them there was nothing that says what you can do once it is running - redirection and pipes, why `mine notes` cannot open `notes`, that TheTaaJ compiles C itself, how to reach its web server from a browser. E8 asks for that page, for the website.

## What it is

One page, `docs/using-taaj.md`, linked from the README twice: starting it (and the variants: UEFI, AArch64, another disk bus, network card, USB), the shell, files and the host tool, what a program may touch (`with files`, `with readonly`, `with programs`), the programs on the disk, windows and the editor, writing and compiling C on TheTaaJ (TCC, make, the C library's own build; Clang on its own disk), the network (and the port forward that lets a browser see `httpd`), and what happens when something goes wrong (services restarted, F11 and F12, the stall commands).

## How it was checked

Every command and fact on it was taken from its source, not from memory: the shell's own `help` text, each program's opening comment (`edit`, `counter`, `ping`, `httpd`), the boot tests' typed lines (`tcc hello.c -o hello`, `ping 10.0.2.2`), the Makefile (what `run-disk` puts on the disk, `RUN_NET`, `RUN_NIC`, `RUN_DISK_BUS`), `net.rs` for the address (fixed at 10.0.2.15 until DHCP, K30). It describes `main`: nothing from branches not yet merged.

**Not checked by a test**: a page is not code. It will go out of date as steps change what a user sees - a step that does should change this page in its branch, as it changes `who-does-what.md`.

## What is still missing

- Pictures: a screenshot of the desktop with the editor open, for the website.
- A page per lane's user-visible work as it lands (the window system's keys and wheel, K42 and K46, when merged).

## Testing note

```
make run64
```

| Do | You should see |
|---|---|
| Follow `docs/using-taaj.md` from the top | Each command doing what the page says |

Automated: none - a document. `make test64` checks the commands it shows, among much else.
