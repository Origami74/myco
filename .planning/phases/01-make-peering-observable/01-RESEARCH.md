# Phase 1: Make Peering Observable - Research

**Researched:** 2026-08-04
**Domain:** Instrumenting a Rust P2P mesh core (fips BLE L2CAP + Wi-Fi Aware/UDP) and surfacing
it through a JNI/JSON reducer into a Jetpack Compose diagnostics screen
**Confidence:** HIGH — every claim below is grounded in the actual source files at their current
commit on `feat/platform-peer-queue` (fips) and `main` (fips-pop), not general BLE/Android
knowledge. This phase adds no external dependencies, so there is no package-legitimacy or
registry-currency risk to assess.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** The peer diagnostics live in the **Dev tab**, not in Circle and not in a
  new top-level destination.
- **D-02:** `developerMode` counts as **in-app**. The DIAG requirements' "User can
  see…" is read as *the person holding the phone, without attaching a debugger* —
  `developerMode` is an ordinary Settings switch, so DIAG-01/03/04/05/07 are
  satisfied by the Dev tab. This reading is **locked** — the planner and verifier
  both honour it; do not re-open it at the coverage gate.
- **D-03:** `DevScreen` is **rebuilt around peering** for this milestone rather than
  gaining a section or a sub-screen. The `DevCard` / `KeyValDot` / `PeerRow` /
  `AdvertRow` / `SectionCard` scaffolding is reused, not rewritten. Reversibility:
  costly.
- **D-04:** The **SPEEDTEST card is kept**, demoted below the peering content. The
  **CACHE card is dropped**.
- **D-05:** Peer rows **expand in place**. No per-peer detail screen.
- **D-06:** `developerMode` defaults **on in debug builds only**. Release APK
  behaviour is unchanged.
- **D-07:** With zero peers the screen leads with a **radio self-check** — BLE
  enabled/scanning, Wi-Fi Aware enabled/scanning, adverts seen recently — then an
  empty peer list.
- **D-08:** One row per **npub**, with transports as attributes of the row. A peer
  is an identity, not a link. Reversibility: one-way.
- **D-09:** Devices **seen but not yet resolved to an npub** do get rows, keyed by
  node address, collapsing into the npub row once the handshake resolves them.
- **D-10:** **Five states**, reusing existing colours: `connected` /
  `reachable-via-relay` / `seen-unidentified` / `paired-offline` / `unreachable`. No
  new colours are invented.
- **D-11:** Rows are ordered **by state, then most recently heard from**.
- **D-12:** A **capped rolling per-peer attempt log** — roughly the last 10-20
  connect attempts per peer, each carrying a timestamp, chosen BLE role, discovery
  duration, and outcome.
- **D-13:** The attempt log is **persisted to disk**. Reversibility: costly. The
  plan MUST give this log a bounded, corruption-tolerant read path — a malformed or
  truncated log file must surface as "no history" without taking down startup, and
  must never silently destroy a good file.
- **D-14:** Retention is **capped per peer and pruned on write** — no background
  job, no rotation logic.
- **D-15:** The log is **read on screen only** — no share sheet, no clipboard
  export.
- **D-16:** Peer diagnostics polls on its **own cadence of roughly 1 second**,
  separate from the existing UI tick.
- **D-17:** That polling runs **only while the Dev screen is visible** — started on
  show, stopped on leave or background, via a Compose lifecycle effect.
- **D-18:** Last-seen is displayed as **exact seconds, counting up** (`3s`, `47s`,
  `4m 12s`).
- **D-19:** The new fields cross the FFI as a **new npub-keyed `peers` array on
  `AppState`**, alongside the existing `bleAdverts` / `circle` / `reachableNpubs`
  fields. The merge happens **once in Rust** rather than being re-derived in
  Kotlin. Existing state fields are left untouched. Reversibility: one-way.

### Claude's Discretion

- The exact field set and Rust type of an attempt record, and what constitutes one
  "attempt" boundary.
- Whether the attempt log rides the same `state()` payload as the peer rows or is
  fetched when a row expands.
- The on-disk encoding of the persisted log (subject to D-13's corruption-tolerance
  requirement).
- The exact value of N in the per-peer cap, and the unseen-peer eviction threshold.
- First-frame behaviour before the first poll lands, and any row transition
  animation.
- The collapsed summary line's exact composition, and how npub and Circle name are
  rendered together.
- Where the Wi-Fi Aware "actively scanning" signal is sourced from —
  `WifiAwareStatus` currently exposes only `enabled` and `port`, so DIAG-05 needs a
  new field.

### Deferred Ideas (OUT OF SCOPE)

- **Promoting the user-facing subset into the Circle tab** — connection state,
  last-seen, transport and pending pairings on Circle rows, for users who never
  enable `developerMode`. Rejected for this phase (D-01, D-02); revisit if field
  reports show release users cannot reach the diagnostics.
- **Exporting an attempt log off the device** — share sheet or clipboard (D-15).
  Rejected as extra surface today.
- **Push-based state updates from the core** instead of polling — needs a new FFI
  direction. Belongs with Phase 3's UX-02 work.
- **Interactive force-directed mesh topology graph** — already out of scope in
  `REQUIREMENTS.md`.
- **Fixing `CircleContact.name`** beyond what DIAG-07 needs — the field is
  commented as "a placeholder for now." This phase surfaces the Circle name; making
  naming robust is not in scope.

