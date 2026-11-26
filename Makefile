# Define subdirectories
BOOTLOADER_DIR = bootloader
KERNEL_DIR = kernel

.PHONY: all clean run iso ramdisk modules servers librt disk tfstool tfsdisk tfstool-rs tfscheck \
        kernel64 iso64 run64 run-disk boot64-test smp64-test big64-test kernel64-test fault64-test test64 \
        uefi-loader run64-uefi boot64-uefi-test aarch64-esp run-aarch64-uefi aarch64-uefi-test user64 libc64 cprogs64 cput libc-test \
        tcc64 tcc-aarch64 tcc64-test aarch64-tcc-test cxx64 cxx-aarch64 cxx64-test aarch64-cxx-test \
        pdpmake64 pdpmake-aarch64 make64-test aarch64-make-test libmake64-test aarch64-libmake-test pipe64-test aarch64-pipe-test thread64-test aarch64-thread-test bigmem64-test aarch64-bigmem-test basics64-test aarch64-basics-test ground64-test aarch64-ground-test ground2-64-test aarch64-ground2-test handles64-test aarch64-handles-test times64-test aarch64-times-test pages64-test aarch64-pages-test zlib64 zlib-aarch64 \
        llvm64 llvm-aarch64 clang-tblgen llvm64-test clang64-test aarch64-clang-test aarch64-llvm-test llvm-put llvm-put-aarch64

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
# The master boot record of an image written to a USB stick.
MBR_BIN    = $(STAGE1_BUILD_DIR)/mbr.bin
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
# tests, typed one after another, then four of cfloat, then `time wait`.
# Each copy must still be running when `time wait` is typed, or the
# shell's wait sees only their last moments and "four programs at once"
# cannot be shown. Emulated (TCG), 24000000 is long enough. Under KVM a
# copy runs about as fast as on the host and ends while the rest is still
# being typed, so the x86_64 boot tests ask for more there (measured on
# MaK, an i7-13620H). AArch64 is emulated everywhere and keeps the first.
CPRIMES_HOST = $(C_BIN)/host-cprimes
# The accelerator every x86_64 QEMU uses, the test scripts' (qemu-boot.sh
# reads it) and make run64's: KVM when /dev/kvm can be opened, else QEMU
# emulates. Worked out once; `make test64 TAAJ_ACCEL=tcg` emulates anyway.
ifndef TAAJ_ACCEL
TAAJ_ACCEL := $(shell [ -r /dev/kvm ] && [ -w /dev/kvm ] && echo kvm || echo tcg)
endif
export TAAJ_ACCEL
CPRIMES_LIMIT_TCG = 24000000
CPRIMES_LIMIT_KVM = 48000000
CPRIMES_LIMIT = $(CPRIMES_LIMIT_TCG)
CPRIMES_LIMIT_X86 = $(if $(filter kvm,$(TAAJ_ACCEL)),$(CPRIMES_LIMIT_KVM),$(CPRIMES_LIMIT_TCG))
# A number as keys to type: 24000000 -> 2 4 0 0 0 0 0 0.
spell-keys = $(shell echo $(1) | sed 's/./& /g')
LIBC_A       = $(C_BIN)/libc.a
LIBC_C       = format stdio fcntl ctype stdlib inet float setjmp time abort fp128 math scan locale atexit signal mmap users unix \
               $(addprefix libm/,$(LIBM))
# libc/c/libm: musl's sin, exp, log, pow and the rest (journal entry 074),
# and floats.c, TheTaaJ's float versions of them. Each its own file, as
# in musl: they share names for their constants. Compiled with
# -ffp-contract=off (a*b+c stays two roundings, as musl's error bounds
# assume) and -Wno-shift-op-parentheses (musl writes x>>64-d, and the
# files stay as musl has them).
LIBM         = __math_oflow __math_uflow __math_xflow __math_invalid __math_divzero __rem_pio2_large __rem_pio2 \
               __sin __cos __tan sin cos tan asin acos atan atan2 __expo2 expm1 exp_data exp exp2 sinh cosh tanh \
               log_data log log2_data log2 log10 log1p pow_data pow cbrt asinh acosh atanh fma floats
# The Rust half is built for targets without floating-point registers,
# and its support library's floating-point helpers (__multf3 and the
# like, for AArch64's 128-bit long double) and the few maths functions
# it has (sqrtf, ...) take and return their numbers the way such a
# target does: in general registers. C here passes them in FP and vector
# registers. So in libc.a those are made local - invisible to the linker
# - before the C half joins it: a C program then links with the C
# library's own (libc/c/fp128.c, float.c), or not at all, never with
# one that would read its arguments from the wrong registers.
# The Rust objects also carry their LLVM bitcode (.llvmbc, .llvmcmd), for
# link-time optimisation by a linker that can do it; no linker here
# does, and it is most of their size, so it goes too.
LIBC_RUST_PRIVATE = --remove-section=.llvmbc --remove-section=.llvmcmd \
                    --regex --localize-symbol='^__[a-z]*tf[a-z]*[0-9]?$$' \
                    --localize-symbol='^(sqrt|rint|cbrt|hypot|fmin|fmax|fma|floor|ceil|trunc|round|exp|exp2|log|log2|log10|pow|sin|cos|tan|atan|atan2|fabs|copysign|fmod|ldexp|frexp|scalbn)[fl]?$$'
# Freestanding: no host headers or libraries, only libc/include and the
# compiler's own (stddef.h, stdarg.h). Floating point in the FPU and
# SSE registers, which the kernel saves for each thread on every switch
# (journal entry 069).
TAAJ_CFLAGS  = --target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding \
               -fno-stack-protector -fno-pic -mno-red-zone \
               -nostdlibinc -isystem $(LIBC_DIR)/include
TAAJ_LDFLAGS = -static -nostdlib --gc-sections -s -T user/user.ld
# In EXPECT_C, what the background programs print (cfloat's lines, and
# cprimes' answers, which check-boot.sh counts) is matched anywhere in a
# line, not only at its start: a background program can print while the
# test is still typing, and the typed echo then comes first in the line -
# as on any terminal. Under KVM this happens every time.
ifeq ($(HAVE_CC),yes)
C_PROGRAMS   = chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib sh
ROOT_FILES   = 39
C_KEYS       = c h e l l o ret c h e l l o spc o n e spc shift-apostrophe t w o spc w o r d s shift-apostrophe ret c h e l l o spc f a i l ret c h e l l o spc e x i t ret c h e l l o spc f l o a t ret w i t h spc f i l e s spc c f i l e spc c h e c k ret c f i l e spc c a t spc w e l c o m e ret w i t h spc r e a d o n l y spc c f i l e spc c a t spc n o f i l e spc w e l c o m e ret w i t h spc r e a d o n l y spc c f i l e spc w r i t e spc c n o spc x ret w i t h spc f i l e s spc c f i l e spc w r i t e spc c n o t e spc m a d e spc b y spc c ret w i t h spc r e a d o n l y spc c f i l e spc c a t spc l o n g spc shift-dot spc c l o n g ret c f i l e spc c a t spc shift-comma spc l o n g spc shift-dot spc c l o n g 2 ret c f i l e ret c a s k ret a d a ret 4 0 ret x ret 2 ret ctrl-d c a s k spc o n e ret f i r s t ret e c h o spc a f t e r ret e c h o spc a d a spc shift-dot spc w h o ret c a s k spc o n e spc shift-comma spc w h o ret c a t spc l o n g spc shift-backslash spc c w o r d s spc minus n spc 3 ret w i t h spc r e a d o n l y spc c w o r d s spc minus n spc 2 5 spc c w o r d s spc l o n g spc shift-dot spc c w 1 ret c w o r d s spc minus n spc 5 spc shift-comma spc l o n g spc shift-dot spc c w 2 ret c w o r d s spc l o n g ret c w o r d s spc minus n spc n o n e ret c w o r d s spc minus x ret w i t h spc f i l e s spc c s t d i o spc c h e c k ret c s t d i o spc l i n e s spc w e l c o m e ret c s t d i o spc l i n e s spc shift-comma spc w e l c o m e ret w i t h spc r e a d o n l y spc c s t d i o spc l i n e s spc l o n g spc shift-dot spc c n u m ret w i t h spc f i l e s spc c s t d i o spc c o p y spc l o n g spc c c o p y ret c s t d i o spc o r d e r ret c s t d i o spc e n d spc shift-dot spc c e n d ret c s t d i o spc l o s t spc shift-dot spc c l o s t ret c s t d i o spc e n d ret c s t d i o ret c p r i n t spc o n e spc shift-apostrophe t w o spc w o r d s shift-apostrophe ret c p r i n t spc l o n g spc shift-backslash spc w c ret c p r i n t spc shift-dot spc p o u t ret c m e m ret c m e m spc s o r t spc p e a r spc a p p l e spc f i g spc a p p l e ret c m e m spc h u g e ret c h e l l o spc e r r spc shift-dot spc c e r r ret c h e l l o spc a spc b spc shift-dot spc c o u t ret c h e l l o spc x spc shift-backslash spc w c ret w i t h spc f i l e s spc c l i b ret c l i b spc a s s e r t ret c p r i m e s spc 1 0 0 ret ~ c p r i m e s spc $(call spell-keys,$(CPRIMES_LIMIT)) spc shift-7 ret c p r i m e s spc $(call spell-keys,$(CPRIMES_LIMIT)) spc shift-7 ret c p r i m e s spc $(call spell-keys,$(CPRIMES_LIMIT)) spc shift-7 ret c p r i m e s spc $(call spell-keys,$(CPRIMES_LIMIT)) spc shift-7 ret c f l o a t spc 1 spc shift-7 ret c f l o a t spc 2 spc shift-7 ret c f l o a t spc 3 spc shift-7 ret c f l o a t spc 4 spc shift-7 ret t i m e spc w a i t ret ~~
EXPECT_C     = ;^Hello from C;^argc 1\s*$$;^argc 3\s*$$;^argv\[1\] one\s*$$;^argv\[2\] two words\s*$$;^chello exited with code 3;^chello exited with code 4;^7.0 / 2 \* 4 = 14\s*$$;^chello: this line is on standard error;^files: 48 checks, all passed;^cfile: welcome: this program was not given the files;^cfile: nofile: no such file or directory;^cno: the files are read-only here;^cnote: 10 bytes;^usage: cfile cat;^hello, ada \(typed\);^cask: not a number: x;^total: 42;^one line: first \(typed\);^after\s*$$;^one line: ada \(from a file or another program\);^ +301  300\s*$$;^ +300  line\s*$$;^1200 words, 302 different;^cwords: long: this program was not given the files;^0 words, 0 different;^cwords: -n needs a number from 1 up, not .none.;^usage: cwords \[-n COUNT\];^streams: 52 checks, all passed;^cstdio: welcome: this program was not given the files;^   1  Hello from the host;^ccopy: 4692 bytes;^write was called second - printf was called first\s*$$;^no new line, and still here;^usage: cstdio lines;^printf: 42 -7 ff 0X2A \[   hi\] \[hi   \] \[00042\] x 100.\s*$$;^sizes: 18446744073709551615 -9223372036854775808\s*$$;^printf returned 6\s*$$;^snprintf: wanted 12, kept <hello, >;^argument 2 of 2: two words\s*$$;^puts: a line of its own\s*$$;^fputs and putchar\s*$$;^cprint: 9 lines went to standard output;^cprint: 7 lines went to standard output;^1 line, 1 word, 601 bytes;^strings: 28 checks, all passed;^memory: 15 checks, all passed;^apple apple fig pear\s*$$;^cmem: no memory for a terabyte;^cmem exited with code 1;^3 lines, 7 words, 30 bytes;^library: [0-9]+ checks, all passed;^library: atexit ran its handlers, newest first;^libc/programs/clib.c:[0-9]+: main: assertion failed: number == 2;^clib was ended by signal 6 \(SIGABRT\);^cprimes: 25 primes below 100\s*$$;^\[4\] cprimes $(CPRIMES_LIMIT);cfloat 1: 30000000 rounds, 0 wrong;cfloat 2: 30000000 rounds, 0 wrong;cfloat 3: 30000000 rounds, 0 wrong;cfloat 4: 30000000 rounds, 0 wrong;^took [0-9]+\.[0-9][0-9] s; the 4 CPUs worked for [0-9]+\.[0-9][0-9] s in all
else
C_PROGRAMS   =
ROOT_FILES   = 27
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
               m v spc d o c s spc d o c s ret m v spc d o c s spc r o o t e d ret m v spc n o n e spc x ret \
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
EXPECT_SHELL = unknown command: Hi;  PID  NAME;init +running +[0-9]+\.[0-9]{3} s;^memory: [0-9]+ MiB used;^hello exited with code 42;^peek was killed
EXPECT_FILES = \[fs\] ready: TaajFS on the disk;^notes: 6 bytes;^notes: 12 bytes;^hello\s*$$;^world\s*$$;^ +12  notes;^ +20  welcome;^ +dir  docs/;^ +dir  tmp/;^3 directories, $(ROOT_FILES) files, [0-9]+ bytes;^cat: notes: no such file;^cat exited with code 1;^kept\s*$$;^Hello from the host;^saved: 5 bytes
EXPECT_DISK  = \[blk\] virtio disk.*MSI-X;^disk: 8192 sectors of 512 bytes \(4096 KiB\);^0000  4a 41 46 53 .*JAFS;^disk: sector 5: the disk is read-only;^disk: sector 99999: no such sector;^disk check: all 9 checks passed;^took [0-9]+\.[0-9][0-9] s;^line 300 of 300
EXPECT_RUN   = \[init\] started primes for the shell, from a file;^primes below 50: 2 3 5 7 11 13 17 19 23 29 31 37 41 43 47;^primes exited with code 0;^error: welcome is not a program;^error: broken is not a program;^error: no such program;\[init\] started ls for the shell, from a file;^cat exited with code 0;^rm: notes: no such file;^rm: saved2: no such file;^rm exited with code 1;^usage: write FILE TEXT;^write exited with code 2;$(EXPECT_GIVEN);$(EXPECT_WATCH);$(EXPECT_KILL)
EXPECT_DIRS  = ^ +23  readme;^1 file, 23 bytes;^Read me, from the host;^docs/deep/note: 7 bytes;^docs:;^ +dir  deep/;^1 directory, 1 file, 23 bytes;^docs/deep:;^ +7  note;^rm: docs: the directory is not empty;^mkdir: docs: there is already something with that name;^cat: docs: is a directory;^write: welcome/x: not a directory;^ls: work: no such file or directory;\[init\] started tools/given for the shell, from a file;^mkdir: x: the files are read-only here
EXPECT_CD    = ^/\s*$$;^/docs\s*$$;^here: 7 bytes;^1 directory, 2 files, 30 bytes;^/docs/deep\s*$$;^inside\s*$$;^/docs/here: 12 bytes;\[init\] started \./given for the shell, from a file;^given: started in /tools;^cd: /welcome: not a directory;^cd: nowhere: no such file or directory;^usage: cd \[DIR\];^ls: \.: no such file or directory;^rooted: 4 bytes;^home: 4 bytes
EXPECT_MOVE  = ^yes\s*$$;^ +4  docs/house;^docs/welcome: 20 bytes;^docs/long2: [0-9]+ bytes;^nested\s*$$;^mv: docs to docs/docs: a directory cannot be moved into itself;^mv: docs to rooted: not a directory;^mv: none to x: no such file or directory;^cp: docs: is a directory;^cp: welcome: there is already something with that name;^usage: mv FROM TO;^mv: rooted to x: the files are read-only here
EXPECT_QUOTE = ^quoted: 11 bytes;^plain: 10 bytes;^my docs/a file: 2 bytes;^/my docs\s*$$;^ +2  a file;^given: arguments .a b. .c. ..\s*$$;^ab cd\s*$$;^the line has a . that is never closed;^the line ends in a . with nothing after it
EXPECT_REDIR = ^first\s*$$;^second\s*$$;^6 lines, [0-9]+ words, [0-9]+ bytes;^long: 300 lines, 1200 words, 4692 bytes;^300 lines, 1200 words, 4692 bytes;^a > b\s*$$;^none: no such file or directory;^docs: is a directory;^there is a > with no file name after it;^unknown command: nosuch
EXPECT_GIVEN = ^given: the screen, no files;^given: arguments .a. .b.\s*$$;^given: the screen, and the files \([0-9]+ of them\), to read and to change;^given: the screen, and the files \([0-9]+ of them\), to read only;^usage: with files COMMAND, with readonly COMMAND;^write: saved: the files are read-only here;^rm: saved: the files are read-only here;\[fs\] asked to stall;^fs: the file service will answer nothing for 6 s;\[init\] fs has not answered for 4 s - stopping it;\[process\] [0-9]+ .fs. stopped: killed by request;\[init\] fs ended with code -1001 - restarting it
EXPECT_WATCH = \[blk\] asked to stall;\[init\] blk has not answered for 2 s - stopping it;\[init\] blk ended with code -1001 - restarting it;^during: 2 bytes;\[display\] asked to stall;\[init\] display has not answered for 2 s - stopping it;\[init\] display ended with code -1001 - restarting it;\[display\] the text on the screen was kept: [0-9]+ lines shown again;\[kbd\] F11: simulating a driver bug;\[init\] kbd has not answered for 2 s - stopping it;\[init\] kbd ended with code -1001 - restarting it;\[shell\] line: .echo keys.
EXPECT_KILL  = ^forever: spin until stopped;^forever: wait until stopped;^forever: sleep until stopped;\[process\] [0-9]+ .forever. ended by signal 2 \(SIGINT\);^\^C;^forever was stopped
EXPECT_PIPE  = ^1 line, 3 words, 14 bytes;^301 lines, 1212 words;^given: arguments .xpipe.;^p . q\s*$$;^there is a . with no command on one side of it;^cd is one of the shell.s own commands;^unknown command: nosuch2;^only the last command of a pipeline
EXPECT_JOBS  = ^\[1\] cat ls ls . cat > big4;^\[2\] forever sleep > flog;^\[3\] forever yield;^forever: yield until stopped;^\[2\] running  forever sleep > flog;^\[3\] running  forever yield;^\[3\] stopped  forever yield;^\[2\] stopped  forever sleep > flog;^kill: no job 9;^\[1\] done  cat ls ls . cat > big4;^no jobs;^echo is one of the shell.s own commands: it cannot run in the background;^a & can only end the line;^x & y\s*$$
EXPECT_NAMES = \[registry\] ready;\[registry\] fs may ask for: disk\s*$$;\[registry\] shell may ask for: keys display init system files files.look tmp tmp.look disk.look;^keys display init system files files.look tmp tmp.look disk.look nic net windows\s*$$;^files: there for the shell;^disk: no such service - for the shell;\[registry\] shell asked for .disk.: it is not in its namespace;^nosuch: no such service - for the shell;\[registry\] asked to crash;\[init\] registry ended with code 1 - restarting it;^files.look: there for the shell
EXPECT_GIVE  = \[registry\] shell gave ls: files=files.look tmp=tmp.look;\[registry\] shell gave write: files tmp\s*$$;\[registry\] shell gave given: files tmp\s*$$;\[registry\] shell gave given: files=files.look tmp=tmp.look;^given: it may ask for: files tmp\s*$$
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
DISK_PROGRAMS = ls cat write append rm mkdir mv cp wc yes primes forever given niccheck ping netinfo udpecho udpsend tcpecho tcpsend httpd counter edit
# The web site httpd gives out (user/www), put in the directory www of a
# disk: put-site DISK. Only the run disks and the HTTP test's disk have
# it - the other test disks are counted, directory by directory.
SITE_DIR     = user/www
SITE_FILES   = index.html style.css about/index.html
define put-site
	@$(TFSTOOL_RS) mkdir $(1) www > /dev/null 2>&1; $(TFSTOOL_RS) mkdir $(1) www/about > /dev/null 2>&1; true
	@for file in $(SITE_FILES); do \
	    $(TFSTOOL_RS) rm $(1) www/$$file > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(1) www/$$file $(SITE_DIR)/$$file > /dev/null || exit 1; \
	done
