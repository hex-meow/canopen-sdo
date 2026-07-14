//! Tokio-based async glue that drives [`crate::SdoClient`] on top of a
//! [`can_transport::CanBus`].
//!
//! This module is enabled by the `tokio` feature (on by default).
//! It exposes the convenience functions [`upload_bytes`] and
//! [`download_bytes`], plus a retrying variant of each.

use std::time::Duration;

use can_transport::{CanBus, CanFilter, CanFrame, CanId, CanIoError, FrameKind};
use thiserror::Error;
use tokio::time::{sleep_until, Instant};

use crate::client::{SdoClient, SdoConfig, SdoOutcome};
use crate::error::SdoError;
use crate::frame::SdoFrame;

/// Nanoseconds elapsed since `epoch` — the monotonic `u64` clock the sans-io
/// client speaks. ~584 years to overflow a u64, so the truncation is safe.
fn ns_since(epoch: Instant) -> u64 {
    epoch.elapsed().as_nanos() as u64
}

fn config_from(timeout: Option<Duration>) -> SdoConfig {
    SdoConfig {
        timeout: timeout
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_else(|| SdoConfig::default().timeout),
    }
}

/// Combined error type for async SDO operations.
#[derive(Debug, Error)]
pub enum AsyncSdoError {
    #[error("SDO protocol error: {0}")]
    Sdo(#[from] SdoError),
    #[error("CAN transport error: {0}")]
    Io(#[from] CanIoError),
}

/// Read an object dictionary entry from a node.
///
/// Performs a full SDO upload (expedited or segmented depending on
/// what the server replies with) and returns the raw bytes.
pub async fn upload_bytes(
    bus: &(impl CanBus + ?Sized),
    node_id: u8,
    od_index: u16,
    od_subindex: u8,
    timeout: Option<Duration>,
) -> Result<Vec<u8>, AsyncSdoError> {
    let mut client = SdoClient::new(config_from(timeout));
    let mut rx = subscribe_for(bus, node_id).await?;
    let epoch = Instant::now();
    client.begin_upload(node_id, od_index, od_subindex, ns_since(epoch))?;
    match drive(bus, &mut client, rx.as_mut(), epoch).await? {
        SdoOutcome::UploadCompleted(data) => Ok(data),
        SdoOutcome::DownloadCompleted => unreachable!("upload never returns DownloadCompleted"),
    }
}

/// Write an object dictionary entry on a node.
pub async fn download_bytes(
    bus: &(impl CanBus + ?Sized),
    node_id: u8,
    od_index: u16,
    od_subindex: u8,
    data: &[u8],
    timeout: Option<Duration>,
) -> Result<(), AsyncSdoError> {
    let mut client = SdoClient::new(config_from(timeout));
    let mut rx = subscribe_for(bus, node_id).await?;
    let epoch = Instant::now();
    client.begin_download(node_id, od_index, od_subindex, data, ns_since(epoch))?;
    match drive(bus, &mut client, rx.as_mut(), epoch).await? {
        SdoOutcome::DownloadCompleted => Ok(()),
        SdoOutcome::UploadCompleted(_) => unreachable!(),
    }
}

/// [`upload_bytes`] with automatic retry on `Io` / timeout failures.
pub async fn upload_bytes_retry(
    bus: &(impl CanBus + ?Sized),
    node_id: u8,
    od_index: u16,
    od_subindex: u8,
    timeout: Option<Duration>,
    retries: u8,
) -> Result<Vec<u8>, AsyncSdoError> {
    let mut attempt = 0u8;
    loop {
        attempt += 1;
        match upload_bytes(bus, node_id, od_index, od_subindex, timeout).await {
            Ok(data) => return Ok(data),
            Err(e) if attempt >= retries.max(1) || !is_transient(&e) => return Err(e),
            Err(e) => log::info!("SDO upload retry {attempt}/{retries}: {e}"),
        }
    }
}

/// [`download_bytes`] with automatic retry on `Io` / timeout failures.
pub async fn download_bytes_retry(
    bus: &(impl CanBus + ?Sized),
    node_id: u8,
    od_index: u16,
    od_subindex: u8,
    data: &[u8],
    timeout: Option<Duration>,
    retries: u8,
) -> Result<(), AsyncSdoError> {
    let mut attempt = 0u8;
    loop {
        attempt += 1;
        match download_bytes(bus, node_id, od_index, od_subindex, data, timeout).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= retries.max(1) || !is_transient(&e) => return Err(e),
            Err(e) => log::info!("SDO download retry {attempt}/{retries}: {e}"),
        }
    }
}

fn is_transient(e: &AsyncSdoError) -> bool {
    matches!(
        e,
        AsyncSdoError::Sdo(SdoError::ClientAborted(
            crate::SdoAbortCode::ProtocolTimeout
        )) | AsyncSdoError::Io(_)
    )
}

async fn subscribe_for(
    bus: &(impl CanBus + ?Sized),
    node_id: u8,
) -> Result<Box<dyn can_transport::CanRx>, CanIoError> {
    // Mask: lower 7 bits open (node id), upper 4 bits == 0xB → TSDO base 0x580.
    let filter = CanFilter::standard(SdoFrame::tsdo_id(node_id), 0x780);
    bus.subscribe(filter).await
}

async fn drive(
    bus: &(impl CanBus + ?Sized),
    client: &mut SdoClient,
    rx: &mut dyn can_transport::CanRx,
    epoch: Instant,
) -> Result<SdoOutcome, AsyncSdoError> {
    loop {
        // 1) Drain anything the state machine wants to send.
        while let Some(out) = client.poll_transmit() {
            let frame = sdo_to_can(out)?;
            bus.send(frame).await?;
        }

        // 2) Wait for either an incoming frame or the next deadline.
        let next_deadline = client.poll_timeout();

        let frame_or_timeout = match next_deadline {
            Some(dl_ns) => {
                // dl_ns is nanoseconds-since-epoch; map back to a tokio Instant.
                let dl_tokio = epoch + Duration::from_nanos(dl_ns);
                tokio::select! {
                    biased;
                    frame = rx.recv() => Some(frame),
                    _ = sleep_until(dl_tokio) => None,
                }
            }
            None => Some(rx.recv().await),
        };

        match frame_or_timeout {
            Some(Ok(frame)) => {
                let Some(sdo) = can_to_sdo(&frame) else {
                    continue;
                };
                if let Some(outcome) = client.handle_frame(sdo, ns_since(epoch))? {
                    return Ok(outcome);
                }
            }
            Some(Err(e)) => return Err(e.into()),
            None => {
                if let Some(outcome) = client.handle_timeout(ns_since(epoch))? {
                    return Ok(outcome);
                }
                // Timeout path queues an abort frame; drain it next loop.
            }
        }
    }
}

// ---------- frame conversion ----------

fn sdo_to_can(f: SdoFrame) -> Result<CanFrame, CanIoError> {
    CanFrame::new_data(CanId::Standard(f.cob_id), &f.data)
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
