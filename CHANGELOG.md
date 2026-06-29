# Changelog

All notable changes to this crate are documented here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — unreleased

Breaking release: the crate is now `no_std` and gains a sans-io SDO **server**
alongside the existing client.

### Added
- Sans-io CANopen SDO **server** ([`SdoServer<const N>`] + the
  [`ObjectDictionary`] trait): a pure state machine that answers a single SDO
  master against a caller-provided dictionary. `no_std` and **no alloc** — a
  fixed `[u8; N]` buffer bounds the largest transferable object. Supports
  expedited and segmented upload/download, toggle handling, the standard abort
  codes, and a segmented-transfer timeout.
- A client↔server loopback test suite proving both halves agree on the full
  CiA-301 wire protocol (expedited + segmented, both directions, aborts).
- `examples/server_demo.rs`: runs the `SdoServer` as a node on a SocketCAN bus
  with a small toy OD (incl. a > 4-byte entry to force segmentation), to be
  driven by an independent SDO master. Validated live on `vcan0` against
  `python-canopen` 2.4.1 (expedited + segmented reads/writes round-trip;
  missing-object → `ObjectDoesNotExist`; write-to-RO → `WriteReadOnly`).

### Changed
- The crate is now `no_std` by default. The `tokio` async glue (on by default)
  still pulls in `std`; the host async API
  ([`asynch::upload_bytes`] / [`asynch::download_bytes`], taking
  `Option<Duration>`) is **unchanged**.
- Feature layout: `client` (sans-io master, needs `alloc`), `server` (`no_std`,
  no alloc), `tokio` (host async glue; implies `client` + std). Default =
  `tokio`.
- Time is now a monotonic `u64` of the caller's chosen unit (nanoseconds in the
  async glue) instead of `std::time::Instant`.
- The core error type dropped `thiserror` for `core::error::Error` (stable since
  Rust 1.81). **MSRV is now 1.81.**

### Validated (0.2.0)
- Build matrix (no warnings): default; `--no-default-features --features client`;
  `--features server`; and `--features server --target thumbv7em-none-eabihf`.
- `cargo test`: 27 sans-io unit tests (client + server + loopback).
- Client regression vs the reference CANopenNode C server on `vcan0`
  (`examples/against_canopennode`): all scenarios pass — the `no_std` refactor
  did not change client behavior on the wire.
- Server vs an independent client (`examples/server_demo` + `python-canopen`):
  expedited and segmented round-trips and both error aborts pass.

[`SdoServer<const N>`]: https://docs.rs/canopen-sdo/0.2.0/canopen_sdo/server/struct.SdoServer.html
[`ObjectDictionary`]: https://docs.rs/canopen-sdo/0.2.0/canopen_sdo/server/trait.ObjectDictionary.html
[`asynch::upload_bytes`]: https://docs.rs/canopen-sdo/0.2.0/canopen_sdo/asynch/fn.upload_bytes.html
[`asynch::download_bytes`]: https://docs.rs/canopen-sdo/0.2.0/canopen_sdo/asynch/fn.download_bytes.html
