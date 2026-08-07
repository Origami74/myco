---
phase: quick-260805-e5h
plan: 01
subsystem: mesh-transport
tags: [fips, ble, transport-lifecycle, backoff, android]

requires:
  - phase: 01-make-peering-observable
    provides: F-01 field finding — BLE transport dark after every fresh app start until manual node restart
provides:
  - "Generic transport restart supervisor in fips (node/transport_restart.rs): a transport whose start() fails is retained, not dropped, and retried on the node's existing 1s tick"
  - "MockBleIo::fail_next_listens() failure-injection knob for deterministic listen-failure tests"
  - "Confirmed on both physical devices: fresh app launch recovers BLE (transport started, scanning, advertising) within ~1.2s, with no manual node restart"
affects: [phase-2-peering-fixes, F-02-psm-resolution, F-03-wifi-aware-onattachfailed]

tech-stack:
  added: []
  patterns:
    - "Quarantine-and-retry supervision: a failed handle goes into Node::pending_transports (invisible to all data-plane paths), and is promoted into Node::transports only once start() returns Ok, via a single shared adopt_started_transport() success path"
    - "Exponential backoff via checked_shl + saturating_mul, capped, never terminal — same idiom as the existing peer RetryState in node/retry.rs"

key-files:
  created:
    - reference/fips/src/node/transport_restart.rs
  modified:
    - reference/fips/src/node/mod.rs
    - reference/fips/src/node/lifecycle.rs
    - reference/fips/src/node/handlers/rx_loop.rs
    - reference/fips/src/transport/ble/io.rs
    - reference/fips/src/transport/ble/mod.rs
    - reference/FIX-TODOS.md

key-decisions:
  - "Retry supervision (shape a), not wake-on-radio-arrival (shape b) — no new signalling, no io-layer-to-node coupling; the 1s tick already exists and closes the 620ms race within ~1-2s"
  - "Quarantine into a separate pending_transports vector rather than admitting Failed handles into Node::transports directly — zero changes to ~50 existing consumers of that map"
  - "Supervisor gates on handle.state().can_start(), not on start() returning Err — UDP/TCP stay in Starting on error and are dropped exactly as today; only BLE's Failed state is retryable"
  - "Advertise/scan partial-failure (transport reports Up while dark) is explicitly scoped out and documented as a Limitations note on start_async, not fixed — closing it would mean re-running io calls on a transport with a live acceptor and spawned accept loop, out of scope for this fix"

requirements-completed: [F-01]

coverage:
  - id: D1
    description: "A transport whose start() fails at node start is retained and retried on the node's existing 1s tick instead of being dropped"
    requirement: F-01
    verification:
      - kind: unit
        ref: "reference/fips/src/node/transport_restart.rs#node_integration_tests::failed_ble_start_is_retried_and_adopted_without_node_restart"
        status: pass
      - kind: unit
        ref: "reference/fips/src/node/transport_restart.rs#node_integration_tests::a_handle_that_cannot_restart_is_not_quarantined"
        status: pass
    human_judgment: false
  - id: D2
    description: "Retries use exponential backoff capped at 30s and never give up"
    requirement: F-01
    verification:
      - kind: unit
        ref: "reference/fips/src/node/transport_restart.rs#tests::backoff_doubles_from_base"
        status: pass
      - kind: unit
        ref: "reference/fips/src/node/transport_restart.rs#tests::backoff_saturates_at_cap"
        status: pass
      - kind: unit
        ref: "reference/fips/src/node/transport_restart.rs#tests::never_gives_up_past_the_cap"
        status: pass
      - kind: unit
        ref: "reference/fips/src/node/transport_restart.rs#tests::is_due_boundary"
        status: pass
    human_judgment: false
  - id: D3
    description: "BleTransport::start_async is re-entrant after a listen failure (the claim the whole supervisor rests on)"
    requirement: F-01
    verification:
      - kind: unit
        ref: "reference/fips/src/transport/ble/mod.rs#tests::test_start_async_is_reentrant_after_listen_failure"
        status: pass
    human_judgment: false
  - id: D4
    description: "After a fresh Android install and launch with no manual node restart, both phones show BLE transport started, MycoBleRadio scanning, and advertising PSM <n>"
    requirement: F-01
    verification:
      - kind: manual_procedural
        ref: "adb logcat on 29131FDH3007HW and R5CR916CDCF after force-stop + clear logcat + monkey launch + 45s wait; see per-device evidence below"
        status: pass
    human_judgment: true
    rationale: "Physical on-device confirmation; the co-located two-phone BLE discovery human-check (holding both phones together) was not performed by the executor — see Deviations."

