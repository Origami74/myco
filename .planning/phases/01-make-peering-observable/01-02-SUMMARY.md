---
phase: 01-make-peering-observable
plan: 02
subsystem: infra
tags: [rust, fips, jni, ffi, android, ble, wifi-aware, diagnostics]

# Dependency graph
requires:
  - "`myco-core::peer_diagnostics::merge_peers()`'s `lane_by_npub` parameter (01-01)"
provides:
  - "Observed (pushed, not computed) BLE `scanning`/`advertising` facts, each with a `_known` sibling"
  - "Observed Wi-Fi Aware `scanning` (discovering) fact, with a `_known` sibling"
  - "`myco-core::lane_observation` — the npub→observed-lane record that disambiguates Wi-Fi Aware from the LAN/AP lane"
  - "`AppRuntime::state()`'s `lane_by_npub` argument to `merge_peers()`, now populated instead of always empty"
affects: [01-03-make-peering-observable, 01-04-make-peering-observable]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pushed-not-polled observed state: Kotlin radio callback threads push a relaxed-atomic boolean or a locked map entry into Rust at every transition site (start/stop/retry-failure/teardown); Rust exposes a plain getter/snapshot. Never a computed proxy from other flags."
    - "Unknown is a first-class value: every observed fact ships with a `_known` sibling (or, for Aware, an 'ever pushed' AtomicBool) so an un-started radio or an absent bridge renders unknown, never a guessed false."
    - "Host-testable JNI-adjacent state: pure lock-based logic that a JNI export merely calls into lives in its own non-JNI module (`lane_observation.rs`), so it unit-tests on the host even though its only real caller is Android-only."

key-files:
  created:
    - myco-core/src/lane_observation.rs
  modified:
    - reference/fips/src/transport/ble/android_io.rs
    - myco-core/src/ble_bridge_jni.rs
    - myco-core/src/aware_bridge_jni.rs
    - myco-core/src/state.rs
    - myco-core/src/runtime.rs
    - myco-core/src/lib.rs
    - android/app/src/main/java/app/myco/core/NativeCore.kt
    - android/app/src/main/java/app/myco/core/AppCoreClient.kt
    - android/app/src/main/java/app/myco/ble/BleRadio.kt
    - android/app/src/main/java/app/myco/aware/AwareRadio.kt
    - android/app/src/main/java/app/myco/ap/ApRadio.kt

key-decisions:
  - "Lane disambiguation could not be fixed in fips: `link_info.transport_type` comes from the UDP transport's own `TransportType { name: \"udp\" }`, so fips is structurally unable to tell Wi-Fi Aware and the LAN/AP lane apart. The fix lives entirely in Kotlin (which radio observed the peer) and myco-core (recording that observation) — zero fips diff for this task, consistent with the plan's own constraint that this task's files_modified never named a fips path."
  - "Never inferred from address shape. Aware pushes link-local `fe80::…%ifindex`; the AP lane pushes routable addresses. That correlation exists but was deliberately not used — inferring the lane from address shape is exactly the sort of inference-presented-as-observation this phase prohibits. The lane is only ever what the pushing radio explicitly labels itself."
  - "Lane tracking logic extracted into `lane_observation.rs`, a plain non-JNI module, rather than living inside `aware_bridge_jni.rs` (which is `#[cfg(target_os = \"android\")]`-gated and so invisible to `cargo test` on the host). This keeps the populate/consume unit tests (below) runnable in ordinary `cargo test -p myco-core`, matching this plan's own precedent of host-testable pure logic behind JNI seams."
  - "A `awarePeerLost` clears an npub's recorded lane only if the clearing call's own lane still matches the one on record. Without this, a stale `AP lane lost` racing a fresher `Aware lane found` for the same npub (a peer moving between lanes) could wipe the newer, correct record — the same staleness hazard Task 1's BLE scanning fix guards against for a dead scan loop."

requirements-completed: [DIAG-05]

