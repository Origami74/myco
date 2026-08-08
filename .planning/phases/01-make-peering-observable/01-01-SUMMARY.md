---
phase: 01-make-peering-observable
plan: 01
subsystem: infra
tags: [rust, fips, jni, ffi, compose, android, peer-diagnostics, state-merge]

# Dependency graph
requires: []
provides:
  - "`PeerView` on fips's `ControlReadHandle` additively carries `last_seen_ms`, `transport`, `display_name`"
  - "`myco-core::peer_diagnostics::merge_peers()` — a pure, unit-tested five-state peer merge"
  - "`AppState.peers: Vec<PeerDiagnosticView>` — the npub-or-address-keyed FFI contract for the Dev tab"
  - "Kotlin `PeerDiagnostic` data class + `AppState.peers` parse"
  - "`DevScreen.kt`'s `PeersOverviewCard` rendering `state.peers` directly (no client-side join)"
  - "`merge_peers()`'s `lane_by_npub` parameter — a typed seam for 01-02 to fill without re-signing the function"
affects: [01-02-make-peering-observable, 01-03-make-peering-observable, 01-04-make-peering-observable]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Merge order: base identity set from ble_peers (npub may be empty) -> union adverts by address -> union pairing/circle-only npubs -> left-join Circle/pairing data -> assign state last -> sort. Prevents npub-first grouping from silently dropping unresolved rows (D-09)."
    - "State() fetches every content-layer snapshot accessor exactly once per call and reuses the local binding for both the merge and the AppState struct literal — no new lock acquisitions."
    - "Observed-not-inferred FFI fields: an empty string/zero value crosses the FFI when the source genuinely has no data, never a guessed default (D-10's prohibition on presenting inference as observation)."

key-files:
  created:
    - myco-core/src/peer_diagnostics.rs
    - .planning/phases/01-make-peering-observable/deferred-items.md
  modified:
    - reference/fips/src/control/read_handle.rs
    - myco-core/src/state.rs
    - myco-core/src/runtime.rs
    - myco-core/src/lib.rs
    - myco-core/src/content.rs
    - android/app/src/main/java/app/myco/core/AppCoreClient.kt
    - android/app/src/main/java/app/myco/ui/screens/DevScreen.kt

key-decisions:
  - "Transport crosses the FFI exactly as fips observed it (empty when unresolved) — the tracer's first draft fabricated a 'ble' default and was corrected before Task 2 landed, per the plan's own prohibition on presenting inference as observation."
  - "merge_peers() takes a lane_by_npub override parameter now (always empty in 01-01) so 01-02 can distinguish Wi-Fi Aware from the LAN/AP lane — both currently report fips transport 'udp' via one shared JNI push site — without re-signing the function or rewriting its unit tests."
  - "Only DIAG-01/03/04 marked complete in REQUIREMENTS.md. DIAG-06 and DIAG-07 are listed in this plan's frontmatter and this plan builds their underlying data (pair_state field, five-way merge), but no UI card yet renders pending pairs or own identity — 01-04's frontmatter also lists DIAG-06/07, confirming the visible completion lands there."

requirements-completed: [DIAG-01, DIAG-03, DIAG-04]

coverage:
  - id: D1
    description: "fips PeerView additively exposes last_seen_ms/transport/display_name, sourced from the tick-published EntitySnapshot"
    verification:
      - kind: unit
        ref: "cd reference/fips && cargo test --lib control:: (72 passed)"
        status: pass
    human_judgment: false
  - id: D2
    description: "merge_peers() produces all five peer states with D-11 ordering (state rank, then last-heard desc, then key asc), never reads the pairing credential"
    verification:
      - kind: unit
        ref: "myco-core/src/peer_diagnostics.rs#tests (18 tests, 0 failures)"
        status: pass
    human_judgment: false
  - id: D3
    description: "A connected peer's row renders end-to-end on the Dev tab: state dot, exact-seconds counter, carrying transport"
    verification:
      - kind: manual_procedural
        ref: "On-device Pixel 7 Pro run: PEERS card showed 3 rows, green connected dots, monospace short npubs, live 'udp · 1s' style trailing value"
        status: pass
    human_judgment: true
    rationale: "Only the UDP/LAN lane was exercised on device this session (all three live peers were LAN, not BLE) — the coordinator's own verification note explicitly flags the BLE-specific path as unexercised, so full DIAG-04 transport-label coverage across all transports still needs a human check."

