//! End-to-end: drive an [`SdoClient`] and [`SdoServer`] against each other with
//! zero IO. This proves the client and server agree on the full CiA-301 SDO
//! protocol — expedited + segmented, both directions, aborts — which is exactly
//! the contract the host tooling (client) and the firmware (server) rely on.

use crate::client::{SdoClient, SdoConfig, SdoOutcome};
use crate::error::SdoError;
use crate::server::{ObjectDictionary, SdoServer, ServerConfig};
use crate::SdoAbortCode;

const NODE: u8 = 0x10;
const N: usize = 256;

/// Minimal read/write object dictionary for the loopback.
#[derive(Default)]
struct LoopOd {
    entries: std::vec::Vec<(u16, u8, std::vec::Vec<u8>)>,
}
impl LoopOd {
    fn with(mut self, idx: u16, sub: u8, val: &[u8]) -> Self {
        self.entries.push((idx, sub, val.to_vec()));
        self
    }
    fn get(&self, idx: u16, sub: u8) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.0 == idx && e.1 == sub)
            .map(|e| e.2.as_slice())
    }
}
impl ObjectDictionary for LoopOd {
    fn read(&mut self, index: u16, sub: u8, buf: &mut [u8]) -> Result<usize, SdoAbortCode> {
        let e = self
            .entries
            .iter()
            .find(|e| e.0 == index && e.1 == sub)
            .ok_or(SdoAbortCode::ObjectDoesNotExist)?;
        if e.2.len() > buf.len() {
            return Err(SdoAbortCode::OutOfMemory);
        }
        buf[..e.2.len()].copy_from_slice(&e.2);
        Ok(e.2.len())
    }
    fn write(&mut self, index: u16, sub: u8, data: &[u8]) -> Result<(), SdoAbortCode> {
        if let Some(e) = self.entries.iter_mut().find(|e| e.0 == index && e.1 == sub) {
            e.2 = data.to_vec();
        } else {
            self.entries.push((index, sub, data.to_vec()));
        }
        Ok(())
    }
}

/// Pump frames between client and server (no IO, no time) until the client
/// produces an outcome or someone errors out.
fn run(
    client: &mut SdoClient,
    server: &mut SdoServer<N>,
    od: &mut LoopOd,
) -> Result<SdoOutcome, SdoError> {
    let now = 0u64;
    for _ in 0..10_000 {
        if let Some(f) = client.poll_transmit() {
            // The server may abort; it still queues the abort frame for the client.
            let _ = server.handle_frame(f, now, od);
            continue;
        }
        if let Some(f) = server.poll_transmit() {
            if let Some(out) = client.handle_frame(f, now)? {
                return Ok(out);
            }
            continue;
        }
        panic!("loopback stalled with no frames pending");
    }
    panic!("loopback did not converge");
}

fn client() -> SdoClient {
    SdoClient::new(SdoConfig::default())
}
fn server() -> SdoServer<N> {
    SdoServer::new(ServerConfig::new(NODE))
}

#[test]
fn roundtrip_upload_all_sizes() {
    // Cover zero-length, expedited (<=4), the segment boundary (7/8), and
    // multi-segment.
    for len in [0usize, 1, 4, 5, 7, 8, 14, 15, 100] {
        let value: std::vec::Vec<u8> = (0..len).map(|i| (i * 7 + 1) as u8).collect();
        let mut od = LoopOd::default().with(0x2000, 0, &value);
        let mut c = client();
        let mut s = server();
        c.begin_upload(NODE, 0x2000, 0, 0).unwrap();
        let out = run(&mut c, &mut s, &mut od).unwrap();
        assert_eq!(out, SdoOutcome::UploadCompleted(value), "upload len={len}");
        assert!(c.is_idle() && s.is_idle());
    }
}

#[test]
fn roundtrip_download_all_sizes() {
    for len in [1usize, 4, 5, 7, 8, 14, 15, 100] {
        let value: std::vec::Vec<u8> = (0..len).map(|i| (i * 3 + 5) as u8).collect();
        let mut od = LoopOd::default().with(0x3000, 1, &[]);
        let mut c = client();
        let mut s = server();
        c.begin_download(NODE, 0x3000, 1, &value, 0).unwrap();
        let out = run(&mut c, &mut s, &mut od).unwrap();
        assert_eq!(out, SdoOutcome::DownloadCompleted, "download len={len}");
        assert_eq!(
            od.get(0x3000, 1),
            Some(value.as_slice()),
            "od value len={len}"
        );
        assert!(c.is_idle() && s.is_idle());
    }
}

#[test]
fn roundtrip_write_then_read_back() {
    let value: std::vec::Vec<u8> = (0u8..50).collect();
    let mut od = LoopOd::default().with(0x4000, 0, &[]);

    let mut c = client();
    let mut s = server();
    c.begin_download(NODE, 0x4000, 0, &value, 0).unwrap();
    assert_eq!(
        run(&mut c, &mut s, &mut od).unwrap(),
        SdoOutcome::DownloadCompleted
    );

    let mut c = client();
    let mut s = server();
    c.begin_upload(NODE, 0x4000, 0, 0).unwrap();
    assert_eq!(
        run(&mut c, &mut s, &mut od).unwrap(),
        SdoOutcome::UploadCompleted(value)
    );
}

#[test]
fn upload_nonexistent_propagates_server_abort() {
    let mut od = LoopOd::default();
    let mut c = client();
    let mut s = server();
    c.begin_upload(NODE, 0x9999, 0, 0).unwrap();
    let res = run(&mut c, &mut s, &mut od);
    assert!(matches!(
        res,
        Err(SdoError::ServerAborted(SdoAbortCode::ObjectDoesNotExist))
    ));
    assert!(c.is_idle() && s.is_idle());
}
