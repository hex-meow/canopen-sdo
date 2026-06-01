//! Error type returned from the sans-io state machine.
//!
//! Note: this is the *protocol-level* error. IO errors are reported
//! through whatever transport layer the async glue uses (e.g.
//! `can_transport::CanIoError`) and converted in the glue.

use thiserror::Error;

use crate::abort::SdoAbortCode;

#[derive(Debug, Clone, Error)]
pub enum SdoError {
    /// The remote server aborted the transfer with the given code.
    #[error("server aborted: {0}")]
    ServerAborted(SdoAbortCode),

    /// The local client aborted the transfer (e.g. on timeout or
    /// protocol violation). The matching abort frame has already been
    /// queued for transmission via `poll_transmit`.
    #[error("client aborted: {0}")]
    ClientAborted(SdoAbortCode),

    /// A protocol violation that doesn't fit any abort code cleanly.
    /// The caller should treat the client as `Idle` and not retry the
    /// transfer without further investigation.
    #[error("protocol violation: {0}")]
    Protocol(&'static str),

    /// Attempted to start a new transfer while the client was busy with
    /// another one.
    #[error("SDO client is busy with another transfer")]
    Busy,

    /// Invalid input passed to `begin_*` (e.g. node id 0 or > 127, or
    /// empty download payload).
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
}