coverage:
  - id: D1
    description: "BLE scanning/advertising are pushed observed facts (not computed from enabled+node_running), each with a known sibling"
    verification:
      - kind: unit
        ref: "cargo test --lib transport::ble (fips) — pinned by prior executor"
        status: pass
      - kind: manual_procedural
        ref: "On-device: system Bluetooth off drove the reported scanning value to idle within one poll; the same run exposed F-01 (see Field Findings below)"
        status: pass
    human_judgment: true
  - id: D2
    description: "Wi-Fi Aware discovering is a pushed observed fact sourced from publish/subscribe session liveness, with a known sibling that is false until Kotlin pushes at least once"
    verification:
      - kind: unit
        ref: "cargo test -p myco-core (existing suite, unaffected)"
        status: pass
    human_judgment: false
  - id: D3
    description: "Wi-Fi Aware and the LAN/AP lane, which both ride fips's plain UDP transport, are distinguishable on the merged peer row via a Kotlin-pushed lane label consumed by merge_peers()'s lane_by_npub parameter"
    verification:
      - kind: unit
        ref: "myco-core/src/lane_observation.rs#tests (5 tests: set/snapshot, overwrite, matching clear, stale-clear-does-not-clobber, absent-npub) + myco-core/src/peer_diagnostics.rs#tests (2 pre-existing tests pinning the lane_by_npub precedence/fallback contract from 01-01)"
        status: pass
      - kind: build
        ref: "cargo build -p myco-core; cargo ndk -t arm64-v8a --platform 29 build -p myco-core --release; cd android && ./gradlew :app:compileDebugKotlin :app:testDebugUnitTest — all exit 0"
        status: pass
    human_judgment: true
    rationale: "The lane label was verified by build + unit test only, not by observing a live Aware NDP connection label 'aware' on a real peer row — no Aware NDP was exercised during this session's device time (see Verification Gap below). The unit tests pin the populate/consume contract precisely, but end-to-end confirmation on a real Aware link is still open."

# Metrics
duration: 90min
completed: 2026-08-06
status: complete
---

# Phase 1 Plan 2: Radios and Lanes Report What They Actually Observe Summary

**BLE scanning/advertising and Wi-Fi Aware discovering are now pushed, observed facts (never computed proxies) with a first-class unknown state, and the Wi-Fi Aware lane is disambiguated from the LAN/AP lane — which fips cannot tell apart on its own — via a Kotlin-pushed, host-unit-tested lane record feeding `merge_peers()`.**

## Performance

- **Duration:** ~90 min across two sessions (Tasks 1-2 on 2026-08-05; the lane-disambiguation addition on 2026-08-06)
- **Tasks:** 3 completed (2 plan-authored: 1 tracer + 1 auto; 1 coordinator-directed addition handed off from plan 01-01)
- **Files modified:** 11 (1 created: `lane_observation.rs`; 1 fips file; 9 myco-core/android files)

## Accomplishments

- **BLE (Task 1, tracer).** Added `AndroidBleBridge::{set,is}_scanning` and `{set,is}_advertising` to fips's `android_io.rs` — two relaxed atomics, no lock, no allocation, callable from the radio's own callback thread. `BleRadio.kt` pushes at every transition site (scan start success, scan start failure/retry, stop; advertiser install/clear/refuse). The previously computed `ble_enabled && node_running` proxy in `runtime.rs` is gone; `BleStatus` now carries `scanning`/`scanning_known`/`advertising`/`advertising_known` as independent facts.
- **Wi-Fi Aware (Task 2, auto).** `aware_bridge_jni.rs` gained a process-global observed-discovering flag (`aware_discovering()` returning `None` until Kotlin has pushed at least once) fed by `AwareRadio.kt`'s publish/subscribe session liveness — pushed on publish-started, subscribe-started, and every teardown path. `WifiAwareStatus` carries `scanning`/`scanning_known`. Zero fips diff for this task (Aware has no byte bridge; the flag lives entirely in myco-core).
- **Lane disambiguation (coordinator-directed addition).** `NativeCore.awarePeerFound`/`awarePeerLost` gained a `lane` argument. `AwareRadio` passes `"aware"`; `ApRadio` passes `"udp"` and stops masquerading through the same seam. `fips::discovery::platform::platform_peer_available`'s `transport_type` argument is untouched — still `"udp"` for both, since that's what fips actually observes. The new `lane_observation` module records npub→lane in a plain, host-testable `Mutex<HashMap>`, with lost-clearing gated on the clearing call's lane still matching the one on record (so a stale loss from a superseded lane can't erase a fresher one). `AppRuntime::state()` now feeds a live snapshot into `merge_peers()`'s `lane_by_npub` parameter, replacing 01-01's always-empty placeholder.
- **The payoff, confirmed on real hardware.** Task 1's `scanning: idle` reading — correct, not a bug — was the observation that exposed **F-01**: a startup race where the node's one-shot BLE transport `start()` fails if the Android foreground service hasn't injected the radio yet, and nothing ever retried it. The pre-Phase-1 computed proxy (`ble_enabled && node_running`) would have rendered `active` in that exact state and hidden the fault entirely; the observed signal is what made it visible. F-01 was fixed separately (quick task 260805-e5h, fips commits `8104849`/`59028d6`) with a quarantine-and-retry supervisor — BLE now comes up ~1.2s after cold launch with no manual node restart, and the user has since confirmed pairing over BLE works end-to-end between the two test phones.

