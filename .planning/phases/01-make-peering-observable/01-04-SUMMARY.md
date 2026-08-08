---
phase: 01-make-peering-observable
plan: 04
subsystem: frontend
tags: [kotlin, compose, android, diagnostics, ui]

# Dependency graph
requires:
  - "`AppState.peers` merged rows with role/discoveryMs/sendDrops/attempts (01-01, 01-03)"
  - "Observed BLE/Aware radio facts with their `_known` siblings (01-02)"
provides:
  - "The Dev tab in its final column order: radio self-check, peers, pending pairings, identity, raw detail, speedtest"
  - "`RadioSelfCheckCard` — six tri-state observed radio facts, fixed order, `unknown` for anything unobservable"
  - "Peer rows that expand in place onto role, discovery latency, send drops, RSSI and the newest 20 attempts"
  - "`app.myco.core.PeerAttempt` and the forensics fields on `PeerDiagnostic`"
  - "A screen-owned 1s poll, lifecycle-gated, that never asserts an absence before its first read"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Tri-state facts: a row renders true / false / `unknown`, where `unknown` is a first-class value in the neutral colour — never the error colour, because a radio that cannot be read is reporting honestly rather than failing."
    - "Absence is a claim: 'No peers yet' and 'none' are assertions about the world, so they are withheld behind a first-read flag and a terse loading line rather than flashed on every cold open."
    - "Screen-local cadence: a screen that needs a faster refresh than the shell drives its own `repeatOnLifecycle`-gated poll rather than raising the shared rate, because `state()` takes many core locks."

key-files:
  created: []
  modified:
    - android/app/src/main/java/app/myco/ui/screens/DevScreen.kt
    - android/app/src/main/java/app/myco/core/AppCoreClient.kt

key-decisions:
  - "`RadioSelfCheckCard` renders its six rows in a fixed compile-time order that never varies with data, so the card's shape is stable enough to read at a glance while comparing two phones."
  - "The expansion set is saved as an `ArrayList`, not a `List`. `rememberSaveable`'s default registry only accepts Bundle-storable types and `toList()` on an empty collection returns Kotlin's `EmptyList` singleton — which crashed the screen on exactly the state every cold open starts in. Caught on device; both compilation and the unit tests were green."
  - "Metric lines render above the attempt list, not below it, so a peer with no recorded history still reports role, drops and RSSI rather than showing only the no-history line."
  - "An unidentified peer renders its address alone. The collapsed row never falls back to a name for a peer whose identity is unresolved."

requirements-completed: [DIAG-01, DIAG-03, DIAG-04, DIAG-05, DIAG-06, DIAG-07]

coverage:
  - id: U1
    description: "The Dev tab leads with a radio self-check whose row set and order are fixed, with `unknown` for any unobservable fact"
    verification:
      - kind: build
        ref: "./gradlew :app:compileDebugKotlin :app:testDebugUnitTest — BUILD SUCCESSFUL, ThemeTest green"
        status: pass
      - kind: manual_procedural
        ref: "On device (SM-A528B): six rows render in order — ble enabled/scanning/advertising, aware supported/available/discovering"
        status: pass
    human_judgment: true
  - id: U2
    description: "Column order is self-check, peers, pending pairings, identity, raw detail, speedtest; the cache card is gone"
    verification:
      - kind: build
        ref: "Line-order assertion: RadioSelfCheckCard invoked before SpeedtestCard; `! grep state.cache`"
        status: pass
      - kind: manual_procedural
        ref: "On device: screenshot confirms the order and the absent cache card"
        status: pass
    human_judgment: true
  - id: U3
    description: "Tapping a peer row expands it in place onto role, discovery latency, drops, RSSI and the newest 20 attempts"
    verification:
      - kind: manual_procedural
        ref: "On device: expanded row rendered role=central, discovery=429ms, send drops=0, rssi=-51dBm and seven timestamped attempts with outcomes"
        status: pass
    human_judgment: true
  - id: U4
    description: "A peer with no recorded history renders its metric lines plus the neutral no-history line, never red and never a blocked screen"
    verification:
      - kind: manual_procedural
        ref: "On device: `slate sammy` rendered em-dashes for role/discovery/rssi, 0 drops, and 'No history for this peer' in the neutral colour"
        status: pass
    human_judgment: true
  - id: U5
    description: "Identity and pending pairings are on screen above the speedtest"
    verification:
      - kind: manual_procedural
        ref: "On device: IDENTITY shows own npub and circle name 'cyan kai'; PENDING PAIRINGS (0) shows 'none'"
        status: pass
    human_judgment: true
  - id: U6
    description: "The screen refreshes on its own ~1s cadence while visible and stops when it is not"
    verification:
      - kind: build
        ref: "LaunchedEffect + repeatOnLifecycle(STARTED) + client.state() + delay(1000); no CircularProgressIndicator"
        status: pass
      - kind: manual_procedural
        ref: "On device: the peer list gained a newly discovered peer between screenshots without user action"
        status: pass
    human_judgment: true
  - id: U7
    description: "No new colour constant, no Material icon, and MycoApp.kt / MainActivity.kt unchanged"
    verification:
      - kind: build
        ref: "`! grep Color(0x`, `! grep material.icons`, `git diff --name-only` empty for both files"
        status: pass
    human_judgment: false

# Metrics
duration: 75min
completed: 2026-08-06
status: complete
---

# Phase 1 Plan 4: The Dev Tab, Rebuilt Around Peering Summary