</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DIAG-01 | User can see every known peer with its current connection state — connected, reachable via relay, or offline | D-08/D-09/D-10 merge design (Architecture Patterns, Pattern 2; Common Pitfall #4) specifies exactly how `ble_peers` + `ble_adverts` + `circle`/pairing arrays union into one npub-or-address-keyed `peers` list covering all five states, a strict superset of the three DIAG-01 requires |
| DIAG-03 | User can see how long ago each peer was last heard from | `PeerView`/`peer_meta` already carries `last_seen_ms` at the fips mesh layer (`control/queries.rs` `show_peers`); Architecture Patterns Pattern 2 shows where to fold a `last_seen` timestamp into the new `PeerDiagnosticView`; D-18 (exact-seconds counting) is a pure Kotlin render concern once the timestamp crosses the FFI |
| DIAG-04 | User can see which transport is currently carrying each connected peer | `DevScreen.kt`'s existing `PeersOverviewCard` already attributes lanes (aware/udp/ble) per npub from three separate radio-state sources (Existing Code Insights / Code Examples); this phase's job is moving that attribution into the Rust-side merge (D-19) instead of Kotlin re-deriving it per poll |
| DIAG-05 | User can see whether each radio is enabled and actively scanning, for both BLE and Wi-Fi Aware | Summary and Common Pitfall #3 identify that `AppState.ble.scanning` is currently computed, not observed, and `WifiAwareStatus` has no scanning field at all; Architecture Pattern 3 and the BLE/Aware code examples specify the exact Kotlin push-bridge fix for both radios |
| DIAG-06 | User can see pending pair requests and whether each is waiting, complete, or failed | `content.rs`'s existing `PairRequestView` (incoming) and `OutboundPairView` (outbound/waiting) already carry the needed fields (`content.rs:162-182`, read in full); this phase surfaces them via the existing `pending_pair_requests`/`outbound_pairs` `AppState` fields, left untouched per D-19, joined into the new `peers` view per-npub |
| DIAG-07 | User can see their own identity and the Circle name other peers see them as | `IdentityView` (`state.rs:83-94`) already carries `own_npub`; `CircleContact.name` (`content.rs:85-91`) is the Circle-visible label — both already exist and only need rendering, per the Deferred Ideas note that `CircleContact.name`'s placeholder status is explicitly out of scope to fix |

</phase_requirements>

## Summary

This phase is 100% a codebase-integration problem, not a library-research problem: nothing new
gets installed, and every data source the phase needs already exists in some form in the
repository — just not connected to each other. The work is entirely about **where to hook
instrumentation** in `fips`'s generic BLE transport (kept upstream-extractable, no Myco types),
**how to carry it** across the `myco-core` JNI/JSON reducer boundary without blocking the FFI
thread, and **how to render it** by extending the Dev tab's already-established
card/row/glyph+color vocabulary — all constraints the UI-SPEC has already locked.

The single hardest fact this research turns up: **the BLE role tiebreaker is already
deterministic** (`fips/src/transport/ble/mod.rs`, cross-probe tiebreaker: smaller `NodeAddr`'s
outbound always wins) and is unit-tested for the *convention* (`test_tiebreaker_convention`).
Pitfall 1 in `PITFALLS.md` — "no deterministic tiebreaker" — is not quite what the code shows;
what's actually missing is *visibility into whether both sides agree at runtime*, since the
decision is logged only via `debug!()` and never recorded per-peer. This phase's job is to turn
that already-correct-looking logic into an observed fact instead of a re-read-the-source
inference — which is exactly what Phase 2 needs before touching it.

The second hardest fact: `AppState.ble.scanning` (today, `runtime.rs:794`) is **computed**, not
observed — `self.ble_enabled && self.node_running` — and `WifiAwareStatus` has no scanning field
at all. Both radios' actual on/off state lives in Kotlin (`BleRadio.kt`'s `scanCallback != null`,
`AwareRadio.kt`'s `publishSession`/`subscribeSession` non-null) and is not currently pushed back
across the JNI bridge. DIAG-05's "actually scanning right now" requires closing that gap on both
radios — it does not yet exist anywhere in the stack.

**Primary recommendation:** Add per-peer instrumentation *inside* `fips/src/transport/ble/mod.rs`
(role decision, discovery timestamp, connect outcome) as a small, generic, serializable struct
attached to the connection pool / discovery buffer — not in `myco-core` — so the change stays a
clean, extractable upstream diff. Bridge two new booleans (BLE scanning, Aware
publishing-or-subscribing) from Kotlin the same way `advert_views()` already bridges scan
adverts. Merge everything into one new npub-keyed `peers: Vec<PeerDiagnosticView>` field on
`AppState`, computed once in `myco-core::runtime::state()` from `ControlReadHandle::peer_views()`
+ the new fips-side per-peer log + `content.rs`'s existing `CircleContact`/`PairRequestView`/
`OutboundPairView`. Persist only the attempt log (D-13/D-14) using the existing atomic
write-tmp-then-rename pattern from `content.rs`, but with the read path made corruption-tolerant
per-entry (not whole-file, like `load_circle` today) since D-13 explicitly forbids repeating that
known defect.

## Architectural Responsibility Map

This project has no browser/CDN tiers; its layers are the five in `ARCHITECTURE.md`. Mapped to
those:

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| BLE role/tiebreaker decision capture | Layer 0 (fips BLE transport, `ble/mod.rs`) | Layer 3 (`myco-core` reads it out) | The decision is made in fips's generic transport; must stay upstream-extractable (FIPS-04), so the *decision* logic and its instrumentation live together in fips, and `myco-core` only reads a snapshot |
| Discovery latency measurement | Layer 0 (fips `discovery.rs` / `scan_probe_loop`) | Layer 3 | `DiscoveryBuffer` already lives in fips; it has no timestamps today — add them there, not in Kotlin or myco-core |
| Connect-attempt outcome / send-drop counters | Layer 0 (fips `BleStats` today is global-only; needs a per-peer variant) | Layer 3 (aggregated into the `peers` array) | Existing `BleStats` (`stats.rs`) is aggregate atomics; per-peer requires a new small structure in fips, not a myco-core workaround |
| Per-peer merged diagnostic view (state, last-seen, transport, role, drops) | Layer 3 (`myco-core::state::AppState`, new `peers` field) | Layer 0 (source data), Layer 2 (`content.rs` Circle/pairing data) | D-19 locks this: merge happens once in Rust, not on every UI poll |
| Attempt-log persistence | Layer 3 (`myco-core`, new file in the app data dir beside `circle.json`/`library.json`) | — | Not fips's concern; fips holds only the live in-memory ring, myco-core owns disk lifecycle |
| BLE "actively scanning" signal | Layer 4 (Android `BleRadio.kt`) bridged via Layer 3 (`android_io.rs`'s `AndroidBleBridge`) | Layer 3 | Radio truth lives in Kotlin; needs a new bridge call-back, mirroring how `deliver_scan` already pushes adverts |
| Wi-Fi Aware "actively scanning" signal | Layer 4 (Android `AwareRadio.kt`) bridged via Layer 3 (`aware_bridge_jni.rs`) | Layer 3 | Same shape as BLE; Aware's "scanning" maps to publish+subscribe session liveness, not a literal scan call |
| Peer diagnostics screen (five-state rows, radio self-check, attempt log, pending pairs, identity) | Layer 4 (`DevScreen.kt`, Compose) | Layer 3 (`AppState`) | Pure render layer; UI-SPEC already locks its composables and polling cadence |
| Own-cadence polling (D-16/D-17) | Layer 4 (Compose `LaunchedEffect`/lifecycle) | — | UI-only; does not touch `dispatch`/`state()`'s existing poll-driven design |

## Standard Stack

### Core

No new libraries. Every crate this phase needs is already a pinned workspace dependency.

| Library | Version (workspace-pinned) | Purpose | Why no alternative needed |
|---------|---------|---------|--------------|
| `serde` / `serde_json` | 1.x (fips-pop `Cargo.toml:25-26`; fips `Cargo.toml:24-25`) [VERIFIED: workspace Cargo.toml] | Serialize the new `peers` array and the persisted attempt log | Already the FFI serialization boundary for all of `AppState` |
| `portable-atomic` | 1, `features = ["std"]` (fips `Cargo.toml:38`) [VERIFIED: workspace Cargo.toml] | Per-peer atomic counters (drops, sends) if a lock-free counter table is chosen | Already used by `BleStats` (`stats.rs`) for exactly this pattern |
| `arc-swap` | 1 (fips `Cargo.toml:42`) [VERIFIED: workspace Cargo.toml] | Optional: publish a per-peer snapshot the same way `ControlReadHandle` publishes `StatsSnapshot`/`EntitySnapshot`/`RoutingSnapshot` | Matches the existing tick-published-snapshot pattern exactly (`fips/src/control/read_handle.rs`) |
| `tokio` (Mutex, spawn) | 1.x (fips-pop `Cargo.toml:33`; fips `Cargo.toml:32`) [VERIFIED: workspace Cargo.toml] | Any new per-peer log needs a `tokio::sync::Mutex` or std `Mutex` guarded briefly, matching `ConnectionPool`'s existing lock shape | Already how `ble/mod.rs`'s `pool`/`connecting` maps are guarded |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `std::collections::VecDeque` | stdlib | Bounded per-peer ring for the attempt log (D-12/D-14: last 10-20 entries, capped and pruned on write) | This is a ~10-line hand-rolled cap — do not add a ring-buffer crate for it (see Don't Hand-Roll) |
| `std::fs::rename` atomic-write pattern | stdlib | Persist the attempt log the same way `content.rs::save_circle`/`save_library` already do (`content.rs:2167-2183`) | Reuse verbatim for the *write* path; the *read* path must diverge (see Common Pitfalls) |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-rolled bounded `VecDeque` ring per peer | A crate like `ringbuf` or `circular-queue` | Rejected: D-12's "10-20 entries, prune on write, no rotation logic" is a five-minute stdlib job; a crate adds a dependency-legitimacy review and a Cargo.lock diff for no capability gain |
| A second `ArcSwap`-published snapshot type in fips (`PeerAttemptSnapshot`, matching `EntitySnapshot`'s pattern) | Piggybacking role/latency/drop fields directly onto the existing `PeerView` in `read_handle.rs` | `PeerView` is deliberately minimal (`node_addr_hex`/`npub`/`connected`) and is the one struct that's already a stable, documented public API (`peer_views()` doc comment references "the Myco app for the reference embedding"). Extending it in place is lower-risk than adding a parallel type, provided the new fields are additive (non-breaking) |

**Installation:** None. This phase adds zero new crates to `Cargo.toml` in either `fips` or
`fips-pop`.

**Version verification:** Verified directly against the checked-out `Cargo.toml` files rather than
a registry lookup, since these are already-vendored workspace dependencies, not new adds.

## Package Legitimacy Audit

**Not applicable — this phase installs no external packages.** Every crate and Kotlin dependency
used is already present in the workspace (`serde`, `serde_json`, `tokio`, `arc-swap`,
`portable-atomic` on the Rust side; no new Gradle dependencies on the Android side — Jetpack
Compose Material3 and the existing `androidx.compose.*` artifacts already in `build.gradle` cover
every composable the UI-SPEC calls for).

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### System Architecture Diagram

```text
                         ┌────────────────────────────────────────────┐
                         │  fips/src/transport/ble/mod.rs (Layer 0)    │
                         │                                              │
  BLE radio (Kotlin) ───▶  scan_probe_loop()  accept_loop()             │
  advertise/scan events   │  ├─ pubkey_exchange()                       │
  (android_io.rs bridge)  │  ├─ cross-probe tiebreaker (role decision)  │──┐ NEW: record
                         │  ├─ io.connect() timeout/err (outcome)      │  │  PeerAttempt{
                         │  └─ pool.insert() / discovery_buffer        │  │    role, t_discovered,
                         └────────────────────────────────────────────┘  │    t_outcome, outcome,
                                          │                                │    drops }
                                          │ DiscoveredPeer / BleStats      │  per node_addr,
                                          ▼                                │  capped ring
                         ┌────────────────────────────────────────────┐  │
                         │  fips/src/control/read_handle.rs            │◀─┘
                         │  ControlReadHandle::peer_views() (existing) │
                         │  + NEW: attempt_log_for(node_addr)           │
                         └────────────────────────────────────────────┘
                                          │ lock-free ArcSwap read
                                          ▼
                         ┌────────────────────────────────────────────┐
                         │  myco-core/src/runtime.rs::state()  (L3)     │
                         │  merges:                                     │
                         │   - read_handle.peer_views() + attempt logs  │
                         │   - content.circle_snapshot() (npub, name)   │
                         │   - content.pending_pairs_snapshot()         │
                         │   - content.outbound_pairs_snapshot()        │
                         │   - NEW: ble "scanning" bool from bridge     │
                         │   - NEW: wifi_aware "scanning" bool          │
                         │  → AppState.peers: Vec<PeerDiagnosticView>   │
                         └────────────────────────────────────────────┘
                                          │ dispatch()/state() JSON over JNI
                                          ▼
                         ┌────────────────────────────────────────────┐
                         │  DevScreen.kt (Layer 4, Compose)             │
                         │  own 1s poll (D-16/D-17, LaunchedEffect)     │
                         │  Radio self-check → Peer list (expand-in-    │
                         │  place, D-05) → Pending pairs → Identity     │
                         │  → Speedtest (demoted, D-04)                 │
                         └────────────────────────────────────────────┘
```

### Recommended Project Structure

No new files at the module level are required by convention; this is an extension of existing
files, consistent with `content.rs` being one file per concern-cluster (its own split is CORE-01,
explicitly out of scope here per `.planning/codebase/CONCERNS.md`).

```
fips/src/transport/ble/
├── mod.rs         # + record per-peer role/outcome at the two tiebreaker sites (existing file)
├── stats.rs       # + a small PerPeerBleStats or attempt-log companion type (existing file)
├── discovery.rs   # + a discovered-at Instant/timestamp on DiscoveredPeer (existing file)
fips/src/control/
├── read_handle.rs # + expose the new per-peer data via a second accessor (existing file)
myco-core/src/
├── state.rs       # + PeerDiagnosticView, PeerAttempt (or similar), AppState.peers (existing file)
├── runtime.rs      # + the merge in state() (existing file)
├── action.rs       # unlikely to need new actions — this phase is read-only observation
├── content.rs      # no functional change needed; CircleContact/PairRequestView/OutboundPairView already carry DIAG-06/07's data
android/app/src/main/java/app/myco/
├── ble/BleRadio.kt      # + push "is scanning" state across the bridge (existing file)
├── aware/AwareRadio.kt  # + push "is publishing/subscribing" state (existing file)
├── ui/screens/DevScreen.kt  # rebuilt around peering per D-03 (existing file)
```

### Pattern 1: Tick-published lock-free snapshot (already established in fips)

**What:** `ControlReadHandle` wraps `Arc<ArcSwap<T>>` cells (`stats`, `routing`, `entities`),
published once per tick from the node's own mutation sites, read lock-free by any clone of the
handle. `AppRuntime` already holds one clone (`runtime.rs:50`) and reads `peer_views()` from it
every `state()` call with zero contention on the node's hot path.

**When to use:** Any new per-peer BLE data this phase adds should follow this exact shape — do
not introduce a second polling/locking mechanism. If the new per-peer attempt data is small
enough to piggyback onto the existing `entities`/`stats` `ArcSwap` cells, prefer that; if it needs
its own publish cadence (e.g., published on every connect-attempt event rather than once per
node tick), a new `Arc<ArcSwap<PeerAttemptSnapshot>>` cell on `ControlReadHandle` is the
established pattern to copy.

**Example:**
```rust
// Source: fips/src/control/read_handle.rs (existing pattern)
pub struct ControlReadHandle {
    context: Arc<NodeContext>,
    metrics: Arc<MetricsRegistry>,
    stats: Arc<ArcSwap<StatsSnapshot>>,
    routing: Arc<ArcSwap<RoutingSnapshot>>,
    entities: Arc<ArcSwap<EntitySnapshot>>,
    // NEW, following the same shape:
    // ble_attempts: Arc<ArcSwap<BleAttemptSnapshot>>,
}
```

### Pattern 2: Spawn-not-block reducer with a single merge point in `state()`

**What:** `AppRuntime::dispatch()` never awaits; all async work is `rt.spawn`'d and observed on
the next `state()` poll (`runtime.rs:354-497`). `state()` itself is synchronous and does the
merge — `ble_peers` is already built this way (`runtime.rs:735-750`), reading
`read_handle.peer_views()` and mapping into `BlePeer`.

**When to use:** The new `peers: Vec<PeerDiagnosticView>` field must be built the same way: a pure
synchronous map/merge inside `state()`, never a spawned task writing into a mutex that `state()`
then reads (that pattern exists for `speedtest` because it's a genuinely long-running op; peer
diagnostics is not).

**Example:**
```rust
// Source: myco-core/src/runtime.rs:731-861 (existing state() method, abbreviated)
pub fn state(&self) -> AppState {
    let ble_peers: Vec<BlePeer> = self.read_handle.as_ref()
        .map(|h| h.peer_views().into_iter().map(|p| BlePeer { .. }).collect())
        .unwrap_or_default();
    // NEW: same shape, additionally joining content.circle_snapshot(),
    // content.pending_pairs_snapshot(), content.outbound_pairs_snapshot(),
    // and the new fips-side attempt-log accessor, all keyed by npub/node_addr.
    AppState { /* ..., peers: merged, */ ..existing_fields }
}
```

### Pattern 3: Android radio → Rust bridge via `deliver_*` push + `Arc<AtomicBool>` state, not a query call

**What:** The existing BLE bridge (`android_io.rs`'s `AndroidBleBridge`) is asymmetric by design:
Kotlin *pushes* events (`deliver_scan`, `deliver_connect_result`) into channels; Rust *never*
calls back into Kotlin to ask "what's your current state" (see the module doc's "Direction of
blocking" section — outbound bytes are pulled with a blocking timeout on a dedicated writer
thread, never on a tokio worker).

**When to use:** DIAG-05's "actively scanning right now" must follow this same push shape, not a
new synchronous JNI query. Concretely: Kotlin's `BleRadio.startScanning()`/`stopScanning()`
(`BleRadio.kt:237,335`) already flip `scanCallback` to non-null/null — the fix is for those two
call sites to also flip a shared `AtomicBool` (or push a `deliver_scanning_state(bool)` event)
that `AndroidBleBridge` exposes as a plain getter, exactly like `advert_views()` today. Do the
identical thing for `AwareRadio.kt`'s `publishSession`/`subscribeSession` transitions via
`aware_bridge_jni.rs`.

**Example:**
```rust
// Source: fips/src/transport/ble/android_io.rs:289-303 (existing advert_views/clear_adverts pattern to mirror)
pub fn advert_views(&self) -> Vec<AdvertView> { /* reads a Mutex<HashMap<..>> */ }
pub fn clear_adverts(&self) { /* .. */ }
// NEW, same shape:
// pub fn is_scanning(&self) -> bool { self.scanning.load(Ordering::Relaxed) }
```

### Anti-Patterns to Avoid

- **Re-deriving the merged `peers` view in Kotlin:** D-19 explicitly locks the merge to happen
  once in Rust. `DevScreen.kt` should consume `AppState.peers` directly, the same way it already
  consumes `state.blePeers`/`state.circle` without re-joining them client-side.
- **A second polling loop inside `myco-core` for the attempt log:** The attempt log records are
  written at the *event site* (inside `accept_loop`/`scan_probe_loop`), not sampled by a timer —
  sampling would miss the very race conditions (Pitfall 1/Pitfall 7) the log exists to catch.
- **Calling into Kotlin synchronously from a tokio worker to ask "are you scanning":** violates
  the established "outbound blocks only on a dedicated writer thread" rule
  (`android_io.rs` module doc) and risks stalling the FFI thread transitively if that call ever
  contends with `dispatch()`'s own lock.
- **Loading the persisted attempt log with `serde_json::from_slice::<Vec<T>>(&bytes).ok()` and
  falling back to empty on any error, like `load_circle`/`load_library` currently do
  (`content.rs:2173-2179`, `2160-2166`):** this is the exact silent-corruption pattern flagged by
  CORE-03 and explicitly forbidden by D-13 for the new log. See Common Pitfalls.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Bounded per-peer history (D-12/D-14) | A generic ring-buffer crate, or a custom lock-free ring | `VecDeque` with `.truncate()`/`.pop_front()` under the existing per-peer `Mutex` | The cap is small (10-20), writes are infrequent (per connect attempt, not per packet); a `VecDeque` is O(1) amortized and needs no new dependency |
| Atomic per-peer counters | A generic metrics crate (e.g. `metrics`, `prometheus`) | Plain `portable_atomic::AtomicU64` fields per peer, mirroring `BleStats`'s existing shape (`stats.rs`) | The project already has exactly this pattern for global counters; a metrics framework is overkill for "how many sends dropped to this one peer" |
| Corruption-tolerant persisted log read (D-13) | A generic embedded-database or WAL crate (e.g. `sled`, `redb`) | Newline-delimited JSON (JSONL): one attempt record per line, parse line-by-line, skip unparseable lines, never touch/rewrite the file on a read failure | A single `serde_json::from_slice::<Vec<T>>` treats the whole file as one document — one bad byte anywhere invalidates everything, which is precisely the defect CORE-03 exists to fix. JSONL degrades one entry at a time instead of catastrophically. No new dependency: `std::io::BufRead::lines()` + `serde_json::from_str` per line |
| "Actively scanning" truth | Inferring `enabled && node_running` (today's `runtime.rs:794` shortcut) | A real signal pushed from the Kotlin radio's own `scanCallback`/`publishSession` non-null state | This is literally what DIAG-05 is asking to stop doing — the phase's whole premise is trading inference for observation |

**Key insight:** Every "don't hand-roll" temptation in this phase is really a temptation to
*reuse an existing bad pattern* (whole-file JSON load-or-empty, computed-not-observed status
booleans) rather than a temptation to reach for a heavyweight new dependency. The fix in each
case is a small, deliberate divergence from an existing convention, not a new library.

## Runtime State Inventory

> Not a rename/refactor/migration phase — this phase adds new fields and a new persisted file; it
> does not rename or move existing runtime state. Included for completeness per the trigger
> condition, all categories come back empty:

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | None — the new attempt log is a **new** file (e.g. `attempts.jsonl` in the app data dir), not a rename of `circle.json`/`library.json`. Those two files are read-only referenced by this phase (via `content.rs`'s existing snapshot accessors), never touched. | None |
| Live service config | None — no n8n/Datadog/Tailscale-style external service config exists in this stack | None |
| OS-registered state | None — no Task Scheduler/pm2/launchd/systemd entities in this Android app | None |
| Secrets/env vars | None — no SOPS/env-var renames | None |
| Build artifacts | None — no package/crate renames; `Cargo.lock` and Gradle artifacts are unaffected since no dependencies change | None |

**Nothing found in any category** — verified by grepping for rename/move patterns across the
phase's touched-files list in `01-CONTEXT.md`; every listed file gains new fields/composables,
none is renamed or relocated.

## Common Pitfalls

### Pitfall 1: Reusing `load_circle`'s whole-file-or-empty read pattern for the attempt log

**What goes wrong:** `content.rs:2173-2179` (`load_circle`) and the equivalent `load_library` both
do `serde_json::from_slice::<Vec<T>>(&bytes).ok()` and silently fall back to an empty `Vec` on
*any* parse error — a single corrupted byte anywhere in the file loses the *entire* Circle or
Library. This is the exact defect CORE-03 (Phase 3) exists to fix for those two files. If the new
attempt log copies this pattern, it inherits the same defect the project has already identified
and prioritized fixing elsewhere — and D-13 explicitly calls this out as a requirement the plan
"MUST" satisfy, not a nice-to-have.

**Why it happens:** It's the path of least resistance — the existing `save_circle`/`load_circle`
functions are right there as a copy-paste template, and they already do the atomic-write half
correctly (`tmp` file + `rename`).

**How to avoid:** Copy the *write* half verbatim (`serde_json::to_vec` → write `.tmp` → `rename`
— this part is genuinely fine and battle-tested). For the *read* half, use a line-delimited
format (JSONL: one attempt record per line) and parse line-by-line, skipping and counting
unparseable lines rather than failing the whole file. A truncated last line (e.g. app killed
mid-write) degrades to "one missing entry," not "no history for this peer" for the whole log —
though per D-13/UI-SPEC's Error State copy, even total unreadability must degrade to the neutral
"No history for this peer" string, never a crash or a file rewrite.

**Warning signs:** Any `.ok()` or `.unwrap_or_default()` sitting directly on a whole-file
deserialize is the smell to grep for during plan review.

### Pitfall 2: Recording BLE role/latency/outcome in `myco-core` instead of `fips`

**What goes wrong:** It's tempting to add the instrumentation in `myco-core` (Android-gated code,
easier to iterate on, no upstream-PR discipline needed) by having `AppRuntime` poll
`android_ble_bridge()`'s adverts and guess at role/timing from advert deltas. This produces
*inferred*, not *observed*, data — exactly the failure mode this whole phase exists to eliminate
— and it violates FIPS-04/the project's "no Myco-specific coupling in the fips tree" constraint in
spirit even though it wouldn't touch fips's files (Phase 2 will need the *real* fips-side
instrumentation regardless, so building a myco-core shadow version is wasted work that has to be
thrown away).

**Why it happens:** The tiebreaker and connect-attempt logic already live in fips
(`ble/mod.rs:801-820`, `1041-1053`), a crate this repo treats as "upstream, don't touch casually."
Editing it feels riskier than editing `myco-core`.

**How to avoid:** Treat this as exactly the kind of small, generic, focused diff FIPS-02 already
asks for in Phase 4 — the instrumentation is inherently generic (any fips embedder benefits from
knowing "which role did I pick, how long did discovery take, how many sends dropped"), so it is
in fact upstream-appropriate, not Myco-specific. Add the recording calls at the two existing
tiebreaker `debug!()` sites and at `scan_probe_loop`'s timeout/error `debug!()` sites — the
log statements are already exactly where the events happen; this phase just also writes a
struct instead of only a trace line.

### Pitfall 3: Treating `AppState.ble.scanning` as already correct and building the UI against it unchanged

**What goes wrong:** `runtime.rs:794` currently sets `scanning: self.ble_enabled &&
self.node_running` — a computed proxy, not a read of the actual scan-loop state. If the new
`peers`/radio-self-check UI is built reading this same field, DIAG-05 is satisfied *on paper*
(the field exists, the UI renders it) while still failing the actual requirement ("whether it is
actively scanning right now") — the field can say `true` while the scan loop crashed or is
throttled by Android's ~5-per-30s `startScan` limit (a real, documented condition
`BleRadio.kt:95-116` already handles with backoff).

**Why it happens:** The field already exists with the right name and the right shape, so it's
easy to assume it's already wired correctly.

**How to avoid:** Grep `runtime.rs` for `self.ble_enabled && self.node_running` and confirm the
01-01 plan replaces it with a genuine bridged signal (Pattern 3 above) rather than leaving it in
place and building the UI on top of the existing shortcut.

**Detection:** Toggle BLE off via Android's system Bluetooth settings (not the app's own switch)
while `ble_enabled` stays true in app state; if the Dev screen still shows "scanning: active,"
the field is still a proxy.

### Pitfall 4: Merging pairing/Circle state into `peers` and losing the "seen but not yet resolved" rows (D-09)

**What goes wrong:** `CircleContact`/`PairRequestView`/`OutboundPairView` are all keyed by
`npub` (`content.rs:85-91,162-166,177-182`) and `BlePeer`/`PeerView` carry a `node_addr_hex` that
is empty-npub until resolved (`state.rs:156-163`, `read_handle.rs:114-121`). A naive merge that
groups by `npub` first will either drop or misplace not-yet-resolved BLE adverts, which D-09
explicitly requires get their own row keyed by address. Get the merge key wrong and the "seen but
unidentified" state (D-10's five-state vocabulary) silently collapses into "unreachable" or
disappears from the list.

**Why it happens:** Every *other* existing collection in `AppState` (`circle`,
`pending_pair_requests`, `outbound_pairs`) is npub-first, so npub-first merging looks like the
established convention — but `ble_peers`/`ble_adverts` are the two collections that are not, and
they're exactly what D-09 is about.

**How to avoid:** Build the merge as: start from `ble_peers` (npub may be empty) as the base
identity set, union in `ble_adverts` (address-only, no identity yet) as additional not-yet-
resolved rows, and only *then* left-join `circle`/`pending_pair_requests`/`outbound_pairs` by
npub onto the rows that have one. A peer with no BLE presence at all but a pending pair request
still needs a row (D-06's scope: "every pending pair request" is visible even before any radio
contact) — so the union must also include pairing-only npubs that never appear in `ble_peers`.

### Pitfall 5: Locking the FFI thread on the merge because `content.rs` has 14 mutex fields

**What goes wrong:** `CONCERNS.md` already documents `state()` locking 10+ mutexes at UI
framerate as a performance bottleneck, and D-16 exists specifically to keep the *new* Dev-tab
polling off the main UI tick's cadence. But the merge itself still runs inside the *existing*
`state()` call (used by every screen, at whatever cadence Kotlin currently polls it at) unless the
01-01/01-02 split is careful — adding `content.circle_snapshot()` + `content.pending_pairs_snapshot()`
+ `content.outbound_pairs_snapshot()` + a new fips-side attempt-log read *all* into the one
already-heaviest reducer call makes every screen pay this phase's cost, not just the Dev tab.

**Why it happens:** `state()` is the only existing seam; there's no per-screen state surface today.

**How to avoid:** Confirm with the 01-01/01-02 plan whether `content.circle_snapshot()` etc. are
already computed on every `state()` call regardless (they are — `runtime.rs:826-845` already
calls them unconditionally today), in which case the new `peers` merge adds work proportional to
peer count (typically single digits at a demo), not proportional to lock count — the existing
snapshot accessors already pay the lock cost. The new work should be additive joins over already-
fetched `Vec`s, not new lock acquisitions. If profiling during 01-01 shows this isn't cheap enough,
D-16's own 1s Dev-tab-only cadence is the intended mitigation (Kotlin simply calls `state()` less
often while off-screen) — do not try to add a second Rust-side polling path to work around it.

## Code Examples

### Existing atomic file write, safe to reuse verbatim for the attempt log's write path

```rust
// Source: myco-core/src/content.rs:2180-2183 (existing save_circle)
fn save_circle(path: &Path, items: &[CircleContact]) {
    if let Ok(json) = serde_json::to_vec(items) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}
```

### Existing lock-free per-connection peer view, the shape any new per-peer type should match

```rust
// Source: fips/src/control/read_handle.rs:113-143 (existing PeerView + peer_views())
#[derive(Debug, Clone)]
pub struct PeerView {
    pub node_addr_hex: String,
    pub npub: String,
    pub connected: bool,
}

pub fn peer_views(&self) -> Vec<PeerView> {
    self.stats.load().peer_meta.iter()
        .map(|(addr, meta)| PeerView {
            node_addr_hex: addr.to_string(),
            npub: meta.npub.clone(),
            connected: meta.is_active,
        })
        .collect()
}
```

### Existing tiebreaker — the two exact sites new instrumentation attaches to

```rust
// Source: fips/src/transport/ble/mod.rs:801-813 (accept_loop, inbound side)
if let Some(ref our_addr) = local_node_addr {
    let peer_addr = NodeAddr::from_pubkey(&peer_pubkey);
    if our_addr < &peer_addr {
        debug!(addr = %ta, "BLE inbound tie-breaker: dropping (our addr < peer, outbound wins)");
        continue; // ← record role=Peripheral-lost / outcome=YieldedToOutbound here
    }
}
```

```rust
// Source: fips/src/transport/ble/mod.rs:1041-1053 (scan_probe_loop, outbound side)
if let Some(ref our_addr) = local_node_addr {
    let peer_addr = NodeAddr::from_pubkey(&peer_pubkey);
    if our_addr >= &peer_addr {
        debug!(addr = %addr, "BLE probe tie-breaker: yielding to peer's outbound");
        buffer.add_peer_with_pubkey(&addr, peer_pubkey);
        continue; // ← record role=Central-lost / outcome=YieldedToPeer here
    }
}
// falls through to pool.insert() on the winning path — record role=Central-won / outcome=Connected here
```

### Existing Kotlin-side "is the radio doing the thing" state, ready to bridge

```kotlin
// Source: android/app/src/main/java/app/myco/ble/BleRadio.kt:237-341 (existing scan lifecycle)
fun startScanning() {
    stopScanning()
    // ... sc.startScan(filters, settings, cb) ...
    // scanCallback is now non-null — this is the "is scanning" truth to bridge
}
fun stopScanning() {
    scanCallback?.let { runCatching { scanner?.stopScan(it) } }
    // scanCallback set to null here — bridge this transition too
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| `AppState.ble.scanning` computed from `enabled && node_running` | Should read a genuine Kotlin-bridged scan-loop state | This phase (01-01) | DIAG-05 becomes literally true instead of "usually true" |
| `WifiAwareStatus` has only `enabled`/`port` | Needs a new `scanning` (or `discovering`) field sourced from `AwareRadio.kt`'s publish/subscribe session liveness | This phase (01-01), flagged explicitly as Claude's Discretion in `01-CONTEXT.md` | Closes the same gap on the second radio |
| BLE role/outcome only in `debug!()` traces (not queryable without `adb logcat`) | Recorded as structured per-peer state reachable from `AppState` | This phase (01-01) | This is literally the phase's thesis — turning inferred root causes into observed facts |
| `AppState` has no unified peer view; UI hand-assembles from `ble_peers` + `circle` + `pending_pair_requests` + `outbound_pairs` separately | New `peers: Vec<...>` merged once in Rust (D-19) | This phase (01-02) | Removes a whole class of "did I join these four arrays correctly" bugs from the UI layer |

**Deprecated/outdated:** Nothing in this codebase is being deprecated by this phase — everything
listed above is additive. `CircleContact.name`'s "a placeholder for now" comment
(`content.rs:88`) is explicitly *not* being fixed here per the Deferred Ideas section of
`01-CONTEXT.md` — DIAG-07 only needs to *surface* the Circle name, not make it robust.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Wi-Fi Aware's "actively scanning" signal should map to `publishSession`/`subscribeSession` non-null in `AwareRadio.kt`, treating NAN publish+subscribe liveness as the Aware analog of BLE scan/advertise | Architecture Patterns (Pattern 3), State of the Art | If a maintainer considers "discovering peers" and "publishing our own presence" meaningfully different signals for the self-check card, a single boolean under-reports; low risk since both sessions start/stop together in `AwareRadio.kt`'s current lifecycle (`start()`/`stop()`), so they're unlikely to diverge in practice |
| A2 | The BLE tiebreaker code path (`ble/mod.rs`'s `accept_loop`/`scan_probe_loop`) is the one actually exercised on Android via `DefaultBleTransport = BleTransport<android_io::AndroidIo>` | Summary, Common Pitfalls #2 | Confirmed by reading the type alias and `BleTransport::start_async()`'s generic `spawn(accept_loop(...))`/`spawn(scan_probe_loop(...))` calls directly — this is source-verified, not assumed, but listed here because it's the load-bearing fact for "where does instrumentation go" |
| A3 | A JSONL (line-delimited) on-disk format is an acceptable divergence from the existing `Vec<T>`-as-one-JSON-document convention used by `circle.json`/`library.json`, for the specific purpose of D-13's corruption tolerance | Don't Hand-Roll, Common Pitfalls #1 | If plan review prefers a different corruption-tolerant scheme (e.g., one-JSON-document-per-peer-file, or a checksum-per-record binary format), the file format changes but the *principle* (never let one bad byte destroy the whole log) still holds — low risk, this is an implementation-detail-level assumption, not a design-level one |

## Open Questions

1. **Should the per-peer attempt log live inside the fips crate's in-memory state (published via
   `ControlReadHandle`) or be built entirely in `myco-core` from raw fips events?**
   - What we know: The role/outcome *decision* must be captured in fips (Common Pitfall #2). The
     *retention/capping/persistence* (D-12/D-13/D-14) is explicitly myco-core's concern (app data
     dir, not fips's).
   - What's unclear: Whether fips should hold its own bounded in-memory ring (published via a new
     `ArcSwap` cell, mirroring `EntitySnapshot`) that myco-core reads and persists, or whether fips
     should expose a raw per-event callback/channel that myco-core itself buffers and caps.
   - Recommendation: Prefer the `ArcSwap`-published-snapshot shape (matches Pattern 1) — it keeps
     fips's public surface consistent with everything else on `ControlReadHandle` and avoids adding
     a second, differently-shaped extension mechanism to fips. This is explicitly listed as
     Claude's Discretion in `01-CONTEXT.md` ("the exact field set and Rust type of an attempt
     record, and what constitutes one 'attempt' boundary") — the 01-01 plan should pin the concrete
     struct shape.

2. **What exactly bounds "one attempt" for the log (D-12)?**
   - What we know: A tiebreaker loss, a connect timeout, a connect error, and a successful
     promotion into the pool are all distinct `debug!()` sites today (four to five call sites
     across `accept_loop`/`scan_probe_loop`).
   - What's unclear: Whether each of those is its own log entry (finer-grained, more entries burn
     through the 10-20 cap faster) or whether a full discover→(win/lose tiebreaker or
     timeout/error)→connected-or-not sequence is coalesced into one entry with an outcome enum.
   - Recommendation: Coalesce per discovery-to-resolution cycle into one entry with an outcome enum
     (`Connected`, `TimedOut`, `ConnectError`, `LostTiebreaker`, `WonTiebreakerButRejected`) — this
     matches D-12's own framing ("connect attempts," singular events with a timestamp/role/duration/
     outcome each), and keeps the 10-20 cap meaningful (10-20 *attempts*, not 10-20 individual log
     lines within one attempt).

## Environment Availability

This phase's only external dependency is the Android device/emulator itself (no new services,
databases, or CLIs). Standard Android build tooling already verified working by the existing
codebase (Gradle, NDK cross-compilation for `aarch64-linux-android`) is unchanged by this phase.

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Android device with BLE + Wi-Fi Aware hardware | On-device verification of DIAG-01/03/04/05 | Assumed ✓ (existing demo hardware — Samsung/Xiaomi/Pixel per `STATE.md`) | — | None needed; this is the phase's actual test target |
| `reference/fips` checked out on `feat/platform-peer-queue` | Every fips-side file this research references (`ble/mod.rs`, `read_handle.rs`, etc.) | ✓ verified this session (`git branch --show-current` → `feat/platform-peer-queue`) | — | `master` does not export the needed control-plane surface (`CONVENTIONS.md` §"Build environment") — do not switch branches mid-phase |

**Missing dependencies with no fallback:** none identified.
**Missing dependencies with fallback:** none — this phase has no dependency with a fallback path;
it either runs against `feat/platform-peer-queue` or it doesn't run.

## Security Domain

`security_enforcement` is enabled (`config.json`, ASVS level 1, block on `high`). This phase is a
read-only diagnostics surface with no new authentication, session, or cryptographic surface — the
relevant category is input handling of peer-supplied strings, which the UI-SPEC has already
addressed at the design-contract level.

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | This phase adds no auth surface — identity display (DIAG-07) reads the existing device keypair, doesn't change how it's established |
| V3 Session Management | No | No session/token handling added |
| V4 Access Control | No | The Circle-gating logic (`CircleGate`) is untouched; this phase only *displays* pairing state that already exists |
| V5 Input Validation | Yes | Peer-supplied strings (Circle contact names from `AddToCircle`, requester names in `PairRequestView`) render through the existing `short()` truncation helper (`DevScreen.kt:375-376`) — UI-SPEC's E2/E4 "long-text" rows already mandate this defensively, not just for layout. The new attempt-log entries and role/outcome enums are locally-generated (not peer-supplied), so they carry no injection risk themselves |
| V6 Cryptography | No | No new cryptographic operations; identity display reads already-derived `npub`/hex values (`IdentityView`, `state.rs:83-94`) |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| A malicious/misbehaving peer sends an oversized or control-character-laden `name` in a pair request or Circle add, expecting it to render unbounded on screen | Denial of Service (UI), Spoofing (visual) | Already mitigated by the existing `short()` truncation applied to every peer-supplied string on this screen (per UI-SPEC's Copywriting/long-text rows for E2/E4) — the 01-02 plan must apply `short()` (or equivalent) to any *new* peer-supplied field this phase surfaces (e.g., if a future attempt-log entry ever carried a peer-echoed string, which per Open Question #2's recommended shape it does not — outcomes are locally-computed enums) |
| A corrupted or truncated attempt-log file on disk (crash mid-write, storage fault) is misread as valid data, or worse, causes a panic on load that takes the app down | Tampering (data integrity), Denial of Service | Corruption-tolerant per-entry read path (Common Pitfall #1) — never let a bad file panic startup or silently destroy a still-good file, per D-13 |
| A peer floods connect attempts to make its own attempt-log ring evict legitimate history before the real race can be diagnosed | Denial of Service (diagnostic integrity) | Out of scope for this phase — the cap (D-12/D-14) is inherently bounded per-peer, so a flood against *one* peer's own log doesn't affect other peers' rings; a flood is itself diagnostic signal (a very full, fast-churning ring for one peer is visible evidence of exactly the kind of instability Phase 2 needs to see) |

## Sources

### Primary (HIGH confidence)
- `fips/src/transport/ble/mod.rs` (read this session, lines 1-260, 760-1254) — the BLE transport, tiebreaker sites, scan/accept loops
- `fips/src/transport/ble/stats.rs` (read in full) — existing global `BleStats` atomic counters
- `fips/src/transport/ble/discovery.rs` (read in full) — `DiscoveryBuffer`, no timestamps today
- `fips/src/transport/ble/android_io.rs` (read this session, lines 1-115) — `AndroidRadio` trait, `AndroidBleBridge`, existing push-only bridge shape
- `fips/src/control/read_handle.rs` (read in full) — `ControlReadHandle`, `PeerView`, `peer_views()`, the tick-published-snapshot pattern
- `fips/src/control/queries.rs` (read this session, lines 1-1458) — off-loop query rendering pattern, confirms `entities`/`stats`/`routing` snapshot shape
- `myco-core/src/state.rs`, `action.rs`, `runtime.rs` (all read in full) — `AppState`, `NativeAppAction`, the reducer/merge pattern this phase extends
- `myco-core/src/content.rs` (grepped for structure; `CircleContact`/`PairRequestView`/`OutboundPairView` read in full; persistence pattern at lines 2160-2183 read in full)
- `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` (read in full) — every composable this phase reuses/rebuilds around
- `android/app/src/main/java/app/myco/ui/theme/Theme.kt` (grepped for status colors) — `StatusConnected`/`StatusReachable`/`StatusThin`/`StatusAlone`
- `android/app/src/main/java/app/myco/ble/BleRadio.kt`, `android/app/src/main/java/app/myco/aware/AwareRadio.kt` (grepped for scan/session lifecycle) — the Kotlin-side truth this phase needs to bridge
- `.planning/phases/01-make-peering-observable/01-CONTEXT.md`, `01-UI-SPEC.md`, `.planning/REQUIREMENTS.md`, `.planning/STATE.md`, `.planning/ROADMAP.md`, `.planning/research/PITFALLS.md`, `.planning/codebase/CONCERNS.md` — project-internal, HIGH confidence, all read in full this session

### Secondary (MEDIUM confidence)
None — no external web sources were needed for this phase; every claim traces to a file read this session.

### Tertiary (LOW confidence)
None.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new libraries, all versions read directly from checked-out `Cargo.toml`
- Architecture: HIGH — every pattern cited is an existing, working pattern in the current codebase, not a proposal
- Pitfalls: HIGH for the fips/myco-core mechanics (source-verified); MEDIUM for the exact Android
  OEM scan-throttling numbers cited only in `PITFALLS.md` (external, not re-verified this session)

**Research date:** 2026-08-04
**Valid until:** This phase must land before the 2026-08-05 release per `ROADMAP.md` — this
research is only valid for that immediate execution window; do not reuse for Phase 2 without
re-checking `reference/fips`'s branch state, since Phase 4 (fips rebase) may move these exact line
numbers.