# Metrics
duration: 60min
completed: 2026-08-04
status: complete
---

# Phase 1 Plan 1: The Merged Peer Diagnostics Row Summary

**One npub-or-address-keyed `peers` array, merged once in Rust from fips's peer snapshot plus Circle/pairing state, rendered on the Dev tab's PEERS card with a five-state dot, an exact-seconds last-heard counter, and the observed (never fabricated) carrying transport.**

## Performance

- **Duration:** ~60 min
- **Started:** 2026-08-04T17:14:40Z (fips Task 1 commit)
- **Completed:** 2026-08-04T18:01:15Z (Task 2 commit)
- **Tasks:** 2 completed (1 tracer, 1 auto/tdd)
- **Files modified:** 7 (1 created: `peer_diagnostics.rs`; 1 fips file; 5 myco-core/android files)

## Accomplishments

- Extended fips's `PeerView` additively with `last_seen_ms`, `transport`, `display_name`, joined from the tick-published `EntitySnapshot` — a 3-field diff that stands alone as an upstream PR (verified `grep -c 'myco\|Myco'` returns 0 on the touched file).
- Built `myco-core::peer_diagnostics::merge_peers()` — a pure, no-I/O, no-lock function producing the full five-state (`connected` / `reachable-via-relay` / `seen-unidentified` / `paired-offline` / `unreachable`) peer array with D-11's total ordering, backed by 18 unit tests covering every `<behavior>` case in the plan plus the lane-override seam added mid-execution.
- Wired `AppRuntime::state()` to call `merge_peers()` once, reusing the Circle/pairing snapshot accessors it already fetches unconditionally — no new lock acquisitions.
- Added the Kotlin `PeerDiagnostic` parse and rebuilt `PeersOverviewCard` to render `state.peers` directly, with a `PeerDiagnosticRow` composable, a five-state dot-colour map onto the existing `StatusConnected`/`StatusReachable`/`StatusThin`/`StatusAlone` theme constants (zero new colour literals), and an `elapsedExact()` exact-seconds counter (`3s`, `47s`, `4m 12s`; em-dash when never heard from).
- Verified end-to-end on a physical device (Pixel 7 Pro): three live peers rendered with green connected dots, correct monospace identity, and a live last-heard counter.

## Task Commits

Each task was committed atomically:

1. **Task 1 (tracer): End-to-end "one peer row tells the truth" — one path only** — fips `e68d69c` (feat), fips-pop `1f80162` (feat)
2. **Task 2: The full five-state merge as a pure, unit-tested function** — fips-pop `65f2255` (feat)

**Plan metadata:** (this commit, following)

## Files Created/Modified

- `reference/fips/src/control/read_handle.rs` — `PeerView` additive fields (`last_seen_ms`, `transport`, `display_name`), joined by `NodeAddr` from `EntitySnapshot::peers` onto `stats.peer_meta`
- `myco-core/src/peer_diagnostics.rs` — new. `merge_peers()`, `peer_state_rank()`, `order_transports()`, `truncate_chars()`, `short()`, 18 unit tests
- `myco-core/src/state.rs` — `PeerDiagnosticView` struct, `AppState.peers` field
- `myco-core/src/runtime.rs` — `state()` fetches every content-layer snapshot once, calls `merge_peers()`, threads an (currently empty) `lane_by_npub` map, adds a `now_ms()` helper
- `myco-core/src/lib.rs` — declares the `peer_diagnostics` module
- `myco-core/src/content.rs` — `#[derive(Default)]` on `PairRequestView` (test-fixture ergonomics only, no behavior change)
- `android/app/src/main/java/app/myco/core/AppCoreClient.kt` — `PeerDiagnostic` data class, `AppState.peers` parse
- `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` — `PeersOverviewCard` rebuilt around `state.peers`, `PeerDiagnosticRow`, `elapsedExact()`; removed the now-dead `uptime()`/`since` client-side lane-joining code

