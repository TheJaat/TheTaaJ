# Define subdirectories
BOOTLOADER_DIR = bootloader
KERNEL_DIR = kernel

.PHONY: all clean run iso ramdisk modules servers librt disk tfstool tfsdisk tfstool-rs tfscheck \
        kernel64 iso64 run64 run-disk boot64-test smp64-test big64-test kernel64-test fault64-test test64 \
        uefi-loader run64-uefi boot64-uefi-test user64 libc64 cprogs64 cput libc-test

# Default target
all: $(BUILD_DIR) build-bootloader build-kernel iso disk tfsdisk

# Build directory
BUILD_DIR = build

# ISO-related variables
ISO_DIR = iso
ISO_IMG = $(BUILD_DIR)/image.iso

# Bootloader build variables
BOOTLOADER_BUILD_DIR = bootloader/$(BUILD_DIR)
STAGE1_BUILD_DIR = $(BOOTLOADER_BUILD_DIR)/stage1
STAGE2_BUILD_DIR = $(BOOTLOADER_BUILD_DIR)/stage2
STAGE1_BIN = $(STAGE1_BUILD_DIR)/stage1.bin
STAGE2_BIN = $(STAGE2_BUILD_DIR)/stage2.bin

KERNEL_BUILD = kernel/$(BUILD_DIR)
KERNEL_ELF = $(KERNEL_BUILD)/kernel.elf

# Ensure the build directory exists
$(BUILD_DIR):
	@echo "Creating build directory..."
	mkdir -p $(BUILD_DIR)

# Build the bootloader
build-bootloader: $(BUILD_DIR)
	@echo "Building bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR)

# Build the kernel
build-kernel: $(BUILD_DIR)
	@echo "Building kernel..."
	$(MAKE) -C $(KERNEL_DIR)

# Ramdisk.
#
# The packing tool is built by kernel/tools/ramdisk as part of the kernel
# build, so the path below points at where that puts it. The tool is a
# HOST binary (plain gcc), not a cross-compiled one - it runs here, on
# the build machine.
RD_TOOL = $(KERNEL_DIR)/build/tools/ramdisk/ramdisk
RAMDISK_IMG = $(BUILD_DIR)/RAMDISK.MDR
MODULE_DIR = $(BUILD_DIR)/modules
MODULE_BINS = $(MODULE_DIR)/hello.mod $(MODULE_DIR)/user.mod \
              $(MODULE_DIR)/echo.mod $(MODULE_DIR)/talker.mod \
              $(MODULE_DIR)/mathsrv.mod $(MODULE_DIR)/mathcli.mod \
              $(MODULE_DIR)/ps2.mod \
              $(MODULE_DIR)/pci.mod $(MODULE_DIR)/serial.mod \
              $(MODULE_DIR)/init.mod $(MODULE_DIR)/regsrv.mod \
              $(MODULE_DIR)/calcsrv.mod $(MODULE_DIR)/calccli.mod \
              $(MODULE_DIR)/ata.mod $(MODULE_DIR)/fat32.mod $(MODULE_DIR)/fsls.mod $(MODULE_DIR)/sh.mod $(MODULE_DIR)/tfs.mod
