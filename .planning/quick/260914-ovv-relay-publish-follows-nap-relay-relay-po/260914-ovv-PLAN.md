---
quick_id: 260914-ovv
description: relay.publish follows NAP-RELAY — relay pool, not the mesh
date: 2026-09-14
mode: quick
---

# relay.publish follows NAP-RELAY

NAP-RELAY: "publishes a Nostr event to the shell's relay pool". Myco's
`relay.publish` flooded the mesh at the default hop budget instead — a mesh
behaviour NAP-MESH now owns. Make `relay` mean relays: the device's own relay
(so this phone's live subscriptions hear it) plus the internet relay pool when
reachable, and never the Circle flood.

## Tasks

1. **core.** `ip_source::publish_to_relay` (one-shot EVENT + OK);
   `RelayHub::accept_pulled` → `accept_unforwarded` (store + live, no gossip,
   for pulled backlog and relay-pool publishes alike); `MeshEventSink` →
   `RelayPoolSink` (local accept, then spawned fan-out to the internet pool,
   empty when offline-only); wire in `napplet_context`. Tests: no gossip on a
   relay publish; fan-out reaches a mock relay.
   - verify: `cargo test -p myco-core`, clippy
2. **runtime.** `EventSink` and `nap/relay.rs` docs say relays, not mesh.
3. **docs.** napplet-runtime.md S5/§7.4 deferred note closed; CHANGELOG.
