#!/usr/bin/env bash
# setup-wsl.sh - make a fresh WSL2 Ubuntu ready to build, run and test
# TheTaaJ, with QEMU using KVM.
#
#   bash setup-wsl.sh            from inside a TheTaaJ checkout, or
#   bash setup-wsl.sh [DIR]      anywhere else: clones TheTaaJ into DIR
#                                (default ~/Workspace/TheTaaJ) first
#
# What it does, each step skipped when already done, so it can be run
# again after a failure:
#   1. checks it is WSL2 on Ubuntu 24.04 (see RELEASE below)
#   2. installs the Ubuntu packages the build and the tests use
#   3. installs Rust with rustup (rust-toolchain.toml picks the rest)
#   4. gives this user /dev/kvm
#   5. clones TheTaaJ with its submodules, if not run from a checkout
#   6. builds the i686 cross compiler (toolchain/i686, ~20-30 minutes)
#   7. builds the 64-bit ISO and says what to run next
#
# Ubuntu 24.04 because its LLVM is 18.1.3: the LLVM, Clang and lld tests
# check that TheTaaJ's tools write byte for byte what the host's write,
# and the host's must be that release. Another release still builds and
# boots; those tests then fail. `--any-release` goes on anyway.

set -euo pipefail

RELEASE=24.04
ANY_RELEASE=no
TARGET_DIR=
for arg in "$@"; do
    case "$arg" in
        --any-release) ANY_RELEASE=yes ;;
        -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) TARGET_DIR=$arg ;;
    esac
done