RAMDISK_SRC = $(wildcard ramdisk-src/*)

# Loadable modules. Cross-compiled, but only to .o - they are relocated
# by the kernel at load time, never linked.
# The user-space runtime must exist before any server links against it.
librt:
	@echo "Building librt..."
	$(MAKE) -C librt ROOT_DIR=$(abspath .)

servers: librt
	@echo "Building servers..."
	$(MAKE) -C servers ROOT_DIR=$(abspath .)

modules: servers
	@echo "Building modules..."
	$(MAKE) -C modules ROOT_DIR=$(abspath .)

# Depends on build-kernel rather than on $(RD_TOOL) directly: the tool
# does not exist until the kernel build has run, and there is no rule
# here that knows how to make it.
tfstool:
	@echo "Building the TaajFS host tool..."
	$(MAKE) -C tools/tfs ROOT_DIR=$(abspath .)

ramdisk: build-kernel modules tfstool
	@echo "Packing ramdisk..."
	mkdir -p $(BUILD_DIR)
	$(RD_TOOL) $(RAMDISK_IMG) $(RAMDISK_SRC) $(MODULE_BINS)

# Create the ISO image
iso: $(STAGE1_BIN) $(STAGE2_BIN) ramdisk
	@echo "Creating ISO image..."
	mkdir -p $(ISO_DIR)/boot
	mkdir -p $(ISO_DIR)/kernel
	mkdir -p $(ISO_DIR)/saample
	cp $(STAGE1_BIN) $(ISO_DIR)/
	cp $(STAGE2_BIN) $(ISO_DIR)/
	cp $(KERNEL_ELF) $(ISO_DIR)/kernel/
	cp $(RAMDISK_IMG) $(ISO_DIR)/
	xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot -boot-load-size 4 -o $(ISO_IMG) $(ISO_DIR)

# A FAT32 disk image, built with the host tools. Using mkfs.vfat rather
# than writing our own formatter is deliberate: when a read comes back
# wrong, an independent implementation settles whether the disk or the
# driver is at fault.
DISK_IMG = $(BUILD_DIR)/disk.img

disk:
	@command -v mkfs.vfat >/dev/null || { \
	    echo "mkfs.vfat not found - install dosfstools and mtools"; exit 1; }
	@command -v mcopy >/dev/null || { \
	    echo "mcopy not found - install mtools"; exit 1; }
	@echo "Building FAT32 disk image..."
	mkdir -p $(BUILD_DIR)
	dd if=/dev/zero of=$(DISK_IMG) bs=1M count=64 2>/dev/null
	mkfs.vfat -F 32 -n TAAJDISK $(DISK_IMG) >/dev/null
	echo "hello from a real FAT32 volume" > $(BUILD_DIR)/hello.txt
	mcopy -i $(DISK_IMG) $(BUILD_DIR)/hello.txt ::HELLO.TXT
	mcopy -i $(DISK_IMG) README.md ::README.MD
	@echo "  FAT32 disk image ready"

# The Rust rewrite of tfstool. Not part of `all` yet: it needs cargo,
# and the C tool remains the one the build uses.
tfstool-rs:
	@echo "Building the Rust tfstool..."
	cd tools/tfstool-rs && cargo build --release

# Unit tests for the Rust tool, then a cross-check of the two tools
# against each other.
tfscheck: tfstool tfstool-rs
	cd tools/tfstool-rs && cargo test
	sh tools/tfstool-rs/crosscheck.sh

TFS_IMG = $(BUILD_DIR)/tfs.img

# The second drive, formatted with our own filesystem and populated by
# the host tool - which is also how we check afterwards what the OS
# wrote to it.
tfsdisk: tfstool
	@echo "Building TaajFS disk image..."
	$(BUILD_DIR)/tools/tfstool mkfs $(TFS_IMG) 4096
	echo "a file on TaajFS, written from the host" > $(BUILD_DIR)/tfs-hello.txt
	$(BUILD_DIR)/tools/tfstool put $(TFS_IMG) hello.txt $(BUILD_DIR)/tfs-hello.txt
	@echo "  TaajFS disk image ready"

# Run the bootloader in QEMU
run: iso
	@echo "Running the bootloader in QEMU..."
	qemu-system-x86_64 -m 512M -cdrom $(ISO_IMG) -boot d \
		-drive file=$(DISK_IMG),format=raw,if=ide,index=0,media=disk \
		-drive file=$(TFS_IMG),format=raw,if=ide,index=1,media=disk \
		-serial stdio

# ---- the 64-bit kernel (Era 1) ---------------------------------------
#
# The same stage1/stage2 boot it: stage2 reads the ELF class and takes the
# long-mode path for an ELF64 kernel. The 32-bit system above is unchanged
# and stays the reference.

# The Rust kernel and its interface (abi) live in the kernel repository,
# beside the 32-bit C kernel.
KERNEL64_DIR = kernel/kernel64

# rustup installs cargo and rustc in ~/.cargo/bin, and adds it to PATH
# only in shells started after the install. Prepend it here so the build
# works from any shell; `export` passes it on to scripts and sub-makes.
export PATH := $(HOME)/.cargo/bin:$(PATH)
KERNEL64_ELF = $(KERNEL64_DIR)/target/x86_64-unknown-none/release/kernel64
ISO64_DIR    = $(BUILD_DIR)/iso64
ISO64_IMG    = $(BUILD_DIR)/image64.iso
SERIAL64_LOG = $(BUILD_DIR)/serial64.log
UEFI64_LOG   = $(BUILD_DIR)/serial64-uefi.log

# What the boot tests type at the idle kernel - with a typo corrected by
# Backspace - and the line it must echo back.
# F12 crashes the keyboard driver on purpose; "/" waits for init to
# restart it; then the line is typed to the new driver.
# Near the end, programs that never end are stopped with Ctrl+C; and a
# line typed and abandoned with Ctrl+C ("oops") must not run - if it
# did, the last line would not be "ps", and the check that ps ran twice
# would fail. The "~" before it waits for the shell to be back at its
# prompt: keys typed while a program runs belong to that run (a Ctrl+C
# among them stops the program), and this one is meant for the prompt.
# C programs: compiled by Clang for a bare x86_64 machine, linked with
# libc.a (libc/, the taaj runtime under C names) by the same linker
# script as the Rust programs, and put on the disk as files. Without
# Clang and lld there are none, and the tests do not ask for them.
HAVE_CC     := $(shell command -v clang >/dev/null && command -v ld.lld >/dev/null && echo yes)
LIBC_DIR     = libc
# The library is made in two halves: the Rust half by cargo, and the
# half written in C (libc/c: printf) by Clang, added to a copy of it.
LIBC_RUST_A  = $(LIBC_DIR)/target/x86_64-unknown-none/release/libc.a
C_BIN        = $(BUILD_DIR)/c
# cwords is standard C and nothing else, so the same file is also
# compiled for this machine, with this machine's compiler and library.
# The boot tests run both on the same input and compare what they print.
HOST_CC      = cc
CWORDS_HOST  = $(C_BIN)/host-cwords
# The same for cprimes. Four copies of it are run at once in the boot
# tests; CPRIMES_LIMIT is chosen so that each takes ten seconds or so
# there - long enough that all four are still running when the last has
# been typed.
CPRIMES_HOST = $(C_BIN)/host-cprimes
CPRIMES_LIMIT = 24000000
LIBC_A       = $(C_BIN)/libc.a
LIBC_C       = format stdio fcntl ctype stdlib
# Freestanding: no host headers or libraries, only libc/include and the
# compiler's own (stddef.h, stdarg.h). General registers only: the
# kernel does not save FPU or SSE state on a switch yet, so a C program
# must not use them - float and double are done by library calls
# (soft float; the functions are in libc.a).
TAAJ_CFLAGS  = --target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding \
               -fno-stack-protector -fno-pic -mno-red-zone -mgeneral-regs-only \
               -nostdlibinc -isystem $(LIBC_DIR)/include
TAAJ_LDFLAGS = -static -nostdlib --gc-sections -s -T user/user.ld
ifeq ($(HAVE_CC),yes)
C_PROGRAMS   = chello cmem cprint cfile cstdio cwords cask cprimes
ROOT_FILES   = 25
C_KEYS       = c h e l l o ret c h e l l o spc o n e spc shift-apostrophe t w o spc w o r d s shift-apostrophe ret c h e l l o spc f a i l ret c h e l l o spc e x i t ret c h e l l o spc f l o a t ret w i t h spc f i l e s spc c f i l e spc c h e c k ret c f i l e spc c a t spc w e l c o m e ret w i t h spc r e a d o n l y spc c f i l e spc c a t spc n o f i l e spc w e l c o m e ret w i t h spc r e a d o n l y spc c f i l e spc w r i t e spc c n o spc x ret w i t h spc f i l e s spc c f i l e spc w r i t e spc c n o t e spc m a d e spc b y spc c ret w i t h spc r e a d o n l y spc c f i l e spc c a t spc l o n g spc shift-dot spc c l o n g ret c f i l e spc c a t spc shift-comma spc l o n g spc shift-dot spc c l o n g 2 ret c f i l e ret c a s k ret a d a ret 4 0 ret x ret 2 ret ctrl-d c a s k spc o n e ret f i r s t ret e c h o spc a f t e r ret e c h o spc a d a spc shift-dot spc w h o ret c a s k spc o n e spc shift-comma spc w h o ret c a t spc l o n g spc shift-backslash spc c w o r d s spc minus n spc 3 ret w i t h spc r e a d o n l y spc c w o r d s spc minus n spc 2 5 spc c w o r d s spc l o n g spc shift-dot spc c w 1 ret c w o r d s spc minus n spc 5 spc shift-comma spc l o n g spc shift-dot spc c w 2 ret c w o r d s spc l o n g ret c w o r d s spc minus n spc n o n e ret c w o r d s spc minus x ret w i t h spc f i l e s spc c s t d i o spc c h e c k ret c s t d i o spc l i n e s spc w e l c o m e ret c s t d i o spc l i n e s spc shift-comma spc w e l c o m e ret w i t h spc r e a d o n l y spc c s t d i o spc l i n e s spc l o n g spc shift-dot spc c n u m ret w i t h spc f i l e s spc c s t d i o spc c o p y spc l o n g spc c c o p y ret c s t d i o spc o r d e r ret c s t d i o spc e n d spc shift-dot spc c e n d ret c s t d i o spc l o s t spc shift-dot spc c l o s t ret c s t d i o spc e n d ret c s t d i o ret c p r i n t spc o n e spc shift-apostrophe t w o spc w o r d s shift-apostrophe ret c p r i n t spc l o n g spc shift-backslash spc w c ret c p r i n t spc shift-dot spc p o u t ret c m e m ret c m e m spc s o r t spc p e a r spc a p p l e spc f i g spc a p p l e ret c m e m spc h u g e ret c h e l l o spc e r r spc shift-dot spc c e r r ret c h e l l o spc a spc b spc shift-dot spc c o u t ret c h e l l o spc x spc shift-backslash spc w c ret c p r i m e s spc 1 0 0 ret ~ c p r i m e s spc 2 4 0 0 0 0 0 0 spc shift-7 ret c p r i m e s spc 2 4 0 0 0 0 0 0 spc shift-7 ret c p r i m e s spc 2 4 0 0 0 0 0 0 spc shift-7 ret c p r i m e s spc 2 4 0 0 0 0 0 0 spc shift-7 ret t i m e spc w a i t ret ~~
EXPECT_C     = ;^Hello from C;^argc 1\s*$$;^argc 3\s*$$;^argv\[1\] one\s*$$;^argv\[2\] two words\s*$$;^chello exited with code 3;^chello exited with code 4;^7.0 / 2 \* 4 = 14\s*$$;^chello: this line is on standard error;^files: 48 checks, all passed;^cfile: welcome: this program was not given the files;^cfile: nofile: no such file or directory;^cno: the files are read-only here;^cnote: 10 bytes;^usage: cfile cat;^hello, ada \(typed\);^cask: not a number: x;^total: 42;^one line: first \(typed\);^after\s*$$;^one line: ada \(from a file or another program\);^ +301  300\s*$$;^ +300  line\s*$$;^1200 words, 302 different;^cwords: long: this program was not given the files;^0 words, 0 different;^cwords: -n needs a number from 1 up, not .none.;^usage: cwords \[-n COUNT\];^streams: 52 checks, all passed;^cstdio: welcome: this program was not given the files;^   1  Hello from the host;^ccopy: 4692 bytes;^write was called second - printf was called first\s*$$;^no new line, and still here;^usage: cstdio lines;^printf: 42 -7 ff 0X2A \[   hi\] \[hi   \] \[00042\] x 100.\s*$$;^sizes: 18446744073709551615 -9223372036854775808\s*$$;^printf returned 6\s*$$;^snprintf: wanted 12, kept <hello, >;^argument 2 of 2: two words\s*$$;^puts: a line of its own\s*$$;^fputs and putchar\s*$$;^cprint: 9 lines went to standard output;^cprint: 7 lines went to standard output;^1 line, 1 word, 601 bytes;^strings: 28 checks, all passed;^memory: 15 checks, all passed;^apple apple fig pear\s*$$;^cmem: no memory for a terabyte;^cmem exited with code 1;^3 lines, 7 words, 30 bytes;^cprimes: 25 primes below 100\s*$$;^\[4\] cprimes 24000000;^took [0-9]+\.[0-9][0-9] s; the 4 CPUs worked for [0-9]+\.[0-9][0-9] s in all
else
C_PROGRAMS   =
ROOT_FILES   = 17
C_KEYS       =
EXPECT_C     =
endif

TEST_KEYS    = f12 / shift-h i spc t h e r x backspace e ret \
               p s ret m e m ret r u n spc h e l l o ret r u n spc p e e k ret \
               w r i t e spc n o t e s spc h e l l o ret \
               a p p e n d spc n o t e s spc w o r l d ret \
               c a t spc n o t e s ret l s ret \
               r m spc n o t e s ret c a t spc n o t e s ret \
               c a t spc w e l c o m e ret w r i t e spc s a v e d spc k e p t ret \
               c a t spc s a v e d spc w e l c o m e ret r u n spc c a t ret e c h o e d ret ctrl-d \
               r m spc n o t e s spc s a v e d 2 ret w r i t e spc o n l y ret \
               r u n spc p r i m e s ret r u n spc w e l c o m e ret r u n spc b r o k e n ret \
               r u n spc n o t h i n g ret \
               l s spc d o c s ret c a t spc d o c s slash r e a d m e ret \
               m k d i r spc d o c s slash d e e p spc w o r k ret \
               w r i t e spc d o c s slash d e e p slash n o t e spc n e s t e d ret \
               l s spc d o c s spc d o c s slash d e e p ret \
               r m spc d o c s ret m k d i r spc d o c s ret c a t spc d o c s ret \
               w r i t e spc w e l c o m e slash x spc y ret \
               r m spc w o r k ret l s spc w o r k ret t o o l s slash g i v e n ret \
               p w d ret c d spc d o c s ret p w d ret \
               w r i t e spc h e r e spc i n s i d e ret l s ret \
               c d spc d e e p ret p w d ret c a t spc dot dot slash h e r e ret \
               a p p e n d spc slash d o c s slash h e r e spc m o r e ret \
               c d spc dot dot slash dot dot slash t o o l s ret dot slash g i v e n ret \
               w i t h spc f i l e s spc g i v e n ret \
               c d spc slash w e l c o m e ret c d spc n o w h e r e ret c d spc a spc b ret \
               m k d i r spc g o n e ret c d spc g o n e ret r m spc dot dot slash g o n e ret l s ret \
               c d spc dot dot slash dot dot slash dot dot ret p w d ret \
               w r i t e spc r o o t e d spc y e s ret \
               c d spc t o o l s ret c d ret w r i t e spc h o m e spc y e s ret \
               m v spc h o m e spc h o u s e ret c a t spc h o u s e ret \
               m v spc h o u s e spc d o c s ret l s spc d o c s slash h o u s e ret \
               c p spc w e l c o m e spc d o c s ret c p spc l o n g spc d o c s slash l o n g 2 ret \
               m v spc d o c s slash d e e p spc t o o l s ret c a t spc t o o l s slash d e e p slash n o t e ret \
               m v spc t o o l s slash d e e p spc d o c s slash d e e p ret \
               m v spc d o c s spc d o c s ret m v spc r o o t e d spc w e l c o m e ret m v spc n o n e spc x ret \
               c p spc d o c s spc x ret c p spc r o o t e d spc w e l c o m e ret m v spc o n e ret \
               w i t h spc r e a d o n l y spc m v spc r o o t e d spc x ret \
               c d spc d o c s ret m v spc h o u s e spc dot dot slash c o t t a g e ret c d ret \
               w r i t e spc q u o t e d spc shift-apostrophe t w o spc spc w o r d s shift-apostrophe ret \
               w r i t e spc p l a i n spc t w o spc spc w o r d s ret \
               m k d i r spc shift-apostrophe m y spc d o c s shift-apostrophe ret \
               w r i t e spc apostrophe m y spc d o c s slash a spc f i l e apostrophe spc x ret \
               c d spc m y backslash spc d o c s ret p w d ret l s ret c d ret \
               g i v e n spc apostrophe a spc b apostrophe spc c spc shift-apostrophe shift-apostrophe ret \
               e c h o spc a shift-apostrophe b spc c shift-apostrophe d ret \
               e c h o spc shift-apostrophe u n c l o s e d ret e c h o spc e n d backslash ret \
               e c h o spc f i r s t spc shift-dot spc o u t ret e c h o spc s e c o n d spc shift-dot shift-dot spc o u t ret \
               c a t spc o u t ret l s spc d o c s shift-dot l i s t ret w c spc shift-comma spc l i s t ret \
               w c spc l o n g ret w c spc shift-comma spc l o n g ret \
               c a t spc shift-comma spc l o n g spc shift-dot spc l o n g 3 ret \
               c a t spc l s spc l s spc shift-dot spc b i g ret c a t spc shift-comma spc b i g spc shift-dot spc b i g 2 ret \
               c a t spc w e l c o m e spc o u t spc shift-dot spc b o t h ret p s spc shift-dot spc p r o c s ret \
               e c h o spc shift-apostrophe a spc shift-dot spc b shift-apostrophe ret \
               w c spc shift-comma spc n o n e ret e c h o spc x spc shift-dot spc d o c s ret e c h o spc x spc shift-dot ret \
               c a t spc n o n e spc shift-dot spc e r r ret n o s u c h spc shift-dot spc e m p t y ret \
               c a t spc l o n g spc shift-backslash spc w c spc shift-dot spc p i p e d ret \
               c a t spc shift-comma spc l o n g spc shift-backslash spc w c spc shift-dot shift-dot spc p i p e d ret \
               c a t spc l s spc l s spc shift-backslash spc c a t spc shift-backslash spc c a t spc shift-dot spc b i g 3 ret \
               e c h o spc o n e spc t w o spc t h r e e spc shift-backslash spc w c ret \
               t i m e spc c a t spc l o n g spc shift-backslash spc w c ret \
               y e s spc shift-backslash spc g i v e n spc x p i p e ret \
               e c h o spc shift-apostrophe p spc shift-backslash spc q shift-apostrophe ret \
               l s spc shift-backslash ret \
               l s spc shift-backslash spc c d ret \
               l s spc shift-backslash spc n o s u c h 2 ret \
               l s spc shift-dot spc x spc shift-backslash spc w c ret \
               c a t spc l s spc l s spc shift-backslash spc c a t spc shift-dot spc b i g 4 spc shift-7 ret \
               f o r e v e r spc s l e e p spc shift-dot spc f l o g spc shift-7 ret \
               f o r e v e r spc y i e l d spc shift-7 ret \
               j o b s ret \
               k i l l spc 3 ret \
               k i l l spc 2 ret \
               k i l l spc 9 ret \
               e c h o spc b g spc shift-backslash spc w c spc shift-dot spc b g c o u n t spc shift-7 ret \
               w a i t ret \
               j o b s ret \
               e c h o spc h i spc shift-7 ret \
               l s spc shift-7 spc w c ret \
               e c h o spc shift-apostrophe x spc shift-7 spc y shift-apostrophe ret \
               $(C_KEYS) \
               $(KEYBOARD_KEYS) \
               s e r v i c e s ret \
               s e r v i c e s spc f i l e s ret \
               s e r v i c e s spc d i s k ret \
               c a t spc n o f i l e spc shift-backslash spc w c ret l s spc n o d i r spc shift-dot spc l s o u t ret \
               s e r v i c e s spc n o s u c h ret \
               r e g i s t r y spc c r a s h ret \
               s e r v i c e s spc f i l e s dot l o o k ret \
               c a t spc w e l c o m e ret \
               w i t h spc r e a d o n l y spc m k d i r spc x ret \
               g i v e n spc a spc b ret w i t h spc f i l e s spc g i v e n ret w i t h ret \
               w i t h spc r e a d o n l y spc g i v e n ret \
               w i t h spc r e a d o n l y spc w r i t e spc s a v e d spc l o s t ret \
               w i t h spc r e a d o n l y spc r m spc s a v e d ret c a t spc s a v e d ret \
               f s spc s t a l l spc 6 ret l s ret ~ l s ret \
               d i s k spc s t a l l spc 6 ret w r i t e spc d u r i n g spc x ret r m spc d u r i n g ret ~ \
               d i s p l a y spc s t a l l spc 6 ret e c h o spc u n s e e n ret ~ \
               f11 / e c h o spc k e y s ret ~ \
               d i s k ret d i s k spc r e a d spc 0 ret \
               d i s k spc w r i t e spc 5 spc o o p s ret d i s k spc r e a d spc 9 9 9 9 9 ret \
               d i s k spc c h e c k ret t i m e spc l s ret \
               f o r e v e r ret ctrl-c f o r e v e r spc w a i t ret ctrl-c \
               r u n spc f o r e v e r spc s l e e p ret ctrl-c \
               ~ o o p s ctrl-c p s ret \
               c a t spc l o n g ret c l e a r ret e c h o spc s c r e e n ret
TEST_TYPED   = Hi there
# What those commands must print (grep patterns, separated by ";").
# (Joined on one line: a continued line would start each pattern with a
# space.)
EXPECT_SHELL = unknown command: Hi;  PID  NAME;init +running;^memory: [0-9]+ MiB used;^hello exited with code 42;^peek was killed
EXPECT_FILES = \[fs\] ready: TaajFS on the disk;^notes: 6 bytes;^notes: 12 bytes;^hello\s*$$;^world\s*$$;^ +12  notes;^ +20  welcome;^ +dir  docs/;^2 directories, $(ROOT_FILES) files, [0-9]+ bytes;^cat: notes: no such file;^cat exited with code 1;^kept\s*$$;^Hello from the host;^saved: 5 bytes
EXPECT_DISK  = \[blk\] virtio disk.*MSI-X;^disk: 8192 sectors of 512 bytes \(4096 KiB\);^0000  4a 41 46 53 .*JAFS;^disk: sector 5: the disk is read-only;^disk: sector 99999: no such sector;^disk check: all 9 checks passed;^took [0-9]+\.[0-9][0-9] s;^line 300 of 300
EXPECT_RUN   = \[init\] started primes for the shell, from a file;^primes below 50: 2 3 5 7 11 13 17 19 23 29 31 37 41 43 47;^primes exited with code 0;^error: welcome is not a program;^error: broken is not a program;^error: no such program;\[init\] started ls for the shell, from a file;^cat exited with code 0;^rm: notes: no such file;^rm: saved2: no such file;^rm exited with code 1;^usage: write FILE TEXT;^write exited with code 2;$(EXPECT_GIVEN);$(EXPECT_WATCH);$(EXPECT_KILL)
EXPECT_DIRS  = ^ +23  readme;^1 file, 23 bytes;^Read me, from the host;^docs/deep/note: 7 bytes;^docs:;^ +dir  deep/;^1 directory, 1 file, 23 bytes;^docs/deep:;^ +7  note;^rm: docs: the directory is not empty;^mkdir: docs: there is already something with that name;^cat: docs: is a directory;^write: welcome/x: not a directory;^ls: work: no such file or directory;\[init\] started tools/given for the shell, from a file;^mkdir: x: the files are read-only here
EXPECT_CD    = ^/\s*$$;^/docs\s*$$;^here: 7 bytes;^1 directory, 2 files, 30 bytes;^/docs/deep\s*$$;^inside\s*$$;^/docs/here: 12 bytes;\[init\] started \./given for the shell, from a file;^given: started in /tools;^cd: /welcome: not a directory;^cd: nowhere: no such file or directory;^usage: cd \[DIR\];^ls: \.: no such file or directory;^rooted: 4 bytes;^home: 4 bytes
EXPECT_MOVE  = ^yes\s*$$;^ +4  docs/house;^docs/welcome: 20 bytes;^docs/long2: [0-9]+ bytes;^nested\s*$$;^mv: docs to docs/docs: a directory cannot be moved into itself;^mv: rooted to welcome: there is already something with that name;^mv: none to x: no such file or directory;^cp: docs: is a directory;^cp: welcome: there is already something with that name;^usage: mv FROM TO;^mv: rooted to x: the files are read-only here
EXPECT_QUOTE = ^quoted: 11 bytes;^plain: 10 bytes;^my docs/a file: 2 bytes;^/my docs\s*$$;^ +2  a file;^given: arguments .a b. .c. ..\s*$$;^ab cd\s*$$;^the line has a . that is never closed;^the line ends in a . with nothing after it
EXPECT_REDIR = ^first\s*$$;^second\s*$$;^6 lines, [0-9]+ words, [0-9]+ bytes;^long: 300 lines, 1200 words, 4692 bytes;^300 lines, 1200 words, 4692 bytes;^a > b\s*$$;^none: no such file or directory;^docs: is a directory;^there is a > with no file name after it;^unknown command: nosuch
EXPECT_GIVEN = ^given: the screen, no files;^given: arguments .a. .b.\s*$$;^given: the screen, and the files \([0-9]+ of them\), to read and to change;^given: the screen, and the files \([0-9]+ of them\), to read only;^usage: with files COMMAND, with readonly COMMAND;^write: saved: the files are read-only here;^rm: saved: the files are read-only here;\[fs\] asked to stall;^fs: the file service will answer nothing for 6 s;\[init\] fs has not answered for 4 s - stopping it;\[process\] [0-9]+ .fs. stopped: killed by request;\[init\] fs ended with code -1001 - restarting it
EXPECT_WATCH = \[blk\] asked to stall;\[init\] blk has not answered for 2 s - stopping it;\[init\] blk ended with code -1001 - restarting it;^during: 2 bytes;\[display\] asked to stall;\[init\] display has not answered for 2 s - stopping it;\[init\] display ended with code -1001 - restarting it;\[display\] the text on the screen was kept: [0-9]+ lines shown again;\[kbd\] F11: simulating a driver bug;\[init\] kbd has not answered for 2 s - stopping it;\[init\] kbd ended with code -1001 - restarting it;\[shell\] line: .echo keys.
EXPECT_KILL  = ^forever: spin until stopped;^forever: wait until stopped;^forever: sleep until stopped;\[process\] [0-9]+ .forever. stopped: killed by request;^\^C;^forever was stopped
EXPECT_PIPE  = ^1 line, 3 words, 14 bytes;^301 lines, 1212 words;^given: arguments .xpipe.;^p . q\s*$$;^there is a . with no command on one side of it;^cd is one of the shell.s own commands;^unknown command: nosuch2;^only the last command of a pipeline
EXPECT_JOBS  = ^\[1\] cat ls ls . cat > big4;^\[2\] forever sleep > flog;^\[3\] forever yield;^forever: yield until stopped;^\[2\] running  forever sleep > flog;^\[3\] running  forever yield;^\[3\] stopped  forever yield;^\[2\] stopped  forever sleep > flog;^kill: no job 9;^\[1\] done  cat ls ls . cat > big4;^no jobs;^echo is one of the shell.s own commands: it cannot run in the background;^a & can only end the line;^x & y\s*$$
EXPECT_NAMES = \[registry\] ready;\[registry\] fs may ask for: disk\s*$$;\[registry\] shell may ask for: keys display init system files files.look disk.look;^keys display init system files files.look disk.look\s*$$;^files: there for the shell;^disk: no such service - for the shell;\[registry\] shell asked for .disk.: it is not in its namespace;^nosuch: no such service - for the shell;\[registry\] asked to crash;\[init\] registry ended with code 1 - restarting it;^files.look: there for the shell
EXPECT_GIVE  = \[registry\] shell gave ls: files=files.look;\[registry\] shell gave write: files\s*$$;\[registry\] shell gave given: files\s*$$;\[registry\] shell gave given: files=files.look;^given: it may ask for: files\s*$$
# The keyboard as a program's standard input: lines typed to wc and to
# cat (into a file) and ended with Ctrl+D; and a program stopped with
# Ctrl+C while it waits for a line - what was typed to it is nobody's,
# and the next line is the shell's, whole.
KEYBOARD_KEYS = w c ret t w o spc w o r d s ret ctrl-d c a t spc shift-dot spc t y p e d ret k e p t spc l i n e ret ctrl-d c a t ret a b c ctrl-c e c h o spc c l e a n ret
EXPECT_KEYBOARD = ^1 line, 2 words, 10 bytes;^\[shell\] line: .echo clean.;^cat was stopped
EXPECT_ERR   = ^cat: nofile: no such file or directory;^0 lines, 0 words, 0 bytes;^ls: nodir: no such file or directory
TEST_EXPECT  = $(EXPECT_SHELL);$(EXPECT_FILES);$(EXPECT_DISK);$(EXPECT_RUN);$(EXPECT_DIRS);$(EXPECT_CD);$(EXPECT_MOVE);$(EXPECT_QUOTE);$(EXPECT_REDIR);$(EXPECT_PIPE);$(EXPECT_JOBS);$(EXPECT_NAMES);$(EXPECT_GIVE);$(EXPECT_ERR);$(EXPECT_KEYBOARD)$(EXPECT_C)
# The disk the boot tests attach: a 4 MiB TaajFS volume made on the host
# by tfstool, with one file in it for the OS to read. After the boot,
# check-boot.sh reads back - with tfstool again - the file the OS wrote.
# Also on it: the programs that exist nowhere else - the kernel image
# does not contain them: ls, cat, write, append, rm, mkdir, mv and cp, which the shell
# runs as commands, and primes - and the first 300 bytes of primes, as a file that looks
# like a program and is not. And `long`, 300 lines: more than a screen,
# printed faster than the display draws. And two directories made on the
# host: docs, with a file in it, and tools, with a program in it - run by
# its path. The test then moves about in them with cd: writes a file by
# a bare name in docs, reads it from docs/deep as ../here, runs ./given
# in tools, stands in a directory while it is removed, and climbs back
# to the root - twice, by ".." and by a bare `cd` - to write there.
# And it moves things: a file to a new name, into a directory, out again
# by a relative path; a directory with a file in it, there and back; and
# copies a short file and one several messages long.
# And it quotes: text with two spaces kept by double quotes (and not
# kept without), a directory and a file with spaces in their names, by
# all three ways of writing one, an empty argument, and two lines that
# must not run - a quote never closed, a backslash with nothing after.
# And it joins commands with |: two programs, and three with 50 KB
# between them, which must arrive exact; an input file at the start and
# an output file at the end; one of the shell's own commands first; a
# writer that never stops, ended by its reader leaving; and the lines
# that must not run.
# And it runs lines that end in & in the background: a pipeline that
# writes 50 KB to a file while the next lines are typed, which must
# arrive exact; two programs that never end, listed by jobs and ended by
# kill; one of the shell's own commands before a program; wait; and the
# lines that must not run.
# And services are found by name: the shell lists the names that exist
# for it, asks for one that does and for one that exists only for fs;
# then the registry is made to crash, and after init has started
# another and told it everything again, a name is still found and a
# file still read.
# And it runs a program written in C: its arguments as argc and argv,
# what it returns from main and what it passes to exit as its exit
# code, and its output - through write - on the screen, into a file,
# and down a pipe.
# And complaints go to standard error: on the screen though the output
# went to a file (which must hold none of it) or down a pipe (where
# the next program must count nothing).
# And it redirects: the shell's own output and a program's into files,
# replacing and adding; a file as a program's input; both at once, for a
# text and for a file of 50 messages - more than a channel holds, so
# both ends have to wait for each other - which must arrive exact;
# and what must not run, or must not write.
USER_BIN     = user/target/x86_64-unknown-none/release
TFSTOOL_RS   = tools/tfstool-rs/target/release/tfstool
DISK_PROGRAMS = ls cat write append rm mkdir mv cp wc yes primes forever given
define make-test-disk
	@rm -f $(1) && $(TFSTOOL_RS) mkfs $(1) 1024 > /dev/null && truncate -s 4M $(1)
	@printf 'Hello from the host\n' > $(1).welcome && $(TFSTOOL_RS) put $(1) welcome $(1).welcome > /dev/null
	@for program in $(DISK_PROGRAMS); do $(TFSTOOL_RS) put $(1) $$program $(USER_BIN)/$$program > /dev/null || exit 1; done
	@for program in $(C_PROGRAMS); do $(TFSTOOL_RS) put $(1) $$program $(C_BIN)/$$program > /dev/null || exit 1; done
	@head -c 300 $(USER_BIN)/primes > $(1).broken && $(TFSTOOL_RS) put $(1) broken $(1).broken > /dev/null
	@seq 300 | sed 's/.*/line & of 300/' > $(1).long && $(TFSTOOL_RS) put $(1) long $(1).long > /dev/null
	@$(TFSTOOL_RS) mkdir $(1) docs > /dev/null && printf 'Read me, from the host\n' > $(1).readme \
	    && $(TFSTOOL_RS) put $(1) docs/readme $(1).readme > /dev/null
	@$(TFSTOOL_RS) mkdir $(1) tools > /dev/null && $(TFSTOOL_RS) put $(1) tools/given $(USER_BIN)/given > /dev/null
endef

# The disk `make run64` attaches: a 16 MiB TaajFS volume that is kept
# from one run to the next, so files written in the OS are still there.
# Made on first use; the programs on it are replaced with the ones just
# built every time.
RUN_DISK     = $(BUILD_DIR)/taaj-disk.img
RUN_DISK_ARGS = -drive file=$(RUN_DISK),format=raw,if=none,id=disk0 -device virtio-blk-pci,drive=disk0
run-disk: user64 cprogs64 tfstool-rs
	@[ -f $(RUN_DISK) ] || { $(TFSTOOL_RS) mkfs $(RUN_DISK) 4096 > /dev/null && truncate -s 16M $(RUN_DISK) \
	    && echo "Made a new disk: $(RUN_DISK)"; }
	@for program in $(DISK_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_DISK) $$program $(USER_BIN)/$$program > /dev/null || exit 1; \
	done
	@for program in $(C_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_DISK) $$program $(C_BIN)/$$program > /dev/null || exit 1; \
	done

# One C program of your own, built and put on that disk (close QEMU
# first: the disk is its while it runs):
#   make cput C=name          (the program is libc/programs/name.c)
# The whole recipe, and how to build one that is not in libc/programs
# at all, is in docs/c-programs.md.
cput: run-disk
	@[ -n "$(C)" ] || { echo "usage: make cput C=name   (builds libc/programs/name.c and puts it on $(RUN_DISK))"; exit 1; }
	@[ -f $(LIBC_DIR)/programs/$(C).c ] || { echo "cput: there is no $(LIBC_DIR)/programs/$(C).c"; exit 1; }
	@$(MAKE) --no-print-directory cprogs64 C_PROGRAMS=$(C)
	@$(TFSTOOL_RS) rm $(RUN_DISK) $(C) > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) $(C) $(C_BIN)/$(C) > /dev/null
	@echo "$(C) is on $(RUN_DISK): make run64, and type $(C)"

