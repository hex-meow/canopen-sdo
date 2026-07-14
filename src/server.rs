//! Sans-IO CANopen SDO *server* (node side), CiA 301.
//!
//! The mirror image of [`crate::client::SdoClient`]: a pure state machine that
//! answers a single SDO master against a caller-provided [`ObjectDictionary`].
//! `no_std` and **no alloc** — a fixed `[u8; N]` buffer bounds the largest
//! object it can transfer (pick `N` for your dictionary's biggest entry).
//!
//! Drive it like the client:
//!
//! * [`SdoServer::handle_frame`] — feed an incoming RSDO request (with the OD)
//! * [`SdoServer::poll_transmit`] — drain the response/abort frame to send
//! * [`SdoServer::poll_timeout`] / [`SdoServer::handle_timeout`] — abort a
//!   stalled segmented transfer
//!
//! Time is a monotonic `u64` of the caller's unit (the firmware can pass
//! `embassy_time::Instant::now().as_ticks()` or nanoseconds).

use crate::abort::SdoAbortCode;
use crate::error::SdoError;
use crate::frame::SdoFrame;
use crate::wire::{
    self, CCS_DOWNLOAD_SEGMENT, CCS_INIT_DOWNLOAD, CCS_INIT_UPLOAD, CCS_UPLOAD_SEGMENT, CS_ABORT,
    CS_MASK, SCS_DOWNLOAD_SEGMENT, SCS_INIT_DOWNLOAD, SCS_INIT_UPLOAD, SCS_UPLOAD_SEGMENT,
    TOGGLE_BIT,
};

/// The object dictionary the server serves requests against.
///
/// All access goes through these two methods; the server handles the SDO
/// framing (expedited vs segmented, toggles, aborts) on top.
pub trait ObjectDictionary {
    /// Read the value at (`index`, `sub`) into `buf`, returning the byte count
    /// (`<= buf.len()`). Return an abort code (e.g.
    /// [`SdoAbortCode::ObjectDoesNotExist`], [`SdoAbortCode::ReadWriteOnly`]) on
    /// failure. If the value is larger than `buf`, return
    /// [`SdoAbortCode::OutOfMemory`].
    fn read(&mut self, index: u16, sub: u8, buf: &mut [u8]) -> Result<usize, SdoAbortCode>;

    /// Write `data` to (`index`, `sub`). Return an abort code (e.g.
    /// [`SdoAbortCode::WriteReadOnly`], [`SdoAbortCode::InvalidValue`]) on
    /// failure.
    fn write(&mut self, index: u16, sub: u8, data: &[u8]) -> Result<(), SdoAbortCode>;
}

/// Server tunables.
#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    /// This node's id (1..=127); the server answers RSDO `0x600 + node_id`.
    pub node_id: u8,
    /// How long to wait for the next frame of an in-progress *segmented*
    /// transfer before aborting it (caller's monotonic `u64` unit). `0` disables
    /// the timeout.
    pub timeout: u64,
}

impl ServerConfig {
    /// Default config: 1-second segmented-transfer timeout (in nanoseconds).
    pub fn new(node_id: u8) -> Self {
        Self {
            node_id,
            timeout: 1_000_000_000,
        }
    }
}

/// SDO server state machine, generic over the max object size `N` (bytes).
pub struct SdoServer<const N: usize> {
    cfg: ServerConfig,
    state: State<N>,
    pending_tx: Option<SdoFrame>,
    deadline: Option<u64>,
}

enum State<const N: usize> {
    Idle,
    /// Serving a segmented upload (read): `buf[..len]` is the value, `sent`
    /// bytes already on the wire, `toggle` expected in the next request.
    Uploading {
        idx: u16,
        sub: u8,
        buf: [u8; N],
        len: usize,
        sent: usize,
        toggle: bool,
    },
    /// Receiving a segmented download (write): `buf[..len]` accumulated so far,
    /// `expected` total if the client indicated size, `toggle` expected next.
    Downloading {
        idx: u16,
        sub: u8,
        buf: [u8; N],
        len: usize,
        expected: Option<usize>,
        toggle: bool,
    },
}