endef
# make-test-disk-from DISK USER_BIN C_BIN: the programs are those
# compiled for one CPU or the other; everything else is the same.
define make-test-disk-from
	@rm -f $(1) && $(TFSTOOL_RS) mkfs $(1) 1024 > /dev/null && truncate -s 4M $(1)
	@printf 'Hello from the host\n' > $(1).welcome && $(TFSTOOL_RS) put $(1) welcome $(1).welcome > /dev/null
	@for program in $(DISK_PROGRAMS); do $(TFSTOOL_RS) put $(1) $$program $(2)/$$program > /dev/null || exit 1; done
	@for program in $(C_PROGRAMS); do $(TFSTOOL_RS) put $(1) $$program $(3)/$$program > /dev/null || exit 1; done
	@head -c 300 $(2)/primes > $(1).broken && $(TFSTOOL_RS) put $(1) broken $(1).broken > /dev/null
	@seq 300 | sed 's/.*/line & of 300/' > $(1).long && $(TFSTOOL_RS) put $(1) long $(1).long > /dev/null
	@$(TFSTOOL_RS) mkdir $(1) docs > /dev/null && printf 'Read me, from the host\n' > $(1).readme \
	    && $(TFSTOOL_RS) put $(1) docs/readme $(1).readme > /dev/null
	@$(TFSTOOL_RS) mkdir $(1) tools > /dev/null && $(TFSTOOL_RS) put $(1) tools/given $(2)/given > /dev/null
endef
define make-test-disk
	$(call make-test-disk-from,$(1),$(USER_BIN),$(C_BIN))
endef

# The disk `make run64` attaches: a 16 MiB TaajFS volume that is kept
# from one run to the next, so files written in the OS are still there.
# Made on first use; the programs on it are replaced with the ones just
# built every time.
RUN_DISK     = $(BUILD_DIR)/taaj-disk.img
# How many 4 KiB blocks a new run disk has: 4096 is 16 MiB. The LLVM
# disk (llvm-put) has more.
RUN_DISK_BLOCKS = 4096
# How the disk is attached: a virtio disk (blk drives it), `make run64
# RUN_DISK_BUS=sata` - a SATA disk on an AHCI controller (ahci drives
# it) - RUN_DISK_BUS=nvme, an NVMe one (nvme drives it), or
# RUN_DISK_BUS=usb, a USB stick on an xHCI controller (usb drives it).
RUN_DISK_BUS ?= virtio
ifeq ($(RUN_DISK_BUS),sata)
RUN_DISK_ARGS = -drive file=$(RUN_DISK),format=raw,if=none,id=disk0 -device ahci,id=sata -device ide-hd,drive=disk0,bus=sata.0
else ifeq ($(RUN_DISK_BUS),nvme)
RUN_DISK_ARGS = -drive file=$(RUN_DISK),format=raw,if=none,id=disk0 -device nvme,serial=taaj0001,drive=disk0
else ifeq ($(RUN_DISK_BUS),usb)
RUN_DISK_ARGS = -device qemu-xhci,id=xhci -drive file=$(RUN_DISK),format=raw,if=none,id=disk0 -device usb-storage,bus=xhci.0,drive=disk0
else
RUN_DISK_ARGS = -drive file=$(RUN_DISK),format=raw,if=none,id=disk0 -device virtio-blk-pci,drive=disk0
endif
# And the network card: on QEMU's own little network ("user"), which
# needs nothing from the host. `make run64 RUN_NET=` for a machine
# without a card; `make run64 RUN_NIC=e1000` (or e1000e) for an Intel
# card in place of the virtio one (e1000 drives it).
RUN_NIC      ?= virtio-net-pci
RUN_NET      = -netdev user,id=n0 -device $(RUN_NIC),netdev=n0
# And USB: `make run64 RUN_USB=yes` adds an xHCI controller with a
# keyboard and a mouse on it (usb drives them; type in QEMU's window,
# and click in it to give it the mouse). Or RUN_USB_ARGS for other
# devices: `RUN_USB_ARGS="-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0"`.
RUN_USB      ?= no
# (With RUN_DISK_BUS=usb the controller is there already.)
ifeq ($(RUN_USB),yes)
RUN_USB_ARGS ?= $(if $(filter usb,$(RUN_DISK_BUS)),,-device qemu-xhci,id=xhci) -device usb-kbd,bus=xhci.0 -device usb-mouse,bus=xhci.0
endif
run-disk: user64 cprogs64 pdpmake64 tcc64 cxx64 zlib64 tfstool-rs
	@[ -f $(RUN_DISK) ] || { $(TFSTOOL_RS) mkfs $(RUN_DISK) $(RUN_DISK_BLOCKS) > /dev/null && truncate -s $$(($(RUN_DISK_BLOCKS) * 4096)) $(RUN_DISK) \
	    && echo "Made a new disk: $(RUN_DISK)"; }
	@for program in $(DISK_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_DISK) $$program $(USER_BIN)/$$program > /dev/null || exit 1; \
	done
	@for program in $(C_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_DISK) $$program $(C_BIN)/$$program > /dev/null || exit 1; \
	done
	$(call put-site,$(RUN_DISK))
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) tools/put-tcc.sh $(RUN_DISK) $(TCC_X86_DIR) $(LIBC_A)
	@TFSTOOL=$(TFSTOOL_RS) tools/put-sources.sh $(RUN_DISK) x86_64 $(TCC_X86_DIR)
	@for program in $(CXX_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_DISK) $$program $(CXX_X86_DIR)/bin/$$program > /dev/null || exit 1; \
	done
	@$(TFSTOOL_RS) rm $(RUN_DISK) make > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) make $(C_BIN)/make > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(THREADS_C) -o $(C_BIN)/threads.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/threads.o $(LIBC_A) -o $(C_BIN)/threads
	@$(TFSTOOL_RS) rm $(RUN_DISK) threads > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) threads $(C_BIN)/threads > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(BIGMEM_C) -o $(C_BIN)/bigmem.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/bigmem.o $(LIBC_A) -o $(C_BIN)/bigmem
	@$(TFSTOOL_RS) rm $(RUN_DISK) bigmem > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) bigmem $(C_BIN)/bigmem > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(BASICS_C) -o $(C_BIN)/basics.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/basics.o $(LIBC_A) -o $(C_BIN)/basics
	@$(TFSTOOL_RS) rm $(RUN_DISK) basics > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) basics $(C_BIN)/basics > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(GROUND_C) -o $(C_BIN)/ground.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/ground.o $(LIBC_A) -o $(C_BIN)/ground
	@$(TFSTOOL_RS) rm $(RUN_DISK) ground > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) ground $(C_BIN)/ground > /dev/null
	@clang $(TAAJ_CFLAGS) $(ZLIB_CFLAGS) -c $(GROUND2_C) -o $(C_BIN)/ground2.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/ground2.o $(C_BIN)/libz.a $(LIBC_A) -o $(C_BIN)/ground2
	@$(TFSTOOL_RS) rm $(RUN_DISK) ground2 > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) ground2 $(C_BIN)/ground2 > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(HANDLES_C) -o $(C_BIN)/handles.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/handles.o $(LIBC_A) -o $(C_BIN)/handles
	@$(TFSTOOL_RS) rm $(RUN_DISK) handles > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) handles $(C_BIN)/handles > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(NAMESPACE_C) -o $(C_BIN)/namespace.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/namespace.o $(LIBC_A) -o $(C_BIN)/namespace
	@$(TFSTOOL_RS) rm $(RUN_DISK) namespace > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) namespace $(C_BIN)/namespace > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(AT_C) -o $(C_BIN)/at.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/at.o $(LIBC_A) -o $(C_BIN)/at
	@$(TFSTOOL_RS) rm $(RUN_DISK) at > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) at $(C_BIN)/at > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(TMP_C) -o $(C_BIN)/tmptest.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/tmptest.o $(LIBC_A) -o $(C_BIN)/tmptest
	@$(TFSTOOL_RS) rm $(RUN_DISK) tmptest > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) tmptest $(C_BIN)/tmptest > /dev/null
	@clang $(TAAJ_CFLAGS) -c $(PAGES_C) -o $(C_BIN)/pages.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/pages.o $(LIBC_A) -o $(C_BIN)/pages
	@$(TFSTOOL_RS) rm $(RUN_DISK) pages > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) pages $(C_BIN)/pages > /dev/null
endif
	@$(TFSTOOL_RS) rm $(RUN_DISK) threadcheck > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_DISK) threadcheck $(USER_BIN)/threadcheck > /dev/null

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
	@mkdir -p $(C_BIN)/lib/libm
	@for part in $(LIBC_C); do \
	    echo "  CC      libc/c/$$part.c"; \
	    clang $(TAAJ_CFLAGS) -ffp-contract=off -Wno-shift-op-parentheses -c $(LIBC_DIR)/c/$$part.c -o $(C_BIN)/lib/$$part.o || exit 1; \
	done
	@cp $(LIBC_RUST_A) $(LIBC_A)
	@$(LLVM_OBJCOPY) --strip-debug $(LIBC_RUST_PRIVATE) $(LIBC_A)
	@ar rs $(LIBC_A) $(addprefix $(C_BIN)/lib/,$(addsuffix .o,$(LIBC_C)))
	@python3 -I tools/thin-archive.py $(LLVM_NM) $(LLVM_AR) $(LIBC_A)
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