## Decisions Made

- Kept `merge_peers()`'s parameter list literal to the plan's "PeerView list, ble_peers, ble_adverts, circle, pending_pairs, outbound_pairs, reachable_npubs, now_ms" spec, plus the mid-execution `lane_by_npub` addition, rather than collapsing `peer_views`/`ble_peers` into one input — keeps the function's test fixtures aligned with the plan's own `<behavior>` prose (which frames cases in terms of `BlePeer`).
- Advert-to-row attribution (D-09/Pitfall 4) is implemented generically as "attach to any row whose `ble_addr` or `key` already equals this advert's address" rather than a real address→node-address map, because that map doesn't exist until plan 01-03's attempt log can resolve one. This correctly handles duplicate-advert dedup today and is forward-compatible with 01-03 without a signature change.
- `also_reachable_via` ordering (`order_transports()`) is implemented and unit-tested directly, but `merge_peers()` never populates it with more than the empty vector today — there is no data source for "other reachable transports" yet (Phase 2 territory per D-08). This is intentional, not a stub bug: `state.rs`'s field doc already says "Empty until Phase 2 populates it."
- Only `DIAG-01`, `DIAG-03`, `DIAG-04` marked complete in `REQUIREMENTS.md` (see Requirements below).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking acceptance criterion] Pre-existing "Myco" mention in fips doc comment**
- **Found during:** Task 1
- **Issue:** `read_handle.rs`'s pre-existing `peer_views()` doc comment named "the Myco app" as the reference embedder, which fails the plan's own acceptance check (`grep -c 'myco\|Myco'` must be 0) — not introduced by this plan's diff, but it blocked the stated gate.
- **Fix:** Reworded to "the reference Android app embedding," preserving the same meaning without naming the app.
- **Files modified:** `reference/fips/src/control/read_handle.rs`
- **Commit:** fips `e68d69c`

**2. [Rule 1 - Bug] `String::truncate(64)` panics on a multi-byte UTF-8 boundary**
- **Found during:** Task 1
- **Issue:** Naive `name.truncate(64)` on a peer-supplied display name can panic if byte 64 falls mid-character.
- **Fix:** Added a char-boundary-safe truncation helper (later consolidated into `peer_diagnostics::truncate_chars`).
- **Files modified:** `myco-core/src/runtime.rs` (Task 1), superseded by `myco-core/src/peer_diagnostics.rs` (Task 2)
- **Commit:** fips-pop `1f80162`, `65f2255`

**3. [Rule 1 - Bug] `state.rs` doc comment tripped its own negative-assertion grep**
- **Found during:** Task 1
- **Issue:** My own new doc comment on `PeerDiagnosticView` used the literal word "secret" to explain what the type does *not* carry, which fails `! grep -Eq 'secret' myco-core/src/state.rs`.
- **Fix:** Reworded to "pairing credential value" — same meaning, no literal match.
- **Files modified:** `myco-core/src/state.rs`
- **Commit:** fips-pop `1f80162`

**4. [Rule 1 - Bug] Fabricated `"ble"` default transport (coordinator-flagged, mid-plan)**
- **Found during:** post-Task-1 device verification (coordinator review)
- **Issue:** The tracer's inline construction defaulted an empty `PeerView.transport` to `"ble"` with a comment claiming BLE is Android's only configured transport — factually wrong (live device run showed `udp` for all three peers) and a direct violation of the plan's own prohibition on presenting an inferred value as an observed fact.
- **Fix:** `merge_peers()` (Task 2) passes `PeerView.transport` straight through with no default; a unit test (`connected_transport_passes_through_without_fabricating_a_default`) pins this.
- **Files modified:** `myco-core/src/peer_diagnostics.rs` (superseded the tracer code in `runtime.rs`)
- **Commit:** fips-pop `65f2255`

