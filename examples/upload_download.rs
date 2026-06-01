//! End-to-end SDO upload/download against a real SocketCAN interface.
//!
//! Run with:
//! ```text
//! # set up a virtual CAN interface (Linux):
//! sudo modprobe vcan
//! sudo ip link add dev vcan0 type vcan
//! sudo ip link set up vcan0
//!
//! # then talk to a server on node id 0x10:
//! cargo run --example upload_download -- vcan0 0x10
//! ```

use std::time::Duration;

use can_transport::socketcan::SocketCanBus;
use canopen_sdo::asynch::{download_bytes, upload_bytes};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let iface = std::env::args().nth(1).unwrap_or_else(|| "vcan0".to_string());
    let node_id: u8 = std::env::args()
        .nth(2)
        .map(|s| {
            u8::from_str_radix(s.trim_start_matches("0x"), 16)
                .or_else(|_| s.parse())
                .expect("node id must be a number")
        })
        .unwrap_or(0x10);

    println!("Opening SocketCAN interface {iface}, talking to node 0x{node_id:02X}");
    let bus = SocketCanBus::open(&iface)?;
    let timeout = Some(Duration::from_millis(200));

    // --- Read the manufacturer device name (0x1008:00, string, segmented) ---
    println!("Uploading 0x1008:00 (Manufacturer device name)...");
    match upload_bytes(&bus, node_id, 0x1008, 0x00, timeout).await {
        Ok(bytes) => println!(
            "  ← {} bytes: {:?}",
            bytes.len(),
            String::from_utf8_lossy(&bytes)
        ),
        Err(e) => println!("  upload failed: {e}"),
    }

    // --- Read the status word (0x6041:00, u16, expedited) ---
    println!("Uploading 0x6041:00 (Status word, expedited)...");
    match upload_bytes(&bus, node_id, 0x6041, 0x00, timeout).await {
        Ok(bytes) => println!("  ← {:02X?}", bytes),
        Err(e) => println!("  upload failed: {e}"),
    }

    // --- Write the control word (0x6040:00, u16, expedited) ---
    println!("Downloading 0x6040:00 (Control word) = 0x0006...");
    match download_bytes(&bus, node_id, 0x6040, 0x00, &[0x06, 0x00], timeout).await {
        Ok(()) => println!("  → ok"),
        Err(e) => println!("  download failed: {e}"),
    }

    // --- Write something long (segmented download) ---
    let big: Vec<u8> = (0u8..30).collect();
    println!(
        "Downloading 0x2000:01 with {} bytes (segmented)...",
        big.len()
    );
    match download_bytes(&bus, node_id, 0x2000, 0x01, &big, timeout).await {
        Ok(()) => println!("  → ok"),
        Err(e) => println!("  download failed: {e}"),
    }

    Ok(())
}