# make: pdpmake, a public-domain POSIX make (ports/pdpmake, journal
# entry 078), built as the C programs are, as build/c/make. On TheTaaJ:
#   with programs make        (it starts each command: posix_spawn)
# Two of its comparisons are of int and size_t, upstream: not warned.
PDPMAKE_PARTS = check input macro main make modtime rules target utils
pdpmake64: cprogs64
ifeq ($(HAVE_CC),yes)
	@mkdir -p $(C_BIN)/pdpmake
	@for part in $(PDPMAKE_PARTS); do \
	    echo "  CC      ports/pdpmake/$$part.c"; \
	    clang $(TAAJ_CFLAGS) -Wno-sign-compare -c ports/pdpmake/$$part.c -o $(C_BIN)/pdpmake/$$part.o || exit 1; \
	done
	@ld.lld $(TAAJ_LDFLAGS) $(addprefix $(C_BIN)/pdpmake/,$(addsuffix .o,$(PDPMAKE_PARTS))) $(LIBC_A) -o $(C_BIN)/make
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
#
# The ISO is also a disk image (a "hybrid"): written to a USB stick, it
# boots from it. Its first sector is a master boot record (mbr.bin, the
# bootloader's), whose partition table xorriso fills in: partition 1 the
# ISO 9660 file system (from 32 KiB on: -partition_offset 16), marked
# active for the BIOS; partition 2 the same FAT image the UEFI El Torito
# entry has, typed 0xEF - the EFI system partition UEFI firmware looks
# for on a disk. hybrid64 OUT EXTRA: EXTRA, more xorriso options (the
# stick's TaajFS partition).
define hybrid64
	xorriso -as mkisofs -R -J -iso-level 3 \
		-isohybrid-mbr $(MBR_BIN) -partition_offset 16 \
		-b stage1.bin -no-emul-boot -boot-load-size 4 \
		-eltorito-alt-boot -e efiboot.img -no-emul-boot \
		-append_partition 2 0xef $(ESP64_IMG) $(2) \
		-o $(1) $(ISO64_DIR) 2>/dev/null
endef

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
	$(call hybrid64,$(ISO64_IMG),)
else
	xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot \
		-isohybrid-mbr $(MBR_BIN) -partition_offset 16 \
		-boot-load-size 4 -o $(ISO64_IMG) $(ISO64_DIR) 2>/dev/null
endif

# A USB stick to boot from and keep files on: the hybrid ISO, and after
# it a third partition (type 0x7F, "for experimental operating
# systems") holding a TaajFS volume - fs finds it there. STICK_DISK is
# that volume, made and kept up to date as the run disk is:
#   make stick64
#   sudo dd if=build/stick64.img of=/dev/sdX bs=4M conv=fsync
# (sdX: the stick - everything on it is lost.) Or in QEMU, booted from
# a USB stick with no CD: make run64-stick (BIOS), run64-stick-uefi.
STICK64_IMG = $(BUILD_DIR)/stick64.img
STICK_DISK ?= $(BUILD_DIR)/stick-disk.img
stick64: iso64
ifneq ($(HAVE_CLANG),yes)
	@echo "ERROR: no UEFI loader - it needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	@$(MAKE) --no-print-directory run-disk RUN_DISK=$(STICK_DISK)
	$(call hybrid64,$(STICK64_IMG),-append_partition 3 0x7f $(STICK_DISK))
	@echo "Made $(STICK64_IMG): write it to a USB stick (see the Makefile) or boot it with make run64-stick"

# How many CPUs `make run64` gives the machine: `make run64 RUN_CPUS=1`.
RUN_CPUS ?= 4

# How much memory `make run64` and `make run-aarch64` give it: `make
# run64 RUN_MEMORY=4G` for programs that want a lot (bigmem big).
RUN_MEMORY ?= 512M

# KVM when /dev/kvm can be opened, else QEMU emulates: `make run64
# RUN_ACCEL=tcg` to emulate anyway. TAAJ_ACCEL (worked out at the top,
# and read by the test scripts) sets it too.
RUN_ACCEL ?= $(TAAJ_ACCEL)

run64: iso64 run-disk
	qemu-system-x86_64 -accel $(RUN_ACCEL) -m $(RUN_MEMORY) -smp $(RUN_CPUS) -cdrom $(ISO64_IMG) -boot d -serial stdio $(RUN_DISK_ARGS) $(RUN_NET) $(RUN_USB_ARGS)

run64-uefi: iso64 run-disk
ifneq ($(HAVE_CLANG),yes)
	@echo "ERROR: no UEFI loader in the ISO - it needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	qemu-system-x86_64 -accel $(RUN_ACCEL) -m $(RUN_MEMORY) -smp $(RUN_CPUS) -bios $(OVMF) -cdrom $(ISO64_IMG) -boot d -serial stdio $(RUN_DISK_ARGS) $(RUN_NET) $(RUN_USB_ARGS)

# Booted from the USB stick (make stick64), with no CD and no other
# disk: a USB keyboard and mouse beside it. The files written land in
# build/stick64.img.
RUN_STICK_ARGS = -device qemu-xhci,id=xhci -drive file=$(STICK64_IMG),format=raw,if=none,id=stick \
	-device usb-storage,bus=xhci.0,drive=stick,bootindex=0 -device usb-kbd,bus=xhci.0 -device usb-mouse,bus=xhci.0
run64-stick: stick64
	qemu-system-x86_64 -accel $(RUN_ACCEL) -m $(RUN_MEMORY) -smp $(RUN_CPUS) -serial stdio $(RUN_STICK_ARGS) $(RUN_NET)

run64-stick-uefi: stick64
	qemu-system-x86_64 -accel $(RUN_ACCEL) -m $(RUN_MEMORY) -smp $(RUN_CPUS) -bios $(OVMF) -serial stdio $(RUN_STICK_ARGS) $(RUN_NET)

# Boots the 64-bit kernel headless and checks what it printed. A start on
# the "boot every architecture on every commit" rule from the roadmap.
# The x86_64 boot tests type the cprimes limit for this machine's
# accelerator (CPRIMES_LIMIT_X86, above); AArch64's keeps CPRIMES_LIMIT_TCG.
boot64-test boot64-uefi-test: CPRIMES_LIMIT = $(CPRIMES_LIMIT_X86)
boot64-test: iso64 cprogs64 tfstool-rs | smp64-test
	@echo "Booting the 64-bit kernel headless via BIOS (serial log: $(SERIAL64_LOG))..."
	$(call make-test-disk,$(SERIAL64_LOG).disk)
	@DISK=$(SERIAL64_LOG).disk KEYS="$(TEST_KEYS)" $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(SERIAL64_LOG)
	@DISK=$(SERIAL64_LOG).disk TFSTOOL=$(TFSTOOL_RS) CWORDS_HOST=$(CWORDS_HOST) CWORDS_TAAJ=$(C_BIN)/cwords CPRIMES_HOST=$(CPRIMES_HOST) CPRIMES_LIMIT=$(CPRIMES_LIMIT) TYPED="$(TEST_TYPED)" EXPECT="$(TEST_EXPECT)" SCREEN_MAX_INK=700 \
	    $(KERNEL64_DIR)/scripts/check-boot.sh $(SERIAL64_LOG) BIOS
	@echo "boot64-test passed"

# The disk on a SATA controller: an AHCI one added to the PC (BIOS), and
# the q35's own (UEFI), both booted at once (scripts/ahci-test.sh).
AHCI64_LOG = $(BUILD_DIR)/ahci64.log
ahci64-test: iso64 cprogs64 tfstool-rs
ifneq ($(HAVE_CLANG),yes)
	@echo "FAIL: the q35 boot needs the UEFI loader, which needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	$(call make-test-disk,$(AHCI64_LOG).disk)
	@TFSTOOL=$(TFSTOOL_RS) OVMF=$(OVMF) $(KERNEL64_DIR)/scripts/ahci-test.sh $(ISO64_IMG) $(AHCI64_LOG).disk $(AHCI64_LOG)
	@echo "ahci64-test passed"

# The disk on an NVMe controller: a PC (BIOS) with one added, and a q35
# (UEFI) with one that moves at most 16 KiB a command, both booted at
# once (scripts/nvme-test.sh).
NVME64_LOG = $(BUILD_DIR)/nvme64.log
nvme64-test: iso64 cprogs64 tfstool-rs
ifneq ($(HAVE_CLANG),yes)
	@echo "FAIL: the q35 boot needs the UEFI loader, which needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	$(call make-test-disk,$(NVME64_LOG).disk)
	@TFSTOOL=$(TFSTOOL_RS) OVMF=$(OVMF) $(KERNEL64_DIR)/scripts/nvme-test.sh $(ISO64_IMG) $(NVME64_LOG).disk $(NVME64_LOG)
	@echo "nvme64-test passed"

# poweroff and reboot: a PC and a q35 (ACPI), and a PC without ACPI
# (scripts/power-test.sh).
POWER64_LOG = $(BUILD_DIR)/power64.log
power64-test: iso64 cprogs64 tfstool-rs
ifneq ($(HAVE_CLANG),yes)
	@echo "FAIL: the q35 boot needs the UEFI loader, which needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	$(call make-test-disk,$(POWER64_LOG).disk)
	@TFSTOOL=$(TFSTOOL_RS) OVMF=$(OVMF) $(KERNEL64_DIR)/scripts/power-test.sh x86 $(ISO64_IMG) $(POWER64_LOG).disk $(POWER64_LOG)
	@echo "power64-test passed"

# The same ISO, the same kernel, booted by UEFI firmware instead.
boot64-uefi-test: iso64 cprogs64 tfstool-rs | smp64-test
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
#
# The 20-CPU boot needs 20 boot slots at once, and while it waits for
# them it holds boot-slots.sh's gate, so no other boot can start. It once
# waited behind boot64-test's typed boot, 7 minutes long, and the whole
# suite stood still for 6 of them, with boot64-uefi-test, as long, queued
# behind (journal entry 108). So the two typed boots wait for smp64-test
# (an order-only prerequisite, about 20 s), and its 20-CPU boot comes first.
SMP64_LOG = $(BUILD_DIR)/smp64
smp64-test: iso64
	@for cpus in 20 2 1; do \
	    echo "Booting the 64-bit kernel with $$cpus CPU(s) (serial log: $(SMP64_LOG)-$$cpus.log)..."; \
	    SMP=$$cpus $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(SMP64_LOG)-$$cpus.log 40; \
	    used=$$cpus; [ $$cpus -le 16 ] || used=16; \
	    SMP=$$used SCREEN_MAX_INK=700 $(KERNEL64_DIR)/scripts/check-boot.sh $(SMP64_LOG)-$$cpus.log BIOS > $(SMP64_LOG)-$$cpus.out \
	        || { tail -5 $(SMP64_LOG)-$$cpus.out; echo "FAIL: with $$cpus CPU(s) (all of it in $(SMP64_LOG)-$$cpus.out)"; exit 1; }; \
	done
	@grep -q "\[smp\] 4 more CPU(s) than the 16 this kernel uses: left asleep" $(SMP64_LOG)-20.log \
	    || { echo "FAIL: 20 CPUs: the kernel did not say it left 4 asleep"; exit 1; }
	@echo "smp64-test passed"

# A kernel made too big on purpose: 512 KiB of ballast put its end well
# above 2 MiB, where the BIOS loader used to refuse it. Booted both
# ways, it checks every byte of the ballast. The BIOS image also carries
# a ramdisk, which for a 64-bit kernel must now land above the kernel -
# at 3 MiB, where the kernel's file was staged - and not at 2 MiB,
# through the middle of it.
BIG64_DIR = $(BUILD_DIR)/big64
BIG64_ISO = $(BUILD_DIR)/big64.iso
BIG64_ELF = $(KERNEL64_DIR)/target/big-image/x86_64-unknown-none/release/kernel64
BIG64_LOG = $(BUILD_DIR)/big64
BIG64_SAID = \[test\] big image: the kernel ends at 0x[0-9a-f]+, above 2 MiB; 512 KiB of it at 0x[0-9a-f]+-0x[0-9a-f]+ is byte for byte what was built
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

# Every unsafe block, impl and public unsafe fn says why it is sound
# (K112): clippy's two lints for it, as errors, over every crate and CPU.
unsafe-check:
	@USER_A64_BINS="$(USER_A64_BINS)" sh tools/unsafe-check.sh

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
	cd ahci && cargo test
	cd nvme && cargo test
	cd acpi && cargo test
	cd e1000 && cargo test
	cd usb && cargo test
	cd net && cargo test
	cd gfx && cargo test
	cd gui && cargo test
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
# And ctype.c, stdlib.c and inet.c the same way: every answer compared
# with this machine's own.
#
# And math.c: every name it defines, and ldexp, which it calls, renamed
# taaj_..., and its answers compared bit for bit with glibc's for
# millions of numbers (math_test.c). And scan.c's sscanf, with glibc's
# beside it (scan_test.c).
HOST_CFLAGS    = -std=c11 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-sanitize-recover=undefined
FORMAT_RENAMED = -Dvsnprintf=taaj_vsnprintf -Dsnprintf=taaj_snprintf -Dvsprintf=taaj_vsprintf -Dsprintf=taaj_sprintf \
                 -Dvasprintf=taaj_vasprintf -Dasprintf=taaj_asprintf
# What unix.c defines, renamed taaj_NAME for its test on the host (its
# calls to malloc, fprintf and the rest go to the host's).
UNIX_NAMES = __errno_location basename dirname getopt optarg optind opterr optopt environ getenv setenv unsetenv putenv \
             fnmatch glob globfree popen pclose errno