**5. [Rule 2 - Missing functionality, coordinator-directed] Lane-origin override seam for 01-02**
- **Found during:** mid-Task-2 (coordinator investigation)
- **Issue:** Wi-Fi Aware and the LAN/AP lane both ride fips's plain UDP transport and share one JNI push site (`aware_bridge_jni.rs`'s hardcoded `TRANSPORT_TYPE = "udp"`), so fips structurally cannot label them apart — only the Kotlin radio push site can, and that fix belongs to plan 01-02 (whose `files_modified` already covers the relevant files).
- **Fix:** Added a `lane_by_npub: &HashMap<String, String>` parameter to `merge_peers()`, consulted in preference to the raw fips transport when an npub has an override. `state()` passes an empty map in 01-01. Two unit tests pin the precedence-and-fallback contract for 01-02 to build against.
- **Files modified:** `myco-core/src/peer_diagnostics.rs`, `myco-core/src/runtime.rs`
- **Commit:** fips-pop `65f2255`

**6. [Rule 1 - Bug, coordinator-flagged] Legend text clipping on device**
- **Found during:** post-Task-1 device verification (coordinator review)
- **Issue:** `PeersOverviewCard`'s legend `Text` had no horizontal padding while `PeerDiagnosticRow` used 16dp, clipping flush to the card edge on a Pixel 7 Pro.
- **Fix:** Added `Modifier.padding(horizontal = 16.dp)` to the legend `Text`.
- **Files modified:** `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt`
- **Commit:** fips-pop `65f2255`

---

**Total deviations:** 6 auto-fixed (5 Rule 1/3 correctness fixes, 1 Rule 2 coordinator-directed scope handoff to 01-02).
**Impact on plan:** All fixes were necessary for correctness, upstream-extractability, or the plan's own stated prohibitions. No unrequested scope creep — the lane-override seam is a typed no-op in 01-01 (always an empty map), not an implementation of 01-02's work.

## Issues Encountered

**`cargo clippy -p myco-core --all-targets -- -D warnings` fails on two pre-existing, unrelated `fips` defects.** `reference/fips/src/transport/udp/darwin_sockopts.rs` (a duplicate `#[cfg]` attribute) and `reference/fips/src/node/lifecycle.rs` (a collapsible `if`) both predate this plan (confirmed via `git log`) and are outside every file this plan touches. Logged to [deferred-items.md](./deferred-items.md) rather than fixed, per the executor's scope-boundary rule — fixing them here would mix an unrelated fips cleanup into a diff the plan's own acceptance criteria requires to stand alone. Verified instead that this plan's own files carry zero clippy warnings (`cargo clippy -p myco-core --all-targets -- -D warnings 2>&1 | grep 'myco-core/src'` — no output).

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

Plans 01-02 (radio self-check + attempt-log wiring's transport signal), 01-03 (attempt log), and 01-04 (full screen rebuild: radio self-check card, expand-in-place, pending pairs card, identity card) can proceed:

- 01-02 has a ready-made, unit-test-pinned seam (`merge_peers()`'s `lane_by_npub` parameter) to fill without touching this plan's function signature or tests again.
- The `peers` FFI array (D-19's contract) is live and stable; Kotlin already renders it, so 01-04's screen rebuild extends `PeerDiagnosticRow` rather than re-deriving anything.
- `DIAG-06`/`DIAG-07` remain open in `REQUIREMENTS.md` — this plan built `pair_state`'s merge logic (incoming/outbound/paired) but no card yet renders pending pairs or own identity; 01-04's own frontmatter already lists both, confirming that's the intended landing point.
- **Verification gap:** only the UDP/LAN transport lane was exercised on-device this session (all three live test peers were LAN, not BLE) — the BLE-specific data path through `merge_peers()` (an empty `PeerView.transport` on an actually-BLE-connected peer) is unit-tested but not yet confirmed on a real BLE link. Worth a spot-check once 01-02's Aware/BLE work lands.

---
*Phase: 01-make-peering-observable*
*Completed: 2026-08-04*

## Self-Check: PASSED

- FOUND: `myco-core/src/peer_diagnostics.rs`
- FOUND: `.planning/phases/01-make-peering-observable/deferred-items.md`
- FOUND: `.planning/phases/01-make-peering-observable/01-01-SUMMARY.md`
- FOUND: `reference/fips/src/control/read_handle.rs`
- FOUND commit `1f80162` (fips-pop)
- FOUND commit `65f2255` (fips-pop)
- FOUND commit `e68d69c` (fips)