## Task Commits

Each task was committed atomically:

1. **Task 1 (tracer): End-to-end "the BLE radio says what it is really doing"** — fips `7e5a056` (feat), fips-pop `7a45600` (feat)
2. **Task 2: Wi-Fi Aware reports whether it is actually discovering** — fips-pop `4545de6` (feat)
3. **Lane disambiguation (coordinator-directed addition, this session)** — fips-pop `76c4be1` (feat)

**Plan metadata:** (this commit, following)

## Files Created/Modified

- `reference/fips/src/transport/ble/android_io.rs` — `AndroidBleBridge::{set,is}_scanning`/`{set,is}_advertising`, two `AtomicBool` fields; zero Myco strings (`grep -c 'myco\|Myco'` returns 0)
- `myco-core/src/ble_bridge_jni.rs` — `bleDeliverScanningState`/`bleDeliverAdvertisingState` JNI exports
- `myco-core/src/aware_bridge_jni.rs` — `awareSetDiscovering` export + `aware_discovering()`/`set_aware_discovering()`; `awarePeerFound`/`awarePeerLost` gained a `lane` parameter, delegating lane bookkeeping to `lane_observation`
- `myco-core/src/lane_observation.rs` — new. Plain, non-JNI npub→lane record (`set_lane`/`clear_lane`/`snapshot`), 5 unit tests
- `myco-core/src/state.rs` — `BleStatus.{scanning_known,advertising,advertising_known}`, `WifiAwareStatus.{scanning,scanning_known}`
- `myco-core/src/runtime.rs` — `ble_radio_state()`, `aware_radio_state()`, `observed_lane_by_npub()` (each with an Android/host cfg split); `state()` populates the new `BleStatus`/`WifiAwareStatus` fields and now passes a real `lane_by_npub` snapshot instead of an empty map
- `myco-core/src/lib.rs` — declares the `lane_observation` module (`#[cfg_attr(not(target_os = "android"), allow(dead_code))]`, matching the existing pattern for Android-only-consumed pure modules)
- `android/app/src/main/java/app/myco/core/NativeCore.kt` — `bleDeliverScanningState`/`bleDeliverAdvertisingState`/`awareSetDiscovering` externals; `awarePeerFound`/`awarePeerLost` gained a `lane: String` parameter
- `android/app/src/main/java/app/myco/core/AppCoreClient.kt` — `AppState` fields for the new BLE/Aware observed facts
- `android/app/src/main/java/app/myco/ble/BleRadio.kt` — pushes scanning/advertising state at every transition site
- `android/app/src/main/java/app/myco/aware/AwareRadio.kt` — pushes discovering state at publish/subscribe-started and teardown; `LANE = "aware"` passed to the found/lost calls
- `android/app/src/main/java/app/myco/ap/ApRadio.kt` — `LANE = "udp"` passed to the found/lost calls; not in this plan's originally declared `files_modified` (see Deviations)

## Decisions Made

- Lane disambiguation could not be fixed in fips (`link_info.transport_type` is structurally `"udp"` for any UDP-riding transport) — the whole fix lives in Kotlin + myco-core, with zero fips diff.
- Lane is only ever what the pushing radio explicitly labels itself, never inferred from address shape (link-local vs. routable) — this phase's own prohibition on presenting inference as observation.
- Lane-tracking logic lives in a new non-JNI module (`lane_observation.rs`) rather than inside the `#[cfg(target_os = "android")]`-gated `aware_bridge_jni.rs`, so its populate/consume contract is unit-testable on the host.
- A lost push clears an npub's recorded lane only if its own lane still matches the one on record — protects against a stale loss from one lane clobbering a fresher record from the other.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing functionality, coordinator-directed] `android/app/src/main/java/app/myco/ap/ApRadio.kt` edited outside this plan's declared `files_modified`**
- **Found during:** the lane-disambiguation addition
- **Issue:** This plan's frontmatter `files_modified` lists `AwareRadio.kt` but not `ApRadio.kt`. The lane argument added to the shared `NativeCore.awarePeerFound`/`awarePeerLost` JNI entry points is a call-site contract change — both Kotlin callers (`AwareRadio` and `ApRadio`, which reuses the same bridge) must pass the new parameter or the build does not compile.
- **Fix:** Added `private const val LANE = "udp"` to `ApRadio`'s companion object and passed it at its three `awarePeerFound`/`awarePeerLost` call sites; updated the class doc comment to say it labels itself `"udp"` rather than implicitly masquerading as Aware through the shared seam.
- **Files modified:** `android/app/src/main/java/app/myco/ap/ApRadio.kt`
- **Commit:** fips-pop `76c4be1`

---