impl<const N: usize> SdoServer<N> {
    pub fn new(cfg: ServerConfig) -> Self {
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

    pub fn poll_transmit(&mut self) -> Option<SdoFrame> {
        self.pending_tx.take()
    }

    pub fn poll_timeout(&self) -> Option<u64> {
        self.deadline
    }

    /// Notify the server the deadline expired. Aborts an in-progress segmented
    /// transfer (queues an abort frame) if `now` is past the deadline.
    pub fn handle_timeout(&mut self, now: u64) -> Result<(), SdoError> {
        let Some(dl) = self.deadline else {
            return Ok(());
        };
        if now < dl {
            return Ok(());
        }
        let (idx, sub) = match &self.state {
            State::Uploading { idx, sub, .. } | State::Downloading { idx, sub, .. } => (*idx, *sub),
            State::Idle => {
                self.deadline = None;
                return Ok(());
            }
        };
        self.abort(idx, sub, SdoAbortCode::ProtocolTimeout)
    }

    /// Feed an incoming RSDO request. Reads/writes go through `od`. Queues the
    /// response (or abort) for [`Self::poll_transmit`].
    ///
    /// Returns `Err(SdoError::ClientAborted(code))` when the *server* aborts
    /// (the abort frame is also queued, for the caller to log/send); `Ok(())`
    /// otherwise (including when the client itself sends an abort).
    pub fn handle_frame(
        &mut self,
        frame: SdoFrame,
        now: u64,
        od: &mut impl ObjectDictionary,
    ) -> Result<(), SdoError> {
        if SdoFrame::node_of_rsdo(frame.cob_id) != Some(self.cfg.node_id) {
            return Ok(()); // not addressed to this node's SDO server
        }
        let cmd = frame.data[0];
        match cmd & CS_MASK {
            CS_ABORT => {
                // Client aborted; drop any in-progress transfer silently.
                self.reset();
                Ok(())
            }
            CCS_INIT_UPLOAD => self.on_init_upload(&frame.data, now, od),
            CCS_UPLOAD_SEGMENT => self.on_upload_segment(cmd, now),
            CCS_INIT_DOWNLOAD => self.on_init_download(cmd, &frame.data, now, od),
            CCS_DOWNLOAD_SEGMENT => self.on_download_segment(cmd, &frame.data, now, od),
            _ => self.abort(
                wire::index_of(&frame.data),
                wire::subindex_of(&frame.data),
                SdoAbortCode::InvalidCommandSpecifier,
            ),
        }
    }

    // ----- request handlers -----

    fn on_init_upload(
        &mut self,
        data: &[u8; 8],
        now: u64,
        od: &mut impl ObjectDictionary,
    ) -> Result<(), SdoError> {
        let idx = wire::index_of(data);
        let sub = wire::subindex_of(data);
        let mut buf = [0u8; N];
        let len = match od.read(idx, sub, &mut buf) {
            Ok(l) => l,
            Err(code) => return self.abort(idx, sub, code),
        };
        // Defend the fixed buffer: a misbehaving OD must not be able to make us
        // index past N (the [u8; N] bound is a real safety boundary, not a
        // trusted contract).
        if len > N {
            return self.abort(idx, sub, SdoAbortCode::OutOfMemory);
        }

        if (1..=4).contains(&len) {
            // expedited: e=1, s=1, n=4-len (1..=4 bytes inline)
            let n = (4 - len) as u8;
            let mut out = [0u8; 8];
            out[0] = SCS_INIT_UPLOAD | (n << 2) | 0b11; // e=1, s=1
            out[1] = idx as u8;
            out[2] = (idx >> 8) as u8;
            out[3] = sub;
            out[4..4 + len].copy_from_slice(&buf[..len]);
            self.reset();
            self.queue(out);
            Ok(())
        } else {
            // segmented — covers len==0 (size 0, then one empty final segment)
            // and len>4: announce the total length, then serve segments
            let mut out = [0u8; 8];
            out[0] = SCS_INIT_UPLOAD | 0b01; // e=0, s=1
            out[1] = idx as u8;
            out[2] = (idx >> 8) as u8;
            out[3] = sub;
            out[4..8].copy_from_slice(&(len as u32).to_le_bytes());
            self.state = State::Uploading {
                idx,
                sub,
                buf,
                len,
                sent: 0,
                toggle: false,
            };
            self.queue_with_deadline(out, now);
            Ok(())
        }
    }

    fn on_upload_segment(&mut self, cmd: u8, now: u64) -> Result<(), SdoError> {
        let (idx, sub, len, sent, toggle) = match &self.state {
            State::Uploading {
                idx,
                sub,
                len,
                sent,
                toggle,
                ..
            } => (*idx, *sub, *len, *sent, *toggle),
            _ => return self.abort(0, 0, SdoAbortCode::InvalidCommandSpecifier),
        };
        if ((cmd & TOGGLE_BIT) != 0) != toggle {
            return self.abort(idx, sub, SdoAbortCode::ToggleBitNotAlternated);
        }
        let remaining = len - sent;
        let chunk = remaining.min(7);
        let last = remaining <= 7;
        let mut payload = [0u8; 7];
        if let State::Uploading { buf, .. } = &self.state {
            payload[..chunk].copy_from_slice(&buf[sent..sent + chunk]);
        }
        let seg_cmd = wire::segment_cmd(SCS_UPLOAD_SEGMENT, toggle, chunk, last);
        let out = wire::segment_frame(seg_cmd, &payload[..chunk]);
        if last {
            self.reset();
            self.queue(out);
        } else {
            if let State::Uploading { sent, toggle, .. } = &mut self.state {
                *sent += chunk;
                *toggle = !*toggle;
            }
            self.queue_with_deadline(out, now);
        }
        Ok(())
    }

    fn on_init_download(
        &mut self,
        cmd: u8,
        data: &[u8; 8],
        now: u64,
        od: &mut impl ObjectDictionary,
    ) -> Result<(), SdoError> {
        let idx = wire::index_of(data);
        let sub = wire::subindex_of(data);
        let e = (cmd & 0b10) != 0;
        let s = (cmd & 0b01) != 0;

        if e {
            // expedited write: if s, len = 4 - n; if size not indicated (s=0),
            // all four data bytes are significant (CiA 301).
            let len = if s {
                (4 - ((cmd >> 2) & 0b11)) as usize
            } else {
                4
            };
            if let Err(code) = od.write(idx, sub, &data[4..4 + len]) {
                return self.abort(idx, sub, code);
            }
            self.reset();
            self.queue(init_download_ack(idx, sub));
            Ok(())
        } else {
            let expected = if s {
                Some(u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize)
            } else {
                None
            };
            if matches!(expected, Some(exp) if exp > N) {
                return self.abort(idx, sub, SdoAbortCode::OutOfMemory);
            }
            self.state = State::Downloading {
                idx,
                sub,
                buf: [0u8; N],
                len: 0,
                expected,
                toggle: false,
            };
            self.queue_with_deadline(init_download_ack(idx, sub), now);
            Ok(())
        }
    }

    fn on_download_segment(
        &mut self,
        cmd: u8,
        data: &[u8; 8],
        now: u64,
        od: &mut impl ObjectDictionary,
    ) -> Result<(), SdoError> {
        let (idx, sub, len, expected, toggle) = match &self.state {
            State::Downloading {
                idx,
                sub,
                len,
                expected,
                toggle,
                ..
            } => (*idx, *sub, *len, *expected, *toggle),
            _ => return self.abort(0, 0, SdoAbortCode::InvalidCommandSpecifier),
        };
        if ((cmd & TOGGLE_BIT) != 0) != toggle {
            return self.abort(idx, sub, SdoAbortCode::ToggleBitNotAlternated);
        }
        let (payload_len, complete) = wire::decode_segment(cmd);
        if len + payload_len > N {
            return self.abort(idx, sub, SdoAbortCode::OutOfMemory);
        }
        if let State::Downloading { buf, len: l, .. } = &mut self.state {
            buf[*l..*l + payload_len].copy_from_slice(&data[1..1 + payload_len]);
            *l += payload_len;
        }

        // The ack echoes the toggle of the segment we just received.
        let ack = download_segment_ack(toggle);

        if complete {
            let final_len = len + payload_len;
            if matches!(expected, Some(exp) if exp != final_len) {
                return self.abort(idx, sub, SdoAbortCode::DataTypeLengthMismatch);
            }
            let result = match &self.state {
                State::Downloading { buf, .. } => od.write(idx, sub, &buf[..final_len]),
                _ => unreachable!(),
            };
            if let Err(code) = result {
                return self.abort(idx, sub, code);
            }
            self.reset();
            self.queue(ack);
        } else {
            if let State::Downloading { toggle, .. } = &mut self.state {
                *toggle = !*toggle;
            }
            self.queue_with_deadline(ack, now);
        }
        Ok(())
    }

    // ----- helpers -----

    fn reset(&mut self) {
        self.state = State::Idle;
        self.deadline = None;
        // Drop any undrained response so an abort/cancel always supersedes it
        // (mirrors the client's clear_transfer); callers that queue right after
        // reset() simply re-set it.
        self.pending_tx = None;
    }

    fn queue(&mut self, bytes: [u8; 8]) {
        self.pending_tx = Some(SdoFrame::new(SdoFrame::tsdo_id(self.cfg.node_id), bytes));
        self.deadline = None;
    }

    fn queue_with_deadline(&mut self, bytes: [u8; 8], now: u64) {
        self.pending_tx = Some(SdoFrame::new(SdoFrame::tsdo_id(self.cfg.node_id), bytes));
        self.deadline = if self.cfg.timeout > 0 {
            Some(now + self.cfg.timeout)
        } else {
            None
        };
    }

    fn abort(&mut self, idx: u16, sub: u8, code: SdoAbortCode) -> Result<(), SdoError> {
        self.state = State::Idle;
        self.deadline = None;
        self.pending_tx = Some(SdoFrame::new(
            SdoFrame::tsdo_id(self.cfg.node_id),
            wire::abort(idx, sub, code),
        ));
        Err(SdoError::ClientAborted(code))
    }
}

fn init_download_ack(idx: u16, sub: u8) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[0] = SCS_INIT_DOWNLOAD;
    out[1] = idx as u8;
    out[2] = (idx >> 8) as u8;
    out[3] = sub;
    out
}

