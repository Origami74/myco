# Phase 1: Make Peering Observable - Pattern Map

**Mapped:** 2026-08-04
**Files analyzed:** 11 (new/modified across fips, myco-core, android)
**Analogs found:** 11 / 11

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `fips/src/transport/ble/mod.rs` (tiebreaker/outcome recording) | transport instrumentation | event-driven | same file, existing `debug!()` sites at tiebreaker/connect outcomes | exact (extend in place) |
| `fips/src/transport/ble/stats.rs` (per-peer counters) | model/counters | event-driven | `BleStats` (same file) | exact (extend in place) |
| `fips/src/transport/ble/discovery.rs` (discovered-at timestamp) | model | event-driven | `DiscoveryBuffer`/`DiscoveredPeer` (same file) | exact (extend in place) |
| `fips/src/control/read_handle.rs` (new `ArcSwap` cell / accessor for attempt log) | provider/snapshot publisher | pub-sub (tick-published) | `ControlReadHandle` + `stats`/`routing`/`entities` `ArcSwap` cells, `PeerView`/`peer_views()` | exact |
| `fips/src/transport/ble/android_io.rs` (`is_scanning()` bridge getter) | provider (JNI bridge) | event-driven push | `AndroidBleBridge::advert_views()`/`clear_adverts()`/`deliver_scan()` | exact |
| `myco-core/src/state.rs` (`PeerDiagnosticView`, `PeerAttempt`, `AppState.peers`, `WifiAwareStatus.scanning`) | model | CRUD (snapshot struct) | `AppState`, `BlePeer`, `BleStatus`, `WifiAwareStatus`, `IdentityView` (same file) | exact |
| `myco-core/src/runtime.rs` (`state()` merge, attempt-log persistence) | service (reducer merge) | request-response (poll) | `state()` method (same file, lines 731-861); `Content::save_circle`/`load_circle` (`content.rs`) for persistence | exact |
| `myco-core/src/content.rs` (no functional change; reference only) | model/service | CRUD | `CircleContact`, `PairRequestView`, `OutboundPairView` (same file) | exact (read-only reuse) |
| `android/app/src/main/java/app/myco/ble/BleRadio.kt` (push scanning state) | radio bridge | event-driven push | `startScanning()`/`stopScanning()` (same file) | exact |
| `android/app/src/main/java/app/myco/aware/AwareRadio.kt` (push scanning state) | radio bridge | event-driven push | `publishSession`/`subscribeSession` lifecycle in `start()`/`stop()` (same file) | exact |
| `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` (rebuilt around peering) | component (Compose screen) | request-response (poll) | Same file: `DevCard`, `KeyValDot`, `PeerRow`, `AwareLinkRow`, `ApNodeRow`, `SectionCard`, `StatusDot` | exact (extend/recompose in place) |

## Pattern Assignments

### `fips/src/control/read_handle.rs` (provider, pub-sub snapshot)

**Analog:** same file — `ControlReadHandle` struct and `stats`/`routing`/`entities` cells (lines 44-59), `peer_views()` (lines 111-141).

**Struct shape to copy** (lines 44-59):
```rust
pub struct ControlReadHandle {
    context: Arc<NodeContext>,
    metrics: Arc<MetricsRegistry>,
    stats: Arc<ArcSwap<StatsSnapshot>>,
    routing: Arc<ArcSwap<RoutingSnapshot>>,
    entities: Arc<ArcSwap<EntitySnapshot>>,
    // NEW, same shape: ble_attempts: Arc<ArcSwap<BleAttemptSnapshot>>,
}
```

**Lock-free published-view accessor pattern** (lines 92-108, 123-141):
```rust
pub(crate) fn stats(&self) -> arc_swap::Guard<Arc<StatsSnapshot>> {
    self.stats.load()
}
// ...
pub fn peer_views(&self) -> Vec<PeerView> {
    self.stats
        .load()
        .peer_meta
        .iter()
        .map(|(addr, meta)| PeerView {
            node_addr_hex: addr.to_string(),
            npub: meta.npub.clone(),
            connected: meta.is_active,
        })
        .collect()
}
```

**Minimal public view struct convention** (lines 111-121):
```rust
/// A minimal, public view of one peer for embedders (e.g. an app UI), read
/// lock-free from the tick-published snapshot.
#[derive(Debug, Clone)]
pub struct PeerView {
    pub node_addr_hex: String,
    pub npub: String,
    pub connected: bool,
}
```
Every public field carries a `///` doc comment — new `PeerAttempt`/`BleAttemptSnapshot` types must match this doc density.

