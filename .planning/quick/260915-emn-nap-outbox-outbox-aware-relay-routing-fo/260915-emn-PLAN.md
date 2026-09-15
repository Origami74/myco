---
quick_id: 260915-emn
description: NAP-OUTBOX — outbox-aware relay routing for napplets
date: 2026-09-15
mode: quick
---

# NAP-OUTBOX

Implement the `outbox` domain per the registry draft (PR #32, pinned copy in
`reference/naps/drafts/NAP-OUTBOX.md`): `getEvent`, `query`, `subscribe`/
`close`, `publish`, `resolveRelays`. Relay selection follows NIP-65 across
the three lanes (`RelayLane::{Local, Mesh, Internet}`): a kind 10002 may name
`ws://<npub>.fips:4870` beside `wss://` relays and the outbox model works
unmodified (design §7.4). The mesh lane is a *directed* relay connection to a
Circle member, never a flood — that stays NAP-MESH's.

## Tasks

1. **Runtime seams + handler.** Replace the unused `OutboxResolver` with
   `plan(direction, authors) -> RelayPlan {lanes, source, missing_authors}`;
   add `LaneTransport` (`query`, `publish`, `pull_into_local`); `nap/outbox.rs`;
   `"outbox"` in `IMPLEMENTED_DOMAINS`; domain-scoped subscriptions deliver
   `outbox.event`; `MemOutbox`/`MemLanes` in `testing.rs`; tests; relay URL
   validation (ws/wss, no private hosts).
   - verify: `cargo test -p myco-napplet-runtime`
2. **Core.** `OutboxService` over the local store (kind 10002), Circle
   membership (policy mesh lane), default relays (fallback), the peer relay
   pool (mesh lane), `ip_source` (internet lane, skipped offline-only), and
   the hub (`accept_unforwarded` for pulled results). Publish the user's own
   kind 10002 (own mesh relay + defaults) with the guest profile on first
   napplet use. Wire into `NappletHost`.
   - verify: `cargo test -p myco-core`, clippy
3. **Docs.** napplet-runtime.md S2/§7.4, CHANGELOG. Grant wording exists.
