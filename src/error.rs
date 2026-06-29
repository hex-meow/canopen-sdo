//! Error type returned from the sans-io state machines.
//!
//! Note: this is the *protocol-level* error. IO errors are reported through
//! whatever transport layer the async glue uses (e.g. `can_transport::CanIoError`)
//! and converted in the glue.

use crate::abort::SdoAbortCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdoError {
    /// The remote server aborted the transfer with the given code.
    ServerAborted(SdoAbortCode),

    /// The local peer aborted the transfer (e.g. on timeout or protocol
    /// violation). The matching abort frame has already been queued for
    /// transmission via `poll_transmit`.
    ClientAborted(SdoAbortCode),

    /// A protocol violation that doesn't fit any abort code cleanly. The caller
    /// should treat the peer as `Idle` and not retry the transfer without
    /// further investigation.
    Protocol(&'static str),

    /// Attempted to start a new transfer while busy with another one.
    Busy,

    /// Invalid input passed to `begin_*` (e.g. node id 0 or > 127, or empty
    /// download payload).
    InvalidArgument(&'static str),
}

impl core::fmt::Display for SdoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SdoError::ServerAborted(c) => write!(f, "server aborted: {c}"),
            SdoError::ClientAborted(c) => write!(f, "client aborted: {c}"),
            SdoError::Protocol(s) => write!(f, "protocol violation: {s}"),
            SdoError::Busy => write!(f, "SDO peer is busy with another transfer"),
            SdoError::InvalidArgument(s) => write!(f, "invalid argument: {s}"),
        }
    }
}

// `core::error::Error` is stable since Rust 1.81 and is re-exported as
// `std::error::Error`, so host users still get `?`/`anyhow`/`Box<dyn Error>`.
impl core::error::Error for SdoError {}
