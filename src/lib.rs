//! Sans-IO CANopen SDO client (CiA 301).
//!
//! The core state machine lives in [`SdoClient`] and does **no I/O**:
//! you feed it CAN frames and it tells you which frames to send next
//! and when to fire a timeout. This makes the protocol logic trivial
//! to test and reusable across any async runtime, blocking driver, or
//! embedded HAL.
//!
//! For a ready-to-use async client on top of
//! [`can_transport::CanBus`], enable the `tokio` feature (on by
//! default) and use [`asynch::upload_bytes`] / [`asynch::download_bytes`].
//!
//! See `examples/upload_download.rs` for a full SocketCAN demo.

pub mod abort;
pub mod client;
pub mod error;
pub mod frame;

pub use abort::SdoAbortCode;
pub use client::{SdoClient, SdoConfig, SdoOutcome};
pub use error::SdoError;
pub use frame::{SdoFrame, COB_RSDO_BASE, COB_TSDO_BASE};

#[cfg(feature = "tokio")]
pub mod asynch;