libc-test:
	@mkdir -p $(LIBC_DIR)/target
	rustc --edition 2021 --test $(LIBC_DIR)/src/string.rs -o $(LIBC_DIR)/target/string-test
	$(LIBC_DIR)/target/string-test
	rustc --edition 2021 --test $(LIBC_DIR)/src/errno.rs -o $(LIBC_DIR)/target/errno-test
	$(LIBC_DIR)/target/errno-test
	rustc --edition 2021 --test $(LIBC_DIR)/src/number.rs -o $(LIBC_DIR)/target/number-test
	$(LIBC_DIR)/target/number-test
	rustc --edition 2021 --test $(LIBC_DIR)/src/words.rs -o $(LIBC_DIR)/target/words-test
	$(LIBC_DIR)/target/words-test
	rustc --edition 2021 --test $(LIBC_DIR)/src/sh.rs -o $(LIBC_DIR)/target/sh-test
	$(LIBC_DIR)/target/sh-test
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
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	    -c $(LIBC_DIR)/c/unix.c -o $(LIBC_DIR)/target/host/unix.o
	objcopy $(foreach name,$(UNIX_NAMES),--redefine-sym $(name)=taaj_$(name)) $(LIBC_DIR)/target/host/unix.o
	clang $(HOST_CFLAGS) -c $(LIBC_DIR)/c/unix_test.c -o $(LIBC_DIR)/target/host/unix_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/unix.o $(LIBC_DIR)/target/host/unix_test.o \
	    -o $(LIBC_DIR)/target/host/unix-test
	@# setenv never frees an entry it replaces (a string getenv gave out
	@# stays good, as in glibc): leaks by design, so not looked for.
	ASAN_OPTIONS=detect_leaks=0 $(LIBC_DIR)/target/host/unix-test
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	    -c $(LIBC_DIR)/c/inet.c -o $(LIBC_DIR)/target/host/inet.o
	objcopy --redefine-syms=$(LIBC_DIR)/c/host_names $(LIBC_DIR)/target/host/inet.o
	clang $(HOST_CFLAGS) -c $(LIBC_DIR)/c/inet_test.c -o $(LIBC_DIR)/target/host/inet_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/inet.o $(LIBC_DIR)/target/host/inet_test.o \
	    -o $(LIBC_DIR)/target/host/inet-test
	$(LIBC_DIR)/target/host/inet-test
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	    -c $(LIBC_DIR)/c/math.c -o $(LIBC_DIR)/target/host/math.o
	{ $(LLVM_NM) --defined-only --extern-only -j $(LIBC_DIR)/target/host/math.o | grep -v '^__\(asan\|ubsan\|odr\)'; \
	  echo ldexp; echo ldexpf; } | awk '{print $$1, "taaj_" $$1}' > $(LIBC_DIR)/target/host/math_names
	objcopy --redefine-syms=$(LIBC_DIR)/target/host/math_names $(LIBC_DIR)/target/host/math.o
	clang $(HOST_CFLAGS) -c $(LIBC_DIR)/c/math_test.c -o $(LIBC_DIR)/target/host/math_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/math.o $(LIBC_DIR)/target/host/math_test.o -lm \
	    -o $(LIBC_DIR)/target/host/math-test
	$(LIBC_DIR)/target/host/math-test
	@for part in math $(addprefix libm/,$(LIBM)); do \
	    clang $(HOST_CFLAGS) -ffp-contract=off -Wno-shift-op-parentheses -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include \
	        -c $(LIBC_DIR)/c/$$part.c -o $(LIBC_DIR)/target/host/libm-$$(basename $$part).o || exit 1; \
	done
	ld -r $(addprefix $(LIBC_DIR)/target/host/libm-,$(addsuffix .o,math $(LIBM))) -o $(LIBC_DIR)/target/host/libm-all.o
	$(LLVM_NM) --defined-only --extern-only -j $(LIBC_DIR)/target/host/libm-all.o | grep -v '^__\(asan\|ubsan\|odr\)' \
	    | awk '{print $$1, "taaj_" $$1}' > $(LIBC_DIR)/target/host/libm_names
	objcopy --redefine-syms=$(LIBC_DIR)/target/host/libm_names $(LIBC_DIR)/target/host/libm-all.o
	clang $(HOST_CFLAGS) -c $(LIBC_DIR)/c/libm_test.c -o $(LIBC_DIR)/target/host/libm_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/libm-all.o $(LIBC_DIR)/target/host/libm_test.o -lm \
	    -o $(LIBC_DIR)/target/host/libm-test
	$(LIBC_DIR)/target/host/libm-test
	clang $(HOST_CFLAGS) -ffreestanding -nostdlibinc -isystem $(LIBC_DIR)/include -Dsscanf=taaj_sscanf -Dvsscanf=taaj_vsscanf \
	    -c $(LIBC_DIR)/c/scan.c -o $(LIBC_DIR)/target/host/scan.o
	clang $(HOST_CFLAGS) -Wno-format -c $(LIBC_DIR)/c/scan_test.c -o $(LIBC_DIR)/target/host/scan_test.o
	clang -fsanitize=address,undefined $(LIBC_DIR)/target/host/scan.o $(LIBC_DIR)/target/host/scan_test.o \
	    -o $(LIBC_DIR)/target/host/scan-test
	$(LIBC_DIR)/target/host/scan-test
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
LLVM_NM      := $(shell command -v llvm-nm || ls /usr/bin/llvm-nm-* 2>/dev/null | sort -V | tail -1)
LLVM_AR      := $(shell command -v llvm-ar || ls /usr/bin/llvm-ar-* 2>/dev/null | sort -V | tail -1)
HAVE_A64     := $(shell command -v qemu-system-aarch64 >/dev/null && [ -n "$(LLVM_OBJCOPY)" ] && echo yes)

