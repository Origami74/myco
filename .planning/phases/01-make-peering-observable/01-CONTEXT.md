# Phase 1: Make Peering Observable - Context

**Gathered:** 2026-08-04
**Status:** Ready for planning

<domain>
## Phase Boundary

This phase turns peering from **inferred** to **observed**. It delivers two things:

1. **Instrumentation on the peering path** — per-peer BLE role decision, discovery
   latency, connect-attempt outcome and send-failure/drop counters, recorded in
   `myco-core` and exposed across the JNI/JSON state surface.
2. **A peer diagnostics screen** — connection state, last-seen, active transport,
   per-radio enabled/scanning status, pending pair requests and own identity,
   readable on the device with no debugger attached.

It does **not** fix peering. The BLE role-tiebreaker race, the advertise/scan
duty-cycle asymmetry and the fire-and-forget fan-out are all *inferred from code,
not confirmed by device logs* — Phase 2 fixes them, using what this phase measures.
Building a fix here would violate the roadmap's first sequencing constraint.

**Requirements in scope:** DIAG-01, DIAG-03, DIAG-04, DIAG-05, DIAG-06, DIAG-07.
**Explicitly not in scope:** DIAG-02 (plain-language reason codes) — it lands in
Phase 2, deliberately last, because a reason code layered on flaky reconnect logic
is noise rather than diagnosis.

**Deadline:** must be usable at the 2026-08-04 demo (today). Release 2026-08-05.

</domain>

<decisions>
## Implementation Decisions

### Screen placement and audience

- **D-01:** The peer diagnostics live in the **Dev tab**, not in Circle and not in a
  new top-level destination.

