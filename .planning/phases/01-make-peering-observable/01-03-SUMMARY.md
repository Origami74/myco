---
phase: 01-make-peering-observable
plan: 03
subsystem: infra
tags: [rust, fips, ble, diagnostics, persistence, jsonl]

# Dependency graph
requires:
  - "`myco-core::peer_diagnostics::merge_peers()` and `PeerDiagnosticView` (01-01)"
  - "`AppRuntime::state()`'s cfg-split radio accessor pattern (01-01, 01-02)"
provides:
  - "`fips::transport::ble::attempts` — a generic, bounded per-peer connect-attempt log inside the BLE transport (role, discovery latency, outcome, send failures)"
  - "Recording at every connect-attempt resolution on BOTH sides of the cross-probe tiebreaker, so a runtime disagreement between two nodes is visible as evidence"
  - "`myco-core::attempt_store::AttemptStore` — corruption-tolerant JSONL persistence so attempt history survives a force-stop"
  - "`PeerDiagnosticView.{role,discoveryMs,sendDrops,attempts}` + `PeerAttemptView` on every peer row"
  - "The learned BLE-address→node-address mapping that completes step 2 of `merge_peers()` (D-09), left open by 01-01"
affects: [01-04-make-peering-observable]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Measurement must not perturb what it measures: every recording call sits beside an existing trace, adds no branch/sleep/await, and both pool sites record only AFTER the pool guard is dropped so the log is never touched inside a lock the transport holds."
    - "One discovery-to-resolution cycle is exactly one log entry with exactly one outcome enum — never several fragments, which would burn the per-peer cap on pieces of a single attempt."
    - "Line-delimited persistence with per-line failure isolation: a truncated final line costs one entry, a garbled line costs that line. Never a whole-document deserialize (the CORE-03 defect this file exists not to repeat)."
    - "A file that mostly fails to parse is COPIED aside before any write, never moved and never rewritten — a still-good file we merely failed to understand must not be replaced by a shorter one."
    - "Absence renders as absence: a peer with no recorded attempts gets an empty list, an empty role and zero counters, never a fabricated entry or a guessed default role."

key-files:
  created:
    - reference/fips/src/transport/ble/attempts.rs
    - myco-core/src/attempt_store.rs
  modified:
    - reference/fips/src/transport/ble/mod.rs
    - myco-core/src/peer_diagnostics.rs
    - myco-core/src/state.rs
    - myco-core/src/runtime.rs
    - myco-core/src/lib.rs

key-decisions:
  - "The attempt log is a process-global singleton in the fips BLE module reached through a free accessor (`ble_attempt_log()`), mirroring the existing global bridge accessor in `android_io.rs`, rather than a value threaded through `accept_loop`/`scan_probe_loop`. Both already carry a too-many-arguments allowance; widening their signatures would be a larger and harder-to-extract upstream diff than a module-local singleton."
  - "`record_outcome()` was added beyond the plan's literal shape. The plan's site pattern (build a `BleAttempt` by hand beside a `discovery_elapsed_ms()` call) takes the lock TWICE per outcome and would have been repeated at eleven sites. `record_outcome` stamps the wall clock and elapsed discovery internally under one lock, and keeps each recording site to a single statement that cannot accidentally reorder around the trace it sits beside. Directly serves the plan's own non-perturbation prohibition."
  - "Send-failure counts are recorded but deliberately NOT persisted. The counter describes the current process's link; a stale count restored from disk would read as current evidence, which is the same class of error as presenting an inferred value as an observed one."
  - "`AttemptStore::observe`/`flush` take `&self` with interior mutability rather than the plan's `&mut self`. The flush must be spawned onto the tokio runtime (never the FFI thread), which requires a shared `Arc`; `&mut self` cannot satisfy that."
  - "The MTU-exceeded arm in `send_async` deliberately does NOT record a send failure. An oversized packet is a caller bug, not a property of the peer's link, and conflating the two would make the drop count useless as evidence. Pinned by a negative source assertion."
  - "Step 2 advert attribution is driven ONLY by address→node-address pairs the log actually learned. Without a learned pair the advert still gets its own row — inferring identity from address shape is exactly the inference-presented-as-observation this phase prohibits. Pinned by a paired positive/negative test."

requirements-completed: [DIAG-01, DIAG-03]