**The Dev tab now opens on six observed radio facts, then a peer list whose rows expand in place onto role, discovery latency, drop count and recorded attempt history — so "is it me or is it them", and "why did that connection fail", are both answerable on the phone with no debugger attached.**

## Performance

- **Duration:** ~75 min
- **Tasks:** 3 completed (1 tracer + 2 auto), plus one device-caught crash fix
- **Files modified:** 2

## Accomplishments

- **Task 1 — the column.** `RadioSelfCheckCard` leads the screen with six tri-state facts in a fixed compile-time order. A new `KeyValTri` row extends the existing `KeyValDot` anatomy from two states to three so an unobservable fact renders `unknown` in the neutral colour — never a confident `false`, and never the error colour. `PendingPairingsCard` (DIAG-06) and `IdentityCard` (DIAG-07) added; the CACHE card deleted; the speedtest demoted below all peering content.
- **Task 2 — expansion.** `PeerAttempt` and the forensics fields land in `AppCoreClient`, each defaulting so a pre-01-03 payload still parses. Rows expand in place onto metric lines plus at most the newest 20 attempts — matching the core store's retention exactly — degrading to the neutral "No history for this peer" line when there is nothing recorded.
- **Task 3 — cadence.** The screen drives its own 1s `repeatOnLifecycle(STARTED)` poll rather than raising the shell's, seeded from the hoisted state so the first frame is real content. Neither the peer list nor the pending-pairings card asserts an absence before the first local read lands.
- **The instrument was read, and it changed the answer.** See `<Field Findings>` below and F-05: the first real reading of the 01-03 attempt log **does not support** the tiebreaker-race hypothesis Phase 2 was going to act on.

## Task Commits

1. **Task 1 (tracer): the column, rebuilt** — `78c5998`
2. **Task 2: peer rows expand onto their forensics** — `5b83243`
3. **Task 3: screen-owned 1s refresh** — `a55ef61`
4. **Device-caught crash fix** — `41ab89b`

## Files Created/Modified

- `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` — `RadioSelfCheckCard`, `PendingPairingsCard`, `IdentityCard`, `KeyValTri`, `PeerForensics`, `ForensicLine`, `AttemptRow`, the expansion map and saver, the screen-owned poll; CACHE card removed.
- `android/app/src/main/java/app/myco/core/AppCoreClient.kt` — `PeerAttempt`; `role`/`discoveryMs`/`sendDrops`/`attempts` on `PeerDiagnostic` plus their parsing.

## Deviations from Plan

**None to the plan's shape.** One defect found and fixed during device verification — the `rememberSaveable` saver crash described in `key-decisions` and commit `41ab89b`.

## Verification

Verified on the Samsung SM-A528B (`R5CR916CDCF`) over the adb bridge, not just by compilation:

| Property | Evidence |
|---|---|
| Column order + no cache card | screenshot |
| Six self-check rows, fixed order | screenshot: all six rendered, all observed |
| Expansion onto forensics | screenshot: role=central, discovery=429ms, drops=0, rssi=-51dBm, 7 timestamped attempts |
| No-history degradation | screenshot: `slate sammy` — em-dashes + "No history for this peer", neutral |
| Identity + pending pairings | screenshot: own npub, circle name "cyan kai", pending "none" |
| Live refresh | a newly discovered peer appeared between screenshots with no user action |
| No crash | `logcat -b crash` clean after the fix |

`./gradlew :app:compileDebugKotlin :app:testDebugUnitTest assembleDebug` — BUILD SUCCESSFUL, `ThemeTest` green.

**The Nyquist gap the plan recorded still stands:** the Android module has one JVM unit test and no Compose UI-test harness, so nothing *automated* pins the layout, the expansion behaviour or the loading frames. The device screenshots above are evidence, not regression protection. The crash that compilation and the unit tests both missed is the concrete demonstration of that gap.

## Field Findings

**F-05 (new, observed): the tiebreaker is not racing — it is thrashing against address rotation.** Reading the attempt log on device gave 48 records: against one peer node identity, 6 `central`/`connected` and 28 `peripheral`/`lost-tiebreaker` — the convention applied *correctly and consistently* on both paths, which is the opposite of a race. But those 28 losses come from 28 **distinct BLE addresses** belonging to that one node: the peer rotates resolvable private addresses, and every rotation dials in, loses the tiebreaker and is dropped.

That is a plausible cause of the "not always connecting with peers" complaint, by a mechanism nobody predicted. Full detail and caveats in [01-FIELD-FINDINGS.md](./01-FIELD-FINDINGS.md).

**This is the phase paying off.** The roadmap's first sequencing rule was that Phase 2 must not act on inference. It was about to act on a tiebreaker race that the first real measurement does not show.

## Next Phase Readiness

- DIAG-01, 03, 04, 05, 06 and 07 are all satisfiable end-to-end on screen. DIAG-02 remains Phase 2 by design.
- Phase 01's four plans are complete. The remaining gate is the phase tail: post-merge build+test and the `gsd-verifier` goal-backward check.
- `DEVICE-TEST-BATCH.md` D-2 (force-stop persistence) is now **done** — 48 attempts before, 48 after, no `.corrupt` sibling. D-1 still wants a second phone to compare two logs; F-05 is one device's answer, not two.

---
*Phase: 01-make-peering-observable*
*Completed: 2026-08-06*

## Self-Check: PASSED

- FOUND: `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt`
- FOUND: `android/app/src/main/java/app/myco/core/AppCoreClient.kt`
- FOUND commit `78c5998`
- FOUND commit `5b83243`
- FOUND commit `a55ef61`
- FOUND commit `41ab89b`
