# modules/user/Makefile
#
# A module is a relocatable object, not an executable - no linker script
# and no fixed load address. The kernel's module loader relocates it
# wherever it lands.
#
# -ffreestanding matters: the module has no libc. Without it gcc may
# rewrite printf("...\n") into puts(), which the kernel does not export,
# and the load would be refused for a symbol the source never mentions.

ROOT_DIR ?= $(abspath ../..)
BUILD_DIR = $(ROOT_DIR)/build/modules

# The cross compiler lives in the same place the kernel finds it.
#
# kernel/config.mk cannot simply be included here: its TOOLCHAIN_DIR is
# the relative path '../toolchain/i686', which $(abspath) resolves
# against whatever directory make is standing in - correct from kernel/,
# wrong from here. Anchor it to ROOT_DIR instead.
TOOLCHAIN_DIR ?= $(ROOT_DIR)/toolchain/i686
export PATH := $(abspath $(TOOLCHAIN_DIR))/bin:$(PATH)

CC = i686-elf-gcc

CFLAGS = -m32 -fno-pie -ffreestanding -nostdlib -O2 -Wall -Wextra \
         -fno-builtin -fno-asynchronous-unwind-tables

all: $(BUILD_DIR)/user.mod

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/user.mod: user.c | $(BUILD_DIR)
	@echo "Building module user..."
	$(CC) $(CFLAGS) -c user.c -o $@

clean:
	rm -f $(BUILD_DIR)/user.mod

.PHONY: all clean
