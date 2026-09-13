# servers/Makefile - ring 3 servers and clients.
#
# Each is linked with `ld -r` against librt, producing a single
# relocatable object with no undefined symbols. That is what the kernel's
# user-module loader requires: ring 3 cannot call a ring-0 address, so a
# module referencing a kernel symbol is refused at load time.

ROOT_DIR ?= $(abspath ..)
BUILD_DIR = $(ROOT_DIR)/build/modules
LIBRT     = $(ROOT_DIR)/build/librt/syscall.o $(ROOT_DIR)/build/librt/rpc.o
SHARED    = $(ROOT_DIR)/shared

TOOLCHAIN_DIR ?= $(ROOT_DIR)/toolchain/i686
export PATH := $(abspath $(TOOLCHAIN_DIR))/bin:$(PATH)

CC = i686-elf-gcc
LD = i686-elf-ld
CFLAGS = -m32 -fno-pie -ffreestanding -nostdlib -O2 -Wall -Wextra \
         -fno-builtin -fno-asynchronous-unwind-tables \
         -I $(ROOT_DIR)/librt/include -I $(SHARED)

SERVERS = echo talker mathsrv mathcli
TARGETS = $(SERVERS:%=$(BUILD_DIR)/%.mod)

all: $(BUILD_DIR) $(TARGETS)

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/%.mod: %/%.c
	@true

define SERVER_RULE
$(BUILD_DIR)/$(1).mod: $(1)/$(1).c $(LIBRT) | $(BUILD_DIR)
	@echo "Building server $(1)..."
	$(CC) $(CFLAGS) -c $(1)/$(1).c -o $(BUILD_DIR)/$(1).tmp.o
	$(LD) -m elf_i386 -r -o $$@ $(BUILD_DIR)/$(1).tmp.o $(LIBRT)
	rm -f $(BUILD_DIR)/$(1).tmp.o
endef
$(foreach s,$(SERVERS),$(eval $(call SERVER_RULE,$(s))))

clean:
	rm -f $(TARGETS)

.PHONY: all clean