# The programs the AArch64 kernel embeds: the user/ crate again, for
# the other CPU. Named one by one, because a few programs are for the PC
# only (they use I/O ports) and would not compile.
USER_A64_BINS := $(filter-out kbd port gs,$(patsubst user/src/bin/%.rs,%,$(wildcard user/src/bin/*.rs)))

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
C_A64_PROGRAMS = $(C_PROGRAMS)
# As TAAJ_CFLAGS. There is no red zone to switch off on this CPU; the
# floating-point and vector registers are saved per thread here too.
TAAJ_A64_CFLAGS  = --target=aarch64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding \
                   -fno-stack-protector -fno-pic \
                   -nostdlibinc -isystem $(LIBC_DIR)/include
# The linker would otherwise line segments up for 64 KiB pages, which
# some AArch64 systems use: 60 KiB of padding in every file.
TAAJ_A64_LDFLAGS = $(TAAJ_LDFLAGS) -z max-page-size=4096
cprogs-aarch64:
ifeq ($(HAVE_CC),yes)
	cd $(LIBC_DIR) && cargo build --release --target $(KERNEL_A64_TARGET)
	@mkdir -p $(C_A64_BIN)/lib/libm
	@for part in $(LIBC_C); do \
	    clang $(TAAJ_A64_CFLAGS) -ffp-contract=off -Wno-shift-op-parentheses -c $(LIBC_DIR)/c/$$part.c -o $(C_A64_BIN)/lib/$$part.o || exit 1; \
	done
	@cp $(LIBC_A64_RUST) $(C_A64_BIN)/libc.a
	@$(LLVM_OBJCOPY) --strip-debug $(LIBC_RUST_PRIVATE) $(C_A64_BIN)/libc.a
	@ar rs $(C_A64_BIN)/libc.a $(addprefix $(C_A64_BIN)/lib/,$(addsuffix .o,$(LIBC_C)))
	@python3 -I tools/thin-archive.py $(LLVM_NM) $(LLVM_AR) $(C_A64_BIN)/libc.a
	@for program in $(C_A64_PROGRAMS); do \
	    echo "  CC      $$program.c (AArch64)"; \
	    clang $(TAAJ_A64_CFLAGS) -c $(LIBC_DIR)/programs/$$program.c -o $(C_A64_BIN)/$$program.o || exit 1; \
	    ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/$$program.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/$$program || exit 1; \
	done
else
	@echo "Clang or lld not found: no C programs for AArch64 (sudo apt install clang lld)"
endif

pdpmake-aarch64: cprogs-aarch64
ifeq ($(HAVE_CC),yes)
	@mkdir -p $(C_A64_BIN)/pdpmake
	@for part in $(PDPMAKE_PARTS); do \
	    echo "  CC      ports/pdpmake/$$part.c (AArch64)"; \
	    clang $(TAAJ_A64_CFLAGS) -Wno-sign-compare -c ports/pdpmake/$$part.c -o $(C_A64_BIN)/pdpmake/$$part.o || exit 1; \
	done
	@ld.lld $(TAAJ_A64_LDFLAGS) $(addprefix $(C_A64_BIN)/pdpmake/,$(addsuffix .o,$(PDPMAKE_PARTS))) $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/make
endif

kernel-aarch64: user-aarch64 cprogs-aarch64
	cd $(KERNEL64_DIR) && cargo build --release --target $(KERNEL_A64_TARGET)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(LLVM_OBJCOPY) -O binary $(KERNEL_A64_ELF) $(KERNEL_A64_IMAGE).new && mv -f $(KERNEL_A64_IMAGE).new $(KERNEL_A64_IMAGE)

# The disk the AArch64 boot test attaches: as the PC's, with the
# programs compiled for the other CPU.
A64_USER_BIN  = user/target/$(KERNEL_A64_TARGET)/release
A64_TEST_DISK = $(BUILD_DIR)/aarch64/keys.disk
aarch64-test-disk: user-aarch64 cprogs-aarch64 tfstool-rs
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(A64_TEST_DISK),$(A64_USER_BIN),$(C_A64_BIN))

# The disk `make run-aarch64` attaches: as the PC's run-disk - a 16 MiB
# TaajFS volume kept from one run to the next, its programs replaced
# with the ones just built.
RUN_A64_DISK = $(BUILD_DIR)/aarch64/taaj-disk.img
run-aarch64-disk: user-aarch64 cprogs-aarch64 pdpmake-aarch64 tcc-aarch64 cxx-aarch64 zlib-aarch64 tfstool-rs
	@mkdir -p $(BUILD_DIR)/aarch64
	@[ -f $(RUN_A64_DISK) ] || { $(TFSTOOL_RS) mkfs $(RUN_A64_DISK) $(RUN_DISK_BLOCKS) > /dev/null && truncate -s $$(($(RUN_DISK_BLOCKS) * 4096)) $(RUN_A64_DISK) \
	    && printf 'Hello from the host\n' > $(RUN_A64_DISK).welcome && $(TFSTOOL_RS) put $(RUN_A64_DISK) welcome $(RUN_A64_DISK).welcome > /dev/null \
	    && echo "Made a new disk: $(RUN_A64_DISK)"; }
	@for program in $(DISK_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_A64_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_A64_DISK) $$program $(A64_USER_BIN)/$$program > /dev/null || exit 1; \
	done
	@for program in $(C_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_A64_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_A64_DISK) $$program $(C_A64_BIN)/$$program > /dev/null || exit 1; \
	done
	$(call put-site,$(RUN_A64_DISK))
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) tools/put-tcc.sh $(RUN_A64_DISK) $(TCC_A64_DIR) $(C_A64_BIN)/libc.a
	@TFSTOOL=$(TFSTOOL_RS) tools/put-sources.sh $(RUN_A64_DISK) aarch64 $(TCC_A64_DIR)
	@for program in $(CXX_PROGRAMS); do \
	    $(TFSTOOL_RS) rm $(RUN_A64_DISK) $$program > /dev/null 2>&1; \
	    $(TFSTOOL_RS) put $(RUN_A64_DISK) $$program $(CXX_A64_DIR)/bin/$$program > /dev/null || exit 1; \
	done
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) make > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) make $(C_A64_BIN)/make > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(THREADS_C) -o $(C_A64_BIN)/threads.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/threads.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/threads
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) threads > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) threads $(C_A64_BIN)/threads > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(BIGMEM_C) -o $(C_A64_BIN)/bigmem.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/bigmem.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/bigmem
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) bigmem > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) bigmem $(C_A64_BIN)/bigmem > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(BASICS_C) -o $(C_A64_BIN)/basics.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/basics.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/basics
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) basics > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) basics $(C_A64_BIN)/basics > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(GROUND_C) -o $(C_A64_BIN)/ground.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/ground.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/ground
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) ground > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) ground $(C_A64_BIN)/ground > /dev/null
	@clang $(TAAJ_A64_CFLAGS) $(ZLIB_CFLAGS) -c $(GROUND2_C) -o $(C_A64_BIN)/ground2.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/ground2.o $(C_A64_BIN)/libz.a $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/ground2
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) ground2 > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) ground2 $(C_A64_BIN)/ground2 > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(HANDLES_C) -o $(C_A64_BIN)/handles.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/handles.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/handles
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) handles > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) handles $(C_A64_BIN)/handles > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(NAMESPACE_C) -o $(C_A64_BIN)/namespace.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/namespace.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/namespace
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) namespace > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) namespace $(C_A64_BIN)/namespace > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(AT_C) -o $(C_A64_BIN)/at.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/at.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/at
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) at > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) at $(C_A64_BIN)/at > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(TMP_C) -o $(C_A64_BIN)/tmptest.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/tmptest.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/tmptest
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) tmptest > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) tmptest $(C_A64_BIN)/tmptest > /dev/null
	@clang $(TAAJ_A64_CFLAGS) -c $(PAGES_C) -o $(C_A64_BIN)/pages.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/pages.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/pages
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) pages > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) pages $(C_A64_BIN)/pages > /dev/null
endif
	@$(TFSTOOL_RS) rm $(RUN_A64_DISK) threadcheck > /dev/null 2>&1; $(TFSTOOL_RS) put $(RUN_A64_DISK) threadcheck $(A64_USER_BIN)/threadcheck > /dev/null

# The serial port is this terminal: what you type goes to the shell.
# Ctrl+A, then X, leaves QEMU. RUN_A64 adds QEMU options:
#   make run-aarch64 RUN_A64="-M virt,gic-version=3"
# RUN_A64_DEVICES is the disk and the network card (on QEMU's own
# little network); give it empty for a machine without them, or
# devices of your own:
#   make run-aarch64 RUN_A64_DEVICES=
RUN_A64_DEVICES = -drive file=$(RUN_A64_DISK),if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 \
                  -netdev user,id=n0 -device virtio-net-device,netdev=n0
run-aarch64: kernel-aarch64 run-aarch64-disk
	@echo "(to leave QEMU: Ctrl+A, then X)"
	qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -m $(RUN_MEMORY) -nographic -kernel $(KERNEL_A64_IMAGE) $(RUN_A64_DEVICES) $(RUN_A64)

# The same machine with a screen: QEMU's window shows the ramfb screen,
# and takes the keys (a virtio keyboard) and the pointer (a virtio
# tablet - the arrow follows QEMU's own pointer, with nothing to grab).
# The serial port is the kernel's log, in this terminal. Close the
# window to leave. RUN_A64_DISPLAY picks QEMU's window kind:
#   make run-aarch64-screen RUN_A64_DISPLAY="-display sdl"
RUN_A64_DISPLAY =
run-aarch64-screen: kernel-aarch64 run-aarch64-disk
	qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -m $(RUN_MEMORY) -serial stdio $(RUN_A64_DISPLAY) -kernel $(KERNEL_A64_IMAGE) \
	    -device ramfb -device virtio-keyboard-device -device virtio-tablet-device $(RUN_A64_DEVICES) $(RUN_A64)

# Boots on several machines, and one deliberate fault.
aarch64-test: user-aarch64
ifeq ($(HAVE_A64),yes)
	$(KERNEL64_DIR)/scripts/aarch64-test.sh $(BUILD_DIR) $(LLVM_OBJCOPY)
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run (sudo apt install qemu-system-arm llvm)"
endif

# The whole system on AArch64, used: the disk the PC's boot test gets
# (with the programs compiled for this CPU), the keys the PC's test
# types (as the bytes a terminal sends for them), and the PC's own
# check of what came of it (which knows the few lines that name the
# PC's own drivers). Four CPUs, as on the PC: the check wants four
# programs to have run at once.
A64_KEYS_LOG    = $(BUILD_DIR)/aarch64/keys.log
# The second boot of that test: one CPU, and a program that never gives
# it up of its own accord. What the shell then says must still reach
# the terminal - the kernel writes programs' output when the CPU is
# idle, and here it never is (console.rs: the timer writes too).
A64_BUSY_LOG    = $(BUILD_DIR)/aarch64/busy.log
A64_BUSY_DISK   = $(BUILD_DIR)/aarch64/busy.disk
#
# Then a program that says nothing itself, started in the background.
# That it is over is said by the kernel, while the shell's prompt sits
# there unfinished - a line the kernel's message waits behind, and must
# not wait behind for ever (console.rs: lines are kept whole, for a few
# ticks).
A64_BUSY_KEYS   = f o r e v e r spc shift-7 ret e c h o spc s t i l l spc h e r e ret \
                  c p r i m e s spc 1 0 0 spc shift-dot spc o u t spc shift-7 ret see:exited
# kernel-aarch64 too: the image it boots carries the user programs (the
# shell among them), and without it, run on its own, this booted the last
# image built - an old shell (journal entry 114).
aarch64-keys-test: aarch64-test-disk kernel-aarch64
ifeq ($(HAVE_A64),yes)
	@echo "Booting the AArch64 kernel and typing at its serial port (log: $(A64_KEYS_LOG))..."
	@KEYS="$(TEST_KEYS)" SLOTS=4 QEMU_ARGS="-smp 4 -drive file=$(A64_TEST_DISK),if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
	    $(KERNEL64_DIR)/scripts/aarch64-boot.sh $(KERNEL_A64_IMAGE) $(A64_KEYS_LOG) 40
	@ARCH=aarch64 DISK=$(A64_TEST_DISK) TFSTOOL=$(TFSTOOL_RS) CWORDS_HOST=$(CWORDS_HOST) CWORDS_TAAJ=$(C_A64_BIN)/cwords CPRIMES_HOST=$(CPRIMES_HOST) CPRIMES_LIMIT=$(CPRIMES_LIMIT) TYPED="$(TEST_TYPED)" EXPECT="$(TEST_EXPECT)" \
	    $(KERNEL64_DIR)/scripts/check-boot.sh $(A64_KEYS_LOG) "" > $(A64_KEYS_LOG).out || { grep -a "^FAIL" $(A64_KEYS_LOG).out; echo "FAIL: the typed session on AArch64 (all of it in $(A64_KEYS_LOG).out)"; exit 1; }
	@echo "Booting it again, to type while a program computes without end (log: $(A64_BUSY_LOG))..."
	@cp $(A64_TEST_DISK) $(A64_BUSY_DISK)
	@KEYS="$(A64_BUSY_KEYS)" QEMU_ARGS="-drive file=$(A64_BUSY_DISK),if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
	    $(KERNEL64_DIR)/scripts/aarch64-boot.sh $(KERNEL_A64_IMAGE) $(A64_BUSY_LOG) 40
	@grep -aq '\[shell\] ran "echo still here"' $(A64_BUSY_LOG) && grep -aq '^still here' $(A64_BUSY_LOG) \
	    || { echo "FAIL: with a program computing in the background, the terminal showed nothing more (log: $(A64_BUSY_LOG))"; exit 1; }
	@grep -aq '"cprimes" exited with code 0' $(A64_BUSY_LOG) \
	    || { echo "FAIL: what the kernel said behind an unfinished line of the terminal never came out (log: $(A64_BUSY_LOG))"; exit 1; }
	@echo "aarch64-keys-test passed"

# The AArch64 kernel on a machine with UEFI firmware: TheTaaJBoot's
# loader for it (bootloader/src/boot/uefi/aarch64.c), on a FAT
# partition with the kernel's image beside it - what a memory stick for
# such a machine would hold.
#
# The firmware is QEMU's, from the qemu-efi-aarch64 package.
UEFI_A64_EFI  = $(BOOTLOADER_BUILD_DIR)/uefi/BOOTAA64.EFI
A64_ESP_IMG   = $(BUILD_DIR)/aarch64/esp.img
AAVMF_DIR     = /usr/share/AAVMF
HAVE_A64_UEFI := $(shell [ "$(HAVE_A64)" = yes ] && [ "$(HAVE_CLANG)" = yes ] && [ -f $(AAVMF_DIR)/AAVMF_CODE.fd ] && echo yes)

aarch64-esp: kernel-aarch64
ifeq ($(HAVE_A64_UEFI),yes)
	@echo "Building the UEFI loader for AArch64, and a partition with it and the kernel..."
	$(MAKE) -C $(UEFI_DIR) ROOT_DIR=$(abspath bootloader) aarch64
	@rm -f $(A64_ESP_IMG)
	$(MKFS_FAT) -C -n TAAJESP $(A64_ESP_IMG) 8192 >/dev/null
	mmd -i $(A64_ESP_IMG) ::/EFI ::/EFI/BOOT ::/kernel
	mcopy -i $(A64_ESP_IMG) $(UEFI_A64_EFI) ::/EFI/BOOT/BOOTAA64.EFI
	mcopy -i $(A64_ESP_IMG) $(KERNEL_A64_IMAGE) ::/kernel/Image
else
	@echo "NOTE: no UEFI boot of AArch64 - it needs clang, lld and QEMU's firmware (sudo apt install clang lld qemu-efi-aarch64)"
endif

# As `make run-aarch64`, started by UEFI firmware. The firmware talks
# on the same serial port first; RUN_A64 adds QEMU options here too.
run-aarch64-uefi: aarch64-esp run-aarch64-disk
ifneq ($(HAVE_A64_UEFI),yes)
	@echo "ERROR: no UEFI boot of AArch64 - it needs clang, lld and QEMU's firmware (sudo apt install clang lld qemu-efi-aarch64)"; exit 1
endif
	@cp $(AAVMF_DIR)/AAVMF_VARS.fd $(BUILD_DIR)/aarch64/run-vars.fd
	@echo "(to leave QEMU: Ctrl+A, then X)"
	qemu-system-aarch64 -M virt,acpi=off -cpu cortex-a72 -smp 4 -m 512M -nographic \
	    -drive if=pflash,format=raw,readonly=on,file=$(AAVMF_DIR)/AAVMF_CODE.fd \
	    -drive if=pflash,format=raw,file=$(BUILD_DIR)/aarch64/run-vars.fd \
	    -drive file=$(A64_ESP_IMG),format=raw,if=none,id=esp -device virtio-blk-pci,drive=esp \
	    $(RUN_A64_DEVICES) $(RUN_A64)

aarch64-uefi-test: aarch64-esp aarch64-test-disk
ifeq ($(HAVE_A64_UEFI),yes)
	@$(KERNEL64_DIR)/scripts/aarch64-uefi-test.sh $(A64_ESP_IMG) $(A64_TEST_DISK) $(BUILD_DIR)/aarch64
else
	@echo "NOTE: aarch64-uefi-test skipped - it needs clang, lld and QEMU's firmware (sudo apt install clang lld qemu-efi-aarch64)"
endif
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# The network card, on both machines: each boots with its card wired to
# a program on this host that answers what `niccheck` sends
# (kernel64/scripts/net-test.sh, wire.py), and once more with the cable
# going nowhere. A disk of its own each: the other tests write to theirs.
NET64_LOG     = $(BUILD_DIR)/net64.log
NET_A64_LOG   = $(BUILD_DIR)/aarch64/net.log
net64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(NET64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/net-test.sh x86_64 $(ISO64_IMG) $(NET64_LOG).disk $(NET64_LOG)
	@echo "net64-test passed"

# USB: an xHCI controller, a keyboard typed on and a mouse clicked
# through QEMU's monitor, a key held, a keyboard plugged in and out;
# under BIOS, UEFI, and with the controller polled.
USB64_LOG = $(BUILD_DIR)/usb-64
usb64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(USB64_LOG).disk)
	@OVMF=$(OVMF) $(KERNEL64_DIR)/scripts/usb-test.sh $(ISO64_IMG) $(USB64_LOG).disk $(USB64_LOG)
	@echo "usb64-test passed"

# The card's test with Intel's cards, driven by `e1000`: the 82540EM
# (QEMU's `e1000`, no MSI - polled) and the 82574L (`e1000e`, MSI); then
# the network's tests over them - IP over the first, TCP the second.
# Booting from a USB stick (BIOS and UEFI) made of the test disk, and
# the loader reading a SATA CD.
STICK64_LOG = $(BUILD_DIR)/stick-64
stick64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(STICK64_LOG).disk)
	$(call hybrid64,$(STICK64_LOG).img,-append_partition 3 0x7f $(STICK64_LOG).disk)
	@OVMF=$(OVMF) $(KERNEL64_DIR)/scripts/stick-test.sh $(ISO64_IMG) $(STICK64_LOG).img $(STICK64_LOG).disk $(STICK64_LOG)
	@echo "stick64-test passed"

E1000_64_LOG = $(BUILD_DIR)/e1000-64
e1000-64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(E1000_64_LOG).disk)
	@NIC=e1000 $(KERNEL64_DIR)/scripts/net-test.sh x86_64 $(ISO64_IMG) $(E1000_64_LOG).disk $(E1000_64_LOG).82540em
	@NIC=e1000e $(KERNEL64_DIR)/scripts/net-test.sh x86_64 $(ISO64_IMG) $(E1000_64_LOG).disk $(E1000_64_LOG).82574l
	@NIC=e1000 $(KERNEL64_DIR)/scripts/ip-test.sh x86_64 $(ISO64_IMG) $(E1000_64_LOG).disk $(E1000_64_LOG).ip
	@NIC=e1000e $(KERNEL64_DIR)/scripts/tcp-test.sh x86_64 $(ISO64_IMG) $(E1000_64_LOG).disk $(E1000_64_LOG).tcp
	@echo "e1000-64-test passed"

aarch64-net-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(NET_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	@$(KERNEL64_DIR)/scripts/net-test.sh aarch64 $(KERNEL_A64_IMAGE) $(NET_A64_LOG).disk $(NET_A64_LOG)
	@echo "aarch64-net-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# poweroff and reboot on AArch64: PSCI, through the power handle
# (scripts/power-test.sh).
POWER_A64_LOG = $(BUILD_DIR)/aarch64/power.log
aarch64-power-test: kernel-aarch64
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@$(KERNEL64_DIR)/scripts/power-test.sh aarch64 $(KERNEL_A64_IMAGE) $(POWER_A64_LOG)
	@echo "aarch64-power-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# The network service, on both machines: ARP, IPv4, ping and UDP against
# a router made by hand (wire-ip.py) and against QEMU's own network
# with this host's sockets behind it (udp-host.py) - two boots each
# (kernel64/scripts/ip-test.sh).
IP64_LOG      = $(BUILD_DIR)/ip64.log
IP_A64_LOG    = $(BUILD_DIR)/aarch64/ip.log
TCP64_LOG     = $(BUILD_DIR)/tcp64.log
TCP_A64_LOG   = $(BUILD_DIR)/aarch64/tcp.log
HTTP64_LOG    = $(BUILD_DIR)/http64.log
HTTP_A64_LOG  = $(BUILD_DIR)/aarch64/http.log
ip64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(IP64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/ip-test.sh x86_64 $(ISO64_IMG) $(IP64_LOG).disk $(IP64_LOG)
	@echo "ip64-test passed"

aarch64-ip-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(IP_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	@$(KERNEL64_DIR)/scripts/ip-test.sh aarch64 $(KERNEL_A64_IMAGE) $(IP_A64_LOG).disk $(IP_A64_LOG)
	@echo "aarch64-ip-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

tcp64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(TCP64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/tcp-test.sh x86_64 $(ISO64_IMG) $(TCP64_LOG).disk $(TCP64_LOG)
	@echo "tcp64-test passed"

aarch64-tcp-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(TCP_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	@$(KERNEL64_DIR)/scripts/tcp-test.sh aarch64 $(KERNEL_A64_IMAGE) $(TCP_A64_LOG).disk $(TCP_A64_LOG)
	@echo "aarch64-tcp-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# The web server: httpd on the machine, curl and sockets on this host
# (kernel64/scripts/http-test.sh). The test disk has the site, and a
# file of 1 MiB to be fetched and compared byte for byte.
define make-http-disk
	$(call put-site,$(1))
	@head -c 1048576 /dev/urandom > $(1).big && $(TFSTOOL_RS) put $(1) www/big.bin $(1).big > /dev/null
endef
http64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(HTTP64_LOG).disk)
	$(call make-http-disk,$(HTTP64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/http-test.sh x86_64 $(ISO64_IMG) $(HTTP64_LOG).disk $(HTTP64_LOG) $(SITE_DIR) $(HTTP64_LOG).disk.big
	@echo "http64-test passed"

aarch64-http-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(HTTP_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	$(call make-http-disk,$(HTTP_A64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/http-test.sh aarch64 $(KERNEL_A64_IMAGE) $(HTTP_A64_LOG).disk $(HTTP_A64_LOG) $(SITE_DIR) $(HTTP_A64_LOG).disk.big
	@echo "aarch64-http-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# The screen and the pointer (journal entry 066): a line typed and the
# pointer moved and clicked - the PS/2 mouse on the PC; on AArch64 a
# ramfb screen with a virtio keyboard, mouse and tablet - and the
# screen read back for the arrow.
SCREEN64_LOG   = $(BUILD_DIR)/screen64.log
SCREEN_A64_LOG = $(BUILD_DIR)/aarch64/screen.log

screen64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(SCREEN64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/screen-test.sh x86_64 $(ISO64_IMG) $(SCREEN64_LOG).disk $(SCREEN64_LOG)
	@echo "screen64-test passed"

aarch64-screen-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(SCREEN_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	@$(KERNEL64_DIR)/scripts/screen-test.sh aarch64 $(KERNEL_A64_IMAGE) $(SCREEN_A64_LOG).disk $(SCREEN_A64_LOG)
	@echo "aarch64-screen-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# Windows (journal entry 067): counter's window opened, clicked, typed
# into and closed by its close box; the focus given back to the
# terminal, and the terminal dragged by its title bar - on the PC with
# the PS/2 mouse, on AArch64 with a virtio tablet.
GUI64_LOG   = $(BUILD_DIR)/gui64.log
GUI_A64_LOG = $(BUILD_DIR)/aarch64/gui.log

gui64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(GUI64_LOG).disk)
	@$(KERNEL64_DIR)/scripts/gui-test.sh x86_64 $(ISO64_IMG) $(GUI64_LOG).disk $(GUI64_LOG)
	@echo "gui64-test passed"

aarch64-gui-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(GUI_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	@$(KERNEL64_DIR)/scripts/gui-test.sh aarch64 $(KERNEL_A64_IMAGE) $(GUI_A64_LOG).disk $(GUI_A64_LOG)
	@echo "aarch64-gui-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# The text editor (journal entry 068): a new file typed and saved, the
# 300-line `long` paged through and changed, a click and the close box;
# then the files read back off the test disk and compared on the host.
EDIT64_LOG   = $(BUILD_DIR)/edit64.log
EDIT_A64_LOG = $(BUILD_DIR)/aarch64/edit.log

edit64-test: iso64 cprogs64 tfstool-rs
	$(call make-test-disk,$(EDIT64_LOG).disk)
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/edit-test.sh x86_64 $(ISO64_IMG) $(EDIT64_LOG).disk $(EDIT64_LOG)
	@echo "edit64-test passed"

aarch64-edit-test: kernel-aarch64 user-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64),yes)
	@mkdir -p $(BUILD_DIR)/aarch64
	$(call make-test-disk-from,$(EDIT_A64_LOG).disk,$(A64_USER_BIN),$(C_A64_BIN))
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/edit-test.sh aarch64 $(KERNEL_A64_IMAGE) $(EDIT_A64_LOG).disk $(EDIT_A64_LOG)
	@echo "aarch64-edit-test passed"
else
	@echo "qemu-system-aarch64 or llvm-objcopy not found: the AArch64 boots are not run"
endif

# TCC, the Tiny C Compiler (ports/tcc, journal entry 069): for each CPU
# a cross compiler that runs here, and the tcc that runs on TheTaaJ,
# compiled by clang and linked with that CPU's libc.a. tools/build-tcc.sh
# says what is built where; tools/put-tcc.sh puts it on a disk.
TCC_X86_DIR  = $(BUILD_DIR)/tcc/x86_64
TCC_A64_DIR  = $(BUILD_DIR)/tcc/aarch64
TCC64_LOG    = $(BUILD_DIR)/tcc64.log
TCC_A64_LOG  = $(BUILD_DIR)/aarch64/tcc.log

tcc64: libc64
ifeq ($(HAVE_CC),yes)
	@tools/build-tcc.sh x86_64 $(TCC_X86_DIR) $(LIBC_A) "$(TAAJ_CFLAGS)" "$(TAAJ_LDFLAGS)"
else
	@echo "Clang or lld not found: no tcc (sudo apt install clang lld)"
endif

tcc-aarch64: cprogs-aarch64
ifeq ($(HAVE_CC),yes)
	@tools/build-tcc.sh aarch64 $(TCC_A64_DIR) $(C_A64_BIN)/libc.a "$(TAAJ_A64_CFLAGS)" "$(TAAJ_A64_LDFLAGS)"
else
	@echo "Clang or lld not found: no tcc for AArch64 (sudo apt install clang lld)"
endif

# tcc on TheTaaJ compiles three programs there, which run there; then
# the C library and every C program, by a script (libc/build.taaj);
# then a program typed into the editor. What it wrote is compared, byte
# for byte, with what the cross compiler writes from the same sources.
tcc64-test: iso64 user64 tcc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) EDIT=$(USER_BIN)/edit $(KERNEL64_DIR)/scripts/tcc-test.sh x86_64 $(ISO64_IMG) $(TCC64_LOG).disk $(TCC64_LOG) $(TCC_X86_DIR) $(LIBC_A)
	@echo "tcc64-test passed"
else
	@echo "FAIL: tcc needs clang and lld (sudo apt install clang lld)"; exit 1
endif

# make on TheTaaJ: pdpmake builds a program with tcc there, and then
# only what each change puts out of date (journal entry 078).
MAKE64_LOG   = $(BUILD_DIR)/make64.log
MAKE_A64_LOG = $(BUILD_DIR)/aarch64/make.log
make64-test: iso64 user64 tcc64 pdpmake64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) APPEND=$(USER_BIN)/append SH=$(C_BIN)/sh $(KERNEL64_DIR)/scripts/make-test.sh x86_64 $(ISO64_IMG) $(MAKE64_LOG).disk $(MAKE64_LOG) $(TCC_X86_DIR) $(LIBC_A) $(C_BIN)/make
	@echo "make64-test passed"
else
	@echo "FAIL: make needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-make-test: kernel-aarch64 user-aarch64 tcc-aarch64 pdpmake-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@TFSTOOL=$(TFSTOOL_RS) APPEND=$(A64_USER_BIN)/append SH=$(C_A64_BIN)/sh $(KERNEL64_DIR)/scripts/make-test.sh aarch64 $(KERNEL_A64_IMAGE) $(MAKE_A64_LOG).disk $(MAKE_A64_LOG) $(TCC_A64_DIR) $(C_A64_BIN)/libc.a $(C_A64_BIN)/make
	@echo "aarch64-make-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 make boot is not run"
endif

# The C library and its C programs, sh among them, built by make on
# TheTaaJ (libc/c/Makefile.taaj), then only what a change puts out of
# date; byte for byte what the host's tcc writes (journal entry 083).
LIBMAKE64_LOG   = $(BUILD_DIR)/libmake64.log
LIBMAKE_A64_LOG = $(BUILD_DIR)/aarch64/libmake.log
libmake64-test: iso64 user64 tcc64 pdpmake64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) APPEND=$(USER_BIN)/append SH=$(C_BIN)/sh $(KERNEL64_DIR)/scripts/libc-make-test.sh x86_64 $(ISO64_IMG) $(LIBMAKE64_LOG).disk $(LIBMAKE64_LOG) $(TCC_X86_DIR) $(LIBC_A) $(C_BIN)/make
	@echo "libmake64-test passed"
else
	@echo "FAIL: make needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-libmake-test: kernel-aarch64 user-aarch64 tcc-aarch64 pdpmake-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@TFSTOOL=$(TFSTOOL_RS) APPEND=$(A64_USER_BIN)/append SH=$(C_A64_BIN)/sh $(KERNEL64_DIR)/scripts/libc-make-test.sh aarch64 $(KERNEL_A64_IMAGE) $(LIBMAKE_A64_LOG).disk $(LIBMAKE_A64_LOG) $(TCC_A64_DIR) $(C_A64_BIN)/libc.a $(C_A64_BIN)/make
	@echo "aarch64-libmake-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 libc make boot is not run"
endif

# Pipes and redirections for programs: pipes.c (next to the test
# script) checks them on TheTaaJ, and make runs recipes that use them
# (journal entry 079).
PIPE64_LOG   = $(BUILD_DIR)/pipe64.log
PIPE_A64_LOG = $(BUILD_DIR)/aarch64/pipe.log
PIPES_C      = $(KERNEL64_DIR)/scripts/pipes.c
pipe64-test: iso64 user64 pdpmake64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(PIPES_C) -o $(C_BIN)/pipes.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/pipes.o $(LIBC_A) -o $(C_BIN)/pipes
	@TFSTOOL=$(TFSTOOL_RS) CAT=$(USER_BIN)/cat SH=$(C_BIN)/sh $(KERNEL64_DIR)/scripts/pipe-test.sh x86_64 $(ISO64_IMG) $(PIPE64_LOG).disk $(PIPE64_LOG) $(C_BIN)/pipes $(C_BIN)/make
	@echo "pipe64-test passed"
else
	@echo "FAIL: the pipes test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-pipe-test: kernel-aarch64 user-aarch64 pdpmake-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(PIPES_C) -o $(C_A64_BIN)/pipes.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/pipes.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/pipes
	@TFSTOOL=$(TFSTOOL_RS) CAT=$(A64_USER_BIN)/cat SH=$(C_A64_BIN)/sh $(KERNEL64_DIR)/scripts/pipe-test.sh aarch64 $(KERNEL_A64_IMAGE) $(PIPE_A64_LOG).disk $(PIPE_A64_LOG) $(C_A64_BIN)/pipes $(C_A64_BIN)/make
	@echo "aarch64-pipe-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 pipes boot is not run"
endif

# Threads in a program (journal entry 095): threads.c (next to the test
# script) checks pthreads on 4 CPUs, and that a program ends whichever
# of its threads ends it. Its _Thread_local variable is a real one
# since step B (journal entry 099): in the thread's own TLS block, found
# from the thread pointer. (C++ is still compiled with -femulated-tls.)
THREAD64_LOG   = $(BUILD_DIR)/thread64.log
THREAD_A64_LOG = $(BUILD_DIR)/aarch64/thread.log
THREADS_C      = $(KERNEL64_DIR)/scripts/threads.c
thread64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(THREADS_C) -o $(C_BIN)/threads.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/threads.o $(LIBC_A) -o $(C_BIN)/threads
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/thread-test.sh x86_64 $(ISO64_IMG) $(THREAD64_LOG).disk $(THREAD64_LOG) $(C_BIN)/threads $(USER_BIN)/threadcheck
	@echo "thread64-test passed"
else
	@echo "FAIL: the threads test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

# The threads and scheduling benchmark (MaK-WSL/sched's K67 and K95, journal
# entry 116): boots with 1, 2 and 4 CPUs and measures a switch, a futex
# hand-off, thread and program creation, wake-to-run latency, scaling and
# fairness. It checks that every measurement ran, never how fast: the
# numbers are in $(SCHEDBENCH64_LOG).summary, for the journal.
SCHEDBENCH64_LOG = $(BUILD_DIR)/schedbench64.log
SCHEDBENCH_C     = $(KERNEL64_DIR)/scripts/schedbench.c
schedbench64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(SCHEDBENCH_C) -o $(C_BIN)/schedbench.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/schedbench.o $(LIBC_A) -o $(C_BIN)/schedbench
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/schedbench-test.sh $(ISO64_IMG) $(SCHEDBENCH64_LOG).disk $(SCHEDBENCH64_LOG) $(C_BIN)/schedbench
	@echo "schedbench64-test passed"
else
	@echo "FAIL: the scheduling benchmark needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-thread-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(THREADS_C) -o $(C_A64_BIN)/threads.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/threads.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/threads
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/thread-test.sh aarch64 $(KERNEL_A64_IMAGE) $(THREAD_A64_LOG).disk $(THREAD_A64_LOG) $(C_A64_BIN)/threads $(A64_USER_BIN)/threadcheck
	@echo "aarch64-thread-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 threads boot is not run"
endif

# A bigger machine (Era 9, R1; journal entry 096): 4 GB, reserved memory
# filled as it is touched, mmap, big stacks, a memory limit per process,
# and a TLB shootdown. bigmem.c (next to the test script) uses it all.
BIGMEM64_LOG   = $(BUILD_DIR)/bigmem64.log
BIGMEM_A64_LOG = $(BUILD_DIR)/aarch64/bigmem.log
BIGMEM_C       = $(KERNEL64_DIR)/scripts/bigmem.c
bigmem64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(BIGMEM_C) -o $(C_BIN)/bigmem.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/bigmem.o $(LIBC_A) -o $(C_BIN)/bigmem
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/bigmem-test.sh x86_64 $(ISO64_IMG) $(BIGMEM64_LOG).disk $(BIGMEM64_LOG) $(C_BIN)/bigmem
	@echo "bigmem64-test passed"
else
	@echo "FAIL: the bigmem test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-bigmem-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(BIGMEM_C) -o $(C_A64_BIN)/bigmem.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/bigmem.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/bigmem
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/bigmem-test.sh aarch64 $(KERNEL_A64_IMAGE) $(BIGMEM_A64_LOG).disk $(BIGMEM_A64_LOG) $(C_A64_BIN)/bigmem
	@echo "aarch64-bigmem-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 bigmem boot is not run"
endif

# The core basics (journal entry 097): the date from the real-time
# clock, a clock finer than a tick, signals from the kernel (handlers,
# alarms, Ctrl+C, faults, children ended by them) and disk flushes.
# basics.c (next to the test script) checks them.
BASICS64_LOG   = $(BUILD_DIR)/basics64.log
BASICS_A64_LOG = $(BUILD_DIR)/aarch64/basics.log
BASICS_C       = $(KERNEL64_DIR)/scripts/basics.c
basics64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(BASICS_C) -o $(C_BIN)/basics.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/basics.o $(LIBC_A) -o $(C_BIN)/basics
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/basics-test.sh x86_64 $(ISO64_IMG) $(BASICS64_LOG).disk $(BASICS64_LOG) $(C_BIN)/basics
	@echo "basics64-test passed"
else
	@echo "FAIL: the basics test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-basics-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(BASICS_C) -o $(C_A64_BIN)/basics.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/basics.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/basics
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/basics-test.sh aarch64 $(KERNEL_A64_IMAGE) $(BASICS_A64_LOG).disk $(BASICS_A64_LOG) $(C_A64_BIN)/basics
	@echo "aarch64-basics-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 basics boot is not run"
endif

# Std's ground, part 1 (The-Jat's step B, journal entry 099): random
# numbers from the kernel (getrandom), real thread-local storage, signal
# stacks (sigaltstack, a stack overflow caught) and SIGCHLD with waitid.
# ground.c (next to the test script) checks them; the test boots twice,
# the second time on a CPU with a random instruction (-cpu max).
GROUND64_LOG   = $(BUILD_DIR)/ground64.log
GROUND_A64_LOG = $(BUILD_DIR)/aarch64/ground.log
GROUND_C       = $(KERNEL64_DIR)/scripts/ground.c
ground64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(GROUND_C) -o $(C_BIN)/ground.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/ground.o $(LIBC_A) -o $(C_BIN)/ground
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/ground-test.sh x86_64 $(ISO64_IMG) $(GROUND64_LOG).disk $(GROUND64_LOG) $(C_BIN)/ground
	@echo "ground64-test passed"
else
	@echo "FAIL: the ground test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-ground-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(GROUND_C) -o $(C_A64_BIN)/ground.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/ground.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/ground
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/ground-test.sh aarch64 $(KERNEL_A64_IMAGE) $(GROUND_A64_LOG).disk $(GROUND_A64_LOG) $(C_A64_BIN)/ground
	@echo "aarch64-ground-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 ground boot is not run"
endif

# zlib 1.3.2 (ports/zlib, journal entry 100): the compression library
# rustc and much else is built with - here as build/c/libz.a, for each
# CPU, compiled as the C library's own C is. The library's
# configure would set Z_HAVE_UNISTD_H; it is said here instead, and a
# program using zlib.h is compiled with the same (ZLIB_CFLAGS), so that
# both agree on what zconf.h declares.
ZLIB_PARTS  = adler32 compress crc32 deflate gzclose gzlib gzread gzwrite infback inffast inflate inftrees trees uncompr zutil
ZLIB_CFLAGS = -DZ_HAVE_UNISTD_H -I ports/zlib
zlib64: libc64
ifeq ($(HAVE_CC),yes)
	@mkdir -p $(C_BIN)/zlib
	@for part in $(ZLIB_PARTS); do \
	    [ $(C_BIN)/zlib/$$part.o -nt ports/zlib/$$part.c ] && continue; \
	    echo "  CC      ports/zlib/$$part.c"; \
	    clang $(TAAJ_CFLAGS) $(ZLIB_CFLAGS) -c ports/zlib/$$part.c -o $(C_BIN)/zlib/$$part.o || exit 1; \
	done
	@rm -f $(C_BIN)/libz.a && $(LLVM_AR) rcs $(C_BIN)/libz.a $(addprefix $(C_BIN)/zlib/,$(addsuffix .o,$(ZLIB_PARTS)))
endif

zlib-aarch64: user-aarch64
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(C_A64_BIN)/zlib
	@for part in $(ZLIB_PARTS); do \
	    [ $(C_A64_BIN)/zlib/$$part.o -nt ports/zlib/$$part.c ] && continue; \
	    echo "  CC      ports/zlib/$$part.c (AArch64)"; \
	    clang $(TAAJ_A64_CFLAGS) $(ZLIB_CFLAGS) -c ports/zlib/$$part.c -o $(C_A64_BIN)/zlib/$$part.o || exit 1; \
	done
	@rm -f $(C_A64_BIN)/libz.a && $(LLVM_AR) rcs $(C_A64_BIN)/libz.a $(addprefix $(C_A64_BIN)/zlib/,$(addsuffix .o,$(ZLIB_PARTS)))
endif

# Std's ground, part 2 (The-Jat's step C, journal entry 100): the C
# library calls std and rustc ask for - poll and select, dl_iterate_phdr
# and getauxval, dlfcn's stubs, the pthread attributes with
# pthread_getattr_np, the exec family with a PATH search, descriptors 3
# and up through spawn and exec, argv[0] - and zlib. ground2.c checks
# them all in one boot.
GROUND2_LOG     = $(BUILD_DIR)/ground2-64.log
GROUND2_A64_LOG = $(BUILD_DIR)/aarch64/ground2.log
GROUND2_C       = $(KERNEL64_DIR)/scripts/ground2.c
ground2-64-test: iso64 user64 libc64 zlib64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) $(ZLIB_CFLAGS) -c $(GROUND2_C) -o $(C_BIN)/ground2.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/ground2.o $(C_BIN)/libz.a $(LIBC_A) -o $(C_BIN)/ground2
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/ground2-test.sh x86_64 $(ISO64_IMG) $(GROUND2_LOG).disk $(GROUND2_LOG) $(C_BIN)/ground2
	@echo "ground2-64-test passed"
else
	@echo "FAIL: the ground2 test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-ground2-test: kernel-aarch64 user-aarch64 zlib-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) $(ZLIB_CFLAGS) -c $(GROUND2_C) -o $(C_A64_BIN)/ground2.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/ground2.o $(C_A64_BIN)/libz.a $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/ground2
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/ground2-test.sh aarch64 $(KERNEL_A64_IMAGE) $(GROUND2_A64_LOG).disk $(GROUND2_A64_LOG) $(C_A64_BIN)/ground2
	@echo "aarch64-ground2-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 ground2 boot is not run"
endif

# Open files as handles (J60, J61, journal entry 115): open gives a
# channel to the file itself, so a descriptor follows its file through a
# rename, keeps it after a removal or a replacing rename, and is handed
# to a child as the file. handles.c checks it in one boot.
HANDLES_LOG     = $(BUILD_DIR)/handles-64.log
HANDLES_A64_LOG = $(BUILD_DIR)/aarch64/handles.log
HANDLES_C       = $(KERNEL64_DIR)/scripts/handles.c
# A namespace per process (J62, journal entry 118): taaj_mount, and the
# mounts handed to a child. namespace.c runs in the same boot.
NAMESPACE_C     = $(KERNEL64_DIR)/scripts/namespace.c
# Names from a directory descriptor (J63, journal entry 120): the *at
# calls, and a child in a tree of its own. at.c runs in the same boot.
AT_C            = $(KERNEL64_DIR)/scripts/at.c
# /tmp in memory (J64, journal entry 122): tmp.c, as the program
# tmptest (a program called tmp would be under the /tmp mount).
TMP_C           = $(KERNEL64_DIR)/scripts/tmp.c
handles64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(HANDLES_C) -o $(C_BIN)/handles.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/handles.o $(LIBC_A) -o $(C_BIN)/handles
	@clang $(TAAJ_CFLAGS) -c $(NAMESPACE_C) -o $(C_BIN)/namespace.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/namespace.o $(LIBC_A) -o $(C_BIN)/namespace
	@clang $(TAAJ_CFLAGS) -c $(AT_C) -o $(C_BIN)/at.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/at.o $(LIBC_A) -o $(C_BIN)/at
	@clang $(TAAJ_CFLAGS) -c $(TMP_C) -o $(C_BIN)/tmptest.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/tmptest.o $(LIBC_A) -o $(C_BIN)/tmptest
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/handles-test.sh x86_64 $(ISO64_IMG) $(HANDLES_LOG).disk $(HANDLES_LOG) $(C_BIN)/handles $(C_BIN)/namespace $(C_BIN)/at $(C_BIN)/tmptest
	@echo "handles64-test passed"
else
	@echo "FAIL: the handles test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-handles-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(HANDLES_C) -o $(C_A64_BIN)/handles.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/handles.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/handles
	@clang $(TAAJ_A64_CFLAGS) -c $(NAMESPACE_C) -o $(C_A64_BIN)/namespace.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/namespace.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/namespace
	@clang $(TAAJ_A64_CFLAGS) -c $(AT_C) -o $(C_A64_BIN)/at.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/at.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/at
	@clang $(TAAJ_A64_CFLAGS) -c $(TMP_C) -o $(C_A64_BIN)/tmptest.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/tmptest.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/tmptest
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/handles-test.sh aarch64 $(KERNEL_A64_IMAGE) $(HANDLES_A64_LOG).disk $(HANDLES_A64_LOG) $(C_A64_BIN)/handles $(C_A64_BIN)/namespace $(C_A64_BIN)/at $(C_A64_BIN)/tmptest
	@echo "aarch64-handles-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 handles boot is not run"
endif

# TaajFS version 6 (The-Jat's step D, journal entry 101): file times,
# a volume and a file over 256 MiB, a rename that replaces, programs in
# /bin. times.c checks them in one boot on a 320 MiB disk; the host's
# tool then reads what TheTaaJ wrote and writes a file of its own.
TIMES_LOG     = $(BUILD_DIR)/times-64.log
TIMES_A64_LOG = $(BUILD_DIR)/aarch64/times.log
TIMES_C       = $(KERNEL64_DIR)/scripts/times.c
times64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(TIMES_C) -o $(C_BIN)/times.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/times.o $(LIBC_A) -o $(C_BIN)/times
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/times-test.sh x86_64 $(ISO64_IMG) $(TIMES_LOG).disk $(TIMES_LOG) $(C_BIN)/times $(USER_BIN)/ls
	@echo "times64-test passed"
else
	@echo "FAIL: the times test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-times-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(TIMES_C) -o $(C_A64_BIN)/times.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/times.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/times
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/times-test.sh aarch64 $(KERNEL_A64_IMAGE) $(TIMES_A64_LOG).disk $(TIMES_A64_LOG) $(C_A64_BIN)/times user/target/$(KERNEL_A64_TARGET)/release/ls
	@echo "aarch64-times-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 times boot is not run"
endif

# Memory for big programs, part 1 (The-Jat's step G, journal entry
# 102): mprotect and PROT_NONE really changing what pages allow,
# madvise(MADV_DONTNEED) and munmap of part of a mapping giving memory
# back, the heap giving freed memory back, and system calls racing a
# thread that takes their buffer away. pages.c checks them in one boot.
PAGES_LOG     = $(BUILD_DIR)/pages-64.log
PAGES_A64_LOG = $(BUILD_DIR)/aarch64/pages.log
PAGES_C       = $(KERNEL64_DIR)/scripts/pages.c
pages64-test: iso64 user64 libc64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@clang $(TAAJ_CFLAGS) -c $(PAGES_C) -o $(C_BIN)/pages.o && ld.lld $(TAAJ_LDFLAGS) $(C_BIN)/pages.o $(LIBC_A) -o $(C_BIN)/pages
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/pages-test.sh x86_64 $(ISO64_IMG) $(PAGES_LOG).disk $(PAGES_LOG) $(C_BIN)/pages
	@echo "pages64-test passed"
else
	@echo "FAIL: the pages test needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-pages-test: kernel-aarch64 user-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@clang $(TAAJ_A64_CFLAGS) -c $(PAGES_C) -o $(C_A64_BIN)/pages.o && ld.lld $(TAAJ_A64_LDFLAGS) $(C_A64_BIN)/pages.o $(C_A64_BIN)/libc.a -o $(C_A64_BIN)/pages
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/pages-test.sh aarch64 $(KERNEL_A64_IMAGE) $(PAGES_A64_LOG).disk $(PAGES_A64_LOG) $(C_A64_BIN)/pages
	@echo "aarch64-pages-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 pages boot is not run"
endif

aarch64-tcc-test: kernel-aarch64 user-aarch64 tcc-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@TFSTOOL=$(TFSTOOL_RS) EDIT=$(A64_USER_BIN)/edit $(KERNEL64_DIR)/scripts/tcc-test.sh aarch64 $(KERNEL_A64_IMAGE) $(TCC_A64_LOG).disk $(TCC_A64_LOG) $(TCC_A64_DIR) $(C_A64_BIN)/libc.a
	@echo "aarch64-tcc-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 tcc boot is not run"
endif

# C++ (journal entry 072): LLVM's runtimes - libunwind, libc++abi and
# libc++, in ports/llvm - cross-compiled for each CPU into one libc++.a
# by tools/build-cxx.sh, which says what is in it and what is left out;
# and the C++ programs in libc/programs, each one .cpp file, linked with
# libc++.a and libc.a by user/cxx.ld (user.ld, keeping the tables a
# throw needs to unwind the stack). libc++.a is rebuilt only when
# something it is made of changes: it takes a few minutes.
#
# To build one of your own: make cxx64 CXX_PROGRAMS=name (with
# libc/programs/name.cpp); it is then build/cxx/x86_64/bin/name.
CXX_X86_DIR   = $(BUILD_DIR)/cxx/x86_64
CXX_A64_DIR   = $(BUILD_DIR)/cxx/aarch64
CXX_PROGRAMS  = cxxhello cxxlib
CXX64_LOG     = $(BUILD_DIR)/cxx64.log
CXX_A64_LOG   = $(BUILD_DIR)/aarch64/cxx.log
CXX_SOURCES  := $(shell find ports/llvm/libcxx ports/llvm/libcxxabi ports/llvm/libunwind $(LIBC_DIR)/include -type f 2>/dev/null) \
                ports/llvm/taaj/__config_site tools/build-cxx.sh
# Not freestanding, as the C programs are: C++'s library is the hosted
# one. __TAAJ__ says which system it is for (libc++'s locale choice, its
# clock), as tcc on TheTaaJ says it of itself.
TAAJ_CXX_BASE     = --target=x86_64-unknown-none-elf -std=c++20 -O2 -fno-stack-protector -fno-pic -mno-red-zone \
                    -nostdlibinc -D__TAAJ__ -femulated-tls -ffunction-sections -fdata-sections
TAAJ_A64_CXX_BASE = --target=aarch64-unknown-none-elf -std=c++20 -O2 -fno-stack-protector -fno-pic \
                    -nostdlibinc -D__TAAJ__ -femulated-tls -ffunction-sections -fdata-sections
# C++'s headers first: libc++'s <stdlib.h> wraps the C library's.
TAAJ_CXXFLAGS     = $(TAAJ_CXX_BASE) -Wall -Wextra -Werror -nostdinc++ -isystem $(CXX_X86_DIR)/include/c++/v1 \
                    -isystem $(CXX_X86_DIR)/include -isystem $(LIBC_DIR)/include
TAAJ_A64_CXXFLAGS = $(TAAJ_A64_CXX_BASE) -Wall -Wextra -Werror -nostdinc++ -isystem $(CXX_A64_DIR)/include/c++/v1 \
                    -isystem $(CXX_A64_DIR)/include -isystem $(LIBC_DIR)/include
TAAJ_CXX_LDFLAGS     = -static -nostdlib --gc-sections -s --eh-frame-hdr -T user/cxx.ld
TAAJ_A64_CXX_LDFLAGS = $(TAAJ_CXX_LDFLAGS) -z max-page-size=4096

$(CXX_X86_DIR)/libc++.a: $(CXX_SOURCES)
	@LLVM_AR=$(LLVM_AR) tools/build-cxx.sh x86_64 $(CXX_X86_DIR) "$(TAAJ_CXX_BASE)"

$(CXX_A64_DIR)/libc++.a: $(CXX_SOURCES)
	@LLVM_AR=$(LLVM_AR) tools/build-cxx.sh aarch64 $(CXX_A64_DIR) "$(TAAJ_A64_CXX_BASE)"

cxx64: libc64
ifeq ($(HAVE_CC),yes)
	@$(MAKE) --no-print-directory $(CXX_X86_DIR)/libc++.a
	@mkdir -p $(CXX_X86_DIR)/bin
	@for program in $(CXX_PROGRAMS); do \
	    echo "  CXX     $$program.cpp"; \
	    clang $(TAAJ_CXXFLAGS) -c $(LIBC_DIR)/programs/$$program.cpp -o $(CXX_X86_DIR)/bin/$$program.o || exit 1; \
	    ld.lld $(TAAJ_CXX_LDFLAGS) $(CXX_X86_DIR)/bin/$$program.o $(CXX_X86_DIR)/libc++.a $(LIBC_A) \
	        -o $(CXX_X86_DIR)/bin/$$program || exit 1; \
	done
else
	@echo "Clang or lld not found: no C++ (sudo apt install clang lld)"
endif

cxx-aarch64: cprogs-aarch64
ifeq ($(HAVE_CC),yes)
	@$(MAKE) --no-print-directory $(CXX_A64_DIR)/libc++.a
	@mkdir -p $(CXX_A64_DIR)/bin
	@for program in $(CXX_PROGRAMS); do \
	    echo "  CXX     $$program.cpp (AArch64)"; \
	    clang $(TAAJ_A64_CXXFLAGS) -c $(LIBC_DIR)/programs/$$program.cpp -o $(CXX_A64_DIR)/bin/$$program.o || exit 1; \
	    ld.lld $(TAAJ_A64_CXX_LDFLAGS) $(CXX_A64_DIR)/bin/$$program.o $(CXX_A64_DIR)/libc++.a $(C_A64_BIN)/libc.a \
	        -o $(CXX_A64_DIR)/bin/$$program || exit 1; \
	done
else
	@echo "Clang or lld not found: no C++ for AArch64 (sudo apt install clang lld)"
endif

# The C++ programs on TheTaaJ: cxxlib's checks of the language's runtime
# and the library, an exception nobody catches, cxxhello.
cxx64-test: iso64 cxx64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/cxx-test.sh x86_64 $(ISO64_IMG) $(CXX64_LOG).disk $(CXX64_LOG) $(CXX_X86_DIR)/bin
	@echo "cxx64-test passed"
else
	@echo "FAIL: C++ needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-cxx-test: kernel-aarch64 cxx-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@TFSTOOL=$(TFSTOOL_RS) $(KERNEL64_DIR)/scripts/cxx-test.sh aarch64 $(KERNEL_A64_IMAGE) $(CXX_A64_LOG).disk $(CXX_A64_LOG) $(CXX_A64_DIR)/bin
	@echo "aarch64-cxx-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 C++ boot is not run"
endif

# LLVM (journal entry 073): LLVM itself, in ports/llvm/llvm, cross-
# compiled for TheTaaJ by tools/build-llvm.py, which does what LLVM's
# CMake build would for this configuration: the configured headers
# (values in ports/llvm/taaj/llvm-config.txt), TableGen with the host's
# llvm-tblgen of the same release, the libraries and the tools. Only
# what changed is built again; the first build compiles some 230 files.
#
# C++17, LLVM's own standard; without exceptions or RTTI, as LLVM is
# built everywhere; and -w, since the warnings are upstream's to mind.
LLVM_X86_DIR  = $(BUILD_DIR)/llvm/x86_64
LLVM_A64_DIR  = $(BUILD_DIR)/llvm/aarch64
LLVM_TOOLS    = llvm-tblgen llvm-mc llc clang lld
# Clang's tables are written by clang-tblgen, which Ubuntu does not
# package: it is built for the host from ports/llvm/clang
# (tools/build-clang-tblgen.sh), against the host's LLVM 18.1.3.
CLANG_TBLGEN  = $(BUILD_DIR)/clang-tblgen/clang-tblgen
LLVM_TBLGEN  := $(shell command -v llvm-tblgen || ls /usr/bin/llvm-tblgen-* /usr/lib/llvm-*/bin/llvm-tblgen 2>/dev/null | sort -V | tail -1)
LLVM64_LOG    = $(BUILD_DIR)/llvm64.log
LLVM_A64_LOG  = $(BUILD_DIR)/aarch64/llvm.log
TAAJ_LLVM_CXXFLAGS     = $(TAAJ_CXX_BASE) -std=c++17 -fno-rtti -fno-exceptions -w -nostdinc++ \
                         -isystem $(CXX_X86_DIR)/include/c++/v1 -isystem $(CXX_X86_DIR)/include -isystem $(LIBC_DIR)/include
TAAJ_A64_LLVM_CXXFLAGS = $(TAAJ_A64_CXX_BASE) -std=c++17 -fno-rtti -fno-exceptions -w -nostdinc++ \
                         -isystem $(CXX_A64_DIR)/include/c++/v1 -isystem $(CXX_A64_DIR)/include -isystem $(LIBC_DIR)/include
TAAJ_LLVM_CFLAGS       = --target=x86_64-unknown-none-elf -std=c11 -O2 -fno-stack-protector -fno-pic -mno-red-zone \
                         -nostdlibinc -isystem $(LIBC_DIR)/include -w
TAAJ_A64_LLVM_CFLAGS   = --target=aarch64-unknown-none-elf -std=c11 -O2 -fno-stack-protector -fno-pic \
                         -nostdlibinc -isystem $(LIBC_DIR)/include -w

clang-tblgen:
	@sh tools/build-clang-tblgen.sh $(dir $(CLANG_TBLGEN))

llvm64: cxx64 clang-tblgen
ifeq ($(HAVE_CC),yes)
	@[ -n "$(LLVM_TBLGEN)" ] || { echo "llvm-tblgen not found (sudo apt install llvm-18)"; exit 1; }
	@LLVM_TBLGEN=$(LLVM_TBLGEN) CLANG_TBLGEN=$(abspath $(CLANG_TBLGEN)) LLVM_AR=$(LLVM_AR) \
	    TAAJ_LLVM_LINK="ld.lld $(TAAJ_CXX_LDFLAGS) {objects} $(CXX_X86_DIR)/libc++.a $(LIBC_A) -o {output}" \
	    python3 tools/build-llvm.py x86_64 $(LLVM_X86_DIR) "$(TAAJ_LLVM_CXXFLAGS)" "$(TAAJ_LLVM_CFLAGS)" $(LLVM_TOOLS)
else
	@echo "Clang or lld not found: no LLVM (sudo apt install clang lld)"
endif

llvm-aarch64: cxx-aarch64 clang-tblgen
ifeq ($(HAVE_CC),yes)
	@[ -n "$(LLVM_TBLGEN)" ] || { echo "llvm-tblgen not found (sudo apt install llvm-18)"; exit 1; }
	@LLVM_TBLGEN=$(LLVM_TBLGEN) CLANG_TBLGEN=$(abspath $(CLANG_TBLGEN)) LLVM_AR=$(LLVM_AR) \
	    TAAJ_LLVM_LINK="ld.lld $(TAAJ_A64_CXX_LDFLAGS) {objects} $(CXX_A64_DIR)/libc++.a $(C_A64_BIN)/libc.a -o {output}" \
	    python3 tools/build-llvm.py aarch64 $(LLVM_A64_DIR) "$(TAAJ_A64_LLVM_CXXFLAGS)" "$(TAAJ_A64_LLVM_CFLAGS)" $(LLVM_TOOLS)
else
	@echo "Clang or lld not found: no LLVM for AArch64 (sudo apt install clang lld)"
endif

# LLVM's tools - llvm-tblgen, llc, llvm-mc, clang, ld.lld - and work for
# them, on a run disk of their own, to try by hand: they take 177 MB, ten
# times the usual run disk. The disk is the usual one in every other way
# (it is made by run-disk), and big: 65536 blocks, 256 MiB (one made
# smaller, before E4, is made again: a link does not fit). Boot it with
#   make run64 RUN_DISK=build/llvm-disk.img
#   make run-aarch64 RUN_A64_DISK=build/aarch64/llvm-disk.img
# Not part of run-disk: the first LLVM build takes many minutes.
LLVM_DISK     = $(BUILD_DIR)/llvm-disk.img
LLVM_A64_DISK = $(BUILD_DIR)/aarch64/llvm-disk.img
llvm-put: llvm64 cprogs64
	@[ ! -f $(LLVM_DISK) ] || [ $$(stat -c %s $(LLVM_DISK)) -ge $$((65536 * 4096)) ] \
	    || { rm -f $(LLVM_DISK) && echo "$(LLVM_DISK) was smaller than the 256 MiB the tools need: making it again"; }
	@$(MAKE) --no-print-directory run-disk RUN_DISK=$(LLVM_DISK) RUN_DISK_BLOCKS=65536
	@TFSTOOL=$(TFSTOOL_RS) LLVM_TBLGEN=$(LLVM_TBLGEN) sh tools/put-llvm.sh $(LLVM_DISK) $(LLVM_X86_DIR)/bin $(C_BIN)

llvm-put-aarch64: llvm-aarch64 cprogs-aarch64
	@[ ! -f $(LLVM_A64_DISK) ] || [ $$(stat -c %s $(LLVM_A64_DISK)) -ge $$((65536 * 4096)) ] \
	    || { rm -f $(LLVM_A64_DISK) && echo "$(LLVM_A64_DISK) was smaller than the 256 MiB the tools need: making it again"; }
	@$(MAKE) --no-print-directory run-aarch64-disk RUN_A64_DISK=$(LLVM_A64_DISK) RUN_DISK_BLOCKS=65536
	@TFSTOOL=$(TFSTOOL_RS) LLVM_TBLGEN=$(LLVM_TBLGEN) sh tools/put-llvm.sh $(LLVM_A64_DISK) $(LLVM_A64_DIR)/bin $(C_A64_BIN)

# llvm-tblgen on TheTaaJ, writing what LLVM's own build writes first -
# the value types and the intrinsics - compared byte for byte with the
# host's llvm-tblgen.
llvm64-test: iso64 llvm64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) LLVM_TBLGEN=$(LLVM_TBLGEN) $(KERNEL64_DIR)/scripts/llvm-test.sh x86_64 $(ISO64_IMG) $(LLVM64_LOG).disk $(LLVM64_LOG) $(LLVM_X86_DIR)/bin
	@echo "llvm64-test passed"
else
	@echo "FAIL: LLVM needs clang and lld (sudo apt install clang lld)"; exit 1
endif

aarch64-llvm-test: kernel-aarch64 llvm-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@TFSTOOL=$(TFSTOOL_RS) LLVM_TBLGEN=$(LLVM_TBLGEN) $(KERNEL64_DIR)/scripts/llvm-test.sh aarch64 $(KERNEL_A64_IMAGE) $(LLVM_A64_LOG).disk $(LLVM_A64_LOG) $(LLVM_A64_DIR)/bin
	@echo "aarch64-llvm-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 LLVM boot is not run"
endif

# clang on TheTaaJ (journal entry 075): it compiles TheTaaJ's eleven C
# programs, with the options the build gives them, and writes the same
# objects, assembly and preprocessed C as the host's clang 18.1.3. And
# lld (entry 076) links them there into the very programs the build
# links here (C_BIN), one of which then runs.
CLANG64_LOG   = $(BUILD_DIR)/clang64.log
CLANG_A64_LOG = $(BUILD_DIR)/aarch64/clang.log
clang64-test: iso64 llvm64 cprogs64 tfstool-rs
ifeq ($(HAVE_CC),yes)
	@TFSTOOL=$(TFSTOOL_RS) LLVM_TBLGEN=$(LLVM_TBLGEN) $(KERNEL64_DIR)/scripts/clang-test.sh x86_64 $(ISO64_IMG) $(CLANG64_LOG).disk $(CLANG64_LOG) $(LLVM_X86_DIR)/bin $(C_BIN)
	@echo "clang64-test passed"
else
	@echo "FAIL: clang needs clang and lld on the host (sudo apt install clang lld)"; exit 1
endif

aarch64-clang-test: kernel-aarch64 llvm-aarch64 cprogs-aarch64 tfstool-rs
ifeq ($(HAVE_A64)$(HAVE_CC),yesyes)
	@mkdir -p $(BUILD_DIR)/aarch64
	@TFSTOOL=$(TFSTOOL_RS) LLVM_TBLGEN=$(LLVM_TBLGEN) $(KERNEL64_DIR)/scripts/clang-test.sh aarch64 $(KERNEL_A64_IMAGE) $(CLANG_A64_LOG).disk $(CLANG_A64_LOG) $(LLVM_A64_DIR)/bin $(C_A64_BIN)
	@echo "aarch64-clang-test passed"
else
	@echo "qemu-system-aarch64, llvm-objcopy, clang or lld not found: the AArch64 clang boot is not run"
endif

# Every layer: host unit tests, a normal boot (BIOS and UEFI) with its
# self-tests, the fault boots, and the AArch64 kernel.
#
# In two steps. First everything the tests share is built, one thing
# after another (two builds of one crate at once only wait for each
# other, and an error is easier to read alone). Then the tests (TEST_JOBS
# of them) run side by side: each writes its own logs and builds whatever only
# it needs in a directory of its own. What keeps that honest is not
# TEST_JOBS but the boot slots (kernel/kernel64/scripts/boot-slots.sh):
# however many tests are running, no more emulated CPUs run at once
# than the host has CPUs for - the boots are timed, and an emulator
# short of CPU fails them for no fault of the kernel's.
#
# The parts are listed longest first - make starts them in this order -
# so the boots that take minutes (the typed sessions, clang) are not
# left to begin when the short ones are done (journal entry 080).
#
#   make test64 TEST_JOBS=1         one test after another, as before
#   TAAJ_BOOT_SLOTS=4 make test64   as if the host had 5 CPUs
TEST_JOBS ?= 12
TEST64_PARTS = \
	boot64-test \
	boot64-uefi-test \
	ahci64-test \
	nvme64-test \
	power64-test \
	aarch64-power-test \
	aarch64-keys-test \
	clang64-test \
	aarch64-clang-test \
	llvm64-test \
	aarch64-llvm-test \
	aarch64-tcc-test \
	tcc64-test \
	make64-test \
	aarch64-make-test \
	libmake64-test \
	aarch64-libmake-test \
	kernel64-test \
	unsafe-check \
	smp64-test \
	big64-test \
	fault64-test \
	aarch64-test \
	aarch64-uefi-test \
	net64-test \
	e1000-64-test \
	usb64-test \
	stick64-test \
	aarch64-net-test \
	ip64-test \
	aarch64-ip-test \
	tcp64-test \
	aarch64-tcp-test \
	http64-test \
	aarch64-http-test \
	screen64-test \
	aarch64-screen-test \
	gui64-test \
	aarch64-gui-test \
	edit64-test \
	aarch64-edit-test \
	cxx64-test \
	aarch64-cxx-test \
	pipe64-test \
	aarch64-pipe-test \
	thread64-test \
	schedbench64-test \
	aarch64-thread-test \
	bigmem64-test \
	aarch64-bigmem-test \
	basics64-test \
	aarch64-basics-test \
	ground64-test \
	aarch64-ground-test \
	ground2-64-test \
	aarch64-ground2-test \
	handles64-test \
	aarch64-handles-test \
	times64-test \
	aarch64-times-test \
	pages64-test \
	aarch64-pages-test

test64:
	@$(MAKE) --no-print-directory test64-shared
	@$(MAKE) --no-print-directory smp64-test
	@$(MAKE) --no-print-directory -j$(TEST_JOBS) --output-sync=target $(addprefix -o ,$(TEST64_SHARED) smp64-test) test64-parts
	@echo "test64: all passed"

# What test64-shared builds is then not built again: the second step is
# told it is up to date (make -o). Most of it is .PHONY, so make would
# otherwise run it again before each part that names it - relinking
# LLVM's tools every time - and a part waiting for that is not started:
# make fills its TEST_JOBS with the parts that are ready, and the long
# AArch64 parts, needing llvm-aarch64, began last (journal entry 117).
# A part run alone (make clang64-test) still builds what it needs.
#
# smp64-test runs alone before the others, about 17 s. Its 20-CPU boot
# needs 20 boot slots at once, and the typed boots wait for it (entry
# 108); with every part ready at once, it found the slots taken and the
# typed boots started 3 minutes late (journal entry 117).
TEST64_SHARED = build-bootloader user64 iso64 cprogs64 tfstool-rs uefi-loader user-aarch64 cprogs-aarch64 kernel-aarch64 aarch64-esp tcc64 tcc-aarch64 cxx64 cxx-aarch64 llvm64 llvm-aarch64 pdpmake64 pdpmake-aarch64

test64-shared: $(TEST64_SHARED)

test64-parts: $(TEST64_PARTS)

# The list of parts, for tools/race-hunt.sh (TEST64_PARTS is one part per
# line, so a script can't read it with sed).
test64-list:
	@echo $(TEST64_PARTS)

# Clean everything
clean:
	@echo "Cleaning bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR) clean
	@echo "Cleaning kernel..."
	$(MAKE) -C $(KERNEL_DIR) clean
	@echo "Cleaning build directory..."
	rm -rf $(BUILD_DIR)
	rm -rf $(ISO_DIR)