coverage:
  - id: A1
    description: "Per peer, the diagnostics carry which BLE role this device chose, how long discovery took, and how many sends failed — without a debugger attached"
    verification:
      - kind: unit
        ref: "myco-core/src/peer_diagnostics.rs#recorded_lost_tiebreaker_reaches_the_serialized_row — asserts role/discoveryMs/sendDrops/outcome/atMs survive serde_json serialization in camelCase"
        status: pass
      - kind: unit
        ref: "reference/fips/src/transport/ble/attempts.rs#tests (9 tests)"
        status: pass
    human_judgment: false
  - id: A2
    description: "Every connect attempt resolves to exactly one recorded outcome, and both sides of the cross-probe tiebreaker are recorded (inbound drop = peripheral losing, outbound yield = central losing)"
    verification:
      - kind: unit
        ref: "cargo test --lib transport::ble (62 passed, incl. pre-existing test_tiebreaker_convention unchanged)"
        status: pass
      - kind: build
        ref: "Structural: 12 ble_attempt_log() sites; accept_loop carries a Peripheral LostTiebreaker record and scan_probe_loop a Central one"
        status: pass
      - kind: manual_procedural
        ref: "Two phones producing a real runtime tiebreaker disagreement — DEFERRED, see Verification Gap"
        status: deferred
    human_judgment: true
    rationale: "The recording sites and their non-perturbation are pinned structurally and by the unchanged pre-existing BLE suite. Whether the two phones actually AGREE at runtime is the hypothesis this instrument exists to test, and it cannot be answered without two devices in a room — that is the deliverable, not the code."
  - id: A3
    description: "Attempt history survives a crash or force-stop; a malformed or truncated file yields the entries that did parse and never renders as a red error; a corrupted file is copied aside rather than silently destroyed"
    verification:
      - kind: unit
        ref: "myco-core/src/attempt_store.rs#tests (8 tests: round-trip, truncated final line, mostly-garbage preserve + original bytes intact, missing file, ring cap, age eviction, idempotent re-observe, mixed good/bad lines)"
        status: pass
      - kind: manual_procedural
        ref: "Force-stop from Android settings and confirm the Dev tab still shows pre-force-stop attempt history — DEFERRED, see Verification Gap"
        status: deferred
    human_judgment: true
    rationale: "The corruption, truncation, cap and eviction contracts are pinned precisely by unit test against real files on disk. The force-stop path itself exercises Android process death, which no host test reproduces."
  - id: A4
    description: "A device seen as a raw BLE advert and later resolved to a node address collapses into that peer's single row (D-09)"
    verification:
      - kind: unit
        ref: "myco-core/src/peer_diagnostics.rs#advert_with_learned_node_addr_collapses_into_the_peer_row and #advert_without_learned_node_addr_stays_a_separate_row"
        status: pass
    human_judgment: false
  - id: A5
    description: "Recording does not alter the tiebreaker outcome, connect timeout, cooldown, retry interval, pool admission decision, or the order of any existing radio call"
    verification:
      - kind: build
        ref: "Negative source assertion: no sleep/await added inside scan_probe_loop; both pool sites record after the guard is dropped"
        status: pass
      - kind: unit
        ref: "Full fips suite: 1402 passed, 0 failed — no pre-existing behavioural test regressed"
        status: pass
      - kind: build
        ref: "cargo ndk -t arm64-v8a --platform 29 build -p myco-core --release (22 MB aarch64 .so); cd android && ./gradlew :app:compileDebugKotlin :app:testDebugUnitTest — BUILD SUCCESSFUL"
        status: pass
    human_judgment: true
    rationale: "Non-perturbation is asserted structurally plus by the unchanged pre-existing suite. A behavioural proof would need a two-node BLE harness that does not exist and was explicitly out of scope."

# Metrics
duration: 150min
completed: 2026-08-06
status: complete
---

# Phase 1 Plan 3: The BLE Attempt Log Summary

**Per peer, the BLE role this device chose, how long discovery took, how the attempt resolved and how many sends failed are now recorded at every outcome site on both sides of the cross-probe tiebreaker, and survive a force-stop in a bounded, corruption-tolerant JSONL store — so a race that resolved thirty seconds ago still leaves evidence.**

## Performance

