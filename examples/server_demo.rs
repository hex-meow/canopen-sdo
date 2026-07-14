//! Stand up our [`SdoServer`] as a CANopen node on a real SocketCAN bus,
//! serving a small toy object dictionary, so an **independent** SDO master
//! (e.g. `python-canopen`, or CANopenNode's `cocomm`) can exercise expedited
//! and segmented reads/writes — and abort handling — against it.
//!
//! This is the server-side mirror of `examples/upload_download.rs`: instead of
//! driving [`crate::SdoClient`](canopen_sdo::SdoClient) over the bus, it pumps
//! incoming RSDO frames into an [`SdoServer`] and writes its responses back.
//!
//! Run (Linux, with `vcan0` up):
//!
//! ```text
//! cargo run --example server_demo --features server -- vcan0 0x10
//! ```
//!
//! Then, from another shell, drive it with any SDO client. With `python-canopen`:
//!
//! ```python
//! import canopen
//! net = canopen.Network(); net.connect(channel="vcan0", interface="socketcan")
//! from canopen.sdo import SdoClient
//! c = SdoClient(0x600 + 0x10, 0x580 + 0x10, canopen.ObjectDictionary())
//! c.network = net; net.subscribe(c.tx_cobid, c.on_response)
//! print(c.upload(0x2000, 0))           # segmented read
//! c.download(0x2001, 0, b"x" * 40)     # segmented write
//! print(c.upload(0x2001, 0))           # read it back
//! ```
//!
//! The OD has, deliberately:
//! * `0x1000:00` — RO `u32` (expedited read; also the write-to-read-only target)
//! * `0x2002:00` — RW `u16` (expedited read/write round-trip)
//! * `0x2000:00` — RO string > 4 bytes (forces a *segmented* upload)
//! * `0x2001:00` — RW buffer (segmented download + readback)
//! * everything else — absent, so reads abort `ObjectDoesNotExist`

use std::time::Duration;

use can_transport::socketcan::SocketCanBus;
use can_transport::{CanBus, CanFilter, CanFrame, CanId, FrameKind};
use canopen_sdo::{ObjectDictionary, SdoAbortCode, SdoFrame, SdoServer, ServerConfig};
use tokio::time::{sleep_until, Instant};

/// Largest object the server can transfer (bytes). Bounds the fixed `[u8; N]`
/// transfer buffer inside the server; pick it for your biggest OD entry.
const N: usize = 256;

/// One object-dictionary entry.
struct Entry {
    idx: u16,
    sub: u8,
    val: Vec<u8>,
    writable: bool,
    readable: bool,
}

/// A toy object dictionary backed by a flat list of entries.
struct DemoOd {
    entries: Vec<Entry>,
}

impl DemoOd {
    fn new() -> Self {
        Self {
            entries: vec![
                // 0x1000:00 deviceType, RO u32 — expedited read, and the target
                // for the "write to a read-only object" abort.
                Entry {
                    idx: 0x1000,
                    sub: 0,
                    val: vec![0x92, 0x01, 0x0F, 0x00],
                    writable: false,
                    readable: true,
                },
                // 0x2002:00 RW u16 — expedited read/write round-trip.
                Entry {
                    idx: 0x2002,
                    sub: 0,
                    val: vec![0x00, 0x00],
                    writable: true,
                    readable: true,
                },
                // 0x2000:00 RO VISIBLE_STRING (> 4 bytes) — forces segmented upload.
                Entry {
                    idx: 0x2000,
                    sub: 0,
                    val: b"canopen-sdo server demo: segmented upload payload 0123456789".to_vec(),
                    writable: false,
                    readable: true,
                },
                // 0x2001:00 RW buffer — segmented download + readback.
                Entry {
                    idx: 0x2001,
                    sub: 0,
                    val: Vec::new(),
                    writable: true,
                    readable: true,
                },
            ],
        }
    }

    fn find(&self, idx: u16, sub: u8) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.idx == idx && e.sub == sub)
    }
}

impl ObjectDictionary for DemoOd {
    fn read(&mut self, index: u16, sub: u8, buf: &mut [u8]) -> Result<usize, SdoAbortCode> {
        let i = self
            .find(index, sub)
            .ok_or(SdoAbortCode::ObjectDoesNotExist)?;
        let e = &self.entries[i];
        if !e.readable {
            return Err(SdoAbortCode::ReadWriteOnly);
        }
        if e.val.len() > buf.len() {
            return Err(SdoAbortCode::OutOfMemory);
        }
        buf[..e.val.len()].copy_from_slice(&e.val);
        log::debug!("OD read  {index:#06X}:{sub:02X} -> {} bytes", e.val.len());
        Ok(e.val.len())
    }