# The UEFI loader needs Clang and lld (`sudo apt install clang lld`).
# Without them the ISO is built BIOS-only, and says so.
UEFI_DIR     = bootloader/src/boot/uefi
UEFI_EFI     = $(BOOTLOADER_BUILD_DIR)/uefi/BOOTX64.EFI
ESP64_IMG    = $(ISO64_DIR)/efiboot.img
HAVE_CLANG  := $(shell command -v clang >/dev/null && command -v lld-link >/dev/null && echo yes)
MKFS_FAT     = /usr/sbin/mkfs.fat
# QEMU's UEFI firmware, from the ovmf package.
OVMF         = /usr/share/ovmf/OVMF.fd

# User programs come first: the kernel embeds them (kernel/kernel64/src/programs.rs).
user64:
	@echo "Building the user programs..."
	cd user && cargo build --release

# The C library, and the C programs in libc/programs: each one file,
# compiled and linked with libc.a. To build one of your own:
#   make cprogs64 C_PROGRAMS=name      (with libc/programs/name.c)
# and copy build/c/name to the disk with tfstool put.
libc64:
	@echo "Building the C library..."
	cd $(LIBC_DIR) && cargo build --release
ifeq ($(HAVE_CC),yes)
	@mkdir -p $(C_BIN)/lib
	@for part in $(LIBC_C); do \
	    echo "  CC      libc/c/$$part.c"; \
	    clang $(TAAJ_CFLAGS) -c $(LIBC_DIR)/c/$$part.c -o $(C_BIN)/lib/$$part.o || exit 1; \
	done
	@cp $(LIBC_RUST_A) $(LIBC_A)
	@ar rs $(LIBC_A) $(addprefix $(C_BIN)/lib/,$(addsuffix .o,$(LIBC_C)))
