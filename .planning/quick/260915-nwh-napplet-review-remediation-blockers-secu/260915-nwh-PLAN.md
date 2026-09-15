---
quick_id: 260915-nwh
description: Napplet review remediation — blockers, security, reliability, active gate, dedupe, tests
date: 2026-09-15
mode: quick
---

# Napplet review remediation

Adversarial review of `feat/napplet-runtime` vs `main` produced the list below. Fix all, one commit per group.

## Blockers
- B1 grants: store `denied` beside `granted`; widen only into domains in neither set; sheet "off" survives relaunch. Test.
- B2 Library match includes `kind` in both `add_to_library` and `add_napplet_to_library`. Test.
- B3 `wipe_cache` keeps napplet manifests + index blobs; Kotlin surfaces open failure. Test.
- B4 `NappletIdentity` carries author; `apply_grants` matches author + d_tag. Test.
- B5.A napplet-named relays: NAP-RELAY/NAP-OUTBOX permit under shell policy — keep, document policy.
- B5.B NappletActivity: refuse subframe navigations, require gesture for external hand-off, 403 every non-shell host.
- B6 relay store: persist only replaceable/addressable kinds; cap regular non-expiring events in memory.

## Security
- S2 `own_relay_list` drops the mesh `r` tag — device npub never in a user-key event. Mesh lanes come from Circle policy.
- S3 resource: total-bytes cap per call, size bound before store.
- S1/S4 → roadmap notes only.

## Reliability
- R1 nsite aggregate mismatch → warn, not refuse (napplets stay strict).
- R2 napplet active-version gate: open resolves the active manifest; ingest sets active after index blob lands.
- R3 `nappletOpen` off main thread; runtime lock released before `block_on`.
- R4 per-session semaphore on frame coroutines.
- R5 `ResolvedNapplet` carries index bytes; ingest stops fetching twice.
- R6 one WebSocket per relay per query, all filters in one REQ.
- R7 parse frames as JSON for `channel` / `shell.init`.
- R8 shared external-navigation helper for both Activities.

## Features (little effort only)
- Update check includes napplets (re-ingest → active gate advances).
- Tile status for napplets via `siteStatus` keyed on shell host.

## Dedupe
- One `subscribe` skeleton and one `deliveries_for` across relay/mesh/outbox.
- `RelayPoolSink` publishes through `LaneTransport`.
- ingest reuses runtime resolve output; source order shared with fetch.

## Docs
- Design doc: denied grants, active gate, relay policy, relay list. CHANGELOG collapse "Fixed"→"Added". Roadmap S1/S4. Doc drift in dispatch.rs / vendor README.
