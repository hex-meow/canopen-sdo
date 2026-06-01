# Makefile for canopend-sdo-test: a CANopenNode-based SDO server with a
# custom Object Dictionary containing 0x2000 (read-only VISIBLE_STRING)
# and 0x2001 (read/write OCTET_STRING) so segmented SDO uploads and
# downloads can be exercised end-to-end against canopen-sdo.
#
# The CANopenLinux + CANopenNode sources are vendored under ./upstream/
# (see ./README.md for upstream URLs, versions and licenses), so the
# build is fully self-contained: just `make`. Object files land under
# ./build/ and never touch the vendored tree.
#
# Override CANOPENLINUX if you'd rather build against a different clone:
#   make CANOPENLINUX=/path/to/CANopenLinux
#
#   make           # build ./canopend-sdo-test
#   make clean     # remove ./build/ and the binary

CANOPENLINUX ?= ./upstream/CANopenLinux

DRV_SRC     = $(CANOPENLINUX)
CANOPEN_SRC = $(CANOPENLINUX)/CANopenNode

BUILD_DIR   = build
LINK_TARGET = canopend-sdo-test

INCLUDE_DIRS = \
	-I$(DRV_SRC) \
	-I$(CANOPEN_SRC) \
	-I.

# Source basenames (all unique across CANopenLinux + CANopenNode).
# Resolved through VPATH below.
SOURCES = \
	CO_driver.c \
	CO_error.c \
	CO_epoll_interface.c \
	CO_storageLinux.c \
	CO_ODinterface.c \
	CO_NMT_Heartbeat.c \
	CO_HBconsumer.c \
	CO_Emergency.c \
	CO_SDOserver.c \
	CO_SDOclient.c \
	CO_TIME.c \
	CO_SYNC.c \
	CO_PDO.c \
	crc16-ccitt.c \
	CO_fifo.c \
	CO_LEDs.c \
	CO_GFC.c \
	CO_SRDO.c \
	CO_LSSslave.c \
	CO_LSSmaster.c \
	CO_gateway_ascii.c \
	CO_storage.c \
	CO_trace.c \
	CANopen.c \
	OD.c \
	CO_main_basic.c

OBJS = $(SOURCES:%.c=$(BUILD_DIR)/%.o)

# Tell make where to find each .c by basename.
vpath %.c . \
	$(DRV_SRC) \
	$(CANOPEN_SRC) \
	$(CANOPEN_SRC)/301 \
	$(CANOPEN_SRC)/303 \
	$(CANOPEN_SRC)/304 \
	$(CANOPEN_SRC)/305 \
	$(CANOPEN_SRC)/309 \
	$(CANOPEN_SRC)/storage \
	$(CANOPEN_SRC)/extra

CC ?= gcc
OPT  =
OPT += -g
OPT += -O2
OPT += -DCO_SINGLE_THREAD

CFLAGS  = -Wall $(OPT) $(INCLUDE_DIRS)
LDFLAGS = -g

.PHONY: all clean

all: $(LINK_TARGET)

clean:
	rm -rf $(BUILD_DIR) $(LINK_TARGET)

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/%.o: %.c | $(BUILD_DIR)
	$(CC) $(CFLAGS) -c $< -o $@

$(LINK_TARGET): $(OBJS)
	$(CC) $(LDFLAGS) $^ -o $@
