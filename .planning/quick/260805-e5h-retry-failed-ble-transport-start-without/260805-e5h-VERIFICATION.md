---
task: quick-260805-e5h
verified: 2026-08-05T11:00:00Z
status: human_needed
score: 6/6 must-haves verified
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Hold both phones (Pixel 7 Pro 29131FDH3007HW, Samsung SM-A528B R5CR916CDCF) together with no shared Wi-Fi network, after a fresh app launch with no manual node restart, and confirm they discover each other and form a mesh link over BLE."
    expected: "Both phones list each other as a peer over the BLE transport."
    why_human: "Requires physical co-location of two devices, which the executor and this verifier cannot perform. Independently confirmed instead: both phones bring their own BLE transport up on their own (scanning + advertising) after a fresh launch — but actual mutual discovery is unverified, and F-02 (PSM never resolving for at least one advertiser, reproduced live on the Pixel against 84:C5:A6:C8:43:F7 during this verification run) is a known, separately-tracked issue that could still prevent these two specific phones from completing discovery of each other even with BLE fully up on both sides."
---

# Quick Task 260805-e5h: Retry failed BLE transport start without restarting the node — Verification Report

**Task Goal:** Fix F-01 — BLE completely dark after every fresh Android app start (no
scanning, no advertising, zero BLE peers) until the node is manually restarted. Retry the
failed transport start without restarting the node.

