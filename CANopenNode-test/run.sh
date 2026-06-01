#!/usr/bin/env bash
#
# Run canopend-sdo-test in the foreground. Ctrl-C to stop.
#
# Defaults:
#   IFACE=vcan0    Virtual CAN interface (must be already up; see setup-vcan.sh).
#   NODE=0x10      CANopen NodeID (1..127).
#
# Override via env vars:
#   IFACE=vcan1 NODE=0x42 ./run.sh

set -euo pipefail

IFACE="${IFACE:-vcan0}"
NODE="${NODE:-0x10}"

cd "$(dirname "$0")"

if [[ ! -x ./canopend-sdo-test ]]; then
    echo "[run] canopend-sdo-test not built yet; running 'make'"
    make
fi

echo "[run] starting canopend-sdo-test on $IFACE as NodeID=$NODE"
exec ./canopend-sdo-test "$IFACE" -i "$NODE"