duration: 35min
completed: 2026-08-05
status: complete
---

# Quick Task 260805-e5h: Retry failed BLE transport start without restarting the node — Summary

**Generic quarantine-and-retry supervisor in fips (`node/transport_restart.rs`) recovers a BLE
transport that failed to start because the Android radio bridge wasn't injected yet — no node
restart, confirmed on both physical devices with ~1.2s recovery.**

## Performance

- **Duration:** ~35 min
- **Completed:** 2026-08-05
- **Tasks:** 3/3
- **Files modified:** 6 (5 in fips, 1 in fips-pop)

## Accomplishments

- A transport whose `start()` fails is now retained in `Node::pending_transports` instead of
  being silently dropped — the defect identified in F-01's root-cause analysis.
- `retry_pending_transports()` runs on the node's existing 1s maintenance tick, with exponential
  backoff (1s → 2s → 4s → ... capped at 30s), and never gives up on a permanently-absent radio.
- A retried transport is promoted through the exact same `adopt_started_transport()` path a
  first-try success uses, including the `cfg(unix)` UDP fd hand-off — a retried transport is
  indistinguishable from a first-try one.
- Only handles whose post-failure state reports `can_start()` are quarantined; UDP/TCP (which
  stay in `Starting` on a start error) are dropped exactly as they were before this change.
- Confirmed on both the Pixel 7 Pro and the Samsung SM-A528B: fresh install + launch, no manual
  node restart, BLE comes up on its own within ~1.2 seconds of the initial listener failure.

## Task Commits

1. **Task 1: Retry a failed transport start on the node tick, without restarting the node** —
   `8104849f094be2c18c848be7c7d7721322c7dff0` (feat, fips repo)
2. **Task 2: Pin the backoff schedule and start re-entrancy, and document the Up-but-dark gap** —
   `59028d6ece149b742945e61675d1e64c689386b4` (test, fips repo)
   - `044673751e863c82dc12875be2abe0a77ee80486` (docs, fips-pop repo — `reference/FIX-TODOS.md`)
3. **Task 3: Prove it on both phones — fresh launch, no manual node restart** — no code changes;
   build + install + on-device log capture only.

_All three fips commits are on `feat/platform-peer-queue` at
`/Users/gump/Documents/development/fips/fips`._

## Files Created/Modified

- `reference/fips/src/node/transport_restart.rs` (new) — quarantine/retry supervisor: constants,
  `PendingTransport` backoff struct, `Node::adopt_started_transport`/`quarantine_transport`/
  `retry_pending_transports`, plus unit + node-integration tests
- `reference/fips/src/node/mod.rs` — `mod transport_restart;`, `pending_transports` field on
  `Node`, initialized in both constructors
- `reference/fips/src/node/lifecycle.rs` — `Node::start()` Ok/Err arms delegate to the new
  supervisor methods; `Node::stop()` clears `pending_transports`
- `reference/fips/src/node/handlers/rx_loop.rs` — `retry_pending_transports(now_ms)` wired into
  the 1s tick, after `poll_pending_connects()` and before `poll_transport_discovery()`
- `reference/fips/src/transport/ble/io.rs` — `MockBleIo::fail_next_listens()` failure-injection
  knob, mirroring the field's exact "BLE radio not available" error text
- `reference/fips/src/transport/ble/mod.rs` — re-entrancy test; Limitations comment on
  `start_async` documenting the advertise/scan partial-failure gap
- `reference/FIX-TODOS.md` — two new GENERAL entries: the advertise/scan operational-but-dark
  gap, and F-03's `AwareRadio.kt onAttachFailed()` one-shot (deliberately not covered)

## Decisions Made

