//! Shared CiA 301 SDO wire-format constants and codec helpers used by both the
//! client and the server, so the two roles can never drift apart on the bit
//! layout (notably the segment encoding the spec puts in byte 0).

use crate::abort::SdoAbortCode;

// ---- Client Command Specifiers (ccs), top 3 bits of byte 0 ----
pub const CCS_DOWNLOAD_SEGMENT: u8 = 0 << 5;
pub const CCS_INIT_DOWNLOAD: u8 = 1 << 5;
pub const CCS_INIT_UPLOAD: u8 = 2 << 5;
pub const CCS_UPLOAD_SEGMENT: u8 = 3 << 5;
pub const CS_ABORT: u8 = 4 << 5;

// ---- Server Command Specifiers (scs) ----
pub const SCS_UPLOAD_SEGMENT: u8 = 0 << 5;
pub const SCS_DOWNLOAD_SEGMENT: u8 = 1 << 5;
pub const SCS_INIT_UPLOAD: u8 = 2 << 5;
pub const SCS_INIT_DOWNLOAD: u8 = 3 << 5;

pub const CS_MASK: u8 = 0b1110_0000;
pub const TOGGLE_BIT: u8 = 0b0001_0000;

/// Index (bytes 1..3) of an init/abort frame.
pub fn index_of(data: &[u8; 8]) -> u16 {
    u16::from_le_bytes([data[1], data[2]])
}

/// Sub-index (byte 3) of an init/abort frame.
pub fn subindex_of(data: &[u8; 8]) -> u8 {
    data[3]
}

/// Build the 8-byte abort payload (identical layout client- or server-side).
pub fn abort(idx: u16, sub: u8, code: SdoAbortCode) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[0] = CS_ABORT;
    out[1] = idx as u8;
    out[2] = (idx >> 8) as u8;
    out[3] = sub;
    out[4..8].copy_from_slice(&code.raw().to_le_bytes());
    out
}

/// Decode a *segment* command byte (upload-segment scs or download-segment ccs):
/// `n=(cmd>>1)&7`, payload is `7-n` bytes in `data[1..1+len]`, `c=cmd&1` marks
/// the final segment. This is the bit layout the spec defines and the one the
/// firmware historically got wrong.
pub fn decode_segment(cmd: u8) -> (usize, bool) {
    let n = ((cmd >> 1) & 0b111) as usize;
    let payload_len = 7 - n;
    let complete = (cmd & 0b1) != 0;
    (payload_len, complete)
}

/// Build a *segment* command byte. `cs` is [`CCS_DOWNLOAD_SEGMENT`] (client) or
/// [`SCS_UPLOAD_SEGMENT`] (server).
pub fn segment_cmd(cs: u8, toggle: bool, payload_len: usize, last: bool) -> u8 {
    // payload_len 0 is only valid for the single final segment of a zero-length
    // object (n=7, c=1); 1..=7 otherwise.
    debug_assert!(payload_len <= 7);
    let n = (7 - payload_len) as u8;
    cs | if toggle { TOGGLE_BIT } else { 0 } | (n << 1) | if last { 1 } else { 0 }
}

/// Build a *segment* frame payload from a command byte and up to 7 data bytes.
pub fn segment_frame(cmd: u8, payload: &[u8]) -> [u8; 8] {
    debug_assert!(payload.len() <= 7);
    let mut out = [0u8; 8];
    out[0] = cmd;
    out[1..1 + payload.len()].copy_from_slice(payload);
    out
}
