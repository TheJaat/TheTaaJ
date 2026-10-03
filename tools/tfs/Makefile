# tools/tfs/Makefile - host tool, built with the system compiler.
#
# It shares tfscore.c with the OS so the format cannot drift, and it is
# built BEFORE anything can mount a volume: a filesystem of your own has
# no fsck, and the replacement is a second implementation you can run
# under a debugger.

ROOT_DIR ?= $(abspath ../..)
BUILD_DIR = $(ROOT_DIR)/build/tools
CORE_DIR  = $(ROOT_DIR)/servers/tfs
SHARED    = $(ROOT_DIR)/shared

HOST_CC = gcc
HOST_CFLAGS = -Wall -Wextra -O2 -I$(CORE_DIR) -I$(SHARED)

all: $(BUILD_DIR)/tfstool

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/tfstool: tfstool.c $(CORE_DIR)/tfscore.c | $(BUILD_DIR)
	@echo "Building tfstool..."
	$(HOST_CC) $(HOST_CFLAGS) -o $@ tfstool.c $(CORE_DIR)/tfscore.c

clean:
	rm -f $(BUILD_DIR)/tfstool

.PHONY: all clean