- Chose retry supervision (shape a) over wake-on-radio-arrival (shape b) — no new signalling, no
  io-layer coupling into node lifecycle, and the existing 1s tick closes the 620ms race within
  1-2s, which is fast enough.
- Quarantined handles go into a new `pending_transports` vector rather than being admitted into
  `Node::transports` in a non-operational state — avoids auditing ~50 existing consumers of that
  map on release day.
- Gated retry eligibility on `TransportState::can_start()` rather than "start returned Err" —
  keeps UDP/TCP behaving exactly as today (dropped on failure) since they leave `Starting`, not
  `Failed`.
- Deliberately left the advertise/scan partial-failure case (BLE reports `Up` while dark)
  unfixed — closing it means re-running `io` calls on a transport that already owns a live
  acceptor and spawned accept loop, judged out of scope for a release-day fix. Documented in
  `start_async` and `FIX-TODOS.md` instead.

## Deviations from Plan

**1. [Process] Task 1's commit included Task 2's pure-policy backoff tests.**
- **Found during:** Task 1 (writing the node-level test slice)
- **What happened:** While writing `src/node/transport_restart.rs` for Task 1, the four pure
  `PendingTransport` policy tests (`backoff_doubles_from_base`, `backoff_saturates_at_cap`,
  `never_gives_up_past_the_cap`, `is_due_boundary`) were written and committed alongside the
  Task 1 node-integration tests, rather than being added separately in Task 2 as the plan's task
  breakdown specified.
- **Impact:** None on correctness or test coverage — all tests the plan calls for exist and pass,
  gated exactly as specified (`#[cfg(test)]` for the pure tests, `#[cfg(all(test, ble_available))]`
  for the ones requiring `TransportHandle::Ble`). Task 2's commit then added the BLE re-entrancy
  test, the `start_async` Limitations note, and the `FIX-TODOS.md` entries as planned.
- **Not a Rule 1-4 auto-fix** — a scheduling deviation only, noted for transparency.

**2. [Rule 3 - Blocking] Force-added a gitignored file the plan explicitly required tracked.**
- **Found during:** Task 2 (committing `reference/FIX-TODOS.md`)
- **Issue:** `/reference/` is gitignored wholesale in fips-pop (`.gitignore:3`), so
  `reference/FIX-TODOS.md` was untracked with no prior history despite the plan and the task's
  `repo_layout_critical` instructions explicitly stating it "lives in the fips-pop repo... commit
  that one in fips-pop."
  This is a case where committing the file requires overriding the gitignore for that one path.
- **Fix:** `git add -f reference/FIX-TODOS.md`, scoped to that single file only — no other
  gitignored path (in particular, nothing under `reference/fips`) was force-added.
- **Files modified:** `reference/FIX-TODOS.md`
- **Verification:** `git log -- reference/FIX-TODOS.md` now shows the commit; `git status` after
  the commit shows no other force-tracked files.
- **Committed in:** `044673751e863c82dc12875be2abe0a77ee80486`

**3. [Not fixed — deferred to human] The two-phone co-located BLE discovery human-check was not performed.**
- **Found during:** Task 3 verification
- **Issue:** Task 3's `<verify>` block includes a `<human-check>`: "Hold the two phones together
  with no shared Wi-Fi network and confirm they discover each other over BLE." This requires
  physically manipulating the two attached devices, which the executor cannot do.
- **What was done instead:** The automated verification was run in full — both devices
  independently confirmed to bring up BLE transport, scanning, and advertising on their own after
  a fresh launch. F-02 (per-peer PSM not resolving) is a known, separately-tracked issue that may
  affect whether the two specific test phones actually connect to each other even with BLE up on
  both — that's explicitly out of scope for this fix (deferred to plan 01-03).
- **Action for user:** Hold the two phones together with no shared network and confirm mesh
  discovery, per the plan's human-check step.

---

**Total deviations:** 1 process note, 1 auto-fixed (Rule 3), 1 deferred human-check.
**Impact on plan:** No scope creep; all fixes were necessary to satisfy the plan's explicit
instructions (FIX-TODOS.md tracking) or are inherent to running as an unattended executor
(physical device manipulation).

## Issues Encountered