---

### `fips/src/transport/ble/mod.rs` (instrumentation at tiebreaker/outcome sites)

**Analog:** same file — the two existing tiebreaker `debug!()` sites are the exact attachment points.

**Inbound tiebreaker-loss site** (lines 800-813, `accept_loop`):
```rust
// Cross-probe tie-breaker: smaller NodeAddr's
// outbound wins. If we're smaller, our outbound
// should win — drop this inbound.
if let Some(ref our_addr) = local_node_addr {
    let peer_addr = NodeAddr::from_pubkey(&peer_pubkey);
    if our_addr < &peer_addr {
        debug!(
            addr = %ta,
            "BLE inbound tie-breaker: dropping (our addr < peer, outbound wins)"
        );
        continue; // ← record role=Peripheral-lost / outcome=YieldedToOutbound here
    }
}
```

**Outbound tiebreaker-loss / win-and-promote site** (lines 1041-1084, `scan_probe_loop`):
```rust
// Cross-probe tie-breaker: smaller NodeAddr's outbound wins.
// If we lose, drop connection — accept_loop handles inbound.
if let Some(ref our_addr) = local_node_addr {
    let peer_addr = NodeAddr::from_pubkey(&peer_pubkey);
    if our_addr >= &peer_addr {
        debug!(
            addr = %addr,
            "BLE probe tie-breaker: yielding to peer's outbound"
        );
        buffer.add_peer_with_pubkey(&addr, peer_pubkey);
        continue; // ← record role=Central-lost / outcome=YieldedToPeer here
    }
}
// falls through to pool.insert() — record role=Central-won / outcome=Connected here
let mut pool_guard = pool.lock().await;
match pool_guard.insert(ta.clone(), conn) {
    Ok(Some(evicted)) => {
        stats.record_pool_eviction();
        debug!(addr = %ta, evicted = %evicted, "BLE probe promoted (evicted peer)");
    }
    Ok(None) => {
        debug!(addr = %ta, "BLE probe promoted to pool");
    }
```
Pattern: every existing state transition is already a `debug!()` call site with `addr`/error context — the new instrumentation is a struct write immediately adjacent to each, not a new code path.

---

### `fips/src/transport/ble/stats.rs` (per-peer counters)

**Analog:** same file — `BleStats`'s existing global atomics.

```rust
use portable_atomic::{AtomicU64, Ordering};

pub struct BleStats {
    pub packets_sent: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub send_errors: AtomicU64,
    pub connections_established: AtomicU64,
    pub connections_accepted: AtomicU64,
    pub connections_rejected: AtomicU64,
    pub connect_timeouts: AtomicU64,
    pub pool_evictions: AtomicU64,
    // ...
}
```
A new per-peer counter type (e.g. `PerPeerBleStats`) should mirror this field style (`portable_atomic::AtomicU64`, plain public fields, no getter boilerplate) rather than introducing a metrics crate — matches `Don't Hand-Roll` guidance in RESEARCH.md.

---

### `fips/src/transport/ble/android_io.rs` (push-bridge getter for "is scanning")

**Analog:** same file — `AndroidBleBridge::advert_views()` / `clear_adverts()` / `deliver_scan()` (lines 276-308).

```rust
/// Kotlin discovered a FIPS peer advertising `psm`... Learns the per-peer PSM,
/// records the advert for the developer UI, and surfaces the address to the scanner.
pub fn deliver_scan(&self, addr: BleAddr, psm: u16, rssi: i32) {
    if psm != 0 {
        self.psm_map.learn(&addr, psm);
    }
    self.adverts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(addr.clone(), (psm, rssi));
    let _ = self.scan_tx.try_send(addr);
}

/// Snapshot of the current scan adverts (address / PSM / RSSI) for the developer UI.
pub fn advert_views(&self) -> Vec<AdvertView> {
    self.adverts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(addr, (psm, rssi))| AdvertView { addr: addr.to_string_repr(), psm: *psm, rssi: *rssi })
        .collect()
}
```
New pattern to add, same shape: `pub fn is_scanning(&self) -> bool { self.scanning.load(Ordering::Relaxed) }`, fed by a Kotlin `deliver_scanning_state(bool)`-style push call, never a synchronous callback into Kotlin (see Anti-Patterns below).

---

### `myco-core/src/state.rs` (new `PeerDiagnosticView`, `AppState.peers`, `WifiAwareStatus.scanning`)

**Analog:** same file — `AppState`, `BleStatus`, `WifiAwareStatus`, `BlePeer` (lines 1-175).

