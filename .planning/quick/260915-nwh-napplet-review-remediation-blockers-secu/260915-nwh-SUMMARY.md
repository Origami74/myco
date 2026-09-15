---
quick_id: 260915-nwh
status: complete
date: 2026-09-15
commit: 54841a6b39aba20871737c2a9c711885c3d5339b
branch: feat/napplet-runtime
---

# Summary

Nine commits, `5646cd9` … `54841a6`.

- Blockers: denied-grants set (B1), Library kind match (B2), cache wipe keeps napplets + open-failure toast (B3), author-scoped `apply_grants` (B4), subframe/gesture/403 in NappletActivity (B5.B), relay store (B6 — first a persist policy + cap, then replaced by `nostr-lmdb`).
- Security: user relay list drops the device `.fips` URL (S2); bounded blob downloads, pre-store size check, 16 MiB per `bytesMany` (S3). S1/S4 to the roadmap.
- Reliability: nsite aggregate mismatch warns (R1); napplet active-version pin via `ManifestStore` (R2); `nappletOpen` off main thread, lock released before resolve (R3); per-window semaphore (R4); single fetch in ingest (R5); one REQ per relay (R6); JSON-parsed frames (R7); shared `ExternalNavigation` (R8).
- Features: napplet update check joined into "Check for updates"; tile ready/missing status.
- Dedupe: one subscribe skeleton + delivery path across relay/mesh/outbox; `OutboxService` is the `EventSink`; one public source builder.
- Docs: design doc (D8, S1, S2 policy, §7.2, §7.3), CHANGELOG as one napplet entry, roadmap, doc drift.
- B5.A: napplet-named relays are conformant shell policy (NAP-RELAY §Security, NAP-OUTBOX `options.relays`); documented, not changed.

`cargo fmt`, `clippy -D warnings`, `cargo test` 390 pass. `assembleDebug` built; `heed`/LMDB cross-compiles. Not yet flashed — no device attached at the time.