None beyond the deviations above. Both devices reproduced the exact F-01 race (initial listener
failure warn, followed ~1.2s later by successful retry) on the first launch attempt — no re-runs
were needed.

## On-Device Verification

Build: `cd android && ./gradlew assembleDebug` (cross-compiled the Rust from
`reference/fips` at commit `59028d6`), then `ANDROID_SERIAL=<serial> ./gradlew installDebug` per
device.

Procedure per device: `am force-stop app.myco` → `logcat -c` → `monkey -p app.myco -c
android.intent.category.LAUNCHER 1` → 45s wait, untouched → resolve current pid via `pidof
app.myco` → `logcat -d --pid=<pid>`. All timestamps below are the `myco` tracing UTC timestamps
(not logcat wall-clock, which differs by timezone between the two phones).

### Pixel 7 Pro (`29131FDH3007HW`)

```
2026-08-05T09:39:45.050779Z  WARN fips::transport::ble: failed to start BLE listener adapter=ble0 error=io error: BLE radio not available
2026-08-05T09:39:45.050808Z  WARN fips::node::lifecycle: Transport failed to start transport_type="ble" error=io error: BLE radio not available
2026-08-05T09:39:46.219632Z  INFO fips::transport::ble: BLE transport started adapter=ble0 psm=133
2026-08-05T09:39:46.219647Z  INFO fips::node::transport_restart: Transport started after retry transport_type="ble" transport_id=transport:2 attempts=1
2026-08-05T09:39:46.219...Z  I MycoBleRadio: scanning for FIPS peers (low-latency)
2026-08-05T09:39:46.497...Z  I MycoBleRadio: advertising PSM 159 (in primary advert)
```

**Recovery interval: 1.169s** (initial warn → transport started).

### Samsung SM-A528B (`R5CR916CDCF`)

```
2026-08-05T09:39:45.262623Z  WARN fips::transport::ble: failed to start BLE listener adapter=ble0 error=io error: BLE radio not available
2026-08-05T09:39:45.262690Z  WARN fips::node::lifecycle: Transport failed to start transport_type="ble" error=io error: BLE radio not available
2026-08-05T09:39:46.494403Z  INFO fips::transport::ble: BLE transport started adapter=ble0 psm=133
2026-08-05T09:39:46.494416Z  INFO fips::node::transport_restart: Transport started after retry transport_type="ble" transport_id=transport:2 attempts=1
2026-08-05T09:39:46.494...Z  I MycoBleRadio: scanning for FIPS peers (low-latency)
2026-08-05T09:39:46.520...Z  I MycoBleRadio: advertising PSM 210 (in primary advert)
```

**Recovery interval: 1.232s** (initial warn → transport started).

Both devices show the exact before/after that previously required a manual node restart: the
initial listener failure warn, immediately followed (within one backoff cycle) by `Transport
started after retry` and then live scanning + advertising — with no node restart anywhere in the
launch sequence. This directly confirms F-01 is fixed.

Note: F-02 (per-peer PSM not resolving) reproduced on both devices during the scan window
(`PSM not in advert yet` against `84:C5:A6:C8:43:F7`) — expected, unrelated, and already tracked
separately for plan 01-03.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- F-01 is resolved and confirmed on-device on both test phones. `pending_transports` and the
  restart supervisor are generic fips additions with no Myco-specific coupling, ready to extract
  upstream.
- **Outstanding manual step:** hold the two phones together (no shared Wi-Fi) and confirm they
  discover each other over BLE — this was not performed by the executor (see Deviations #3).
- F-02 (PSM never resolving for at least one advertiser) and F-03 (Aware's `onAttachFailed()`
  one-shot) remain open, tracked in `reference/FIX-TODOS.md` and deferred to later Phase 2 work.
- The advertise/scan partial-failure gap (BLE reporting `Up` while dark) is documented but not
  fixed — flagged as a known limitation, not a blocker for this task's scope.

---
*Quick task: 260805-e5h*
*Completed: 2026-08-05*

## Self-Check: PASSED

All referenced files confirmed present on disk; all three commit hashes (fips `8104849`,
fips `59028d6`, fips-pop `0446737`) confirmed present in their respective repo histories via
`git cat-file -e`.
