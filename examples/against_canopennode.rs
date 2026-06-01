//! End-to-end SDO scenarios against the CANopenNode-based test server in
//! [`CANopenNode-test/`](../CANopenNode-test/README.md).
//!
//! Bring the server up first (it provides the custom OD entries 0x2000 /
//! 0x2001 we need for segmented transfers):
//!
//! ```text
//! cd CANopenNode-test
//! ./setup-vcan.sh         # one-time, sudo
//! ./run.sh                # leaves canopend-sdo-test running on vcan0:0x10
//! ```
//!
//! Then in another shell:
//!
//! ```text
//! cargo run --example against_canopennode -- vcan0 0x10
//! ```
//!
//! Each scenario prints `PASS` or `FAIL`. The process exits non-zero if
//! any scenario failed.

use std::time::Duration;

use can_transport::socketcan::SocketCanBus;
use canopen_sdo::asynch::{download_bytes, upload_bytes, AsyncSdoError};
use canopen_sdo::{SdoAbortCode, SdoError};

/// Default per-transfer timeout. CANopenNode's mainline thread runs at a
/// 100 ms tick by default, so leave plenty of slack.
const TIMEOUT: Option<Duration> = Some(Duration::from_millis(500));

/// Must match the literal in `CANopenNode-test/OD.c`'s
/// `OD_RAM.x2000_testString` initializer. CANopenNode reports
/// `strnlen()` rather than the buffer size for `ODA_STR` entries, so
/// uploads return exactly this many bytes (no trailing NUL pad).
const X2000_LITERAL: &[u8] = b"canopen-sdo segmented upload test 0123456789 ABCDEFGHIJ";

/// Must match `OD_LEN_X2001_TEST_BUFFER` from `CANopenNode-test/OD.h`.
/// 0x2001 has no `ODA_STR`, so reads always return exactly this many
/// bytes and writes must match this size.
const X2001_TOTAL_LEN: usize = 256;

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

    println!(
        "Driving SDO scenarios against canopend-sdo-test on {iface} (node 0x{node_id:02X})\n"
    );
    let bus = SocketCanBus::open(&iface)?;

    let mut runner = ScenarioRunner::default();

    // ------------------------------------------------------------------
    // Expedited uploads
    // ------------------------------------------------------------------
    runner
        .step("expedited upload 4B  (0x1000:00 deviceType, ro u32)", async {
            let v = upload_bytes(&bus, node_id, 0x1000, 0x00, TIMEOUT).await?;
            ensure(v.len() == 4, format!("expected 4 bytes, got {}", v.len()))?;
            Ok(format!("← {:02X?}", v))
        })
        .await;

    runner
        .step("expedited upload 4B  (0x1018:01 vendor-ID, ro u32)", async {
            let v = upload_bytes(&bus, node_id, 0x1018, 0x01, TIMEOUT).await?;
            ensure(v.len() == 4, format!("expected 4 bytes, got {}", v.len()))?;
            Ok(format!("← {:02X?}", v))
        })
        .await;

    runner
        .step(
            "expedited upload 2B  (0x1017:00 producerHeartbeatTime, rw u16)",
            async {
                let v = upload_bytes(&bus, node_id, 0x1017, 0x00, TIMEOUT).await?;
                ensure(v.len() == 2, format!("expected 2 bytes, got {}", v.len()))?;
                Ok(format!("← {:02X?}", v))
            },
        )
        .await;

    runner
        .step(
            "expedited upload 1B  (0x1019:00 syncCounterOverflow, rw u8)",
            async {
                let v = upload_bytes(&bus, node_id, 0x1019, 0x00, TIMEOUT).await?;
                ensure(v.len() == 1, format!("expected 1 byte, got {}", v.len()))?;
                Ok(format!("← {:02X?}", v))
            },
        )
        .await;

    // ------------------------------------------------------------------
    // Expedited downloads + readback
    // ------------------------------------------------------------------
    runner
        .step(
            "expedited download 1B + readback (0x1019:00 ← 0x05)",
            async {
                let want: u8 = 0x05;
                download_bytes(&bus, node_id, 0x1019, 0x00, &[want], TIMEOUT).await?;
                let got = upload_bytes(&bus, node_id, 0x1019, 0x00, TIMEOUT).await?;
                ensure(got == [want], format!("readback {:02X?} != {:02X?}", got, [want]))?;
                Ok(format!("→ {:02X} → {:02X?}", want, got))
            },
        )
        .await;

    runner
        .step(
            "expedited download 2B + readback (0x1017:00 ← 1000 ms)",
            async {
                let want: u16 = 1000;
                let want_bytes = want.to_le_bytes();
                download_bytes(&bus, node_id, 0x1017, 0x00, &want_bytes, TIMEOUT).await?;
                let got = upload_bytes(&bus, node_id, 0x1017, 0x00, TIMEOUT).await?;
                ensure(
                    got == want_bytes,
                    format!("readback {:02X?} != {:02X?}", got, want_bytes),
                )?;
                Ok(format!("→ {} → {:02X?}", want, got))
            },
        )
        .await;

    runner
        .step(
            "expedited download 4B + readback (0x1006:00 ← 0)",
            async {
                let want: u32 = 0;
                let want_bytes = want.to_le_bytes();
                download_bytes(&bus, node_id, 0x1006, 0x00, &want_bytes, TIMEOUT).await?;
                let got = upload_bytes(&bus, node_id, 0x1006, 0x00, TIMEOUT).await?;
                ensure(
                    got == want_bytes,
                    format!("readback {:02X?} != {:02X?}", got, want_bytes),
                )?;
                Ok(format!("→ {} → {:02X?}", want, got))
            },
        )
        .await;

    // ------------------------------------------------------------------
    // Segmented upload — read 64-byte fixed string from custom OD entry
    // ------------------------------------------------------------------
    runner
        .step(
            "segmented upload (0x2000:00 testString, ro VISIBLE_STRING)",
            async {
                let v = upload_bytes(&bus, node_id, 0x2000, 0x00, TIMEOUT).await?;
                ensure(
                    v.as_slice() == X2000_LITERAL,
                    format!(
                        "got {} bytes ({:?}), expected {} bytes ({:?})",
                        v.len(),
                        String::from_utf8_lossy(&v),
                        X2000_LITERAL.len(),
                        String::from_utf8_lossy(X2000_LITERAL),
                    ),
                )?;
                Ok(format!(
                    "← {} bytes (\"{}\")",
                    v.len(),
                    String::from_utf8_lossy(&v)
                ))
            },
        )
        .await;

    // ------------------------------------------------------------------
    // Segmented download + readback — write 200B pattern to 256B buffer
    // ------------------------------------------------------------------
    runner
        .step(
            "segmented download + readback (0x2001:00 testBuffer, rw 256B)",
            async {
                let payload: Vec<u8> = (0..X2001_TOTAL_LEN).map(|i| (i % 251) as u8).collect();
                download_bytes(&bus, node_id, 0x2001, 0x00, &payload, TIMEOUT).await?;
                let got = upload_bytes(&bus, node_id, 0x2001, 0x00, TIMEOUT).await?;
                ensure(
                    got.len() == X2001_TOTAL_LEN,
                    format!("expected {} bytes, got {}", X2001_TOTAL_LEN, got.len()),
                )?;
                ensure(
                    got == payload,
                    "readback bytes don't match the pattern we wrote".to_string(),
                )?;
                Ok(format!(
                    "→ {} B pattern, ← {} B (full match)",
                    payload.len(),
                    got.len()
                ))
            },
        )
        .await;

    // ------------------------------------------------------------------
    // Server abort — read a non-existent index
    // ------------------------------------------------------------------
    runner
        .step("server abort on missing index (0x9999:00 upload)", async {
            match upload_bytes(&bus, node_id, 0x9999, 0x00, TIMEOUT).await {
                Ok(v) => Err(format!("unexpected success: {:02X?}", v).into()),
                Err(AsyncSdoError::Sdo(SdoError::ServerAborted(code))) => {
                    Ok(format!("got expected ServerAborted({code})"))
                }
                Err(other) => Err(format!("expected ServerAborted, got {other}").into()),
            }
        })
        .await;

    // ------------------------------------------------------------------
    // Client timeout — talk to a NodeID that nobody is answering
    // ------------------------------------------------------------------
    runner
        .step(
            "client timeout abort (bogus node 0x7E, 100 ms)",
            async {
                let bogus = pick_bogus_node(node_id);
                match upload_bytes(&bus, bogus, 0x1000, 0x00, Some(Duration::from_millis(100))).await {
                    Ok(v) => Err(format!("unexpected success from 0x{bogus:02X}: {:02X?}", v).into()),
                    Err(AsyncSdoError::Sdo(SdoError::ClientAborted(SdoAbortCode::ProtocolTimeout))) => {
                        Ok(format!("got expected ClientAborted(ProtocolTimeout) from 0x{bogus:02X}"))
                    }
                    Err(other) => Err(format!("expected ProtocolTimeout, got {other}").into()),
                }
            },
        )
        .await;

    // ------------------------------------------------------------------
    runner.print_summary();
    if runner.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Pick any 1..=127 node id different from the one canopend uses.