fn download_segment_ack(toggle: bool) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[0] = SCS_DOWNLOAD_SEGMENT | if toggle { TOGGLE_BIT } else { 0 };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny object dictionary: a handful of (index, sub) -> bytes, plus access
    /// flags, enough to drive every server path.
    #[derive(Default)]
    struct TestOd {
        // (index, sub, value, writable, readable)
        entries: std::vec::Vec<(u16, u8, std::vec::Vec<u8>, bool, bool)>,
    }
    impl TestOd {
        fn ro(mut self, idx: u16, sub: u8, val: &[u8]) -> Self {
            self.entries.push((idx, sub, val.to_vec(), false, true));
            self
        }
        fn rw(mut self, idx: u16, sub: u8, val: &[u8]) -> Self {
            self.entries.push((idx, sub, val.to_vec(), true, true));
            self
        }
        fn wo(mut self, idx: u16, sub: u8, val: &[u8]) -> Self {
            self.entries.push((idx, sub, val.to_vec(), true, false));
            self
        }
        fn get(&self, idx: u16, sub: u8) -> Option<&[u8]> {
            self.entries
                .iter()
                .find(|e| e.0 == idx && e.1 == sub)
                .map(|e| e.2.as_slice())
        }
        fn find_mut(
            &mut self,
            idx: u16,
            sub: u8,
        ) -> Option<&mut (u16, u8, std::vec::Vec<u8>, bool, bool)> {
            self.entries.iter_mut().find(|e| e.0 == idx && e.1 == sub)
        }
    }
    impl ObjectDictionary for TestOd {
        fn read(&mut self, index: u16, sub: u8, buf: &mut [u8]) -> Result<usize, SdoAbortCode> {
            let e = self
                .entries
                .iter()
                .find(|e| e.0 == index && e.1 == sub)
                .ok_or(SdoAbortCode::ObjectDoesNotExist)?;
            if !e.4 {
                return Err(SdoAbortCode::ReadWriteOnly);
            }
            if e.2.len() > buf.len() {
                return Err(SdoAbortCode::OutOfMemory);
            }
            buf[..e.2.len()].copy_from_slice(&e.2);
            Ok(e.2.len())
        }
        fn write(&mut self, index: u16, sub: u8, data: &[u8]) -> Result<(), SdoAbortCode> {
            let e = self
                .find_mut(index, sub)
                .ok_or(SdoAbortCode::ObjectDoesNotExist)?;
            if !e.3 {
                return Err(SdoAbortCode::WriteReadOnly);
            }
            e.2 = data.to_vec();
            Ok(())
        }
    }

    const NODE: u8 = 0x10;

    fn srv() -> SdoServer<64> {
        SdoServer::new(ServerConfig::new(NODE))
    }
    fn req(data: [u8; 8]) -> SdoFrame {
        SdoFrame::new(SdoFrame::rsdo_id(NODE), data)
    }

    #[test]
    fn ignores_other_nodes() {
        let mut s = srv();
        let mut od = TestOd::default().ro(0x1000, 0, &[1, 2]);
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_UPLOAD;
        data[1] = 0x00;
        data[2] = 0x10;
        // wrong node id in cob-id
        let f = SdoFrame::new(SdoFrame::rsdo_id(0x20), data);
        s.handle_frame(f, 0, &mut od).unwrap();
        assert!(s.poll_transmit().is_none());
    }

    #[test]
    fn upload_expedited() {
        let mut s = srv();
        let mut od = TestOd::default().ro(0x1000, 0, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_UPLOAD;
        data[1] = 0x00;
        data[2] = 0x10;
        s.handle_frame(req(data), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.cob_id, 0x590);
        assert_eq!(r.data[0], SCS_INIT_UPLOAD | 0b11); // e=1,s=1,n=0
        assert_eq!(&r.data[4..8], &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(s.is_idle());
    }

    #[test]
    fn upload_segmented() {
        let mut s = srv();
        let value: std::vec::Vec<u8> = (0u8..11).collect();
        let mut od = TestOd::default().ro(0x2000, 1, &value);

        let mut init = [0u8; 8];
        init[0] = CCS_INIT_UPLOAD;
        init[1] = 0x00;
        init[2] = 0x20;
        init[3] = 1;
        s.handle_frame(req(init), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.data[0], SCS_INIT_UPLOAD | 0b01); // e=0,s=1
        assert_eq!(
            u32::from_le_bytes([r.data[4], r.data[5], r.data[6], r.data[7]]),
            11
        );

        // request segment, toggle=0 -> 7 bytes, c=0
        let mut seg_req = [0u8; 8];
        seg_req[0] = CCS_UPLOAD_SEGMENT; // toggle 0
        s.handle_frame(req(seg_req), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.data[0], SCS_UPLOAD_SEGMENT); // toggle 0, n=0, c=0
        assert_eq!(&r.data[1..8], &value[0..7]);

        // request segment, toggle=1 -> 4 bytes, c=1
        let mut seg_req2 = [0u8; 8];
        seg_req2[0] = CCS_UPLOAD_SEGMENT | TOGGLE_BIT;
        s.handle_frame(req(seg_req2), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.data[0], SCS_UPLOAD_SEGMENT | TOGGLE_BIT | (3 << 1) | 1); // n=3,c=1
        assert_eq!(&r.data[1..5], &value[7..11]);
        assert!(s.is_idle());
    }

    #[test]
    fn download_expedited_writes_od() {
        let mut s = srv();
        let mut od = TestOd::default().rw(0x6040, 0, &[0, 0]);
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_DOWNLOAD | (2 << 2) | 0b11; // e=1,s=1,n=2 -> 2 bytes
        data[1] = 0x40;
        data[2] = 0x60;
        data[4] = 0x06;
        data[5] = 0x00;
        s.handle_frame(req(data), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.data[0], SCS_INIT_DOWNLOAD);
        assert_eq!(od.get(0x6040, 0), Some(&[0x06, 0x00][..]));
    }

    #[test]
    fn download_segmented_writes_od() {
        let mut s = srv();
        let mut od = TestOd::default().rw(0x2001, 2, &[]);
        let payload: std::vec::Vec<u8> = (0u8..11).collect();

        // init download, segmented, size 11
        let mut init = [0u8; 8];
        init[0] = CCS_INIT_DOWNLOAD | 0b01; // e=0,s=1
        init[1] = 0x01;
        init[2] = 0x20;
        init[3] = 2;
        init[4..8].copy_from_slice(&11u32.to_le_bytes());
        s.handle_frame(req(init), 0, &mut od).unwrap();
        assert_eq!(s.poll_transmit().unwrap().data[0], SCS_INIT_DOWNLOAD);

        // segment 1: 7 bytes, toggle 0, c=0
        let seg1 = wire::segment_frame(
            wire::segment_cmd(CCS_DOWNLOAD_SEGMENT, false, 7, false),
            &payload[0..7],
        );
        s.handle_frame(req(seg1), 0, &mut od).unwrap();
        assert_eq!(s.poll_transmit().unwrap().data[0], SCS_DOWNLOAD_SEGMENT); // toggle 0

        // segment 2: 4 bytes, toggle 1, c=1
        let seg2 = wire::segment_frame(
            wire::segment_cmd(CCS_DOWNLOAD_SEGMENT, true, 4, true),
            &payload[7..11],
        );
        s.handle_frame(req(seg2), 0, &mut od).unwrap();
        assert_eq!(
            s.poll_transmit().unwrap().data[0],
            SCS_DOWNLOAD_SEGMENT | TOGGLE_BIT
        );
        assert_eq!(od.get(0x2001, 2), Some(&payload[..]));
        assert!(s.is_idle());
    }

    #[test]
    fn upload_unknown_object_aborts() {
        let mut s = srv();
        let mut od = TestOd::default();
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_UPLOAD;
        data[1] = 0x99;
        data[2] = 0x99;
        let res = s.handle_frame(req(data), 0, &mut od);
        assert!(matches!(
            res,
            Err(SdoError::ClientAborted(SdoAbortCode::ObjectDoesNotExist))
        ));
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.data[0], CS_ABORT);
        assert_eq!(
            u32::from_le_bytes([r.data[4], r.data[5], r.data[6], r.data[7]]),
            SdoAbortCode::ObjectDoesNotExist.raw()
        );
    }

    #[test]
    fn write_to_readonly_aborts() {
        let mut s = srv();
        let mut od = TestOd::default().ro(0x1008, 0, &[1, 2, 3, 4]);
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_DOWNLOAD | (0 << 2) | 0b11; // e=1,s=1,n=0 -> 4 bytes
        data[1] = 0x08;
        data[2] = 0x10;
        data[4..8].copy_from_slice(&[9, 9, 9, 9]);
        let res = s.handle_frame(req(data), 0, &mut od);
        assert!(matches!(
            res,
            Err(SdoError::ClientAborted(SdoAbortCode::WriteReadOnly))
        ));
        assert_eq!(s.poll_transmit().unwrap().data[0], CS_ABORT);
    }

    #[test]
    fn write_only_then_read_aborts() {
        let mut s = srv();
        let mut od = TestOd::default().wo(0x3000, 0, &[0]);
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_UPLOAD;
        data[1] = 0x00;
        data[2] = 0x30;
        let res = s.handle_frame(req(data), 0, &mut od);
        assert!(matches!(
            res,
            Err(SdoError::ClientAborted(SdoAbortCode::ReadWriteOnly))
        ));
    }

    #[test]
    fn upload_segment_toggle_mismatch_aborts() {
        let mut s = srv();
        let value: std::vec::Vec<u8> = (0u8..11).collect();
        let mut od = TestOd::default().ro(0x2000, 0, &value);
        let mut init = [0u8; 8];
        init[0] = CCS_INIT_UPLOAD;
        init[1] = 0x00;
        init[2] = 0x20;
        s.handle_frame(req(init), 0, &mut od).unwrap();
        s.poll_transmit().unwrap();
        // request with WRONG toggle (1 instead of 0)
        let mut bad = [0u8; 8];
        bad[0] = CCS_UPLOAD_SEGMENT | TOGGLE_BIT;
        let res = s.handle_frame(req(bad), 0, &mut od);
        assert!(matches!(
            res,
            Err(SdoError::ClientAborted(
                SdoAbortCode::ToggleBitNotAlternated
            ))
        ));
        assert!(s.is_idle());
    }

    #[test]
    fn upload_zero_length_is_segmented() {
        let mut s = srv();
        let mut od = TestOd::default().ro(0x2000, 0, &[]);
        let mut init = [0u8; 8];
        init[0] = CCS_INIT_UPLOAD;
        init[1] = 0x00;
        init[2] = 0x20;
        s.handle_frame(req(init), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        assert_eq!(r.data[0], SCS_INIT_UPLOAD | 0b01); // segmented (e=0, s=1)
        assert_eq!(
            u32::from_le_bytes([r.data[4], r.data[5], r.data[6], r.data[7]]),
            0
        );
        // The single final segment carries zero payload bytes, c=1.
        let mut seg = [0u8; 8];
        seg[0] = CCS_UPLOAD_SEGMENT; // toggle 0
        s.handle_frame(req(seg), 0, &mut od).unwrap();
        let r = s.poll_transmit().unwrap();
        let (plen, c) = wire::decode_segment(r.data[0]);
        assert_eq!(plen, 0);
        assert!(c);
        assert!(s.is_idle());
    }

    #[test]
    fn upload_oversized_od_aborts_not_panics() {
        // An OD that lies and reports more bytes than fit in the buffer must be
        // rejected, never indexed past N.
        struct LyingOd;
        impl ObjectDictionary for LyingOd {
            fn read(&mut self, _i: u16, _s: u8, _buf: &mut [u8]) -> Result<usize, SdoAbortCode> {
                Ok(100)
            }
            fn write(&mut self, _i: u16, _s: u8, _d: &[u8]) -> Result<(), SdoAbortCode> {
                Ok(())
            }
        }
        let mut s: SdoServer<8> = SdoServer::new(ServerConfig::new(NODE));
        let mut od = LyingOd;
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_UPLOAD;
        data[1] = 0x00;
        data[2] = 0x20;
        let res = s.handle_frame(SdoFrame::new(SdoFrame::rsdo_id(NODE), data), 0, &mut od);
        assert!(matches!(
            res,
            Err(SdoError::ClientAborted(SdoAbortCode::OutOfMemory))
        ));
        assert_eq!(s.poll_transmit().unwrap().data[0], CS_ABORT);
        assert!(s.is_idle());
    }

    #[test]
    fn download_expedited_no_size_writes_four_bytes() {
        let mut s = srv();
        let mut od = TestOd::default().rw(0x6000, 0, &[0, 0, 0, 0]);
        let mut data = [0u8; 8];
        data[0] = CCS_INIT_DOWNLOAD | 0b10; // e=1, s=0 (size not indicated)
        data[1] = 0x00;
        data[2] = 0x60;
        data[4..8].copy_from_slice(&[1, 2, 3, 4]);
        s.handle_frame(req(data), 0, &mut od).unwrap();
        assert_eq!(s.poll_transmit().unwrap().data[0], SCS_INIT_DOWNLOAD);
        assert_eq!(od.get(0x6000, 0), Some(&[1, 2, 3, 4][..]));
    }

    #[test]
    fn segmented_timeout_aborts() {
        let mut s = srv();
        let value: std::vec::Vec<u8> = (0u8..11).collect();
        let mut od = TestOd::default().ro(0x2000, 0, &value);
        let mut init = [0u8; 8];
        init[0] = CCS_INIT_UPLOAD;
        init[1] = 0x00;
        init[2] = 0x20;
        s.handle_frame(req(init), 0, &mut od).unwrap();
        s.poll_transmit().unwrap();
        let dl = s.poll_timeout().unwrap();
        // No client follow-up; deadline passes.
        let res = s.handle_timeout(dl + 1);
        assert!(matches!(
            res,
            Err(SdoError::ClientAborted(SdoAbortCode::ProtocolTimeout))
        ));
        assert_eq!(s.poll_transmit().unwrap().data[0], CS_ABORT);
        assert!(s.is_idle());
    }
}
