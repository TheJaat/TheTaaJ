# Using TheTaaJ

What you can do once TheTaaJ is running: the shell, files, programs, windows, writing and compiling programs on TheTaaJ itself, and the network. How to build it and start it the first time is in the [README](../README.md); how each part works, and why, is in the [journal](journal/README.md).

## Starting it

```
make run64
```

builds everything and boots TheTaaJ in QEMU from BIOS, on four CPUs with 512 MiB. Two things open:

- **QEMU's window**: the screen. The terminal, where the shell runs, is a window on a desktop; programs with windows of their own open beside it. Type here.
- **The terminal you ran `make` in**: the kernel's log, the serial port. Every service and program says what it is doing there. Ctrl+C *there* ends QEMU.

| Instead | What it does |
|---|---|
| `make run64-uefi` | Boots from UEFI firmware instead of BIOS |
| `make run-aarch64` | TheTaaJ on a 64-bit ARM machine (emulated: slower) |
| `make run64 RUN_DISK_BUS=nvme` (or `sata`, `usb`) | The disk on an NVMe drive, a SATA controller or a USB stick, each with its own driver |
| `make run64 RUN_NIC=e1000` | An Intel network card instead of the virtio one |
| `make run64 RUN_USB=yes` | A USB keyboard and mouse too (click in QEMU's window to give it the mouse) |

To stop it from inside: `poweroff` (or `reboot`). The services are stopped first, so the disk is left in order.

## The shell

`help` lists everything. The shape of a command line is the one you know:

| Line | What it does |
|---|---|
| `ls`, `cat notes`, `wc notes` | Run a program from the disk, with its arguments |
| `cat notes > copy`, `>> log`, `< input` | Output into a file, added to one; input from one |
| `cat long \| wc` | What the first prints is what the second reads |
| `primes &` | Run in the background; `jobs` lists those, `kill 1` ends job 1, `wait` waits for them all |
| `time cat long` | Run it, and say how long it took |
| `cd docs`, `cd ..`, `cd`, `pwd` | Move around the directories; `cd` alone goes to the root |
| `source script` | Run a file's lines as commands; stop at the first that fails |
| `echo "two words"` | Quotes keep spaces in one argument (`'two words'` and `two\ words` too) |

**Ctrl+C** in QEMU's window stops the program running in the foreground. **Ctrl+D** ends what you type to a program that reads its input from the keyboard.

Things about the machine: `uptime`, `mem` (memory), `ps` (processes), `services` (what the shell may ask for).

## Files

The disk `make run64` attaches, `build/taaj-disk.img`, is kept from one run to the next: what you write in TheTaaJ is still there next time. It is TaajFS, TheTaaJ's own file system, which survives the power going off in the middle of a write.

| Command | What it does |
|---|---|
| `ls [DIR...]` | List a directory (this one) |
| `cat FILE...`, `wc FILE...` | Show files; count lines, words and bytes |
| `write FILE TEXT`, `append FILE TEXT` | Set a file to TEXT, or add TEXT to it |
| `mkdir DIR...` | Make directories |
| `mv FROM TO`, `cp FROM TO` | Move or rename; copy a file |
| `rm NAME...` | Remove files and empty directories |

A name is a path from where you are (`notes`, `../plan`) or from the root (`/docs/readme`).

**From your own machine**, with QEMU closed:

```
T=tools/tfstool-rs/target/release/tfstool
$T ls  build/taaj-disk.img              # the root directory
$T put build/taaj-disk.img notes my.txt # copy my.txt in, as "notes"
$T get build/taaj-disk.img notes        # print a file
```

## What a program may touch

A program you start by name can open **no files at all**: it gets its arguments, the keyboard and the screen, and nothing else. You decide what more it gets:

| Line | The program may |
|---|---|
| `mine notes` | open no files (`fopen` fails: "this program was not given the files") |
| `with readonly mine notes` | read every file, change none |
| `with files mine notes` | read and change files |
| `with programs mine` | that, and start programs of its own (`make`, a compiler driver) |

`mine < notes > out` needs none of them: the shell opened those two files and handed them over. This is TheTaaJ's way all through - a program reaches only what it was given (journal entries on *capabilities*).

## Programs on the disk

| Program | What it does |
|---|---|
| `ls`, `cat`, `wc`, `write`, `append`, `mkdir`, `mv`, `cp`, `rm` | Files (above) |
| `yes [TEXT]` | Prints TEXT (`y`) for ever - try `yes \| wc`, then Ctrl+C |
| `primes` | Counts primes; it uses the heap |
| `edit FILE` | A text editor in a window (below) |
| `counter` | A window with a count and two buttons |
| `ping`, `netinfo`, `httpd`, `udpecho`, `udpsend`, `tcpecho`, `tcpsend` | The network (below) |
| `chello`, `cwords`, `cprimes`, `cask`, `cfloat`, ... | C programs (`libc/programs/`) |
| `forever [spin\|yield\|sleep\|wait]` | Never ends, in the way you choose: for trying Ctrl+C and `kill` |
| `log [-n LINES \| -a] [-l LEVEL] [-s WRITER] [-b]` | The system log (below) |
| `top [-n TIMES] [-d SECONDS]` | The processes, refreshed: each one's CPU (the busiest first), memory, threads and handles; Ctrl+C ends it |
| `trace [-s SECONDS] [-e EVENTS]` | What the kernel does for a while - system calls, messages, switches, page faults, interrupts - counted by kind, CPU and process, and the last events |
| `profile [-s SECONDS] [-n PLACES]` | Where the CPUs spend their time, for a while (5 s): by program, and the busiest places - the kernel's functions by name, programs' addresses |
| `debug PROGRAM [ARGUMENTS]` | A small debugger: starts the program, then `stop`, `wait`, `cont`, `regs`, `x ADDR [N]` (its memory), `w ADDR BYTE...`, `break ADDR` or `break pc`, `kill`, `quit` |

## Windows

`counter &` opens a window beside the terminal, with the keyboard's focus. Click `+1`, or type `+` while it has the focus; click the terminal to give the focus back to it. Drag a window by its title bar; close it with the box at its right end. Every window is an ordinary program that draws into memory it shares with the display.

**`edit FILE`** - a text editor in a window, 76 by 24 characters:

| Key | What it does |
|---|---|
| characters, Enter, Tab | Typed at the cursor |
| Backspace, Delete | Remove before, or under, the cursor |
| arrows, Home, End, Page Up, Page Down | Move |
| Ctrl+S | Save |
| Ctrl+Q | Quit (twice, if there are unsaved changes) |

## Writing a program on TheTaaJ

TheTaaJ compiles C itself. The disk has TCC (a small C compiler), TheTaaJ's C library and its headers, `make`, and the sources of the C programs.

```
edit hello.c                 (write the program; Ctrl+S, Ctrl+Q)
tcc hello.c -o hello
hello
```

and with `make`, for a program of several files:

```
cd /src/libc/programs
with programs make
```

`source /src/build` rebuilds the whole C library and its programs on TheTaaJ, with TCC.

**Clang and LLVM** run on TheTaaJ too, but are big: they go on a disk of their own. On your machine, `make llvm-put` builds them and puts them on `build/llvm-disk.img` (the first time takes many minutes); then `make run64 RUN_DISK=build/llvm-disk.img`.

**On your own machine**, building with clang for TheTaaJ and putting the program on the disk: see [Writing a C program for TheTaaJ](c-programs.md) - `make cput C=mine`, then `mine` at the prompt.

## The network

`make run64` gives TheTaaJ a network card on QEMU's own small network, which needs nothing from your machine: TheTaaJ is 10.0.2.15, the way out is 10.0.2.2.

| Line | What it does |
|---|---|
| `netinfo` | This machine's address, and what the network service has counted |
| `ping 10.0.2.2` | Is the gateway there? (`ping ADDRESS COUNT BYTES`) |
| `httpd` | A web server for the files under `/www` |

QEMU's network lets nothing in from outside unless asked. To see `httpd`'s pages in your own browser, start TheTaaJ with a door from your port 8080 to TheTaaJ's 80:

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=tcp::8080-:80 -device virtio-net-pci,netdev=n0"
```

then `httpd` at TheTaaJ's prompt, and open `http://localhost:8080` on your machine.

## When something goes wrong

Every driver and service is an ordinary program, watched by `init`. One that crashes or stops answering is started again, and the kernel's log says so (`[init] fs ended ... - restarting it`). You can try it: **F12** crashes the keyboard driver, **F11** makes it hang - typing comes back by itself a moment later. `display stall 5` makes the screen freeze for five seconds, the same way; `fs stall 5` and `disk stall 5` the file service and the disk driver.

A program that does something it must not - reads memory it was not given, writes to its own code - is ended, and the log says why; the rest of the system goes on. It leaves a crash report: what it did, its registers, the addresses on its stack that point into its code, its memory, and the last lines it printed - in the system log (`log -s NAME`) and in a file of its own in `/var/log/crashes`, the newest also as `/var/log/crashes/latest`. If the kernel itself stops, its last words are in the system log after the next warm restart (`log -s crash`).

**Debugging the kernel with gdb.** A kernel built with the `gdb-stub` feature (`cargo build --release --features gdb-stub` in `kernel/kernel64`) waits for gdb at boot on COM2, the second serial port, and stops for it at its breakpoints, steps and panics until gdb detaches. On a real PC, connect COM2 to another machine with a serial cable and `target remote /dev/ttyS0` (or the USB adapter's device) in gdb, with the kernel's ELF file for its symbols. In QEMU, `-serial ... -serial tcp:127.0.0.1:1234,server=on,wait=off` and `target remote 127.0.0.1:1234`.

**The system log.** Everything the kernel and the programs write to the debug log is also kept on the disk, in `/var/log/system.log` (by the `logd` service): a line each, with the time, who wrote it and a level - `info`, `warn` (something went wrong and was dealt with: a driver restarted), `error` (something failed), `crit` (the system stopped). It is kept across a switch-off; at 1 MiB it becomes `system.log.1` and a new one begins. `log` shows the last 20 lines; `log -l warn` only what went wrong, `log -s fs` only what `fs` wrote (`kernel` for the kernel's), `log -b` only since the last start, `log -a` all of it.

## More

- [The journal](journal/README.md): how every part was built, step by step, with what went wrong.
- [Writing a C program for TheTaaJ](c-programs.md).
- `make test64` checks all of the above, and much more, automatically.
