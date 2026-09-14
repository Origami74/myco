---
quick_id: 260914-ovv
status: complete
date: 2026-09-14
commit: 7be0749703933b17df9dbbdd9b88c1580cac3214
branch: feat/napplet-runtime
---

# Summary: relay.publish follows NAP-RELAY

`relay.publish` now reaches the shell's relay pool and never the Circle flood:
`RelayPoolSink` (replacing `MeshEventSink`) accepts the event unforwarded via
`RelayHub::accept_unforwarded` (renamed from `accept_pulled`; stored, live
subscriptions woken, no gossiper), then spawns a bounded fan-out to
`ip_source::default_relays()` through the new one-shot
`ip_source::publish_to_relay` — skipped when offline-only. The napplet's result
does not wait on the internet.

Verified: fmt, clippy, `cargo test` (344 passed). Not run on device.

Still NAP-OUTBOX's, not done: NIP-65 relay selection; the pool is the default
relay set. `relay.subscribe`/`query` still read the local store only.