- **Duration:** ~150 min
- **Tasks:** 3 completed (1 tracer + 2 auto)
- **Files modified:** 7 (2 created: `attempts.rs` in fips, `attempt_store.rs` in myco-core)

## Accomplishments

- **Task 1 (tracer) — one recorded fact, end to end.** New `fips::transport::ble::attempts` module: `BleRole`, `BleAttemptOutcome` (six variants with stable kebab-case wire labels), `BleAttempt`, `BlePeerAttempts`, and a `BleAttemptLog` holding one `Mutex` over per-address rings, in-flight discovery stamps, send-failure counters and learned address→node-address pairs. One recording site (the outbound tiebreaker yield) carried a lost-tiebreaker all the way to serialized `AppState` JSON, proven by a unit test asserting the camelCase payload.
- **Task 2 — every outcome, both sides.** Twelve recording sites: connect error, connect timeout, pubkey-exchange failure, pool rejection and success on the outbound path; the same five plus a discovery stamp on the inbound path; and per-peer link send failures in `send_async`. **The inbound tiebreaker drop is the point of the exercise** — its outbound counterpart was already recorded, so two nodes that disagree at runtime now leave two recorded losses instead of no evidence at all.
- **Task 3 — history survives process death.** `myco-core::attempt_store::AttemptStore` reads `<data_dir>/ble-attempts.jsonl` line by line with per-line failure isolation, writes atomically via temp-then-rename, caps at 20 per address, evicts addresses idle over 24h on write, and copies a mostly-unparseable file to a `.corrupt` sibling *before* it is ever allowed to write. `observe()` runs inside `state()` on the FFI thread and does no I/O; `flush()` is spawned onto tokio and rate-limited to once per 5s.
- **Completed the D-09 attribution 01-01 left open.** `merge_peers()` now uses the log's learned address→node-address pairs to collapse a raw advert into the peer row it belongs to, instead of emitting a second row for the same device — with a paired negative test proving that *without* a learned pair the advert still gets its own row.
- **fips stays upstream-extractable.** `attempts.rs` contains zero `myco`/`Myco`/`android` substrings (asserted by grep), names no embedder type or platform API, and the whole 01-03 fips footprint is exactly two files.

## Task Commits

Each task was committed atomically, in both trees where it touched both:

1. **Task 1 (tracer): end-to-end recorded lost tiebreaker** — fips `4e3dfa8` (feat), fips-pop `3563885` (feat)
2. **Task 2: every outcome site, both sides of the tiebreaker** — fips `5c49a44` (feat)
3. **Task 3: crash-surviving JSONL store** — fips-pop `86c233a` (feat)

## Files Created/Modified

- `reference/fips/src/transport/ble/attempts.rs` — **new.** The whole attempt log: types, bounded ring, discovery stamps, send-failure counters, learned address pairs, `ble_attempt_log()` global accessor, 9 unit tests. Zero Myco/platform strings.
- `reference/fips/src/transport/ble/mod.rs` — `pub mod attempts;` plus 12 recording sites beside existing traces; no control flow, timing, ordering, timeout, cooldown or pool decision changed.
- `myco-core/src/attempt_store.rs` — **new.** Corruption-tolerant JSONL store, 8 unit tests.
- `myco-core/src/peer_diagnostics.rs` — `merge_peers()` gained a `&[BlePeerAttempts]` parameter; step 2 advert attribution completed via learned pairs; new step 6 joins role/discovery/send-drops/attempts onto each row; 4 new tests.
- `myco-core/src/state.rs` — `PeerAttemptView` plus `role`/`discovery_ms`/`send_drops`/`attempts` on `PeerDiagnosticView`.
- `myco-core/src/runtime.rs` — cfg-split `ble_attempts()` accessor; `state()` folds the live snapshot into the store, spawns the rate-limited flush, and passes the merged history to `merge_peers()`.
- `myco-core/src/lib.rs` — declares `attempt_store`.

## Decisions Made

See `key-decisions` in the frontmatter. The two worth restating:

