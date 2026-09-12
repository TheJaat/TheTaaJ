# librt/Makefile - the user-space runtime.
#
# Built to a relocatable object, not an archive. User modules are linked
# with `ld -r`, which merges this in so the finished module has no
# undefined symbols - which is exactly what the kernel's loader requires
# of a ring-3 module.

ROOT_DIR ?= $(abspath ..)
BUILD_DIR = $(ROOT_DIR)/build/librt
SHARED    = $(ROOT_DIR)/shared

TOOLCHAIN_DIR ?= $(ROOT_DIR)/toolchain/i686
export PATH := $(abspath $(TOOLCHAIN_DIR))/bin:$(PATH)

CC = i686-elf-gcc
CFLAGS = -m32 -fno-pie -ffreestanding -nostdlib -O2 -Wall -Wextra \
         -fno-builtin -fno-asynchronous-unwind-tables \
         -I include -I $(SHARED)

OBJECTS = $(BUILD_DIR)/syscall.o

all: $(BUILD_DIR) $(OBJECTS)

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/%.o: libos/%.c | $(BUILD_DIR)
	@echo "Compiling librt/$<..."
	$(CC) $(CFLAGS) -c $< -o $@

clean:
	rm -f $(OBJECTS)

.PHONY: all clean
