# canopen-sdo

Sans-IO CANopen SDO **client and server** (CiA 301) — expedited and
segmented transfers, with aborts and timeouts.

The protocol state machines have **zero IO and zero async** in them. You
feed them CAN frames, they tell you which frames to send next and when to
fire a timeout. This makes them trivial to unit-test, easy to port across
runtimes (tokio, embassy, blocking), and tiny to embed.

Two roles, picked by feature:

- **`client`** (master) — read/write a remote node's object dictionary.
  Needs an allocator. The default `tokio` feature adds a one-line async
  client over [`can-transport`](https://crates.io/crates/can-transport).
- **`server`** (node) — answer a master against your own object
  dictionary. **`no_std`, no alloc** (a fixed `[u8; N]` buffer), built for
  embedded targets.

Both roles share one wire codec, validated by a client⟷server loopback
test, so they can never drift apart.

Time is a monotonic `u64` of the caller's unit (nanoseconds in the async
glue); a `u64` of nanoseconds takes ~584 years to overflow.

## Quick start — async (tokio + SocketCAN)

```toml
[dependencies]
canopen-sdo = "0.2"
can-transport = { version = "0.1", features = ["socketcan"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use std::time::Duration;
use can_transport::socketcan::SocketCanBus;
use canopen_sdo::asynch::{upload_bytes, download_bytes};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bus = SocketCanBus::open("can0")?;
    let node = 0x10;
    let timeout = Some(Duration::from_millis(200));

    // Read 0x1008:00 (manufacturer device name, segmented string)
    let name = upload_bytes(&bus, node, 0x1008, 0x00, timeout).await?;
    println!("device name: {}", String::from_utf8_lossy(&name));

    // Write control word
    download_bytes(&bus, node, 0x6040, 0x00, &[0x06, 0x00], timeout).await?;
    Ok(())
}
```

You can hand `upload_bytes` any type implementing
`can_transport::CanBus`, so swapping SocketCAN for PCAN, embassy-CAN,
a serial-CAN bridge, or an in-memory loopback (for tests) requires zero
changes to the SDO code.

### Off Linux (or driverless CAN-FD): gs_usb / candleLight

To run against a gs_usb adapter — on Windows, macOS, or Linux without the
kernel driver — enable `can-transport`'s `gs_usb` feature and open a
`GsUsbBus` instead; the SDO calls are identical:

```rust
use can_transport::gs_usb::{GsUsbBus, GsUsbConfig};
let bus = GsUsbBus::open(GsUsbConfig::fd_1m_5m()).await?; // 1 Mbit / 5 Mbit FD
let name = upload_bytes(&bus, node, 0x1008, 0x00, timeout).await?;
```

See `examples/sdo_gs_usb.rs` for a runnable Identity-object (0x1018) read,
and the `can-transport` README for the per-platform driver story.

## Sans-IO client (any runtime)

If you don't want the tokio glue, drive the state machine yourself. `now`
is any monotonic `u64` you supply (here, nanoseconds):

```rust
use canopen_sdo::{SdoClient, SdoConfig, SdoOutcome, SdoFrame};

let mut client = SdoClient::new(SdoConfig::default());
client.begin_upload(0x10, 0x1008, 0x00, now_ns())?;

loop {
    // 1) Drain anything that wants to go out on the wire
    while let Some(out_frame) = client.poll_transmit() {
        my_can_send(out_frame.cob_id, out_frame.data);
    }

    // 2) Block on either an incoming frame or the next deadline (also ns)
    let next_deadline = client.poll_timeout();
    match my_wait_for_frame_or_deadline(next_deadline) {
        WokeBy::Frame(cob_id, data) => {
            let frame = SdoFrame::new(cob_id, data);
            if let Some(SdoOutcome::UploadCompleted(bytes)) =
                client.handle_frame(frame, now_ns())?
            {
                return Ok(bytes);
            }
        }
        WokeBy::Timeout => {
            client.handle_timeout(now_ns())?;
            // an abort frame is now sitting in poll_transmit()
        }
    }
}
```

The `client` needs an allocator (payload buffers are `Vec<u8>`). Disable
the default features to drop tokio:

```toml
canopen-sdo = { version = "0.2", default-features = false, features = ["client"] }
```

## Sans-IO server (no_std, no alloc)

On an embedded node, enable `server` and implement `ObjectDictionary`:

```toml
canopen-sdo = { version = "0.2", default-features = false, features = ["server"] }
```

```rust
use canopen_sdo::{SdoServer, ServerConfig, ObjectDictionary, SdoAbortCode, SdoFrame};

struct MyOd { /* ... */ }
impl ObjectDictionary for MyOd {
    fn read(&mut self, index: u16, sub: u8, buf: &mut [u8]) -> Result<usize, SdoAbortCode> { /* ... */ }
    fn write(&mut self, index: u16, sub: u8, data: &[u8]) -> Result<(), SdoAbortCode> { /* ... */ }
}

// N bounds the largest object this node can transfer (bytes).
let mut server = SdoServer::<256>::new(ServerConfig::new(0x10));
let mut od = MyOd { /* ... */ };

// In your CAN RX path:
let _ = server.handle_frame(SdoFrame::new(cob_id, data), now_ns(), &mut od);
while let Some(tx) = server.poll_transmit() {
    my_can_send(tx.cob_id, tx.data);
}
// Periodically: server.handle_timeout(now_ns()) to drop a stalled transfer.
```

## Live-bus tests against CANopenNode

![canopen-sdo-test](CANopenNodeTest.png)

In addition to the in-process unit tests in `src/`, this repo ships a
small end-to-end harness that drives the client against a real
[CANopenNode](https://github.com/CANopenNode/CANopenNode) SDO server
running on a Linux `vcan` interface. It covers expedited and segmented
uploads/downloads, server aborts and client timeouts.

See [`CANopenNode-test/README.md`](CANopenNode-test/README.md) for
build/run instructions. In short:

```text
# one terminal: build & run the C server (uses CANopenLinux + a custom OD)
cd CANopenNode-test
./setup-vcan.sh && make && ./run.sh

# another terminal: drive every SDO scenario from Rust
cargo run --example against_canopennode -- vcan0 0x10
```

## Design notes

- **One transfer at a time per `SdoClient`.** Starting another while
  one is in flight returns `SdoError::Busy`. For parallel transfers to
  multiple nodes, create one client per node and drive them in
  separate tasks; they don't share any state.
- **No NMT handling.** Neither role cares about NMT state — the server
  answers SDO regardless; the client just gets timeout aborts if the node
  isn't reachable. (A node stack layers NMT/heartbeat/PDO on top.)
- **Time is a monotonic `u64`.** The caller picks the unit (the async glue
  uses nanoseconds); `now` and any timeout must share it. No wraparound
  handling — a `u64` of nanoseconds lasts ~584 years.
- **Server buffer is fixed.** `SdoServer<N>` bounds the largest object to
  `N` bytes (no alloc); larger reads/writes abort with `OutOfMemory`.
- **Block transfer is not implemented.** Block mode is rare in
  practice; PR welcome.

## License

MIT OR Apache-2.0