- **`record_outcome()` halves the lock traffic** relative to the plan's literal site pattern, and was chosen specifically to serve the plan's own non-perturbation prohibition rather than for brevity.
- **Send-failure counts are not persisted**, because a stale count restored from disk would read as current evidence.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 — interface shape] `AttemptStore::observe`/`flush` take `&self`, not the plan's `&mut self`**
- **Found during:** Task 3 wiring
- **Issue:** The plan requires the flush to be spawned onto the tokio runtime and never run on the FFI thread. A spawned task needs an owned `Arc<AttemptStore>`; `&mut self` cannot be shared that way.
- **Fix:** Interior mutability (`Mutex<Inner>`), with the lock released before the write.
- **Commit:** fips-pop `86c233a`

**2. [Rule 2 — added helper] `BleAttemptLog::record_outcome()` beyond the plan's literal record-site shape**
- **Found during:** Task 2, at the point of repeating the plan's site pattern eleven times
- **Issue:** The plan's shape takes the lock twice per outcome (once for `discovery_elapsed_ms`, once for `record`) and would have duplicated a six-field struct literal at every site.
- **Fix:** One helper stamping clock and discovery internally under a single lock; `record()` retained for callers that build a full `BleAttempt`.
- **Commit:** fips `5c49a44`

---

**Total deviations:** 2 auto-fixed, both Rule 2. Neither changes what is recorded or when; both reduce lock traffic and duplication in service of the plan's own non-perturbation constraint.

## Verification Gap

**Everything host-verifiable passed. Two device checks are deferred and batched for the next on-device session — see [DEVICE-TEST-BATCH.md](./DEVICE-TEST-BATCH.md).**

- **The tiebreaker-agreement question itself is unanswered.** This plan built the instrument; it did not read it. Whether the two phones actually agree at runtime — the hypothesis the roadmap forbids Phase 2 from acting on by inference — needs both devices in a room. Everything structural about the recording is pinned, but that is the deliverable.
- **The force-stop path is not exercised.** `AttemptStore`'s corruption, truncation, cap and eviction contracts are pinned by unit tests against real files, but Android process death is not reproduced by any host test.
- **F-02 remains uncharacterised.** This log is the instrument built for it; reading it against a live advertiser is device work.

Note also that this log will **not** characterise F-04 — the `ENETUNREACH` relay dials never reach a BLE connect attempt, since that fault lives in the relay layer above the transport. Recorded in 01-FIELD-FINDINGS.md.

## Issues Encountered

- The Linux build host cannot satisfy `cargo clippy --all-targets -- -D warnings` for environment reasons unrelated to this plan (ARM `char` signedness making three `unnecessary_cast` sites fire that are *required* on x86_64/macOS, plus newer-clippy lints). Worse, with `-D warnings` clippy **aborts at the first failing crate**, and `nsite-deck` fails — so the plan's own verify command never reaches myco-core and would silently pass a myco-core regression. Verified by injecting a lint and watching it go unreported. Replaced on this host by `reference/clippy-gate.sh`, which collects findings without `-D`, diffs against a recorded 9-entry baseline keyed by lint+file with counts, and fails only on genuinely new findings. Self-tested by injection in both directions.
- No functional issues. All three tasks passed their gates on the first attempt after formatting.

## User Setup Required

None for this plan. Device-side verification is batched in `DEVICE-TEST-BATCH.md`.

## Next Phase Readiness

Plan 01-04 (Dev tab rebuild) can proceed:

- `PeerDiagnosticView` now carries `role`, `discoveryMs`, `sendDrops` and `attempts` on every row, riding the same `state()` payload as the rows themselves — so 01-04's expanding peer row needs **no second fetch**, closing the open question UI-SPEC left at E3.
- Attempt outcome labels are stable kebab-case wire strings, safe to switch on in Kotlin.
- DIAG-01 and DIAG-03 are satisfiable end-to-end. DIAG-06 and DIAG-07 remain open and close in 01-04.

---
*Phase: 01-make-peering-observable*
*Completed: 2026-08-06*

## Self-Check: PASSED

- FOUND: `reference/fips/src/transport/ble/attempts.rs`
- FOUND: `myco-core/src/attempt_store.rs`
- FOUND: `myco-core/src/peer_diagnostics.rs`
- FOUND: `myco-core/src/state.rs`
- FOUND: `myco-core/src/runtime.rs`
- FOUND commit `4e3dfa8` (fips)
- FOUND commit `5c49a44` (fips)
- FOUND commit `3563885` (fips-pop)
- FOUND commit `86c233a` (fips-pop)