**AppState field-addition convention** (lines 8-56):
```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub rev: u64,
    pub error: String,
    // ...
    pub ble_peers: Vec<BlePeer>,
    pub ble_adverts: Vec<BleAdvert>,
    pub wifi_aware: WifiAwareStatus,
    // NEW: pub peers: Vec<PeerDiagnosticView>,   (D-19)
    // ...
}
```

**Existing computed-not-observed field to replace** (lines 124-134):
```rust
pub struct BleStatus {
    pub enabled: bool,
    pub role: String,
    /// Whether the scan loop is currently running.
    pub scanning: bool,   // currently set from self.ble_enabled && self.node_running — see runtime.rs
    pub adapter_name: String,
}
```

**`WifiAwareStatus` needs a new field** (lines 140-148):
```rust
pub struct WifiAwareStatus {
    pub enabled: bool,
    pub port: u16,
    // NEW: pub scanning: bool,  (sourced from AwareRadio.kt publish/subscribe liveness)
}
```

**Row-model convention to follow for `PeerDiagnosticView`** (`BlePeer`, lines 150-163):
```rust
/// One peer seen or connected over BLE. Keyed by `node_addr` from the in-band
/// `[0x00][pubkey:32]` exchange (never the rotating MAC); `npub` resolves once
/// the Noise handshake completes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlePeer {
    pub node_addr_hex: String,
    pub npub: String,
    pub connected: bool,
    pub psm: u16,
    pub rssi: Option<i32>,
}
```
`camelCase` serde rename, doc comment on struct explaining the key, doc comments on any non-obvious field — this is the template for `PeerDiagnosticView` (state enum, transport, last_seen, role, discovery latency, drop counters, attempt log) and for `PeerAttempt`.

---

### `myco-core/src/runtime.rs` (`state()` merge point)

**Analog:** same file — `state()` (lines 731-861), specifically the `ble_peers` build (735-750) and the unconditional snapshot calls (811-850).

```rust
pub fn state(&self) -> AppState {
    let ble_peers: Vec<BlePeer> = self
        .read_handle
        .as_ref()
        .map(|h| {
            h.peer_views()
                .into_iter()
                .map(|p| BlePeer { node_addr_hex: p.node_addr_hex, npub: p.npub, connected: p.connected, psm: 0, rssi: None })
                .collect()
        })
        .unwrap_or_default();
    // ...
    AppState {
        // ...
        ble: BleStatus {
            enabled: self.ble_enabled,
            role: "peripheral+central".to_string(),
            scanning: self.ble_enabled && self.node_running,  // ← Pitfall 3: replace with genuine bridged signal
            adapter_name: if self.node_running { "ble0".to_string() } else { "—".to_string() },
        },
        ble_peers,
        ble_adverts: self.ble_adverts(),
        wifi_aware: WifiAwareStatus { enabled: self.wifi_aware_enabled, port: if self.wifi_aware_enabled { WIFI_AWARE_PORT } else { 0 } },
        // ...
        circle: self.content.as_ref().map(|c| c.circle_snapshot()).unwrap_or_default(),
        outbound_pairs: self.content.as_ref().map(|c| c.outbound_pairs_snapshot()).unwrap_or_default(),
        pending_pair_requests: self.content.as_ref().map(|c| c.pending_pairs_snapshot()).unwrap_or_default(),
        // NEW: peers: merge(ble_peers, ble_adverts, circle, pending_pair_requests, outbound_pairs, attempt_log)
    }
}
```
This confirms RESEARCH.md's Pitfall 5 claim: `circle_snapshot()`/`outbound_pairs_snapshot()`/`pending_pairs_snapshot()` are already called unconditionally on every `state()` call — the new `peers` merge is additive joins over already-fetched `Vec`s, no new lock acquisitions needed.

**Anti-pattern confirmed at line 794** — `scanning: self.ble_enabled && self.node_running` is exactly the computed-not-observed shortcut Pitfall 3 warns about; must be replaced with a call through the new bridge getter (`android_ble_bridge().is_scanning()`), not left in place.

---

### `myco-core/src/content.rs` — write pattern to REUSE, read pattern to AVOID

**Analog:** same file — `save_circle`/`load_circle` (lines 2159-2185), `CircleContact`/`PairRequestView`/`OutboundPairView` (lines 85-91, 162-182).

**Atomic write path — copy verbatim for the attempt log:**
```rust
fn save_circle(path: &Path, items: &[CircleContact]) {
    if let Ok(json) = serde_json::to_vec(items) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}
```

