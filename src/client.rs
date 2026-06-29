//! Sans-IO CANopen SDO client state machine (CiA 301).
//!
//! The client knows nothing about CAN drivers, async runtimes, or timers. It
//! exposes four primitives:
//!
//! * [`SdoClient::handle_frame`] — feed an incoming TSDO frame
//! * [`SdoClient::poll_transmit`] — drain frames the caller must send
//! * [`SdoClient::poll_timeout`]  — query the next deadline
//! * [`SdoClient::handle_timeout`] — notify the machine the deadline expired
//!
//! Transfers are started with [`SdoClient::begin_upload`] /
//! [`SdoClient::begin_download`]. Only one transfer is in flight at a time;
//! trying to start another returns [`SdoError::Busy`].
//!
//! Time is a monotonic `u64` (caller's unit; nanoseconds in the tokio glue).

use alloc::vec::Vec;

use crate::abort::SdoAbortCode;
use crate::error::SdoError;
use crate::frame::SdoFrame;
use crate::wire::{
    self, CCS_INIT_DOWNLOAD, CCS_INIT_UPLOAD, CCS_UPLOAD_SEGMENT, CS_ABORT, CS_MASK,
    SCS_DOWNLOAD_SEGMENT, SCS_INIT_DOWNLOAD, SCS_INIT_UPLOAD, SCS_UPLOAD_SEGMENT, TOGGLE_BIT,
};

/// Tunable SDO client behaviour.
#[derive(Debug, Clone, Copy)]
pub struct SdoConfig {
    /// How long to wait for any single server response before declaring
    /// `ProtocolTimeout` and aborting, in the caller's monotonic `u64` time
    /// unit (nanoseconds in the tokio glue).
    pub timeout: u64,
}

impl Default for SdoConfig {
    fn default() -> Self {
        Self {
            timeout: 150_000_000, // 150 ms in ns
        }
    }
}

/// What a successful transfer produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdoOutcome {
    /// Upload (read from server) finished. The bytes are the payload.
    UploadCompleted(Vec<u8>),
    /// Download (write to server) finished.
    DownloadCompleted,
}

/// SDO client state machine. Construct once and reuse across many sequential
/// transfers.
#[derive(Debug)]
pub struct SdoClient {
    cfg: SdoConfig,
    state: State,
    pending_tx: Option<SdoFrame>,
    deadline: Option<u64>,
}

#[derive(Debug)]
enum State {
    Idle,
    Uploading(UploadCtx),
    Downloading(DownloadCtx),
}