- **D-02:** `developerMode` counts as **in-app**. The DIAG requirements' "User can
  see…" is read as *the person holding the phone, without attaching a debugger* —
  `developerMode` is an ordinary Settings switch ([SettingsScreen.kt:264](android/app/src/main/java/app/myco/ui/screens/SettingsScreen.kt#L264)),
  so DIAG-01/03/04/05/07 are satisfied by the Dev tab. This reading is **locked** —
  the planner and verifier both honour it; do not re-open it at the coverage gate.

- **D-03:** `DevScreen` is **rebuilt around peering** for this milestone rather than
  gaining a section or a sub-screen. Much of what it already carries is
  peering-relevant (`PeersOverviewCard`, the BLE / WI-FI AWARE / RADIO ADVERTS
  cards) and the `DevCard` / `KeyValDot` / `PeerRow` / `AdvertRow` / `SectionCard`
  scaffolding is reused, not rewritten. — **Reversibility:** costly — the Dev tab is
  the only home for this UI, so a later move to Circle means re-siting every
  composable and re-deciding the audience question in D-02.

- **D-04:** The **SPEEDTEST card is kept** (it proves a link actually carries data,
  which is peering-adjacent) and demoted below the peering content. The **CACHE card
  is dropped** — it is content-layer state with no bearing on Phase 1, and the screen
  is about to get much denser.

- **D-05:** Peer rows **expand in place**. The collapsed row is a summary line;
  tapping it reveals role, discovery latency, drop counters, RSSI and the attempt
  log. No per-peer detail screen — at a demo you are comparing two phones side by
  side and must not lose your place in the list.

- **D-06:** `developerMode` defaults **on in debug builds only**. The release APK's
  behaviour is unchanged. Note the tension this creates with D-02: for a user on a
  zapstore release build the diagnostics remain behind a Settings toggle they must
  find first.

- **D-07:** With zero peers the screen leads with a **radio self-check** — BLE
  enabled/scanning, Wi-Fi Aware enabled/scanning, adverts seen recently — then an
  empty peer list. Zero peers is the actual field complaint, so that state must
  answer "is it me or is it them" without a second screen. DIAG-05 already requires
  those facts.

### Peer row model

- **D-08:** One row per **npub**, with transports as attributes of the row — the
  currently-active transport plus which others can reach that peer. A peer is an
  identity, not a link. This is the same pubkey-keyed model Phase 2 needs for
  MAC-rotation survival (PEER-03, FIPS#130), so Phase 2 inherits it instead of
  rebuilding it. — **Reversibility:** one-way — the npub-keyed peers array becomes
  the published FFI state contract that the Kotlin UI and Phase 2's churn-recovery
  work both read; reverting to address-keyed rows means changing the FFI shape after
  two consumers depend on it.

- **D-09:** Devices **seen but not yet resolved to an npub** (raw BLE adverts,
  unpaired phones) **do get rows**, keyed by node address, marked not-yet-identified,
  collapsing into the npub row once the handshake resolves them. "Seen but never
  resolved" is precisely the observation Phase 2 needs, and DIAG-01 says *every known
  peer*.

- **D-10:** **Five states**, reusing colours that already exist:
  `connected` / `reachable-via-relay` / `seen-unidentified` / `paired-offline` /
  `unreachable`. These map onto `StatusConnected`, `StatusReachable` and the amber
  pending tone already defined in [Theme.kt](android/app/src/main/java/app/myco/ui/theme/Theme.kt) —
  **no new colours are invented**, so `ThemeTest.kt` stays green. This is a strict
  superset of DIAG-01's three states, so the requirement is still met literally.

- **D-11:** Rows are ordered **by state, then most recently heard from** — connected,
  reachable, unidentified, paired-offline, unreachable; each group sorted by
  last-seen. A peer visibly climbs the list as it comes good, which is what you watch
  during convergence.

### Failure forensics

- **D-12:** A **capped rolling per-peer attempt log** — roughly the last 10–20
  connect attempts per peer, each carrying a timestamp, the chosen BLE role, the
  discovery duration and the outcome. Current-values-only was rejected: a race that
  resolved thirty seconds ago would leave no trace, and races are exactly what is
  being hunted. This log is the artefact that confirms or kills the tiebreaker-race
  hypothesis before Phase 2 acts on it.

- **D-13:** The attempt log is **persisted to disk** so a crash or force-stop during
  a field test still yields evidence. — **Reversibility:** costly — undoing this means
  removing an on-disk format that field builds have already written, plus its
  migration/cleanup path. **Risk accepted and recorded:** CORE-03 exists precisely
  because a corrupt `circle.json` / `library.json` currently loads silently as empty
  in this codebase. The plan MUST give this log a bounded, corruption-tolerant read
  path — a malformed or truncated log file must surface as "no history" without
  taking down startup, and must never silently destroy a good file.

- **D-14:** Retention is **capped per peer and pruned on write** — the same N-per-peer
  ring flushed to disk, old entries falling off as new ones land, and peers unseen for
  a long while dropped entirely. File size is bounded by peer count, which is small.
  No background job, no rotation logic.

- **D-15:** The log is **read on screen only** — no share sheet, no clipboard export.
  The expanded peer row showing attempt history is what "without attaching a debugger"
  asks for; nothing more ships this phase.

### Liveness and polling cost

- **D-16:** Peer diagnostics polls on its **own cadence of roughly 1 second**, separate
  from the existing UI tick. `.planning/codebase/CONCERNS.md` flags `state()` locking
  10+ mutexes at UI framerate, and this screen becomes the heaviest state consumer in
  the app. A separate, slower cadence is fast enough to watch convergence while
  keeping lock pressure an order of magnitude lower — and it does not pre-empt UX-02,
  which is Phase 3's job.

- **D-17:** That polling runs **only while the Dev screen is visible** — started on
  show, stopped on leave or background, via a Compose lifecycle effect. The
  instrumentation inside the core keeps recording regardless, so the attempt log is
  still complete when you come back.

- **D-18:** Last-seen is displayed as **exact seconds, counting up** (`3s`, `47s`,
  `4m 12s`). At a 1s cadence you can watch it reset the instant a peer is heard from,
  which is how a live link is told from a stale one. Coarse buckets would hide the
  2s-versus-25s difference that a duty-cycle asymmetry shows up at.

- **D-19:** The new fields cross the FFI as a **new npub-keyed `peers` array on
  `AppState`**, alongside the existing `bleAdverts` / `circle` / `reachableNpubs`
  fields, carrying the merged view the UI renders. The merge happens **once in Rust**
  rather than being re-derived in Kotlin on every poll. Existing state fields are left
  untouched. — **Reversibility:** one-way — this array is the FFI contract the Kotlin
  UI and Phase 2 both consume; changing its shape later breaks both consumers at once.

### Claude's Discretion

The following were not discussed and the planner should choose sensibly, consistent
with the decisions above:

- The exact field set and Rust type of an attempt record, and what constitutes one
  "attempt" boundary.
- Whether the attempt log rides the same `state()` payload as the peer rows or is
  fetched when a row expands.
- The on-disk encoding of the persisted log (subject to D-13's corruption-tolerance
  requirement).
- The exact value of N in the per-peer cap, and the unseen-peer eviction threshold.
- First-frame behaviour before the first poll lands, and any row transition animation.
- The collapsed summary line's exact composition, and how npub and Circle name are
  rendered together.
- Where the Wi-Fi Aware "actively scanning" signal is sourced from — `WifiAwareStatus`
  currently exposes only `enabled` and `port`, so DIAG-05 needs a new field.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase intent and sequencing
- `.planning/ROADMAP.md` §"Sequencing Constraints" — instrumentation before fixes;
  reason codes last; why Phase 1 exists at all
- `.planning/ROADMAP.md` §"Phase 1: Make Peering Observable" — goal, 5 success
  criteria, the 2-plan split
- `.planning/REQUIREMENTS.md` §"Peer diagnostics" — DIAG-01 … DIAG-07 verbatim
- `.planning/research/PITFALLS.md` — the source of the sequencing constraints

### Codebase ground truth
- `.planning/codebase/ARCHITECTURE.md` — JNI/JSON reducer boundary, spawn-not-block
  rule, `ControlReadHandle` lock-free peer reads, the "Blocking the FFI Thread"
  anti-pattern
- `.planning/codebase/CONVENTIONS.md` §"Android UI (Compose)" — **theme rules that
  bind this phase**: read `MaterialTheme.colorScheme` semantic roles, never hardcode
  colours; `StatusConnected` / `StatusReachable` / `StatusThin` / `StatusAlone` already
  exist and are theme-independent; no Material You (API 29+ target); `ThemeTest.kt`
  enforces this in CI
- `.planning/codebase/CONVENTIONS.md` §"Build environment" — `reference/fips` must stay
  on `feat/platform-peer-queue`; `master` does not export `fips::discovery::platform`
  or a public `ControlReadHandle`
- `.planning/codebase/CONCERNS.md` — `state()` polling locking 10+ mutexes at UI
  framerate; corrupt `circle.json` / `library.json` loading silently as empty

### Files this phase touches
- `myco-core/src/state.rs` — `AppState`, `BlePeer`, `BleStatus`, `WifiAwareStatus`,
  `IdentityView`; where the new `peers` array lands
- `myco-core/src/content.rs` — `CircleContact`, `PairRequestView`, `OutboundPairView`
  (note: `CircleContact.name` is commented "a placeholder for now" — relevant to
  DIAG-07)
- `myco-core/src/runtime.rs` — `dispatch`, `run_rx_loop`, `keepwarm_tick`
- `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` — the screen being
  rebuilt; source of the reusable `DevCard` / `KeyValDot` / `PeerRow` / `AdvertRow`
  composables
- `android/app/src/main/java/app/myco/ui/MycoApp.kt` — tab registry and the
  `developerMode` gate (line 208)
- `android/app/src/main/java/app/myco/ui/screens/SettingsScreen.kt` — the
  `developerMode` switch (line 264)
- `android/app/src/main/java/app/myco/ui/theme/Theme.kt` — the peer-state colours

### Design docs
- `docs/design/ble-interop.md` — BLE role and PSM discovery behaviour, the subject of
  the role-decision instrumentation
- `docs/design/wifi-aware-interop.md` — the Wi-Fi Aware bulk lane (`WIFI_AWARE_PORT`
  4871) and why it is symmetric with no listener/dialer roles
- `docs/design/identity-pairing.md` — pairing handshake and Circle semantics,
  relevant to DIAG-06 and DIAG-07
- `reference/FIX-TODOS.md` — field-reported TODOs (gitignored path dependency)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- **`DevCard`, `KeyValDot`, `PeerRow`, `AwareLinkRow`, `ApNodeRow`, `AdvertRow`,
  `SectionCard`, `ScreenHeader`** ([DevScreen.kt](android/app/src/main/java/app/myco/ui/screens/DevScreen.kt)):
  the whole card-and-row vocabulary already exists and matches the visual language.
  The rebuild is a re-composition, not a from-scratch screen.
- **`PeersOverviewCard`, and the BLE / WI-FI AWARE / RADIO ADVERTS cards**: already
  render much of DIAG-04 and DIAG-05's raw material; they get absorbed rather than
  replaced.
- **`approxMeters(rssi)`** (RSSI → distance, `TX_POWER_AT_1M` / `PATH_LOSS_N`):
  existing helper for making adverts legible.
- **`StatusConnected` / `StatusReachable` / `StatusThin` / `StatusAlone`**
  ([Theme.kt](android/app/src/main/java/app/myco/ui/theme/Theme.kt)): the five-state
  vocabulary in D-10 is built on these; no new colour constants.
- **`SpeedtestView` + `SpeedtestCard`**: kept and demoted per D-04.

### Established Patterns
- **Redux-style reducer over JNI**: Kotlin calls `dispatch(actionJson)` and polls
  `state()`; all async work is spawned on Tokio and never awaited on the FFI thread.
  Adding the `peers` array must not turn `dispatch` into a blocking call.
- **Lock-free peer reads** via FIPS's `ControlReadHandle` — the peer view already
  avoids the mutex path; the merge in D-19 should stay on that side of the line
  wherever it can.
- **Rust doc conventions**: every public type and enum variant carries `///` docs;
  `NativeAppAction`'s 20+ variants are fully documented. New public state types are
  expected to match.
- **`cargo fmt --all --check`** is CI-enforced; new code must not add clippy warnings.

### Integration Points
- **`AppState`** — the new npub-keyed `peers` array (D-19) is the single FFI seam
  between the Rust instrumentation plan (01-01) and the UI plan (01-02).
- **`MycoApp.kt` tab registry** — Dev tab already exists and is already gated; only
  its default in debug builds changes (D-06).
- **`run_rx_loop` / `keepwarm_tick`** in `runtime.rs` — where peer liveness is
  actually observed, and therefore where `last_seen` and transport attribution have
  to be captured.
- **FIPS `ControlReadHandle`** — source of peer identity and reachability; the BLE
  role decision and discovery latency have to be captured at the point the transports
  make them, which may sit inside `reference/fips`. Any fips-side change must stay
  minimal, generic and upstreamable (project constraint), and `reference/fips` must
  remain on `feat/platform-peer-queue`.

</code_context>

<specifics>
## Specific Ideas

- The screen's job at zero peers is to answer **"is it me or is it them"** without a
  second screen — that framing drove D-07.
- **Watching a peer climb the list** as it transitions from unidentified to reachable
  to connected is the intended demo moment; it is why ordering is state-then-recency
  (D-11) and why last-seen counts up in exact seconds (D-18).
- The attempt log exists to **settle the tiebreaker-race question with evidence**, not
  to be a general logging facility. Its shape should follow that purpose.
- Comparing two phones side by side is the expected demo posture — hence expand-in-place
  over a detail screen (D-05).

</specifics>

<deferred>
## Deferred Ideas

- **Promoting the user-facing subset into the Circle tab** — connection state,
  last-seen, transport and pending pairings on Circle rows, for users who never enable
  `developerMode`. Considered and rejected for this phase (D-01, D-02); revisit if
  field reports show release users cannot reach the diagnostics.
- **Exporting an attempt log off the device** — share sheet or clipboard (D-15).
  Rejected as extra surface today; the natural home is alongside Phase 2's reason
  codes, when there is something conclusive to share.
- **Push-based state updates from the core** instead of polling — the correct
  long-term fix and the natural answer to UX-02, but it needs a new FFI direction
  (the JNI boundary is poll-only today). Belongs with Phase 3's UX-02 work.
- **Interactive force-directed mesh topology graph** — already recorded as out of
  scope in `REQUIREMENTS.md`; the reason-code list answers the real field complaint.
- **Fixing `CircleContact.name`** beyond what DIAG-07 needs — the field is commented
  as "a placeholder for now". This phase surfaces the Circle name; making naming
  robust is not in scope.

</deferred>

---

*Phase: 1-Make Peering Observable*
*Context gathered: 2026-08-04*
