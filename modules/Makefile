# modules/Makefile
ROOT_DIR ?= $(abspath ..)
SUBDIRS = hello user

all: $(SUBDIRS)

$(SUBDIRS):
	$(MAKE) -C $@ ROOT_DIR=$(ROOT_DIR)

clean:
	for dir in $(SUBDIRS); do $(MAKE) -C $$dir ROOT_DIR=$(ROOT_DIR) clean; done

.PHONY: all clean $(SUBDIRS)