#[derive(Debug)]
struct UploadCtx {
    node_id: u8,
    idx: u16,
    sub: u8,
    received: Vec<u8>,
    expected_len: Option<u32>,
    /// Toggle to send in the *next* segment request.
    next_toggle: bool,
    phase: UploadPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadPhase {
    AwaitingInit,
    AwaitingSegment,
}

#[derive(Debug)]
struct DownloadCtx {
    node_id: u8,
    idx: u16,
    sub: u8,
    data: Vec<u8>,
    /// How many payload bytes have been *sent* on the wire so far.
    sent_pos: usize,
    /// Toggle to use in the *next* segment we send.
    next_toggle: bool,
    phase: DownloadPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadPhase {
    AwaitingInit { expedited: bool },
    AwaitingSegmentAck { last: bool },
}

impl SdoClient {
    pub fn new(cfg: SdoConfig) -> Self {
        Self {
            cfg,
            state: State::Idle,
            pending_tx: None,
            deadline: None,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    // ----- transfer kick-off -----

    pub fn begin_upload(&mut self, node_id: u8, idx: u16, sub: u8, now: u64) -> Result<(), SdoError> {
        validate_node_id(node_id)?;
        if !self.is_idle() {
            return Err(SdoError::Busy);
        }
        self.queue_request(node_id, enc::init_upload(idx, sub), now);
        self.state = State::Uploading(UploadCtx {
            node_id,
            idx,
            sub,
            received: Vec::new(),
            expected_len: None,
            next_toggle: false,
            phase: UploadPhase::AwaitingInit,
        });
        Ok(())
    }

    pub fn begin_download(
        &mut self,
        node_id: u8,
        idx: u16,
        sub: u8,
        data: &[u8],
        now: u64,
    ) -> Result<(), SdoError> {
        validate_node_id(node_id)?;
        if data.is_empty() {
            return Err(SdoError::InvalidArgument("download payload empty"));
        }
        if !self.is_idle() {
            return Err(SdoError::Busy);
        }
        let expedited = data.len() <= 4;
        let init_bytes = if expedited {
            enc::init_download_expedited(idx, sub, data)
        } else {
            enc::init_download_segmented(idx, sub, data.len() as u32)
        };
        self.queue_request(node_id, init_bytes, now);
        self.state = State::Downloading(DownloadCtx {
            node_id,
            idx,
            sub,
            data: data.to_vec(),
            sent_pos: 0,
            next_toggle: false,
            phase: DownloadPhase::AwaitingInit { expedited },
        });
        Ok(())
    }

    // ----- sans-io driving methods -----

    pub fn poll_transmit(&mut self) -> Option<SdoFrame> {
        self.pending_tx.take()
    }

    pub fn poll_timeout(&self) -> Option<u64> {
        self.deadline
    }

    pub fn handle_timeout(&mut self, now: u64) -> Result<Option<SdoOutcome>, SdoError> {
        let Some(deadline) = self.deadline else {
            return Ok(None);
        };
        if now < deadline {
            return Ok(None);
        }
        // Time really is up. Abort.
        self.abort_with(SdoAbortCode::ProtocolTimeout);
        Err(SdoError::ClientAborted(SdoAbortCode::ProtocolTimeout))
    }

    pub fn handle_frame(&mut self, frame: SdoFrame, now: u64) -> Result<Option<SdoOutcome>, SdoError> {
        let Some(frame_node) = SdoFrame::node_of_tsdo(frame.cob_id) else {
            return Ok(None);
        };
        let cmd = frame.data[0];
        let cs = cmd & CS_MASK;

        if cs == CS_ABORT {
            if !self.matches_in_flight(frame_node, &frame.data) {
                return Ok(None);
            }
            let code = SdoAbortCode::from(u32::from_le_bytes([
                frame.data[4],
                frame.data[5],
                frame.data[6],
                frame.data[7],
            ]));
            self.state = State::Idle;
            self.pending_tx = None;
            self.deadline = None;
            return Err(SdoError::ServerAborted(code));
        }

        match core::mem::replace(&mut self.state, State::Idle) {
            State::Idle => Ok(None),
            State::Uploading(ctx) => self.advance_upload(ctx, frame_node, cmd, &frame.data, now),
            State::Downloading(ctx) => self.advance_download(ctx, frame_node, cmd, &frame.data, now),
        }
    }

    /// Manually abort the current transfer with a custom code. Queues the
    /// matching abort frame for transmission; the caller must still drain
    /// `poll_transmit` to actually put it on the wire.
    pub fn abort(&mut self, code: SdoAbortCode) {
        self.abort_with(code);
    }

    // ----- internal: state-machine arms -----

    fn advance_upload(
        &mut self,
        mut ctx: UploadCtx,
        frame_node: u8,
        cmd: u8,
        data: &[u8; 8],
        now: u64,
    ) -> Result<Option<SdoOutcome>, SdoError> {
        if frame_node != ctx.node_id {
            self.state = State::Uploading(ctx);
            return Ok(None);
        }
        let scs = cmd & CS_MASK;
        match ctx.phase {
            UploadPhase::AwaitingInit => {
                if scs != SCS_INIT_UPLOAD {
                    self.state = State::Uploading(ctx);
                    return Ok(None);
                }
                let frame_idx = wire::index_of(data);
                let frame_sub = wire::subindex_of(data);
                if frame_idx != ctx.idx || frame_sub != ctx.sub {
                    self.state = State::Uploading(ctx);
                    return Ok(None);
                }
                let e = (cmd & 0b10) != 0;
                let s = (cmd & 0b01) != 0;
                if e {
                    let len = if s {
                        let n = (cmd >> 2) & 0b11;
                        4 - n as usize
                    } else {
                        4
                    };
                    let mut out = Vec::with_capacity(len);
                    out.extend_from_slice(&data[4..4 + len]);
                    self.clear_transfer();
                    Ok(Some(SdoOutcome::UploadCompleted(out)))
                } else {
                    if s {
                        ctx.expected_len = Some(u32::from_le_bytes([data[4], data[5], data[6], data[7]]));
                        ctx.received.reserve(ctx.expected_len.unwrap_or(0) as usize);
                    }
                    let toggle = ctx.next_toggle;
                    self.queue_request(ctx.node_id, enc::upload_segment_request(toggle), now);
                    ctx.phase = UploadPhase::AwaitingSegment;
                    self.state = State::Uploading(ctx);
                    Ok(None)
                }
            }
            UploadPhase::AwaitingSegment => {
                if scs != SCS_UPLOAD_SEGMENT {
                    self.state = State::Uploading(ctx);
                    return Ok(None);
                }
                let frame_toggle = (cmd & TOGGLE_BIT) != 0;
                if frame_toggle != ctx.next_toggle {
                    self.queue_abort(ctx.node_id, ctx.idx, ctx.sub, SdoAbortCode::ToggleBitNotAlternated);
                    return Err(SdoError::ClientAborted(SdoAbortCode::ToggleBitNotAlternated));
                }
                let (payload_len, c) = wire::decode_segment(cmd);
                ctx.received.extend_from_slice(&data[1..1 + payload_len]);
                if c {
                    if let Some(exp) = ctx.expected_len {
                        if ctx.received.len() != exp as usize {
                            self.queue_abort(ctx.node_id, ctx.idx, ctx.sub, SdoAbortCode::DataTypeLengthMismatch);
                            return Err(SdoError::ClientAborted(SdoAbortCode::DataTypeLengthMismatch));
                        }
                    }
                    let out = core::mem::take(&mut ctx.received);
                    self.clear_transfer();
                    Ok(Some(SdoOutcome::UploadCompleted(out)))
                } else {
                    ctx.next_toggle = !ctx.next_toggle;
                    let toggle = ctx.next_toggle;
                    self.queue_request(ctx.node_id, enc::upload_segment_request(toggle), now);
                    self.state = State::Uploading(ctx);
                    Ok(None)
                }
            }
        }
    }

    fn advance_download(
        &mut self,
        mut ctx: DownloadCtx,
        frame_node: u8,
        cmd: u8,
        data: &[u8; 8],
        now: u64,
    ) -> Result<Option<SdoOutcome>, SdoError> {
        if frame_node != ctx.node_id {
            self.state = State::Downloading(ctx);
            return Ok(None);
        }
        let scs = cmd & CS_MASK;
        match ctx.phase {
            DownloadPhase::AwaitingInit { expedited } => {
                if scs != SCS_INIT_DOWNLOAD {
                    self.state = State::Downloading(ctx);
                    return Ok(None);
                }
                let frame_idx = wire::index_of(data);
                let frame_sub = wire::subindex_of(data);
                if frame_idx != ctx.idx || frame_sub != ctx.sub {
                    self.state = State::Downloading(ctx);
                    return Ok(None);
                }
                if expedited {
                    self.clear_transfer();
                    Ok(Some(SdoOutcome::DownloadCompleted))
                } else {
                    self.queue_next_download_segment(&mut ctx, now);
                    self.state = State::Downloading(ctx);
                    Ok(None)
                }
            }
            DownloadPhase::AwaitingSegmentAck { last } => {
                if scs != SCS_DOWNLOAD_SEGMENT {
                    self.state = State::Downloading(ctx);
                    return Ok(None);
                }
                let frame_toggle = (cmd & TOGGLE_BIT) != 0;
                // The toggle of the ack is the same as the toggle of the segment
                // we just sent, i.e. the *previous* value of `next_toggle`
                // (which we flipped after sending).
                let expected = !ctx.next_toggle;
                if frame_toggle != expected {
                    self.queue_abort(ctx.node_id, ctx.idx, ctx.sub, SdoAbortCode::ToggleBitNotAlternated);
                    return Err(SdoError::ClientAborted(SdoAbortCode::ToggleBitNotAlternated));
                }
                if last {
                    self.clear_transfer();
                    Ok(Some(SdoOutcome::DownloadCompleted))
                } else {
                    self.queue_next_download_segment(&mut ctx, now);
                    self.state = State::Downloading(ctx);
                    Ok(None)
                }
            }
        }
    }

    fn queue_next_download_segment(&mut self, ctx: &mut DownloadCtx, now: u64) {
        let remaining = ctx.data.len() - ctx.sent_pos;
        let chunk_len = remaining.min(7);
        let last = remaining <= 7;
        let toggle = ctx.next_toggle;
        let payload = &ctx.data[ctx.sent_pos..ctx.sent_pos + chunk_len];
        let bytes = enc::download_segment(toggle, payload, last);
        ctx.sent_pos += chunk_len;
        ctx.next_toggle = !ctx.next_toggle;
        ctx.phase = DownloadPhase::AwaitingSegmentAck { last };
        self.queue_request(ctx.node_id, bytes, now);
    }

    // ----- internal helpers -----

    fn queue_request(&mut self, node_id: u8, bytes: [u8; 8], now: u64) {
        self.pending_tx = Some(SdoFrame::new(SdoFrame::rsdo_id(node_id), bytes));
        self.deadline = Some(now + self.cfg.timeout);
    }

    fn clear_transfer(&mut self) {
        self.state = State::Idle;
        self.pending_tx = None;
        self.deadline = None;
    }

    fn abort_with(&mut self, code: SdoAbortCode) {
        let (node_id, idx, sub) = match &self.state {
            State::Idle => return,
            State::Uploading(u) => (u.node_id, u.idx, u.sub),
            State::Downloading(d) => (d.node_id, d.idx, d.sub),
        };
        self.queue_abort(node_id, idx, sub, code);
    }

    /// Like [`Self::abort_with`] but for use inside `advance_*` after
    /// `self.state` has been moved out via `mem::replace`.
    fn queue_abort(&mut self, node_id: u8, idx: u16, sub: u8, code: SdoAbortCode) {
        self.pending_tx = Some(SdoFrame::new(SdoFrame::rsdo_id(node_id), wire::abort(idx, sub, code)));
        self.deadline = None;
        self.state = State::Idle;
    }

    fn matches_in_flight(&self, frame_node: u8, data: &[u8; 8]) -> bool {
        let frame_idx = wire::index_of(data);
        let frame_sub = wire::subindex_of(data);
        match &self.state {
            State::Idle => false,
            State::Uploading(u) => u.node_id == frame_node && u.idx == frame_idx && u.sub == frame_sub,
            State::Downloading(d) => d.node_id == frame_node && d.idx == frame_idx && d.sub == frame_sub,
        }
    }
}

fn validate_node_id(node_id: u8) -> Result<(), SdoError> {
    if node_id == 0 || node_id > 127 {
        Err(SdoError::InvalidArgument("node id must be 1..=127"))
    } else {
        Ok(())
    }
}

// ---------- Client-side wire-format encoders ----------

mod enc {
    use super::*;
    use crate::wire::{segment_cmd, segment_frame, CCS_DOWNLOAD_SEGMENT};

    pub fn init_upload(idx: u16, sub: u8) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0] = CCS_INIT_UPLOAD;
        out[1] = idx as u8;
        out[2] = (idx >> 8) as u8;
        out[3] = sub;
        out
    }

