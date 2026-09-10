# Define subdirectories
BOOTLOADER_DIR = bootloader
KERNEL_DIR = kernel

.PHONY: all clean run iso ramdisk

# Default target
all: $(BUILD_DIR) build-bootloader build-kernel iso

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
RAMDISK_SRC = $(wildcard ramdisk-src/*)

# Depends on build-kernel rather than on $(RD_TOOL) directly: the tool
# does not exist until the kernel build has run, and there is no rule
# here that knows how to make it.
ramdisk: build-kernel
	@echo "Packing ramdisk..."
	mkdir -p $(BUILD_DIR)
	$(RD_TOOL) $(RAMDISK_IMG) $(RAMDISK_SRC)

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

# Run the bootloader in QEMU
run: iso
	@echo "Running the bootloader in QEMU..."
	qemu-system-x86_64 -m 512M -cdrom $(ISO_IMG)

# Clean everything
clean:
	@echo "Cleaning bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR) clean
	@echo "Cleaning kernel..."
	$(MAKE) -C $(KERNEL_DIR) clean
	@echo "Cleaning build directory..."
	rm -rf $(BUILD_DIR)
	rm -rf $(ISO_DIR)