**Whole-file-or-empty read path — the CORE-03 anti-pattern D-13 explicitly forbids repeating:**
```rust
fn load_circle(path: &Path) -> Vec<CircleContact> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}
```
`serde_json::from_slice::<Vec<T>>(&bytes).ok()` treats the entire file as one document; any single corrupt byte anywhere silently loses the whole Circle/Library. The attempt log's read path must NOT copy this — use JSONL (one `PeerAttempt` record per line), parse `BufRead::lines()` and skip/count unparseable lines individually, so a truncated last line loses one entry, not the whole log, and total unreadability degrades to the UI-SPEC's "No history for this peer" string rather than an empty `Vec` masquerading as "confirmed no history."

**Reference-only structs already carrying DIAG-06/07 data (no changes needed):**
```rust
pub struct CircleContact { pub npub: String, pub name: String, pub added_at: u64 }
pub struct PairRequestView { pub npub: String, pub name: String, pub secret: String }
pub struct OutboundPairView { pub npub: String, pub name: String, pub since: u64 }
```

---

### `android/app/src/main/java/app/myco/ble/BleRadio.kt` (push "is scanning" state)

**Analog:** same file — `startScanning()`/`stopScanning()` (lines 237-337).

```kotlin
fun startScanning() {
    if (stopped) return
    stopScanning()
    val sc = adapter?.bluetoothLeScanner ?: return
    scanner = sc
    // ... builds filters/settings/cb ...
    scanCallback = cb   // ← the "is scanning" truth; bridge this transition
}

fun stopScanning() {
    scanCallback?.let { runCatching { scanner?.stopScan(it) } }
    scanCallback = null   // ← bridge this transition too
}
```
Fix: both call sites also flip a shared signal pushed to the Rust bridge (e.g. `NativeActions`/JNI `deliverScanningState(true/false)`), mirroring the `deliver_scan` push shape already used for adverts — never a query call from Rust back into Kotlin.

---

### `android/app/src/main/java/app/myco/aware/AwareRadio.kt` (push "is scanning/publishing" state)

**Analog:** same file — `publishSession`/`subscribeSession` lifecycle in `start()`/`stop()` (lines 70-260).

```kotlin
private var publishSession: PublishDiscoverySession? = null
private var subscribeSession: SubscribeDiscoverySession? = null
// start(): startPublish()/startSubscribe() set these non-null on success
// stop(): runCatching { publishSession?.close() }; runCatching { subscribeSession?.close() }; both set to null
```
Same push-bridge shape as BleRadio: treat `publishSession != null || subscribeSession != null` (or both explicitly) as the Aware "actively scanning" signal and push it across the bridge on every transition, per RESEARCH.md Assumption A1.

---

### `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` (rebuilt around peering, D-03)

**Analog:** same file — the entire card/row vocabulary already exists; the phase re-composes it rather than replacing it.

**Imports convention** (lines 3-42):
```kotlin
import androidx.compose.foundation.layout.*
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppState
import app.myco.core.BlePeer
import app.myco.ui.SectionCard
import app.myco.ui.StatusDot
import app.myco.ui.theme.StatusConnected
```

**Card scaffold to reuse** (lines 280-294):
```kotlin
@Composable
private fun DevCard(title: String, content: @Composable () -> Unit) {
    Column {
        Text(title, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.Bold,
            style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 4.dp, bottom = 6.dp))
        SectionCard {
            Column(modifier = Modifier.padding(vertical = 6.dp)) { content() }
        }
    }
}
```

**Status-dot + value row (KeyValDot) — the pattern the radio self-check card and new emphasized values follow** (lines 296-314):
```kotlin
@Composable
private fun KeyValDot(label: String, value: String, ok: Boolean) {
    Row(modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically) {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            StatusDot(if (ok) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, color = if (ok) StatusConnected else MaterialTheme.colorScheme.onSurface,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace))
        }
    }
}
```
D-10's five-state peer dot and D-18's monospace last-seen counter both extend this exact row anatomy (glyph/dot + label Regular + value SemiBold-in-state-color), not a new component.

