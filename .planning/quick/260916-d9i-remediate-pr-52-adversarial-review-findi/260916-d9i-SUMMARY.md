---
quick_id: 260916-d9i
status: complete
date: 2026-09-16
commit: 91f76500b3b1ada3f47ae3b924a0c73da4f4169f
branch: feat/napplet-runtime
---

# Summary

Six commits, `09bfed6` … `91f7650`, one per plan task, in the review's fix order. All 29 findings of `260916-d9i-REVIEW.md` addressed.

- **High.** H1: `strip_scheme` no longer byte-indexes; non-ASCII pointers are refused, not a panic. H2: mesh relay URLs are parsed strictly (`seams::mesh_relay_npub`) and rebuilt from the npub (`mesh_relay_url`) before any dial — `validate_relay_url`, `relay_list_lanes` and `PeerRelayPool::spawn_or_get` all refuse or canonicalise userinfo/port/path variants.
- **Bounds (M4, M5, M7, L15, M9, M10, L10).** `remove_from_library`/`is_in_library` match `kind`. `validate_relay_url` on `nostr::Url`: userinfo, v4-mapped v6, non-dotted v4, CGNAT, multicast refused; `OutboxService` re-checks resolved addresses at dial time (test opt-out `allowing_private_dials`). `MAX_HINT_RELAYS`, `MAX_SUBSCRIPTIONS = 64` per session with orphan removal on failed backlog, `INBOUND_CAPACITY = 64` frames in Kotlin.
- **Android (M12, M8, L9, M6, L11).** `onRenderProcessGone` in both `NappletActivity` and `NsiteActivity` closes the window, returns true. Open/destroy race resolved under a `sessionLock` (exactly one side closes the session). Subframe requests for the shell get 403. `settle_napplet_review` only lands a fetch whose pointer still matches the slot. Home-screen offer compares against the pre-fetch Library.
- **Grants, migration, key (M1, M2, L5, L3, L2, L12, L13).** `LibraryItem.reviewed` bounds launch-time widening; a wider `requires` on update returns to the review sheet. `flush_legacy` behind a `OnceCell`, keeps `events.json` (writes the failed subset back) when any save errs. `user.nsec` written temp+rename, 0600 at creation, load failure logged. `fetch_napplet` honours offline-only. First-use kind 0/10002 go to the embedded store only. `wipe_cache` keeps the user key's own events.
- **Shim contract (M3, L1, L7, L4, L6, L8).** `relay.subscribe` reads top-level `relay` (shim) with `options.relay` fallback; refusals go out as `relay.closed`. `identity.getRelays` reflects the user's kind 10002. `NADDR1…` accepted. SVG sniff scans the whole body. `BlobStore::size` seam; size check before any local read.
- **Policy (M11, L14).** Interim: `sign_template` refuses kinds 0, 3, 5, 10000–19999 with an error result. Design doc: secure-context note, accepted-policy list, URL check, `.fips` rebuild. CHANGELOG under `[Unreleased]`.

Tests added: 21 host tests across `myco-core`, `myco-napplet-runtime`, `myco-relay`, `myco-blossom`. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (413 pass) green; `./gradlew assembleDebug` built after tasks 2 and 3; `cargo ndk -t arm64-v8a check -p myco-core` green after task 6; `Cargo.lock` clean.

## Deviations from the plan

- M8: the plan's `NonCancellable` + post-`withContext` check still leaks under prompt cancellation; implemented the hand-off inside the IO block under `sessionLock` instead.
- M7: DNS resolution lives at the dial site in `OutboxService` (`myco-napplet-runtime` has no tokio); syntactic checks in the runtime. Connect-time re-resolve TOCTOU noted as accepted interim in the design doc.
- M2 test runs its second phase in a fresh dir (heed caches `Env` per process); `tokio` promoted from dev-dep to dep in `myco-relay` (`sync`), lockfile unchanged.
- L3: `write_private` uses `create_new` + removes a stale `.tmp` so the 0600 mode is guaranteed at creation.
- L4: mixed-case `naddr` is accepted after lowering (the plan expected refusal).
- `PeerRelayPool` got `canonical_urls` with a `#[cfg(test)] dialing_as_given()` opt-out for the loopback mock tests.

## Not host-testable — on device before merge

- M12: force a renderer crash in a napplet and an nsite; window closes, app/mesh/relay stay up.
- M8: Back during the napplet splash; `open_count()` returns to 0.
- L9: napplet `location.href = "http://<shellHost>/"` → 403 + log line.
- L10: napplet spamming `postMessage`; memory flat, `frame queue full` in logcat.
- L11: share an already-installed napplet; no home-screen dialog.
- M1: install v1, publish v2 with wider `requires`, Check for updates, open → review sheet on Apps with the wider list.
- M2: upgrade from a real pre-LMDB `events.json`.
- L12: custom relay configured; first napplet open publishes nothing remotely.