    fn write(&mut self, index: u16, sub: u8, data: &[u8]) -> Result<(), SdoAbortCode> {
        let i = self
            .find(index, sub)
            .ok_or(SdoAbortCode::ObjectDoesNotExist)?;
        let e = &mut self.entries[i];
        if !e.writable {
            return Err(SdoAbortCode::WriteReadOnly);
        }
        e.val = data.to_vec();
        log::debug!("OD write {index:#06X}:{sub:02X} <- {} bytes", data.len());
        Ok(())
    }
}

/// Nanoseconds since `epoch` — the monotonic `u64` clock the server speaks.
fn ns_since(epoch: Instant) -> u64 {
    epoch.elapsed().as_nanos() as u64
}

fn sdo_to_can(f: SdoFrame) -> CanFrame {
    // SDO frames are always 8 data bytes with a standard COB-ID, so this is
    // infallible.
    CanFrame::new_data(CanId::Standard(f.cob_id), &f.data).expect("8-byte SDO frame")
}

fn can_to_sdo(f: &CanFrame) -> Option<SdoFrame> {
    if !matches!(f.kind(), FrameKind::Data) {
        return None;
    }
    let CanId::Standard(cob_id) = f.id() else {
        return None;
    };
    let payload = f.data();
    if payload.len() != 8 {
        return None;
    }
    let mut data = [0u8; 8];
    data.copy_from_slice(payload);
    Some(SdoFrame::new(cob_id, data))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let iface = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "vcan0".to_string());
    let node_id: u8 = std::env::args()
        .nth(2)
        .map(|s| {
            u8::from_str_radix(s.trim_start_matches("0x"), 16)
                .or_else(|_| s.parse())
                .expect("node id must be a number")
        })
        .unwrap_or(0x10);

    let bus = SocketCanBus::open(&iface)?;
    let mut od = DemoOd::new();
    let mut server = SdoServer::<N>::new(ServerConfig::new(node_id));

    // Only deliver RSDO requests addressed to this node (0x600 + node_id).
    let filter = CanFilter::standard(SdoFrame::rsdo_id(node_id), 0x7FF);
    let mut rx = bus.subscribe(filter).await?;
    let epoch = Instant::now();

    println!("SdoServer<{N}> demo on {iface} as node 0x{node_id:02X}");
    println!("Serving OD entries:");
    println!("  0x1000:00  RO u32        (expedited read / write-RO abort target)");
    println!("  0x2002:00  RW u16        (expedited read+write)");
    println!("  0x2000:00  RO string 60B (segmented upload)");
    println!("  0x2001:00  RW buffer     (segmented download + readback)");
    println!("Ctrl-C to stop. (RUST_LOG=debug to trace each OD access.)\n");

    loop {
        // 1) Drain anything the state machine wants to send (response or abort).
        while let Some(out) = server.poll_transmit() {
            bus.send(sdo_to_can(out)).await?;
        }

        // 2) Wait for the next request, or for a stalled segmented transfer's
        //    deadline to expire.
        let frame_or_timeout = match server.poll_timeout() {
            Some(dl_ns) => {
                let dl = epoch + Duration::from_nanos(dl_ns);
                tokio::select! {
                    biased;
                    f = rx.recv() => Some(f),
                    _ = sleep_until(dl) => None,
                }
            }
            None => Some(rx.recv().await),
        };

        match frame_or_timeout {
            Some(Ok(frame)) => {
                let Some(sdo) = can_to_sdo(&frame) else {
                    continue;
                };
                // The server queues a response (or an abort) for poll_transmit
                // either way; an Err just tells us *it* aborted, which we log.
                if let Err(e) = server.handle_frame(sdo, ns_since(epoch), &mut od) {
                    log::warn!("aborted transfer: {e}");
                }
            }
            Some(Err(e)) => {
                // A lagged subscriber or a transient bus error: log and carry on.
                log::warn!("CAN rx error: {e}");
            }
            None => {
                // Deadline expired with no follow-up: abort the stalled transfer.
                if let Err(e) = server.handle_timeout(ns_since(epoch)) {
                    log::warn!("segmented transfer timed out: {e}");
                }
            }
        }
    }
}
