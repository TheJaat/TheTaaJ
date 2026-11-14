# Define subdirectories
BOOTLOADER_DIR = bootloader
KERNEL_DIR = kernel

.PHONY: all clean run iso ramdisk modules servers librt disk tfstool tfsdisk tfstool-rs tfscheck \
        kernel64 iso64 run64 boot64-test kernel64-test fault64-test test64 \
        uefi-loader run64-uefi boot64-uefi-test user64

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

KERNEL64_DIR = kernel64

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
TEST_KEYS    = f12 / shift-h i spc t h e r x backspace e ret \
               p s ret m e m ret r u n spc h e l l o ret r u n spc p e e k ret \
               w r i t e spc n o t e s spc h e l l o ret \
               a p p e n d spc n o t e s spc w o r l d ret \
               c a t spc n o t e s ret l s ret \
               r m spc n o t e s ret c a t spc n o t e s ret
TEST_TYPED   = Hi there
# What those commands must print (grep patterns, separated by ";").
# (Joined on one line: a continued line would start each pattern with a
# space.)
EXPECT_SHELL = unknown command: Hi;  PID  NAME;init +running;^memory: [0-9]+ MiB used;^hello exited with code 42;^peek was killed
EXPECT_FILES = \[fs\] ready;^notes: 6 bytes;^notes: 12 bytes;^hello\s*$$;^world\s*$$;^ +12  notes;^1 file, 12 bytes;^cat: notes: no such file
TEST_EXPECT  = $(EXPECT_SHELL);$(EXPECT_FILES)

# The UEFI loader needs Clang and lld (`sudo apt install clang lld`).
# Without them the ISO is built BIOS-only, and says so.
UEFI_DIR     = bootloader/src/boot/uefi
UEFI_EFI     = $(BOOTLOADER_BUILD_DIR)/uefi/BOOTX64.EFI
ESP64_IMG    = $(ISO64_DIR)/efiboot.img
HAVE_CLANG  := $(shell command -v clang >/dev/null && command -v lld-link >/dev/null && echo yes)
MKFS_FAT     = /usr/sbin/mkfs.fat
# QEMU's UEFI firmware, from the ovmf package.
OVMF         = /usr/share/ovmf/OVMF.fd

# User programs come first: the kernel embeds them (kernel64/src/programs.rs).
user64:
	@echo "Building the user programs..."
	cd user && cargo build --release

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

run64: iso64
	qemu-system-x86_64 -m 512M -cdrom $(ISO64_IMG) -boot d -serial stdio

run64-uefi: iso64
ifneq ($(HAVE_CLANG),yes)
	@echo "ERROR: no UEFI loader in the ISO - it needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	qemu-system-x86_64 -m 512M -bios $(OVMF) -cdrom $(ISO64_IMG) -boot d -serial stdio

# Boots the 64-bit kernel headless and checks what it printed. A start on
# the "boot every architecture on every commit" rule from the roadmap.
boot64-test: iso64
	@echo "Booting the 64-bit kernel headless via BIOS (serial log: $(SERIAL64_LOG))..."
	@KEYS="$(TEST_KEYS)" $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(SERIAL64_LOG)
	@TYPED="$(TEST_TYPED)" EXPECT="$(TEST_EXPECT)" $(KERNEL64_DIR)/scripts/check-boot.sh $(SERIAL64_LOG) BIOS
	@echo "boot64-test passed"

# The same ISO, the same kernel, booted by UEFI firmware instead.
boot64-uefi-test: iso64
ifneq ($(HAVE_CLANG),yes)
	@echo "FAIL: the UEFI loader needs clang and lld (sudo apt install clang lld)"; exit 1
endif
	@echo "Booting the 64-bit kernel headless via UEFI (serial log: $(UEFI64_LOG))..."
	@QEMU_ARGS="-bios $(OVMF)" KEYS="$(TEST_KEYS)" $(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(UEFI64_LOG) 30
	@TYPED="$(TEST_TYPED)" EXPECT="$(TEST_EXPECT)" $(KERNEL64_DIR)/scripts/check-boot.sh $(UEFI64_LOG) UEFI
	@echo "boot64-uefi-test passed"

# Unit tests of the kernel's pure logic (frames, page tables, heap,
# locks), compiled for and run on this machine rather than in a VM.
HOST_TRIPLE = $(shell PATH="$(PATH)" rustc -vV | sed -n 's/^host: //p')

kernel64-test:
	cd $(KERNEL64_DIR) && cargo test --lib --target $(HOST_TRIPLE)
	cd input && cargo test
	cd console && cargo test
	cd heap && cargo test
	cd ramfs && cargo test

# One boot per deliberate fault: unmapped read, double fault, write to
# rodata, executing data, stack overflow. Each must be caught and named.
fault64-test: build-bootloader user64
	$(KERNEL64_DIR)/scripts/fault-test.sh $(STAGE1_BIN) $(STAGE2_BIN) $(BUILD_DIR)

# Every layer: host unit tests, a normal boot (BIOS and UEFI) with its
# self-tests, and the fault boots.
test64: kernel64-test boot64-test boot64-uefi-test fault64-test
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