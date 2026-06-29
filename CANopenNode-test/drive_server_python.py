#!/usr/bin/env python3
"""Drive the canopen-sdo *server* from an INDEPENDENT SDO master.

This is the mirror of the `against_canopennode` Rust example: there, our SDO
*client* is checked against the CANopenNode C *server*; here, our SDO *server*
(`examples/server_demo.rs`) is checked against a third-party SDO *client*
(`python-canopen`), so the test is genuinely independent of our own client.

Usage
-----
1. Bring up vcan0 (see setup-vcan.sh) and start our server:

       cargo run --example server_demo --features server -- vcan0 0x10

2. In another shell (a venv is recommended on Debian/Ubuntu):

       python3 -m venv venv && . venv/bin/activate && pip install canopen
       python3 CANopenNode-test/drive_server_python.py vcan0 0x10

Each scenario prints PASS/FAIL; the process exits non-zero if any failed.
"""
import sys

import canopen
from canopen.sdo import SdoClient
from canopen.sdo.exceptions import SdoAbortedError

IFACE = sys.argv[1] if len(sys.argv) > 1 else "vcan0"
NODE = int(sys.argv[2], 16) if len(sys.argv) > 2 else 0x10

passed = 0
failed = 0


def check(name, cond, detail=""):
    global passed, failed
    if cond:
        passed += 1
        print(f"  {name:<55} PASS  {detail}")
    else:
        failed += 1
        print(f"  {name:<55} FAIL  {detail}")


net = canopen.Network()
net.connect(channel=IFACE, interface="socketcan")
try:
    # Raw SDO client, no EDS needed: rx_cobid is what the *server* receives on
    # (0x600 + node), tx_cobid is what it answers with (0x580 + node).
    sdo = SdoClient(0x600 + NODE, 0x580 + NODE, canopen.ObjectDictionary())
    sdo.network = net
    net.subscribe(sdo.tx_cobid, sdo.on_response)
    sdo.RESPONSE_TIMEOUT = 1.0

    print(f"Driving canopen-sdo SdoServer on {IFACE} (node 0x{NODE:02X}) "
          f"with python-canopen {canopen.__version__}\n")

    # 1) Expedited upload (read) — 0x1000:00 RO u32.
    v = sdo.upload(0x1000, 0)
    check("expedited upload 0x1000:00 (RO u32)",
          v == bytes([0x92, 0x01, 0x0F, 0x00]), f"<- {v.hex()}")

    # 2) Expedited download (write) + readback — 0x2002:00 RW u16.
    sdo.download(0x2002, 0, bytes([0xCD, 0xAB]))
    v = sdo.upload(0x2002, 0)
    check("expedited download+readback 0x2002:00 (RW u16)",
          v == bytes([0xCD, 0xAB]), f"-> ABCD -> {v.hex()}")

    # 3) Forced-segmented download of small data + readback — proves segmented
    #    write works even when the payload would fit an expedited transfer.
    sdo.download(0x2002, 0, bytes([0x34, 0x12]), force_segment=True)
    v = sdo.upload(0x2002, 0)
    check("forced-segmented download+readback 0x2002:00",
          v == bytes([0x34, 0x12]), f"-> 1234 (segmented) -> {v.hex()}")

    # 4) Segmented upload (read) — 0x2000:00 RO string (60 bytes).
    expected = b"canopen-sdo server demo: segmented upload payload 0123456789"
    v = sdo.upload(0x2000, 0)
    check("segmented upload 0x2000:00 (RO string)",
          v == expected, f"<- {len(v)} bytes")

    # 5) Segmented download (write) + readback — 0x2001:00 RW buffer (200 B).
    payload = bytes((i * 7 + 3) & 0xFF for i in range(200))
    sdo.download(0x2001, 0, payload)
    v = sdo.upload(0x2001, 0)
    check("segmented download+readback 0x2001:00 (200B)",
          v == payload, f"-> 200 B, <- {len(v)} B "
          f"{'(match)' if v == payload else '(MISMATCH)'}")

    # 6) Read a missing object -> ObjectDoesNotExist (0x06020000).
    try:
        sdo.upload(0x9999, 0)
        check("read missing 0x9999:00 -> abort", False, "unexpected success")
    except SdoAbortedError as e:
        check("read missing 0x9999:00 -> abort",
              e.code == 0x06020000, f"got abort 0x{e.code:08X} ({e})")

    # 7) Write a read-only object -> WriteReadOnly (0x06010002).
    try:
        sdo.download(0x1000, 0, bytes([1, 2, 3, 4]))
        check("write RO 0x1000:00 -> abort", False, "unexpected success")
    except SdoAbortedError as e:
        check("write RO 0x1000:00 -> abort",
              e.code == 0x06010002, f"got abort 0x{e.code:08X} ({e})")

    print(f"\nsummary: {passed} passed, {failed} failed "
          f"(out of {passed + failed})")
finally:
    net.disconnect()

sys.exit(1 if failed else 0)
