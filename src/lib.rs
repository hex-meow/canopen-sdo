//! Sans-IO CANopen SDO (CiA 301) — client *and* server.
//!
//! The protocol state machines do **no I/O**: you feed them CAN frames and
//! they tell you which frames to send next and when to fire a timeout. This
//! makes the logic trivial to test and reusable across any async runtime,
//! blocking driver, or embedded HAL.
//!
//! Two roles, picked by feature:
//!
//! * [`client::SdoClient`] (`client` feature, needs `alloc`) — the master that
//!   reads/writes a remote node's object dictionary. For a ready-to-use async
//!   client over [`can_transport::CanBus`], enable `tokio` (on by default) and
//!   use [`asynch::upload_bytes`] / [`asynch::download_bytes`].
//! * [`server::SdoServer`] (`server` feature, `no_std` + no alloc) — the node
//!   side that answers requests against a caller-provided
//!   [`server::ObjectDictionary`]. Built for embedded targets (heapless).
//!
//! Time is a monotonic `u64` of caller-chosen unit (the convenience glue uses
//! **nanoseconds**); `now` and any timeout must share that unit. A `u64` of
//! nanoseconds takes ~584 years to overflow, so wraparound is not handled.
//!
//! See `examples/upload_download.rs` for a full SocketCAN demo.

// no_std for the embedded (client/server) builds; the `tokio` glue and the test
// harness need std.
#![cfg_attr(not(any(test, feature = "tokio")), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod abort;
pub mod error;
pub mod frame;
#[cfg(any(feature = "client", feature = "server"))]
mod wire;

pub use abort::SdoAbortCode;
pub use error::SdoError;
pub use frame::{SdoFrame, COB_RSDO_BASE, COB_TSDO_BASE};

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub use client::{SdoClient, SdoConfig, SdoOutcome};

#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub use server::{ObjectDictionary, SdoServer, ServerConfig};

#[cfg(feature = "tokio")]
pub mod asynch;

#[cfg(all(test, feature = "client", feature = "server"))]
mod loopback_tests;
