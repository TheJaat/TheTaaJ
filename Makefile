# Define subdirectories
BOOTLOADER_DIR = bootloader

.PHONY: all clean

# Default target
all: build-bootloader


# Build the bootloader
build-bootloader:
	@echo "Building bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR)

# Clean the bootloader
clean:
	@echo "Cleaning bootloader..."
	$(MAKE) -C $(BOOTLOADER_DIR) clean
