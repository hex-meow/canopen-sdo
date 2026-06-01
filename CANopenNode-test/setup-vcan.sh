#!/usr/bin/env bash
#
# Bring up a virtual CAN interface so canopend-sdo-test (and the Rust
# example) have something to talk over. Idempotent: safe to re-run.
#
# Default interface: vcan0. Override via:
#   IFACE=vcan1 ./setup-vcan.sh

set -euo pipefail

IFACE="${IFACE:-vcan0}"

if ! lsmod | grep -q '^vcan '; then
    echo "[setup-vcan] modprobe vcan"
    sudo modprobe vcan
fi

if ! ip link show "$IFACE" >/dev/null 2>&1; then
    echo "[setup-vcan] ip link add dev $IFACE type vcan"
    sudo ip link add dev "$IFACE" type vcan
fi

if ! ip link show "$IFACE" | grep -q 'state UP'; then
    echo "[setup-vcan] ip link set up $IFACE"
    sudo ip link set up "$IFACE"
fi

echo "[setup-vcan] $IFACE is ready."
ip -details link show "$IFACE"
