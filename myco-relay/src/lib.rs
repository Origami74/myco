//! `myco-relay` — a generic embedded Nostr relay (NIP-01 event store + WS
//! server) implementing `nsite-deck`'s `RelayBackend` seam.
//!
//! P0 stub. Implemented in **P2** on top of `nostr-relay-builder` (rust-nostr),
//! with NIP-77 (negentropy) for set reconciliation. See
//! `docs/design/nsite-layer.md` §2.1.