**Collapsed peer row to extend for expand-in-place (D-05)** (lines 316-329):
```kotlin
@Composable
private fun PeerRow(peer: BlePeer) {
    Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
        Text(
            if (peer.connected) "● connected" else "○ seen",
            color = if (peer.connected) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.labelMedium,
        )
        Text(
            "${short(peer.nodeAddrHex)}  ${peer.npub.ifEmpty { "(handshake pending)" }.let { if (it.length > 18) it.take(14) + "…" else it }}",
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
        )
    }
}
```
The new five-state peer row (D-10) is this same two-line anatomy (state glyph/dot line, then monospace identity line) plus a `clickable { expanded = !expanded }` toggle and a conditional detail block below — not a new row type from scratch. `AwareLinkRow`/`ApNodeRow` (same file, lines 331-359) are siblings of the identical shape, useful if a transport-specific sub-row is needed inside the expanded detail.

**Theme colors** (`android/app/src/main/java/app/myco/ui/theme/Theme.kt`, grepped lines 23-29):
```kotlin
val StatusConnected = Color(0xFF22C55E)
val StatusReachable = Color(0xFF14B8A6)
val StatusThin = Color(0xFFF59E0B)
val StatusAlone = Color(0xFFEF4444)
```
D-10's five states map: `connected`→`StatusConnected`, `reachable-via-relay`→`StatusReachable`, `paired-offline`→`StatusThin`, `unreachable`→`StatusAlone`, `seen-unidentified`→`onSurfaceVariant` (no color constant — neutral, matches `PeerRow`'s existing "○ seen" convention). No new colors declared, per D-10 and `ThemeTest.kt`.

## Shared Patterns

### Tick-published lock-free snapshot (fips-side instrumentation)
**Source:** `fips/src/control/read_handle.rs` lines 44-59, 92-108, 123-141
**Apply to:** Any new per-peer BLE data (role, discovery latency, drop counters, attempt log) — publish via a new `Arc<ArcSwap<T>>` cell on `ControlReadHandle`, read lock-free via a `peer_views()`-shaped accessor. Do not introduce a second locking/polling mechanism.

### Spawn-not-block reducer with single merge point
**Source:** `myco-core/src/runtime.rs` `state()`, lines 731-861
**Apply to:** `AppState.peers` construction — a pure synchronous map/merge inside `state()`, joining `ble_peers` + `ble_adverts` + `content.circle_snapshot()` + `content.pending_pairs_snapshot()` + `content.outbound_pairs_snapshot()` + the new fips-side attempt-log accessor. Never spawn a task that writes into a mutex `state()` then reads.

### Push-bridge getter, never a synchronous Kotlin callback
**Source:** `fips/src/transport/ble/android_io.rs` `advert_views()`/`clear_adverts()`/`deliver_scan()`, lines 276-308
**Apply to:** BLE "is scanning" (`myco-core` + `BleRadio.kt`) and Wi-Fi Aware "is scanning" (`aware_bridge_jni.rs` + `AwareRadio.kt`). Kotlin pushes state transitions in; Rust exposes a plain getter. Never call into Kotlin synchronously from a tokio worker.

### Atomic write-tmp-then-rename (reuse verbatim) vs. whole-file read (do NOT reuse)
**Source:** `myco-core/src/content.rs` lines 2159-2185 (`save_circle`/`load_circle`)
**Apply to:** The persisted attempt log (D-13/D-14). Copy the write half (`serde_json::to_vec` → `.tmp` → `rename`) verbatim. Do NOT copy the read half's `serde_json::from_slice::<Vec<T>>(&bytes).ok()` — this is the CORE-03 corruption defect D-13 explicitly forbids repeating. Use JSONL with per-line parsing instead.

### `DevCard`/`KeyValDot`/`PeerRow`/`SectionCard`/`StatusDot` composable vocabulary
**Source:** `android/app/src/main/java/app/myco/ui/screens/DevScreen.kt` lines 280-359, `android/app/src/main/java/app/myco/ui/theme/Theme.kt` lines 23-29
**Apply to:** Every new Dev-tab card (radio self-check, peer list, pending pairings, identity) per D-03 — reuse composables, extend row anatomy for expand-in-place, reuse existing status colors only (no new palette).

## No Analog Found

None — every file in scope has a same-file or same-crate exact analog; this phase is entirely an extension of existing structures, not new architecture.

## Metadata

**Analog search scope:** `/Users/gump/Documents/development/fips/fips/src/control/`, `/Users/gump/Documents/development/fips/fips/src/transport/ble/`, `/Users/gump/Documents/development/fips/fips-pop/myco-core/src/`, `/Users/gump/Documents/development/fips/fips-pop/android/app/src/main/java/app/myco/`
**Files scanned:** 10 (read in full or targeted ranges) + RESEARCH.md's own source list (all HIGH confidence, previously read in full by gsd-phase-researcher this session)
**Pattern extraction date:** 2026-08-04