endif

cprogs64: libc64
ifeq ($(HAVE_CC),yes)
	@mkdir -p $(C_BIN)
	@for program in $(C_PROGRAMS); do \
	    echo "  CC      $$program.c"; \
	    clang $(TAAJ_CFLAGS) -c $(LIBC_DIR)/programs/$$program.c -o $(C_BIN)/$$program.o || exit 1; \
	    ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/$$program.o $(LIBC_A) -o $(C_BIN)/$$program || exit 1; \
	done
	@$(HOST_CC) -std=c11 -O2 -Wall -Wextra -Werror $(LIBC_DIR)/programs/cwords.c -o $(CWORDS_HOST)
	@$(HOST_CC) -std=c11 -O2 -Wall -Wextra -Werror $(LIBC_DIR)/programs/cprimes.c -o $(CPRIMES_HOST)
else
	@echo "Clang or lld not found: no C programs (sudo apt install clang lld)"
endif

kernel64: user64
	@echo "Building the 64-bit kernel..."
	cd $(KERNEL64_DIR) && cargo build --release

uefi-loader:
ifeq ($(HAVE_CLANG),yes)
	@echo "Building the UEFI loader..."
	$(MAKE) -C $(UEFI_DIR) ROOT_DIR=$(abspath bootloader)
else
	@echo "NOTE: clang/lld-link not found - skipping the UEFI loader (sudo apt install clang lld)"