say()  { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
note() { printf '   %s\n' "$*"; }
fail() { printf '\n\033[1;31msetup-wsl: %s\033[0m\n' "$*" >&2; exit 1; }

# --- 1. Where are we -------------------------------------------------------

say "1/7 Checking the system"
if grep -qi microsoft /proc/sys/kernel/osrelease 2>/dev/null; then
    note "WSL2: $(uname -r)"
else
    note "not WSL - fine, the steps are the same on a plain Ubuntu"
fi
. /etc/os-release
note "$PRETTY_NAME"
if [ "${VERSION_ID:-}" != "$RELEASE" ]; then
    if [ "$ANY_RELEASE" = yes ]; then
        note "not Ubuntu $RELEASE: going on, but the LLVM/Clang/lld tests will fail"
    else
        fail "this is Ubuntu ${VERSION_ID:-?}, not $RELEASE.
   TheTaaJ's LLVM, Clang and lld tests compare with the host's LLVM 18.1.3,
   which is Ubuntu $RELEASE's. Install that release next to this one, in
   PowerShell:   wsl --install -d Ubuntu-$RELEASE
   then run this script in it. Or run again with --any-release to set up
   here anyway (everything but those tests works)."
    fi
fi
case "$(pwd)" in
    /mnt/*) note "WARNING: you are under /mnt (the Windows disk); builds there are"
            note "many times slower. Keep the checkout in your Linux home." ;;
esac

# --- 2. Packages -----------------------------------------------------------

say "2/7 Installing Ubuntu packages (sudo)"
PACKAGES=(
    # building
    build-essential git curl wget ca-certificates python3 pkg-config
    nasm xorriso mtools dosfstools
    # Clang and LLVM 18: C programs, the UEFI loaders, the LLVM ports
    clang lld llvm llvm-18 llvm-18-dev zlib1g-dev libzstd-dev
    # QEMU and its firmware, both machines
    qemu-system-x86 qemu-system-arm qemu-utils ovmf qemu-efi-aarch64
    # the tests: nc -U -N types keys into QEMU's monitor
    netcat-openbsd
    # building the i686 cross compiler (binutils and GCC from source)
    bison flex texinfo libgmp-dev libmpc-dev libmpfr-dev libisl-dev
    # handy: reading AArch64's device tree, debugging
    device-tree-compiler gdb
)
sudo apt-get update -q
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${PACKAGES[@]}"

# --- 3. Rust ---------------------------------------------------------------

say "3/7 Rust"
export PATH="$HOME/.cargo/bin:$PATH"
if command -v rustup >/dev/null; then
    note "rustup is here: $(rustup --version 2>/dev/null | head -1)"
else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain stable
fi
# clippy for the lint steps; the bare-metal targets come from each
# directory's rust-toolchain.toml the first time cargo runs there.
rustup component add clippy rustfmt >/dev/null
rustup target add x86_64-unknown-none aarch64-unknown-none-softfloat >/dev/null
note "$(rustc --version)"

# --- 4. KVM ----------------------------------------------------------------

say "4/7 KVM"
if [ -e /dev/kvm ]; then
    if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
        note "/dev/kvm is usable: QEMU will run x86_64 at full speed"
    else
        sudo usermod -aG kvm "$USER"
        note "added $USER to the kvm group; it takes effect in a new shell."
        note "Until then: sudo setfacl -m u:$USER:rw /dev/kvm (done now)"
        sudo setfacl -m "u:$USER:rw" /dev/kvm 2>/dev/null || true
    fi
else
    note "no /dev/kvm: QEMU will emulate (slow, but everything works)."
    note "For KVM: Intel VT-x on in the BIOS, and in C:\\Users\\<you>\\.wslconfig"
    note "  [wsl2]"
    note "  nestedVirtualization=true"
    note "then in PowerShell: wsl --shutdown, and open Ubuntu again."
fi

# --- 5. The checkout -------------------------------------------------------

say "5/7 The TheTaaJ checkout"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if [ -z "$TARGET_DIR" ] && [ -f "$HERE/../Makefile" ] && [ -d "$HERE/../kernel" ]; then
    ROOT=$(cd "$HERE/.." && pwd)
    note "using the checkout this script is in: $ROOT"
else
    ROOT=${TARGET_DIR:-$HOME/Workspace/TheTaaJ}
    if [ -d "$ROOT/.git" ]; then
        note "already cloned: $ROOT"
    else
        mkdir -p "$(dirname "$ROOT")"
        # The submodules are named by SSH (git@github.com:...). With a
        # GitHub SSH key here, clone that way so you can push; without
        # one, read-only over HTTPS.
        if ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -T git@github.com 2>&1 \
                | grep -q "successfully authenticated"; then
            note "cloning over SSH"
            git clone --recursive git@github.com:TheJaat/TheTaaJ.git "$ROOT"
        else
            note "no GitHub SSH key: cloning read-only over HTTPS"
            note "(to push later: ssh-keygen, add the key to GitHub, and"
            note " git remote set-url origin git@github.com:TheJaat/TheTaaJ.git)"
            git -c url."https://github.com/".insteadOf=git@github.com: \
                clone --recursive https://github.com/TheJaat/TheTaaJ.git "$ROOT"
        fi
    fi
fi
cd "$ROOT"
mkdir -p build
git -c url."https://github.com/".insteadOf=git@github.com: \
    submodule update --init --recursive

# --- 6. The i686 cross compiler -------------------------------------------

say "6/7 The i686 cross compiler (toolchain/i686)"
if [ -x toolchain/i686/bin/i686-elf-gcc ]; then
    note "already built: $(toolchain/i686/bin/i686-elf-gcc --version | head -1)"
else
    note "building binutils 2.42 and GCC 14.1.0 from source; this takes a while"
    (cd toolchain && bash toolchain_i686.sh) > build/build-toolchain.log 2>&1 \
        || fail "the cross compiler did not build; see $ROOT/build/build-toolchain.log"
    note "$(toolchain/i686/bin/i686-elf-gcc --version | head -1)"
fi

# --- 7. A first build ------------------------------------------------------

say "7/7 A first build: make iso64"
make iso64 > build/build-first.log 2>&1 || fail "make iso64 failed; see $ROOT/build/build-first.log"
note "build/image64.iso is built"

if ! grep -q 'cargo/bin' "$HOME/.bashrc" 2>/dev/null; then
    echo '. "$HOME/.cargo/env"' >> "$HOME/.bashrc"
fi

ACCEL=tcg
[ -r /dev/kvm ] && [ -w /dev/kvm ] && ACCEL=kvm
LLVM=$(llvm-config-18 --version 2>/dev/null || echo none)

say "Done"
cat <<EOF
   Checkout:   $ROOT
   QEMU:       $(qemu-system-x86_64 --version | head -1) (-accel $ACCEL)
   LLVM:       $LLVM$( [ "$LLVM" = 18.1.3 ] || echo "  (not 18.1.3: the LLVM/Clang/lld tests will fail)")
   Rust:       $(rustc --version)

   Next, in a new terminal (so PATH and the kvm group apply):
     cd $ROOT
     make run64             boot it; the serial console is this terminal
                            (QEMU's window opens through WSLg)
     make test64            every test; ends with "test64: all passed"
EOF