**Total deviations:** 1 auto-fixed (Rule 2 — a necessary consequence of the coordinator-directed lane-argument addition, not unrequested scope creep; both call sites of a shared JNI entry point had to pass the new parameter for the crate to compile).
**Impact on plan:** Necessary for correctness (the build would not link/compile otherwise) and directly required by the coordinator's own required-shape spec: "`ApRadio` passes `\"udp\"` and stops masquerading."

## Field Findings (Reference)

Full detail lives in [01-FIELD-FINDINGS.md](./01-FIELD-FINDINGS.md), carried forward from Task 1/2 execution on 2026-08-05 and confirmed fixed since:

- **F-01 (fixed, quick task 260805-e5h):** the node's one-shot BLE transport `start()` raced the Android foreground service's radio injection and was never retried on failure — Task 1's honest `scanning: idle` reading is what surfaced it (the old computed proxy would have hidden it). Fixed with a fips quarantine-and-retry supervisor (`8104849`, `59028d6`); BLE now recovers ~1.2s after cold launch, and pairing over BLE has since been confirmed working end-to-end between the two test phones.
- **F-02 (open, Phase 2 territory):** PSM never resolves for at least one advertising peer on both test phones — not yet diagnosed; plan 01-03's per-peer attempt log is the right artefact to characterise it.
- **F-03 (code-read finding, not yet observed failing):** Wi-Fi Aware has the same one-shot-start shape as F-01 at the Kotlin layer (`onAttachFailed` logs and gives up), but is partially rescued by a `ACTION_WIFI_AWARE_STATE_CHANGED` broadcast receiver that F-01's BLE path lacks.

These are peering faults surfaced *by* Phase 1's instrumentation, not instrumentation faults — they belong to Phase 2, not this plan.

## Verification Gap

The lane label was verified by unit test (`lane_observation.rs`'s 5 tests pinning populate/set/overwrite/matching-clear/stale-clear-does-not-clobber, plus `peer_diagnostics.rs`'s pre-existing 2 tests pinning the `lane_by_npub` precedence/fallback contract from 01-01) and by successful build across all three targets (`cargo build -p myco-core`, `cargo ndk … build -p myco-core --release` for `aarch64-linux-android`, `./gradlew :app:compileDebugKotlin :app:testDebugUnitTest`). It was **not** verified by observing a live Wi-Fi Aware NDP connection render `transport: "aware"` (as opposed to `"udp"`) on a real peer row during this session — no Aware NDP was exercised on-device in this execution window. Both test phones (Pixel 7 Pro `29131FDH3007HW`, Samsung SM-A528B `R5CR916CDCF`) remain attached; a spot-check with Wi-Fi Aware enabled on both and an NDP actually up is worth doing before Phase 1 is declared fully closed, in the same spirit as 01-01's own noted BLE-transport verification gap.

## Issues Encountered

None beyond the deviation above. `cargo fmt --all --check`, `cargo test -p myco-core` (53 passed, 1 ignored — network test, 0 failed), `cargo ndk` (arm64-v8a release), and `./gradlew :app:compileDebugKotlin :app:testDebugUnitTest` all passed clean on the first attempt after the lane-disambiguation edits.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

Plan 01-03 (attempt log) and 01-04 (full screen rebuild) can proceed:

- The `lane_by_npub` seam 01-01 built and this plan filled is now live end-to-end: Kotlin push → `lane_observation` → `merge_peers()`. Any future lane (e.g. a third UDP-riding transport) follows the same pattern without touching `merge_peers()`'s signature again.
- `BleStatus`/`WifiAwareStatus`'s observed scanning/advertising/discovering facts are ready for 01-04's radio self-check card to render directly — no new data source needed.
- DIAG-05 is now satisfiable end-to-end; marked complete in REQUIREMENTS.md alongside this summary.
- The Verification Gap above (no live Aware NDP exercised this session) is the one open item worth a device spot-check before Phase 1 closes, alongside 01-01's still-open BLE-transport gap.

---
*Phase: 01-make-peering-observable*
*Completed: 2026-08-06*

## Self-Check: PASSED

- FOUND: `myco-core/src/lane_observation.rs`
- FOUND: `reference/fips/src/transport/ble/android_io.rs`
- FOUND: `myco-core/src/aware_bridge_jni.rs`
- FOUND: `android/app/src/main/java/app/myco/ap/ApRadio.kt`
- FOUND: `android/app/src/main/java/app/myco/aware/AwareRadio.kt`
- FOUND commit `7a45600` (fips-pop)
- FOUND commit `4545de6` (fips-pop)
- FOUND commit `76c4be1` (fips-pop)
- FOUND commit `7e5a056` (fips)
