//! `nsite-deck` — the reusable, transport-agnostic **nsite host**: the localhost
//! gateway (manifest -> path -> sha256 -> serve), the sync engine, and the
//! **propagator** (subscribe to local + peer relays, publish fanout, eager
//! pinned-refresh). It reaches relay/Blossom/transport through four trait seams
//! (`RelayBackend`, `BlobStore`, `PeerSource`, `FanoutSink`) and names no
//! concrete relay, blob store, or radio.
//!
//! P0 stub. Gateway + sync land in **P2**, the propagator in **P3**. See
//! `docs/design/nsite-layer.md`.
