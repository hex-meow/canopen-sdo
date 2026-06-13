//! SDO upload over a gs_usb / candleLight adapter (CAN-FD bus, classic
//! SDO frames). Reads the standard Identity object 0x1018 from a node to
//! exercise the `can-transport` gs_usb backend's transmit path end to end.
//!
//! ```sh
//! # node id defaults to 1 (we saw a 0x701 heartbeat); override as arg:
//! cargo run --example sdo_gs_usb -- 1
//! ```
//!
//! On Linux the kernel `gs_usb` driver must be detached first (the backend
//! does this itself but needs usbfs access — run with sudo or a udev rule).

use std::time::Duration;

use can_transport::gs_usb::{GsUsbBus, GsUsbConfig};
use can_transport::CanBus;
use canopen_sdo::asynch::upload_bytes;

const TIMEOUT: Option<Duration> = Some(Duration::from_millis(500));

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let node_id: u8 = std::env::args()
        .nth(1)
        .map(|s| {
            u8::from_str_radix(s.trim_start_matches("0x"), 16)
                .or_else(|_| s.parse())
                .expect("node id must be a number")
        })
        .unwrap_or(1);

    println!("opening gs_usb (CAN-FD 1M/5M)...");
    let bus = GsUsbBus::open(GsUsbConfig::fd_1m_5m()).await?;
    println!("opened: {:?}", bus.capabilities());
    println!("reading Identity object 0x1018 from node 0x{node_id:02X}\n");

    // 0x1018:00 — number of sub-entries.
    let n = read_u8(&bus, node_id, 0x1018, 0x00).await?;
    println!("  0x1018:00 highest sub-index = {n}");

    let names = [
        (0x01u8, "Vendor-ID"),
        (0x02, "Product code"),
        (0x03, "Revision number"),
        (0x04, "Serial number"),
    ];
    for (sub, name) in names {
        if sub > n {
            break;
        }
        match upload_bytes(&bus, node_id, 0x1018, sub, TIMEOUT).await {
            Ok(v) => {
                let val = as_u32(&v);
                println!(
                    "  0x1018:{sub:02X} {name:<16} = 0x{val:08X}  ({:02X?})",
                    v
                );
            }
            Err(e) => println!("  0x1018:{sub:02X} {name:<16} = <error: {e}>"),
        }
    }

    println!("\nTX path works ✅ (SDO request frames were sent and answered)");
    Ok(())
}

async fn read_u8(
    bus: &(impl CanBus + ?Sized),
    node: u8,
    index: u16,
    sub: u8,
) -> anyhow::Result<u8> {
    let v = upload_bytes(bus, node, index, sub, TIMEOUT).await?;
    Ok(v.first().copied().unwrap_or(0))
}

fn as_u32(bytes: &[u8]) -> u32 {
    let mut b = [0u8; 4];
    let n = bytes.len().min(4);
    b[..n].copy_from_slice(&bytes[..n]);
    u32::from_le_bytes(b)
}
