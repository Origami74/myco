---
quick_id: 260915-emn
status: complete
date: 2026-09-15
branch: feat/napplet-runtime
commits:
  - be35bf113aaf2998d7429e4b829fba0528883c2d (runtime: seams, handler, fixture)
  - 451b57bcb6e9dd513745cf33beb6c10b02f87bc0 (core: OutboxService, own kind 10002)
  - c0986a76999eb97bfc6efe78d466c6da32e64439 (docs)
---

# Summary: NAP-OUTBOX

`outbox` domain per the registry draft: `getEvent`, `query`, `subscribe`/
`close`, `publish`, `resolveRelays`.

- Runtime: `OutboxResolver::plan(direction, authors) -> RelayPlan` and
  `LaneTransport::{query, publish, pull_into_local}` over `RelayLane::{Local,
  Mesh, Internet}`; `nap/outbox.rs`; `"outbox"` implemented; domain-scoped
  subscriptions deliver `outbox.event`; relay URL validation; `OutboxFixture`.
- Core: `OutboxService` — NIP-65 lists from the local store read from the
  author's side; policy (Circle-only mesh lanes, never our own, no internet
  when offline-only, fallback = default relays with `missing_authors`); lanes
  over hub (`accept_unforwarded`), peer pool (verified; push only when
  connected), `ip_source`; spawned pull fed back through the hub. Own kind
  10002 published with the guest profile on first napplet use.
- Existing grant wording covers `outbox`; vendored shim already has the domain.

Verified: fmt, clippy, `cargo test` (359 passed). Not run on device.

Not done: fetching a missing NIP-65 list from relays (fallback instead;
spec says SHOULD cache/refresh); NIP-66 relay intelligence; per-napplet ACL
on relay overrides beyond the private-host check.