    pub fn upload_segment_request(toggle: bool) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0] = CCS_UPLOAD_SEGMENT | if toggle { TOGGLE_BIT } else { 0 };
        out
    }

    pub fn init_download_expedited(idx: u16, sub: u8, data: &[u8]) -> [u8; 8] {
        debug_assert!(!data.is_empty() && data.len() <= 4);
        let n = (4 - data.len()) as u8;
        let cmd = CCS_INIT_DOWNLOAD | (n << 2) | 0b11; // e=1, s=1
        let mut out = [0u8; 8];
        out[0] = cmd;
        out[1] = idx as u8;
        out[2] = (idx >> 8) as u8;
        out[3] = sub;
        out[4..4 + data.len()].copy_from_slice(data);
        out
    }

    pub fn init_download_segmented(idx: u16, sub: u8, total_len: u32) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0] = CCS_INIT_DOWNLOAD | 0b01; // e=0, s=1
        out[1] = idx as u8;
        out[2] = (idx >> 8) as u8;
        out[3] = sub;
        out[4..8].copy_from_slice(&total_len.to_le_bytes());
        out
    }

    pub fn download_segment(toggle: bool, payload: &[u8], last: bool) -> [u8; 8] {
        let cmd = segment_cmd(CCS_DOWNLOAD_SEGMENT, toggle, payload.len(), last);
        segment_frame(cmd, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{CCS_DOWNLOAD_SEGMENT, CCS_INIT_DOWNLOAD, CCS_UPLOAD_SEGMENT};

    // Sans-io: time is an opaque monotonic u64; tests just use 0 and explicit
    // offsets.
    fn t0() -> u64 {
        0
    }

    fn server_frame(node_id: u8, data: [u8; 8]) -> SdoFrame {
        SdoFrame::new(SdoFrame::tsdo_id(node_id), data)
    }

    // ---------- upload expedited ----------

    #[test]
    fn upload_expedited_4_bytes() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_upload(0x10, 0x6041, 0x00, t0()).unwrap();
        let req = c.poll_transmit().unwrap();
        assert_eq!(req.cob_id, 0x610);
        assert_eq!(req.data[0], CCS_INIT_UPLOAD);
        assert_eq!(u16::from_le_bytes([req.data[1], req.data[2]]), 0x6041);

        // Server replies expedited (e=1, s=1, n=0): 4 bytes of data
        let mut data = [0u8; 8];
        data[0] = SCS_INIT_UPLOAD | 0b11;
        data[1] = 0x41;
        data[2] = 0x60;
        data[3] = 0x00;
        data[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let out = c.handle_frame(server_frame(0x10, data), t0()).unwrap();
        assert_eq!(out, Some(SdoOutcome::UploadCompleted(vec![0xDE, 0xAD, 0xBE, 0xEF])));
        assert!(c.is_idle());
    }

    #[test]
    fn upload_expedited_2_bytes() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_upload(1, 0x1000, 0x00, t0()).unwrap();
        c.poll_transmit().unwrap();

        let mut data = [0u8; 8];
        // n=2 → 2 bytes data
        data[0] = SCS_INIT_UPLOAD | (2 << 2) | 0b11;
        data[1] = 0x00;
        data[2] = 0x10;
        data[3] = 0x00;
        data[4] = 0x12;
        data[5] = 0x34;
        let out = c.handle_frame(server_frame(1, data), t0()).unwrap();
        assert_eq!(out, Some(SdoOutcome::UploadCompleted(vec![0x12, 0x34])));
    }

    // ---------- upload segmented ----------

    #[test]
    fn upload_segmented_happy_path() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_upload(2, 0x2000, 0x01, t0()).unwrap();
        c.poll_transmit().unwrap();

        // Init response: segmented, total length 11.
        let mut init = [0u8; 8];
        init[0] = SCS_INIT_UPLOAD | 0b01; // e=0, s=1
        init[1] = 0x00;
        init[2] = 0x20;
        init[3] = 0x01;
        init[4..8].copy_from_slice(&11u32.to_le_bytes());
        let out = c.handle_frame(server_frame(2, init), t0()).unwrap();
        assert_eq!(out, None);
        let req = c.poll_transmit().unwrap();
        assert_eq!(req.data[0], CCS_UPLOAD_SEGMENT); // toggle=0

        // Segment 1: 7 bytes, c=0, toggle=0
        let mut seg1 = [0u8; 8];
        seg1[0] = SCS_UPLOAD_SEGMENT; // toggle=0, n=0, c=0
        seg1[1..8].copy_from_slice(b"hello, ");
        let out = c.handle_frame(server_frame(2, seg1), t0()).unwrap();
        assert_eq!(out, None);
        let req = c.poll_transmit().unwrap();
        assert_eq!(req.data[0], CCS_UPLOAD_SEGMENT | TOGGLE_BIT); // toggle=1

        // Segment 2: 4 bytes ("worl"), c=1, toggle=1 → n=3
        let mut seg2 = [0u8; 8];
        seg2[0] = SCS_UPLOAD_SEGMENT | TOGGLE_BIT | (3 << 1) | 1;
        seg2[1..5].copy_from_slice(b"worl");
        let out = c.handle_frame(server_frame(2, seg2), t0()).unwrap();
        assert_eq!(out, Some(SdoOutcome::UploadCompleted(b"hello, worl".to_vec())));
        assert!(c.is_idle());
    }

    #[test]
    fn upload_aborts_on_toggle_mismatch() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_upload(3, 0x3000, 0x00, t0()).unwrap();
        c.poll_transmit().unwrap();

        // Init response: segmented, total length 14
        let mut init = [0u8; 8];
        init[0] = SCS_INIT_UPLOAD | 0b01;
        init[1] = 0x00;
        init[2] = 0x30;
        init[3] = 0x00;
        init[4..8].copy_from_slice(&14u32.to_le_bytes());
        c.handle_frame(server_frame(3, init), t0()).unwrap();
        c.poll_transmit().unwrap();

        // Server replies with WRONG toggle (1 instead of 0)
        let mut seg = [0u8; 8];
        seg[0] = SCS_UPLOAD_SEGMENT | TOGGLE_BIT;
        seg[1..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
        let res = c.handle_frame(server_frame(3, seg), t0());
        assert!(matches!(res, Err(SdoError::ClientAborted(SdoAbortCode::ToggleBitNotAlternated))));
        // Abort frame must be queued
        let abort = c.poll_transmit().unwrap();
        assert_eq!(abort.data[0], CS_ABORT);
        assert!(c.is_idle());
    }

    // ---------- download expedited ----------

    #[test]
    fn download_expedited() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_download(0x10, 0x6040, 0x00, &[0x06, 0x00], t0()).unwrap();
        let req = c.poll_transmit().unwrap();
        // e=1, s=1, n=2
        assert_eq!(req.data[0], CCS_INIT_DOWNLOAD | (2 << 2) | 0b11);
        assert_eq!(&req.data[4..6], &[0x06, 0x00]);

        let mut ack = [0u8; 8];
        ack[0] = SCS_INIT_DOWNLOAD;
        ack[1] = 0x40;
        ack[2] = 0x60;
        ack[3] = 0x00;
        let out = c.handle_frame(server_frame(0x10, ack), t0()).unwrap();
        assert_eq!(out, Some(SdoOutcome::DownloadCompleted));
    }

    // ---------- download segmented ----------

    #[test]
    fn download_segmented() {
        let mut c = SdoClient::new(SdoConfig::default());
        let payload: Vec<u8> = (0u8..11).collect(); // 11 bytes -> needs segmenting
        c.begin_download(7, 0x2001, 0x02, &payload, t0()).unwrap();

        // Init request: segmented, size indicated
        let init = c.poll_transmit().unwrap();
        assert_eq!(init.data[0], CCS_INIT_DOWNLOAD | 0b01);
        assert_eq!(u32::from_le_bytes([init.data[4], init.data[5], init.data[6], init.data[7]]), 11);

        // Server acks the init
        let mut ack = [0u8; 8];
        ack[0] = SCS_INIT_DOWNLOAD;
        ack[1] = 0x01;
        ack[2] = 0x20;
        ack[3] = 0x02;
        c.handle_frame(server_frame(7, ack), t0()).unwrap();

        // First segment: 7 bytes, toggle=0, c=0
        let seg1 = c.poll_transmit().unwrap();
        assert_eq!(seg1.data[0], CCS_DOWNLOAD_SEGMENT); // toggle=0, n=0, c=0
        assert_eq!(&seg1.data[1..8], &payload[0..7]);

        // Server acks segment 1 with toggle=0
        let mut ack1 = [0u8; 8];
        ack1[0] = SCS_DOWNLOAD_SEGMENT;
        c.handle_frame(server_frame(7, ack1), t0()).unwrap();

        // Second segment: 4 bytes, toggle=1, c=1 (last) → n=3
        let seg2 = c.poll_transmit().unwrap();
        assert_eq!(seg2.data[0], CCS_DOWNLOAD_SEGMENT | TOGGLE_BIT | (3 << 1) | 1);
        assert_eq!(&seg2.data[1..5], &payload[7..11]);

        // Server acks segment 2 with toggle=1
        let mut ack2 = [0u8; 8];
        ack2[0] = SCS_DOWNLOAD_SEGMENT | TOGGLE_BIT;
        let out = c.handle_frame(server_frame(7, ack2), t0()).unwrap();
        assert_eq!(out, Some(SdoOutcome::DownloadCompleted));
        assert!(c.is_idle());
    }

    // ---------- server abort ----------

    #[test]
    fn server_abort_during_upload() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_upload(5, 0x1234, 0x00, t0()).unwrap();
        c.poll_transmit().unwrap();

        let mut abort = [0u8; 8];
        abort[0] = CS_ABORT;
        abort[1] = 0x34;
        abort[2] = 0x12;
        abort[3] = 0x00;
        abort[4..8].copy_from_slice(&SdoAbortCode::ObjectDoesNotExist.raw().to_le_bytes());

        let res = c.handle_frame(server_frame(5, abort), t0());
        assert!(matches!(res, Err(SdoError::ServerAborted(SdoAbortCode::ObjectDoesNotExist))));
        assert!(c.is_idle());
        // Client does not echo an abort frame in this case.
        assert!(c.poll_transmit().is_none());
    }

    // ---------- timeout ----------

    #[test]
    fn timeout_aborts() {
        let mut c = SdoClient::new(SdoConfig {
            timeout: 10_000_000, // 10 ms in ns
        });
        let now = 0u64;
        c.begin_upload(8, 0x1000, 0x00, now).unwrap();
        let _ = c.poll_transmit();
        let dl = c.poll_timeout().unwrap();

        // Pretend time advanced past the deadline
        let later = dl + 5_000_000;
        let res = c.handle_timeout(later);
        assert!(matches!(res, Err(SdoError::ClientAborted(SdoAbortCode::ProtocolTimeout))));
        let f = c.poll_transmit().unwrap();
        assert_eq!(f.data[0], CS_ABORT);
        assert!(c.is_idle());
    }

    // ---------- busy ----------

    #[test]
    fn cannot_begin_two_transfers() {
        let mut c = SdoClient::new(SdoConfig::default());
        c.begin_upload(1, 0x1000, 0, t0()).unwrap();
        assert!(matches!(c.begin_upload(1, 0x1001, 0, t0()), Err(SdoError::Busy)));
        assert!(matches!(c.begin_download(1, 0x1001, 0, &[1, 2], t0()), Err(SdoError::Busy)));
    }

    #[test]
    fn invalid_node_id() {
        let mut c = SdoClient::new(SdoConfig::default());
        assert!(c.begin_upload(0, 0x1000, 0, t0()).is_err());
        assert!(c.begin_upload(128, 0x1000, 0, t0()).is_err());
    }
}
