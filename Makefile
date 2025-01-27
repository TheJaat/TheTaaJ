# Define subdirectories
BOOTLOADER_DIR = bootloader
KERNEL_DIR = kernel

.PHONY: all clean

# Default target
all: build-bootloader build-kernel


# Build the bootloader
build-bootloader:
	@echo "Building bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR)

# Build the kernel
build-kernel:
	@echo "Building bootloader..."
	$(MAKE) -C $(KERNEL_DIR)

# Clean the bootloader
clean:
	@echo "Cleaning bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR) clean
	@echo "Cleaning kernel..."
	$(MAKE) -C $(KERNEL_DIR) clean