endif

# One ISO that boots both ways. El Torito, the CD boot standard, allows
# several boot entries: BIOS firmware takes the first (stage1.bin), UEFI
# firmware the one marked EFI - a small FAT image holding the loader as
# EFI/BOOT/BOOTX64.EFI (the path firmware looks for when nothing else is
# configured) and its own copy of the kernel.
#
# The kernel is a copy without debug info: stage2 reads the whole file
# through a 1 MiB staging window, and the symbols stay in the original.
iso64: build-bootloader kernel64 uefi-loader
	@echo "Creating the 64-bit ISO image..."
	rm -rf $(ISO64_DIR)
	mkdir -p $(ISO64_DIR)/kernel
	cp $(STAGE1_BIN) $(STAGE2_BIN) $(ISO64_DIR)/
	strip --strip-debug -o $(ISO64_DIR)/kernel/kernel.elf $(KERNEL64_ELF)
ifeq ($(HAVE_CLANG),yes)
	$(MKFS_FAT) -C -n TAAJESP $(ESP64_IMG) 4096 >/dev/null
	mmd -i $(ESP64_IMG) ::/EFI ::/EFI/BOOT ::/kernel
	mcopy -i $(ESP64_IMG) $(UEFI_EFI) ::/EFI/BOOT/BOOTX64.EFI
	mcopy -i $(ESP64_IMG) $(ISO64_DIR)/kernel/kernel.elf ::/kernel/kernel.elf
	xorriso -as mkisofs -R -J -iso-level 3 \
		-b stage1.bin -no-emul-boot -boot-load-size 4 \
		-eltorito-alt-boot -e efiboot.img -no-emul-boot \
		-o $(ISO64_IMG) $(ISO64_DIR) 2>/dev/null
