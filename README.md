# **The TaaJ OS**
The TaaJ OS is an open-source operating system developed by [The Jaat](https://github.com/TheJaat). This repository serves as the core OS repository, integrating components such as the bootloader and kernel. It aims to provide a modular, lightweight, and efficient environment for learning and experimentation in systems programming.

## **Repository Structure**
The system is split across four repositories. This one ties them together; the other three are git submodules:

| Directory | Repository | What it is |
|---|---|---|
| `bootloader/` | [TheBootloader](https://github.com/TheJaat/TheBootloader) | The BIOS and UEFI loaders |
| `kernel/` | [TheKernel](https://github.com/TheJaat/TheKernel) | The 64-bit Rust kernel (`kernel/kernel64`), its interface (`kernel/abi`), and the original 32-bit C kernel |
| `toolchain/` | [TheToolchain](https://github.com/TheJaat/TheToolchain) | The i686 cross compiler, used for the bootloader and the 32-bit system |
| everything else | this repository | User space (`user/`), libraries (`fileserver/`, `taajfs/`, `console/`, `input/`, `heap/`, `virtio/`), host tools (`tools/`), the 32-bit user space (`servers/`, `modules/`, `librt/`), and the journal (`docs/journal/`) |

## **Features**
- A 64-bit microkernel written in Rust, booted by the project's own BIOS and UEFI loaders.
- Drivers and services are ordinary processes: `init`, `shell`, `fs` (files), `blk` (disk), `display`, `kbd`. A crashed or hung one is restarted.
- Capabilities: a program can reach only what it was handed.
- TaajFS: a copy-on-write file system with directories, which survives power loss mid-write.
- Programs are ELF files loaded from the disk: `ls`, `cat`, `write`, `append`, `rm`, `mkdir`, and your own.
- The original 32-bit system (C kernel) still builds and runs.

How each piece works, and why, is written up step by step in the [journal](docs/journal/README.md).

## **Building and Running**

Developed and tested on Ubuntu (x86_64). Other Linux distributions should work with the equivalent packages.

### 1. Get the source

```bash
git clone --recurse-submodules git@github.com:TheJaat/TheTaaJ.git
cd TheTaaJ
```

If you cloned without `--recurse-submodules`:

```bash
git submodule update --init
```

### 2. Install what the build needs

```bash
sudo apt-get update
sudo apt install build-essential bison flex libgmp3-dev libmpc-dev libmpfr-dev texinfo \
    libisl-dev libzstd-dev zlib1g-dev libexpat1-dev python3 gawk wget curl git \
    nasm xorriso mtools dosfstools qemu-system-x86 ovmf clang lld netcat-openbsd
```

| Package | Needed for |
|---|---|
| `build-essential` … `libexpat1-dev` | Building the cross compiler (step 4) |
| `nasm` | The bootloader's assembly |
| `xorriso`, `mtools`, `dosfstools` | Making the bootable ISO and the disk images |
| `qemu-system-x86` | Running the OS |
| `clang`, `lld` | The UEFI loader. Without them the ISO is built BIOS-only, and the build says so |
| `ovmf` | UEFI firmware for QEMU (`make run64-uefi`, and the UEFI boot test) |
| `netcat-openbsd`, `python3` | The automated boot tests (typing keys into QEMU, checking screenshots) |

### 3. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Nothing else: the bare-metal target (`x86_64-unknown-none`) is installed automatically the first time the kernel is built, from the `rust-toolchain.toml` files. No nightly compiler is needed.

### 4. Build the cross compiler (once)

The bootloader — for the 64-bit system too — is built with an `i686-elf` GCC:

```bash
cd toolchain
sudo bash toolchain_i686.sh
cd ..
```

This takes a while. It leaves the compiler in `toolchain/i686/`, which is where the build looks for it; you do not need to add it to your `PATH`. See `toolchain/README.md` for details.

### 5. Build and run the 64-bit system

```bash
make run64
```

This builds the user programs, the kernel, the bootloader and a bootable ISO, and starts QEMU with a disk attached. A QEMU window opens with the shell; the kernel's log appears in the terminal you started it from.

| Command | What it does |
|---|---|
| `make run64` | Build and boot from BIOS |
| `make run64-uefi` | Build and boot from UEFI firmware |
| `make iso64` | Build only: the ISO is `build/image64.iso` |
| `make clean` | Remove what was built |

Things to try at the prompt: `help`, `ls`, `ps`, `mem`, `mkdir docs`, `write docs/note hello`, `cat docs/note`. Ctrl+C **in the QEMU window** stops a running program; Ctrl+C in the terminal ends QEMU.

**The disk.** `make run64` attaches `build/taaj-disk.img`, a 16 MiB TaajFS volume. It is created the first time and kept afterwards, so files you write in the OS are still there on the next run. (`make clean` removes it.)

**Files between your machine and the OS** — with QEMU closed, using the host tool:

```bash
T=tools/tfstool-rs/target/release/tfstool
$T ls   build/taaj-disk.img              # list the root directory
$T ls   build/taaj-disk.img docs         # list a directory
$T put  build/taaj-disk.img notes my.txt # copy my.txt in, as "notes"
$T get  build/taaj-disk.img notes        # print a file
$T dump build/taaj-disk.img              # everything that is on the volume
```

### 6. Run the tests

```bash
make test64
```

Runs the host unit tests of every crate, boots the system in QEMU from BIOS and from UEFI while typing a long list of commands and checking the output, and boots seven kernels built to fail on purpose. It ends with `test64: all passed`.

| Command | What it does |
|---|---|
| `make test64` | Everything below |
| `make kernel64-test` | Host unit tests only (fast, no QEMU) |
| `make boot64-test` | The BIOS boot test |
| `make boot64-uefi-test` | The UEFI boot test |
| `make fault64-test` | The seven deliberate kernel faults |
| `make tfscheck` | The two TaajFS host tools, checked against each other |

Run one test command at a time: they share the `build/` directory.

### The original 32-bit system

```bash
make        # bootloader, C kernel, servers, ISO and disk images
make run    # boot it in QEMU
```

### If something goes wrong

| Symptom | Cause |
|---|---|
| `i686-elf-gcc: command not found` | The cross compiler is not built (step 4), or `toolchain/` is empty — run `git submodule update --init` |
| `cargo: command not found` | Rust is not installed, or `source "$HOME/.cargo/env"` was not run in this terminal |
| `NOTE: clang/lld-link not found - skipping the UEFI loader` | Install `clang` and `lld`; BIOS boot still works without them |
| `mcopy not found` | Install `mtools` |
| `make run64-uefi` cannot find its firmware | Install `ovmf`; the build expects `/usr/share/ovmf/OVMF.fd` |
| An empty `kernel/` or `bootloader/` directory | The submodules were not fetched: `git submodule update --init` |

### Working on the kernel or the bootloader

They are separate repositories, so a change there is two commits — one inside the submodule, and one here to record which version this repository now uses. Push the submodule first:

```bash
cd kernel
git add -A && git commit -m "..." && git push
cd ..
git add kernel && git commit -m "..." && git push
```

## Contribution Guidelines
We welcome contributions! To contribute:
1. Fork this repository and clone your fork.
2. Create a new branch for your feature or bugfix.
3. Submit a pull request with a detailed description.

Please ensure your changes align with the project's coding standards and include appropriate documentation.