**Verified:** 2026-08-05T11:00:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | A transport whose `start()` fails at node start is retained and retried on the node's existing 1s tick instead of being dropped | ✓ VERIFIED | `src/node/lifecycle.rs` `Err` arm now calls `self.quarantine_transport(handle, Self::now_ms())` instead of letting `handle` drop; `src/node/handlers/rx_loop.rs:265` calls `self.retry_pending_transports(now_ms).await` on the tick arm. `cargo test --lib transport_restart` passes 6/6, including the node-integration test proving a quarantined handle is absent from `Node::transports`, present in `pending_transports`, not retried before its due time, and promoted into `Node::transports` with state `Up` on the due tick. |
| 2 | Retries use exponential backoff capped at 30s and never give up | ✓ VERIFIED | `PendingTransport::backoff_ms()` in `transport_restart.rs:55-60` uses `checked_shl` + `saturating_mul` + `.min(RESTART_MAX_MS)`; `backoff_doubles_from_base`, `backoff_saturates_at_cap` (incl. `u32::MAX` attempts, no panic), and `never_gives_up_past_the_cap` all pass. |
| 3 | A retry that succeeds promotes the transport into `Node::transports` through the same code path a first-try success uses, including the `cfg(unix)` UDP fd hand-off | ✓ VERIFIED | `git show 8104849...` on `lifecycle.rs` shows the original `Ok` arm's fd-hand-off block moved verbatim into `adopt_started_transport()`; both `Node::start()`'s `Ok` arm and `retry_pending_transports`'s `Ok` arm call the same function. |
| 4 | The node is never restarted to recover BLE — peers, sessions and routes survive the retry | ✓ VERIFIED | `grep -n ".stop()\|Node::stop" src/node/transport_restart.rs src/node/handlers/rx_loop.rs` — no matches. `Node::stop()` (lifecycle.rs:1682) only adds `self.pending_transports.clear()`; no code path in the retry supervisor calls `stop()`, rebuilds the node, or touches `self.transports` other than inserting on success. |
| 5 | Only transports whose post-failure state reports `can_start()` are supervised; UDP/TCP (which stay in `Starting`) are dropped as today | ✓ VERIFIED | `quarantine_transport` guards on `!handle.state().can_start()` and returns early (drop) otherwise. `a_handle_that_cannot_restart_is_not_quarantined` reproduces a UDP bind failure (state stays `Starting`, `can_start()` false) and asserts it is dropped, not queued. |
| 6 | After a fresh Android install/launch with no manual node restart, fips logs "BLE transport started" and MycoBleRadio logs scanning plus "advertising PSM \<n\>" | ✓ VERIFIED (independently reproduced, not just SUMMARY's pasted logs) | Re-ran the exact Task 3 procedure myself on both attached devices (force-stop, clear logcat, `monkey` launch, 45s wait, `logcat -d`). **Pixel 7 Pro:** listener failure warn at `09:46:45.809943Z`, `BLE transport started` + `Transport started after retry attempts=1` at `09:46:46.981224Z` (~1.17s), `MycoBleRadio: scanning for FIPS peers` at `10:46:46.980`, `advertising PSM 161` at `10:46:47.158`. **Samsung SM-A528B:** listener failure warn at `09:47:36.665545Z`, transport started/retry at `09:47:37.847271Z` (~1.18s), `advertising PSM 223` at `11:47:37.877`. Both fresh, live runs, not read from the SUMMARY. |

**Score:** 6/6 truths verified (0 present-but-behavior-unverified)

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `reference/fips/src/node/transport_restart.rs` | Quarantine/retry supervisor module | ✓ VERIFIED | Exists, 339 lines, `PendingTransport`, `adopt_started_transport`/`quarantine_transport`/`retry_pending_transports`, 6 tests all passing. No `myco`/`Myco` string anywhere in the module. |
| `MockBleIo::fail_next_listens()` in `src/transport/ble/io.rs` | Failure-injection knob | ✓ VERIFIED | Present at line 628, `AtomicUsize` + `fetch_update`/`checked_sub` (not load-then-store), error text `"BLE radio not available"` matches the field failure exactly. |
| Unit test: failed BLE start retried and adopted without node restart | — | ✓ VERIFIED | `node_integration_tests::failed_ble_start_is_retried_and_adopted_without_node_restart` — passes. |
| Unit tests: backoff doubling, cap, never-gives-up, is_due boundary | — | ✓ VERIFIED | All 4 present in `tests` mod, all pass. |
| Unit test: `BleTransport::start_async` re-entrant after listen failure | — | ✓ VERIFIED | `transport::ble::tests::test_start_async_is_reentrant_after_listen_failure` — passes. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|-----|-----|--------|---------|
| `Node::start()` Err arm (lifecycle.rs) | `quarantine_transport()` | direct call, replacing the drop | ✓ WIRED | Confirmed in current source and in the `8104849` diff. |
| rx_loop tick (rx_loop.rs:265) | `retry_pending_transports(now_ms)` | `.await` call, positioned between `poll_pending_connects()` (264) and `poll_transport_discovery()` (285) | ✓ WIRED | Ordering matches plan exactly — a transport promoted this tick is discoverable this tick. |
| retry success | `adopt_started_transport()` → `Node::transports` + `cfg(unix)` fd send | direct call | ✓ WIRED | Same function used by both the first-try and retry success paths. |
| `AndroidIo::from_global()` late bridge resolution | retry observing the late-injected bridge | not re-audited (unchanged, pre-existing per-op resolution from commit 9121925) | ✓ WIRED (unchanged) | Out of scope for this diff — `9121925` already made per-operation resolution demand-driven; this task only adds the missing retry trigger for the one-shot `start()` sequence. |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| F-01 | 260805-e5h-PLAN.md | BLE dark after fresh app start until manual node restart | ✓ SATISFIED | All 6 truths above verified; independently reproduced live on both physical devices during this verification. |

### Anti-Patterns Found

None. Grepped all 6 touched fips files plus `reference/FIX-TODOS.md` for `TBD`/`FIXME`/`XXX`/`TODO`/`HACK`/`PLACEHOLDER` — zero matches.

### Behavioral Spot-Checks / Test Execution

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| transport_restart unit + integration tests | `cargo test --lib transport_restart -- --nocapture` | 6/6 passed | ✓ PASS |
| BLE transport suite (incl. re-entrancy test) | `cargo test --lib transport::ble::` | 55/55 passed | ✓ PASS (SUMMARY claimed "62-test BLE suite"; actual count is 55 — cosmetic discrepancy in the summary text, not a correctness issue) |
| Build | `cargo build --lib` | Clean, 0 warnings/errors | ✓ PASS |
| Clippy on touched files | `cargo clippy --lib --all-targets` | 0 new warnings/errors | ✓ PASS |
| No new dependency | `git diff 8104849~1..59028d6 -- Cargo.toml Cargo.lock` | Empty | ✓ PASS |
| No embedder-specific vocabulary | `grep -rniE 'myco' <6 touched files>` | Empty | ✓ PASS |
| On-device: Pixel 7 Pro fresh launch | force-stop, clear logcat, monkey launch, 45s wait, logcat -d (verifier-run, independent of SUMMARY) | listener-fail warn → `Transport started after retry` (~1.17s) → scanning → `advertising PSM 161` | ✓ PASS |
| On-device: Samsung SM-A528B fresh launch | same procedure (verifier-run) | listener-fail warn → `Transport started after retry` (~1.18s) → `advertising PSM 223` | ✓ PASS |

### Human Verification Required

### 1. Two-phone co-located BLE discovery

**Test:** Hold the Pixel 7 Pro (`29131FDH3007HW`) and Samsung SM-A528B (`R5CR916CDCF`)
together with no shared Wi-Fi network, after a fresh app launch on both with no manual node
restart, and confirm they discover each other and form a mesh link.
**Expected:** Both phones list each other as a peer over the BLE transport.
**Why human:** Requires physical co-location of two devices — the executor explicitly could
not perform this (Deviation #3 in SUMMARY.md), and this verifier cannot either. What has been
independently confirmed instead is that each phone brings its own BLE transport up on its own
(scanning + advertising) after a fresh launch, with no manual restart — that is the F-01 fix
and it is proven. Whether the two specific test phones actually complete discovery of *each
other* is a separate question gated by F-02 (PSM never resolving for at least one advertiser),
which this verifier's own fresh Pixel run reproduced live (`PSM not in advert yet` against
`84:C5:A6:C8:43:F7`) — a known, separately-tracked issue explicitly out of scope for this task
and deferred to plan 01-03.

## Gaps Summary

No gaps against this task's stated must-haves. All 6 declared truths, all 5 declared
artifacts, and all 4 declared key links are verified in the current codebase and confirmed by
both automated tests (executed by this verifier, not just cited from the SUMMARY) and two
independently-reproduced on-device runs. The node-restart constraint — the single most
important guard, given commit `9121925`'s prior finding that bouncing the node drops every
peer, session and route — holds: no `stop()`/rebuild path exists anywhere in the quarantine or
retry code. fips-side changes are upstream-extractable (no Myco vocabulary, no new
dependencies, capability-gated on `can_start()` rather than transport-type). The only open item
is the physical two-phone discovery check, which was never in this task's must-haves as a
truth (it appeared only as the plan's `<human-check>` step) and is correctly carried forward as
an outstanding human-verification item rather than silently dropped or falsely marked done.

---

_Verified: 2026-08-05T11:00:00Z_
_Verifier: Claude (gsd-verifier)_