else
	xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot \
		-boot-load-size 4 -o $(ISO64_IMG) $(ISO64_DIR) 2>/dev/null
endif

# How many CPUs `make run64` gives the machine: `make run64 RUN_CPUS=1`.
RUN_CPUS ?= 4

run64: iso64 run-disk
	qemu-system-x86_64 -m 512M -smp $(RUN_CPUS) -cdrom $(ISO64_IMG) -boot d -serial stdio $(RUN_DISK_ARGS)

run64-uefi: iso64 run-disk
ifneq ($(HAVE_CLANG),yes)
	@echo "ERROR: no UEFI loader in the ISO - it needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	qemu-system-x86_64 -m 512M -smp $(RUN_CPUS) -bios $(OVMF) -cdrom $(ISO64_IMG) -boot d -serial stdio $(RUN_DISK_ARGS)

# Boots the 64-bit kernel headless and checks what it printed. A start on
# the "boot every architecture on every commit" rule from the roadmap.
boot64-test: iso64 cprogs64 tfstool-rs
	@echo "Booting the 64-bit kernel headless via BIOS (serial log: $(SERIAL64_LOG))..."
	$(call make-test-disk,$(SERIAL64_LOG).disk)
	@DISK=$(SERIAL64_LOG).disk KEYS="$(TEST_KEYS)" $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(SERIAL64_LOG)
	@DISK=$(SERIAL64_LOG).disk TFSTOOL=$(TFSTOOL_RS) CWORDS_HOST=$(CWORDS_HOST) CWORDS_TAAJ=$(C_BIN)/cwords CPRIMES_HOST=$(CPRIMES_HOST) CPRIMES_LIMIT=$(CPRIMES_LIMIT) TYPED="$(TEST_TYPED)" EXPECT="$(TEST_EXPECT)" SCREEN_MAX_INK=700 \
	    $(KERNEL64_DIR)/scripts/check-boot.sh $(SERIAL64_LOG) BIOS
	@echo "boot64-test passed"

# The same ISO, the same kernel, booted by UEFI firmware instead.
boot64-uefi-test: iso64 cprogs64 tfstool-rs
ifneq ($(HAVE_CLANG),yes)
	@echo "FAIL: the UEFI loader needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	@echo "Booting the 64-bit kernel headless via UEFI (serial log: $(UEFI64_LOG))..."
	$(call make-test-disk,$(UEFI64_LOG).disk)
	@DISK=$(UEFI64_LOG).disk QEMU_ARGS="-bios $(OVMF)" KEYS="$(TEST_KEYS)" $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(UEFI64_LOG) 30
	@DISK=$(UEFI64_LOG).disk TFSTOOL=$(TFSTOOL_RS) CWORDS_HOST=$(CWORDS_HOST) CWORDS_TAAJ=$(C_BIN)/cwords CPRIMES_HOST=$(CPRIMES_HOST) CPRIMES_LIMIT=$(CPRIMES_LIMIT) TYPED="$(TEST_TYPED)" EXPECT="$(TEST_EXPECT)" SCREEN_MAX_INK=700 \
	    $(KERNEL64_DIR)/scripts/check-boot.sh $(UEFI64_LOG) UEFI
	@echo "boot64-uefi-test passed"