/// 0x7E is far from typical defaults (0x01, 0x10, 0x40 …).
fn pick_bogus_node(server: u8) -> u8 {
    let candidate = if server == 0x7E { 0x7D } else { 0x7E };
    debug_assert!(candidate >= 1 && candidate <= 127);
    candidate
}

// ---------------------------------------------------------------------
// Tiny scenario runner so each step's pass/fail line stays uniform.
// ---------------------------------------------------------------------

#[derive(Default)]
struct ScenarioRunner {
    passed: u32,
    failed: u32,
}

impl ScenarioRunner {
    async fn step<F, R>(&mut self, name: &str, fut: F)
    where
        F: std::future::Future<Output = Result<R, StepError>>,
        R: std::fmt::Display,
    {
        print!("  {:<60} ", name);
        match fut.await {
            Ok(detail) => {
                println!("PASS  {detail}");
                self.passed += 1;
            }
            Err(StepError(msg)) => {
                println!("FAIL  {msg}");
                self.failed += 1;
            }
        }
    }

    fn print_summary(&self) {
        println!();
        println!(
            "summary: {} passed, {} failed (out of {})",
            self.passed,
            self.failed,
            self.passed + self.failed
        );
    }
}

/// Wrapper so any string-y or `AsyncSdoError`-y failure becomes `FAIL <msg>`.
struct StepError(String);

impl From<String> for StepError {
    fn from(s: String) -> Self {
        StepError(s)
    }
}

impl From<&str> for StepError {
    fn from(s: &str) -> Self {
        StepError(s.to_string())
    }
}

impl From<AsyncSdoError> for StepError {
    fn from(e: AsyncSdoError) -> Self {
        StepError(format!("{e}"))
    }
}

fn ensure(cond: bool, msg: impl Into<String>) -> Result<(), StepError> {
    if cond {
        Ok(())
    } else {
        Err(StepError(msg.into()))
    }
}
