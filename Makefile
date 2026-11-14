# Define subdirectories
BOOTLOADER_DIR = bootloader
KERNEL_DIR = kernel

.PHONY: all clean run iso ramdisk modules servers librt disk tfstool tfsdisk tfstool-rs tfscheck \
        kernel64 iso64 run64 boot64-test kernel64-test fault64-test test64

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

kernel64:
	@echo "Building the 64-bit kernel..."
	cd $(KERNEL64_DIR) && cargo build --release

# The ISO gets a copy without debug info: stage2 reads the whole file
# through a 1 MiB staging window, and the symbols stay in the original.
iso64: build-bootloader kernel64
	@echo "Creating the 64-bit ISO image..."
	rm -rf $(ISO64_DIR)
	mkdir -p $(ISO64_DIR)/kernel
	cp $(STAGE1_BIN) $(STAGE2_BIN) $(ISO64_DIR)/
	strip --strip-debug -o $(ISO64_DIR)/kernel/kernel.elf $(KERNEL64_ELF)
	xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot \
		-boot-load-size 4 -o $(ISO64_IMG) $(ISO64_DIR) 2>/dev/null

run64: iso64
	qemu-system-x86_64 -m 512M -cdrom $(ISO64_IMG) -boot d -serial stdio

# Boots the 64-bit kernel headless and checks what it printed. A start on
# the "boot every architecture on every commit" rule from the roadmap.
boot64-test: iso64
	@echo "Booting the 64-bit kernel headless (serial log: $(SERIAL64_LOG))..."
	@$(KERNEL64_DIR)/scripts/qemu-boot.sh $(ISO64_IMG) $(SERIAL64_LOG)
	@grep -q "Hello from TheTaaJ" $(SERIAL64_LOG) || { echo "FAIL: no greeting"; cat $(SERIAL64_LOG); exit 1; }
	@grep -q "resumed after the breakpoint" $(SERIAL64_LOG) || { echo "FAIL: exception path broken"; cat $(SERIAL64_LOG); exit 1; }
	@grep -q "\[selftest\] memory: all passed" $(SERIAL64_LOG) || { echo "FAIL: memory self-test"; cat $(SERIAL64_LOG); exit 1; }
	@grep -q "Nothing more to do yet" $(SERIAL64_LOG) || { echo "FAIL: did not finish"; cat $(SERIAL64_LOG); exit 1; }
	@! grep -qE "KERNEL PANIC|CPU EXCEPTION" $(SERIAL64_LOG) || { echo "FAIL: panic or exception"; cat $(SERIAL64_LOG); exit 1; }
	@cat $(SERIAL64_LOG)
	@echo "boot64-test passed"

# Unit tests of the kernel's pure logic (frames, page tables, heap,
# locks), compiled for and run on this machine rather than in a VM.
HOST_TRIPLE = $(shell PATH="$(PATH)" rustc -vV | sed -n 's/^host: //p')

kernel64-test:
	cd $(KERNEL64_DIR) && cargo test --lib --target $(HOST_TRIPLE)

# One boot per deliberate fault: unmapped read, double fault, write to
# rodata, executing data, stack overflow. Each must be caught and named.
fault64-test: build-bootloader
	$(KERNEL64_DIR)/scripts/fault-test.sh $(STAGE1_BIN) $(STAGE2_BIN) $(BUILD_DIR)

# All three layers: host unit tests, a normal boot with self-tests, and
# the fault boots.
test64: kernel64-test boot64-test fault64-test
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