# The other numbers of CPUs. The two boots above have 4, and do
# everything; these only boot, with one CPU (nothing to start), with
# two, and with more than the kernel uses (the rest are left asleep).
SMP64_LOG = $(BUILD_DIR)/smp64
smp64-test: iso64
	@for cpus in 1 2 20; do \
	    echo "Booting the 64-bit kernel with $$cpus CPU(s) (serial log: $(SMP64_LOG)-$$cpus.log)..."; \
	    SMP=$$cpus $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(SMP64_LOG)-$$cpus.log 40; \
	    used=$$cpus; [ $$cpus -le 16 ] || used=16; \
	    SMP=$$used SCREEN_MAX_INK=700 $(KERNEL64_DIR)/scripts/check-boot.sh $(SMP64_LOG)-$$cpus.log BIOS > $(SMP64_LOG)-$$cpus.out \
	        || { tail -5 $(SMP64_LOG)-$$cpus.out; echo "FAIL: with $$cpus CPU(s) (all of it in $(SMP64_LOG)-$$cpus.out)"; exit 1; }; \
	done
	@grep -q "\[smp\] 4 more CPU(s) than the 16 this kernel uses: left asleep" $(SMP64_LOG)-20.log \
	    || { echo "FAIL: 20 CPUs: the kernel did not say it left 4 asleep"; exit 1; }
	@echo "smp64-test passed"

# A kernel made too big on purpose: 768 KiB of ballast put its end well
# above 2 MiB, where the BIOS loader used to refuse it. Booted both
# ways, it checks every byte of the ballast. The BIOS image also carries
# a ramdisk, which for a 64-bit kernel must now land above the kernel -
# at 3 MiB, where the kernel's file was staged - and not at 2 MiB,
# through the middle of it.
BIG64_DIR = $(BUILD_DIR)/big64
BIG64_ISO = $(BUILD_DIR)/big64.iso
BIG64_ELF = $(KERNEL64_DIR)/target/big-image/x86_64-unknown-none/release/kernel64
BIG64_LOG = $(BUILD_DIR)/big64
BIG64_SAID = \[test\] big image: the kernel ends at 0x[0-9a-f]+, above 2 MiB; 768 KiB of it at 0x[0-9a-f]+-0x[0-9a-f]+ is byte for byte what was built
big64-test: build-bootloader user64 uefi-loader
	@echo "Building a kernel that ends above 2 MiB..."
	cd $(KERNEL64_DIR) && cargo build --release --quiet --features big-image --target-dir target/big-image
	rm -rf $(BIG64_DIR)
	mkdir -p $(BIG64_DIR)/kernel
	cp $(STAGE1_BIN) $(STAGE2_BIN) $(BIG64_DIR)/
	strip --strip-debug -o $(BIG64_DIR)/kernel/kernel.elf $(BIG64_ELF)
	head -c 8192 /dev/urandom > $(BIG64_DIR)/RAMDISK.MDR
ifeq ($(HAVE_CLANG),yes)
	$(MKFS_FAT) -C -n TAAJESP $(BIG64_DIR)/efiboot.img 8192 >/dev/null
	mmd -i $(BIG64_DIR)/efiboot.img ::/EFI ::/EFI/BOOT ::/kernel
	mcopy -i $(BIG64_DIR)/efiboot.img $(UEFI_EFI) ::/EFI/BOOT/BOOTX64.EFI
	mcopy -i $(BIG64_DIR)/efiboot.img $(BIG64_DIR)/kernel/kernel.elf ::/kernel/kernel.elf
	xorriso -as mkisofs -R -J -iso-level 3 \
		-b stage1.bin -no-emul-boot -boot-load-size 4 \
		-eltorito-alt-boot -e efiboot.img -no-emul-boot \
		-o $(BIG64_ISO) $(BIG64_DIR) 2>/dev/null
else
	xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot \
		-boot-load-size 4 -o $(BIG64_ISO) $(BIG64_DIR) 2>/dev/null
endif
	@echo "Booting it via BIOS (serial log: $(BIG64_LOG)-bios.log)..."
	@$(KERNEL64_DIR)/scripts/qemu-boot.sh $(BIG64_ISO) $(BIG64_LOG)-bios.log 40
	@grep -qE "$(BIG64_SAID)" $(BIG64_LOG)-bios.log \
	    || { tail -5 $(BIG64_LOG)-bios.log; echo "FAIL: BIOS: the big kernel did not boot, or was not loaded whole"; exit 1; }
	@grep -q "ramdisk        0x2000 bytes at 0x300000" $(BIG64_LOG)-bios.log \
	    || { grep "ramdisk" $(BIG64_LOG)-bios.log; echo "FAIL: BIOS: the ramdisk is not at 3 MiB, above the kernel"; exit 1; }
	@! grep -q "FAILED\|PANIC\|EXCEPTION" $(BIG64_LOG)-bios.log || { echo "FAIL: BIOS: the big kernel reported a failure"; exit 1; }
ifeq ($(HAVE_CLANG),yes)
	@echo "Booting it via UEFI (serial log: $(BIG64_LOG)-uefi.log)..."
	@QEMU_ARGS="-bios $(OVMF)" $(KERNEL64_DIR)/scripts/qemu-boot.sh $(BIG64_ISO) $(BIG64_LOG)-uefi.log 40
	@grep -qE "$(BIG64_SAID)" $(BIG64_LOG)-uefi.log \
	    || { tail -5 $(BIG64_LOG)-uefi.log; echo "FAIL: UEFI: the big kernel did not boot, or was not loaded whole"; exit 1; }
	@! grep -q "FAILED\|PANIC\|EXCEPTION" $(BIG64_LOG)-uefi.log || { echo "FAIL: UEFI: the big kernel reported a failure"; exit 1; }
endif
	@echo "big64-test passed"

# Unit tests of the kernel's pure logic (frames, page tables, heap,
# locks), compiled for and run on this machine rather than in a VM.
HOST_TRIPLE = $(shell PATH="$(PATH)" rustc -vV | sed -n 's/^host: //p')

kernel64-test:
	cd $(KERNEL64_DIR) && cargo test --lib --target $(HOST_TRIPLE)
	cd kernel/abi && cargo test
	cd input && cargo test
	cd console && cargo test
	cd heap && cargo test
	cd fileserver && cargo test
	cd registry && cargo test
	cd taajfs && cargo test
	cd virtio && cargo test
	$(MAKE) libc-test

# The C library's string functions, compiled for this machine and
# tested here. They are one file that asks the system for nothing, so
# it is given to rustc on its own: libc as a whole is built only for
# TheTaaJ.
#
# And its printf: format.c compiled for this machine with its functions
# renamed, and compared format by format with this machine's own
# snprintf - with the sanitizers watching for a byte written outside a
# buffer or arithmetic that C does not define.
#
# And its streams: stdio.c compiled for this machine as it is, and then
# every name in the object file changed by objcopy (libc/c/host_names:
# fopen to taaj_fopen, read to taaj_read), so that it sits in one
# program with this machine's stdio and its system calls are the
# test's own, which count and can be made to fail.
#
# And ctype.c and stdlib.c the same way: every answer compared with
# this machine's own.
HOST_CFLAGS    = -std=c11 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=undefined
FORMAT_RENAMED = -Dvsnprintf=taaj_vsnprintf -Dsnprintf=taaj_snprintf -Dvsprintf=taaj_vsprintf -Dsprintf=taaj_sprintf
libc-test:
	@mkdir -p $(LIBC_DIR)/target
	rustc --edition 2021 --test $(LIBC_DIR)/src/string.rs -o $(LIBC_DIR)/target/string-test
	$(LIBC_DIR)/target/string-test
	rustc --edition 2021 --test $(LIBC_DIR)/src/errno.rs -o $(LIBC_DIR)/target/errno-test
	$(LIBC_DIR)/target/errno-test
