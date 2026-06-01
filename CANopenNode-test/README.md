# CANopenNode-test

A small, self-contained CANopenNode-based SDO **server** used to drive
end-to-end tests of the [`canopen-sdo`](../) Rust client over a Linux
virtual CAN interface.

`canopend-sdo-test` is essentially
[CANopenLinux](https://github.com/CANopenNode/CANopenLinux)'s `canopend`
with a custom Object Dictionary that adds two manufacturer-specific
entries needed to exercise the segmented SDO transfers our client
supports:

| Index   | Sub | Type            | Access | Size  | Purpose                       |
| ------- | --- | --------------- | ------ | ----- | ----------------------------- |
| 0x2000  | 00  | VISIBLE_STRING  | ro     | 64 B  | Segmented upload target.      |
| 0x2001  | 00  | OCTET_STRING    | rw     | 256 B | Segmented download / readback. |

All standard DS301 entries (0x1000, 0x1014, 0x1017, 0x1018, 0x1019, 0x1200,
the PDO records, …) come straight from the upstream CANopenNode example
OD and are still available for expedited-transfer tests.

## Vendored upstream sources & licenses

To keep this repo build-self-contained without adding a git submodule,
the C code is vendored under [`upstream/`](upstream/):

- [`upstream/CANopenLinux/`](upstream/CANopenLinux/) — Linux driver +
  `CO_main_basic.c`. Upstream:
  <https://github.com/CANopenNode/CANopenLinux>.
  License: Apache-2.0
  (see [`upstream/CANopenLinux/LICENSE`](upstream/CANopenLinux/LICENSE)).
- [`upstream/CANopenLinux/CANopenNode/`](upstream/CANopenLinux/CANopenNode/)
  — the CANopen protocol stack itself. Upstream:
  <https://github.com/CANopenNode/CANopenNode>.
  License: Apache-2.0
  (see
  [`upstream/CANopenLinux/CANopenNode/LICENSE`](upstream/CANopenLinux/CANopenNode/LICENSE)).

Both upstream `LICENSE` and `README.md` files are preserved in place.
Only built artifacts (`*.o`, the compiled `canopend` binary, `.git/`,
the unrelated `cocomm/` tool) were stripped during vendoring; no source
file was modified.

To resync against the latest upstream:

```text
rm -rf upstream/CANopenLinux
cp -r /path/to/CANopenLinux upstream/CANopenLinux
find upstream/CANopenLinux -name '*.o' -delete
rm -rf upstream/CANopenLinux/.git upstream/CANopenLinux/.github
rm -rf upstream/CANopenLinux/cocomm upstream/CANopenLinux/canopend
# CANopenNode is already a sibling at upstream/CANopenLinux/CANopenNode.
```

## Prerequisites

- Linux with the `vcan` kernel module available (Debian/Ubuntu:
  `sudo apt install linux-modules-extra-$(uname -r)`).
- `build-essential` (gcc, make).
- Optional: `can-utils` for sniffing
  (`sudo apt install can-utils`, then `candump -td -a vcan0`).

## Build

```text
make
```

This compiles every needed `.c` from [`upstream/`](upstream/) into
[`build/`](.gitignore) and produces `./canopend-sdo-test`. Nothing
under `upstream/` is written to.

`make clean` removes both `./build/` and the binary.

If you'd rather build against an external CANopenLinux clone:

```text
make CANOPENLINUX=/path/to/CANopenLinux
```

## One-time vcan setup (needs sudo)

```text
./setup-vcan.sh
```

Brings up `vcan0`. Override with `IFACE=vcan1 ./setup-vcan.sh`.
Idempotent; safe to re-run.

If you'd rather do it by hand:

```text
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

## Run the server

```text
./run.sh
```

By default starts on `vcan0` as NodeID `0x10`. Override with
`IFACE=vcan1 NODE=0x42 ./run.sh`. Ctrl-C to stop.

You should see something like:

```text
canopend-sdo-test: starting
canopend-sdo-test: communication reset
canopend-sdo-test: running ...
```

In another shell you can sniff traffic:

```text
candump -td -a vcan0
```

## Drive it from the Rust client

With the server running on `vcan0`, in another shell:

```text
cd ..   # back to the canopen-sdo crate root
cargo run --example against_canopennode -- vcan0 0x10
```

This walks through every important SDO scenario (expedited up/down at
1, 2 and 4 bytes; segmented up against 0x2000; segmented round-trip
against 0x2001; server abort on a missing index; client timeout against
a bogus node) and prints `PASS` / `FAIL` per scenario, exiting non-zero
on the first failure.

## What's in here

```text
CANopenNode-test/
├── Makefile              # builds canopend-sdo-test from vendored sources
├── OD.h                  # patched copy of CANopenNode/example/OD.h with 0x2000/0x2001
├── OD.c                  # patched copy of CANopenNode/example/OD.c with 0x2000/0x2001
├── README.md             # this file
├── run.sh                # convenience wrapper: ./canopend-sdo-test vcan0 -i 0x10
├── setup-vcan.sh         # idempotent vcan0 bring-up (needs sudo)
├── .gitignore            # ignores ./build, ./canopend-sdo-test, *.o
├── build/                # gcc output (created by `make`, gitignored)
└── upstream/             # vendored upstream sources (Apache-2.0, see above)
    └── CANopenLinux/
        ├── LICENSE
        ├── CO_*.c, CO_*.h, CO_main_basic.c, …
        └── CANopenNode/
            ├── LICENSE
            ├── 301/, 303/, 304/, 305/, 309/, storage/, extra/
            └── CANopen.{c,h}
```