ifeq ($(HAVE_CC),yes)
	@mkdir -p $(LIBC_DIR)/target/host
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include $(FORMAT_RENAMED) \
	    -c $(LIBC_DIR)/c/format.c -o $(LIBC_DIR)/target/host/format.o
	clang $(HOST_CFLAGS) -Wno-format -c $(LIBC_DIR)/c/format_test.c -o $(LIBC_DIR)/target/host/format_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/format.o $(LIBC_DIR)/target/host/format_test.o \
	    -o $(LIBC_DIR)/target/host/format-test
	$(LIBC_DIR)/target/host/format-test
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	    -c $(LIBC_DIR)/c/format.c -o $(LIBC_DIR)/target/host/stdio_format.o
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	    -c $(LIBC_DIR)/c/stdio.c -o $(LIBC_DIR)/target/host/stdio.o
	objcopy --redefine-syms=$(LIBC_DIR)/c/host_names $(LIBC_DIR)/target/host/stdio_format.o
	objcopy --redefine-syms=$(LIBC_DIR)/c/host_names $(LIBC_DIR)/target/host/stdio.o
	clang $(HOST_CFLAGS) -c $(LIBC_DIR)/c/stdio_test.c -o $(LIBC_DIR)/target/host/stdio_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/stdio.o $(LIBC_DIR)/target/host/stdio_format.o \
	    $(LIBC_DIR)/target/host/stdio_test.o -o $(LIBC_DIR)/target/host/stdio-test
	$(LIBC_DIR)/target/host/stdio-test
	@for part in ctype stdlib; do \
	    clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	        -c $(LIBC_DIR)/c/$$part.c -o $(LIBC_DIR)/target/host/$$part.o || exit 1; \
	    objcopy --redefine-syms=$(LIBC_DIR)/c/host_names $(LIBC_DIR)/target/host/$$part.o || exit 1; \
	done
	clang $(HOST_CFLAGS) -c $(LIBC_DIR)/c/stdlib_test.c -o $(LIBC_DIR)/target/host/stdlib_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/ctype.o $(LIBC_DIR)/target/host/stdlib.o \
	    $(LIBC_DIR)/target/host/stdlib_test.o -o $(LIBC_DIR)/target/host/stdlib-test
	$(LIBC_DIR)/target/host/stdlib-test
else
	@echo "Clang not found: the printf comparison and the stream tests are not run (sudo apt install clang lld)"
endif

# One boot per deliberate fault: unmapped read, double fault, write to
# rodata, executing data, stack overflow. Each must be caught and named.
fault64-test: build-bootloader user64
	$(KERNEL64_DIR)/scripts/fault-test.sh $(STAGE1_BIN) $(STAGE2_BIN) $(BUILD_DIR)

# ---- AArch64 (Era 3) -------------------------------------------------
#
# The same kernel crate, built for a second architecture. There is no
# loader of TheTaaJ's own for it yet: QEMU's "virt" machine is given a
# flat image with the header AArch64 loaders look for, and loads it as a
# board's firmware would. The image is made from the ELF file by
# llvm-objcopy (the PC's objcopy does not know AArch64 files).
KERNEL_A64_TARGET = aarch64-unknown-none-softfloat
KERNEL_A64_ELF    = $(KERNEL64_DIR)/target/$(KERNEL_A64_TARGET)/release/kernel64
KERNEL_A64_IMAGE  = $(BUILD_DIR)/aarch64/Image
# llvm-objcopy, or the newest llvm-objcopy-NN: some systems install it
# only under a name with its version.
LLVM_OBJCOPY := $(shell command -v llvm-objcopy || ls /usr/bin/llvm-objcopy-* 2>/dev/null | sort -V | tail -1)
HAVE_A64     := $(shell command -v qemu-system-aarch64 >/dev/null && [ -n "$(LLVM_OBJCOPY)" ] && echo yes)

# The programs the AArch64 kernel embeds: the user/ crate again, for
# the other CPU. Named one by one, because a few programs are for the PC
# only (they use I/O ports) and would not compile.
USER_A64_BINS := hello peek cli spin echod echoc fs fscheck memro heapcheck forever regs

user-aarch64:
	cd user && cargo build --release --target $(KERNEL_A64_TARGET) $(addprefix --bin ,$(USER_A64_BINS))

# The C library and four C programs, for AArch64: the same sources and
# the same two halves as libc64 and cprogs64, with the other CPU's
# compiler flags. Nothing in the library is written for one CPU - the
# only code that is, the system call itself, is in the user library
# underneath it. Until AArch64 has a disk the programs are built into
# its kernel (the c-programs feature) and run by a self-test.
C_A64_BIN      = $(BUILD_DIR)/c-aarch64
LIBC_A64_RUST  = $(LIBC_DIR)/target/$(KERNEL_A64_TARGET)/release/libc.a
C_A64_PROGRAMS = chello cmem cprint cprimes
# As TAAJ_CFLAGS. There is no red zone to switch off on this CPU, and
# "general registers only" here means no floating point or vector
# registers - which the kernel leaves switched off for programs.
TAAJ_A64_CFLAGS  = --target=aarch64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding \
                   -fno-stack-protector -fno-pic -mgeneral-regs-only \
                   -nostdlibinc -isystem $(LIBC_DIR)/include
# The linker would otherwise line segments up for 64 KiB pages, which
# some AArch64 systems use: 60 KiB of padding in every file.
TAAJ_A64_LDFLAGS = $(TAAJ_LDFLAGS) -z max-page-size=4096
ifeq ($(HAVE_CC),yes)
KERNEL_A64_FEATURES = c-programs
else
KERNEL_A64_FEATURES =
endif

cprogs-aarch64:
ifeq ($(HAVE_CC),yes)
	cd $(LIBC_DIR) && cargo build --release --target $(KERNEL_A64_TARGET)
	@mkdir -p $(C_A64_BIN)/lib
	@for part in $(LIBC_C); do \
	    clang $(TAAJ_A64_CFLAGS) -c $(LIBC_DIR)/c/$$part.c -o $(C_A64_BIN)/lib/$$part.o || exit 1; \
	done
	@cp $(LIBC_A64_RUST) $(C_A64_BIN)/libc.a
	@ar rs $(C_A64_BIN)/libc.a $(addprefix $(C_A64_BIN)/lib/,$(addsuffix .o,$(LIBC_C)))
	@for program in $(C_A64_PROGRAMS); do \
	    echo "  CC      $$program.c (AArch64)"; \
	    clang $(TAAJ_A64_CFLAGS) -c $(LIBC_DIR)/programs/$$program.c -o $(C_A64_BIN)/$$program.o || exit 1; \
	    ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/$$program.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/$$program || exit 1; \
	done
else
	@echo "Clang or lld not found: no C programs for AArch64 (sudo apt install clang lld)"
endif

kernel-aarch64: user-aarch64 cprogs-aarch64
	cd $(KERNEL64_DIR) && cargo build --release --target $(KERNEL_A64_TARGET) $(addprefix --features ,$(KERNEL_A64_FEATURES))
	@mkdir -p $(BUILD_DIR)/aarch64
	$(LLVM_OBJCOPY) -O binary $(KERNEL_A64_ELF) $(KERNEL_A64_IMAGE)

# The serial port is this terminal. Ctrl+A, then X, leaves QEMU.
# RUN_A64 adds QEMU options: make run-aarch64 RUN_A64="-M virt,virtualization=on"
run-aarch64: kernel-aarch64
	@echo "(to leave QEMU: Ctrl+A, then X)"
	qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel $(KERNEL_A64_IMAGE) $(RUN_A64)

# Boots on several machines, and one deliberate fault.
aarch64-test: user-aarch64 cprogs-aarch64
ifeq ($(HAVE_A64),yes)
	$(KERNEL64_DIR)/scripts/aarch64-test.sh $(BUILD_DIR) $(LLVM_OBJCOPY) $(KERNEL_A64_FEATURES)
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run (sudo apt install qemu-system-arm llvm)"
endif

# Every layer: host unit tests, a normal boot (BIOS and UEFI) with its
# self-tests, the fault boots, and the AArch64 kernel.
test64: kernel64-test boot64-test boot64-uefi-test smp64-test big64-test fault64-test aarch64-test
	@echo "test64: all passed"

# Clean everything
clean:
	@echo "Cleaning bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR) clean
	@echo "Cleaning kernel..."
	$(MAKE) -C $(KERNEL_DIR) clean
	@echo "Cleaning build directory..."
	rm -rf $(BUILD_DIR)
	rm -rf $(ISO_DIR)