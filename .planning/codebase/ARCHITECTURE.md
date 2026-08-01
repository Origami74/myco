<!-- refreshed: 2026-08-01 -->
# Architecture

**Analysis Date:** 2026-08-01

## System Overview

Myco is a peer-to-peer app-sharing network where devices sync and share nsites (static web apps published on Nostr) over a mesh network. The system runs on one Android device, split across a Jetpack Compose UI (Kotlin) and a native library (`libmyco_core.so`, built from four Rust crates), communicating via a JNI/JSON reducer boundary.

```text
┌─────────────────────────────────────────────────────────────┐
│  1. Android UI (Jetpack Compose)                             │
│     Library / Pair / Discover / Settings                     │
└────────────────┬────────────────────────────────────────────┘
                 │ JNI/JSON FFI: dispatch(actionJson) → stateJson
┌────────────────▼────────────────────────────────────────────┐
│  2. NsiteActivity WebView (per nsite)                        │
│     Loads http://<host>.nsite (localhost)                    │
└────────────────┬────────────────────────────────────────────┘
                 │ HTTP via `:127.0.0.1:4870`
┌────────────────▼────────────────────────────────────────────┐
│  3. Local Gateway + DNS Interception                         │
│     *.nsite → 127.0.0.1; manifest resolve; file serve       │
│     `nsite-deck::gateway`                                    │
└────────────────┬────────────────────────────────────────────┘
                 │ Event queries, blob gets
┌────────────────▼────────────────────────────────────────────┐
│  4. Embedded Relay + Blossom Servers                         │
│     `myco-relay` :4870 (WebSocket) & :127.0.0.1:4870 (local)│
│     `myco-blossom` :24243 (HTTP, IPv6-only mesh)            │
│     RelayStore + FsBlobStore                                │
└────────────────┬────────────────────────────────────────────┘
                 │ Dispatch, content management
┌────────────────▼────────────────────────────────────────────┐
│  5. myco-core (App Crate)                                    │
│     Redux reducer, FFI binding, content wiring              │
│     `AppRuntime` + `Content` layer                          │
│     Gossip, Peer Relay Pool, DNS intercept                 │
└────────────────┬────────────────────────────────────────────┘
                 │ Mesh identity & routing
┌────────────────▼────────────────────────────────────────────┐
│  6. FIPS Core + Transports                                   │
│     Embedded node: BLE (L2CAP), UDP (WiFi Aware), TCP/Tor   │
│     Mesh routing, Noise handshakes, peer discovery          │
└────────────────┬────────────────────────────────────────────┘
                 │ TUN packets
┌────────────────▼────────────────────────────────────────────┐
│  Android VpnService / TUN Device                             │
│  Routes fd00::/8 mesh traffic, intercepts .fips/.nsite DNS  │
└─────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| **AppRuntime** | Holds device identity, embeds FIPS node, drives Tokio runtime with tasks | `myco-core/src/runtime.rs` |
| **AppState** | Immutable view snapshot: identity, node status, BLE, peer lists, content status | `myco-core/src/state.rs` |
| **NativeAppAction** | Redux actions dispatched from Kotlin: lifecycle, BLE toggle, nsite open | `myco-core/src/action.rs` |
| **Content** | Content layer orchestrator: wires relay, Blossom, gateway; manages Library & Circle | `myco-core/src/content.rs` (102KB) |
| **PeerRelayPool** | Persistent per-peer WebSocket connections to mesh relays; fan-out + pull | `myco-core/src/peer_relay.rs` |
| **MeshGossiper** | Subscribes local relay + peer relays, publishes events onward (propagation) | `myco-core/src/gossip.rs` |
| **DnsIntercept** | Intercepts `.fips` DNS queries via TUN, resolves to mesh IPv6 addresses | `myco-core/src/dns_intercept.rs` |
| **nsite-deck** | Transport-agnostic nsite host: gateway engine, sync/import, propagator (P3) | `nsite-deck/src/` |
| **myco-relay** | Embedded Nostr relay: event store + WebSocket server (NIP-01) | `myco-relay/src/` |
| **myco-blossom** | Embedded Blossom blob store: SHA256-addressed file store + HTTP server | `myco-blossom/src/` |
| **AndroidBleIo** | JNI bridge to Android BLE radio; called by FIPS for L2CAP I/O | `myco-core/src/ble_bridge_jni.rs` |
| **TunBridge** | Reads/writes IPv6 packets from VpnService TUN device | `myco-core/src/tun_bridge.rs` |

## Pattern Overview

**Overall:** Redux-style reducer with async spawned tasks. A single `AppRuntime` holds mutable state; Kotlin calls `dispatch(actionJson)` and polls `state()`. Actions are processed synchronously, spawning background work (relay sync, peer discovery) that updates state. The next `Tick` or `state()` poll reads the updated view.

**Key Characteristics:**
- Single-threaded Kotlin ↔ multi-threaded Tokio via mutex-wrapped `AppRuntime`
- Trait-seamed layers: `nsite-deck` consumes storage (`RelayBackend`, `BlobStore`) and transport (`PeerSource`, `FanoutSink`) through abstractions
- Lock-free peer state reads via FIPS's `ControlReadHandle`; writes to peer relays go through the `PeerRelayPool` actor
- **Spawn-not-block:** All async work (relay sync, peer discovery, nsite sync) spawns on Tokio and writes to shared state; Kotlin never waits
- Persistent identity: one Nostr keypair per device, persisted in app data dir

## Layers

**Layer 1: Android UI**
- Purpose: User-facing manager for Library (installed nsites), Pair (QR pairing), Discover (nsites on Circle peers), Settings
- Location: `android/app/src/main/kotlin/...` (not in this repo; Kotlin frontend)
- Contains: Jetpack Compose UI, per-nsite WebView launch, JNI call site
- Depends on: `libmyco_core.so` (FFI)
- Used by: End user

**Layer 2: NsiteActivity WebView**
- Purpose: Render one nsite per task; load `http://<host>.nsite` with no chrome
- Location: Android app (not in repo)
- Contains: WebView setup, per-nsite origin isolation
- Depends on: Local gateway (`AppRuntime`)
- Used by: User navigating an app

**Layer 3: Local Gateway + DNS Interception**
- Purpose: Hostname-to-manifest routing; manifest → file serve
- Location: `nsite-deck/src/gateway.rs`, `myco-core/src/dns_intercept.rs`
- Contains: HTTP request parsing, manifest lookup, blob serving, loading page for incomplete sites
- Depends on: Relay (`RelayBackend`), Blossom (`BlobStore`)
- Used by: WebView; pulls from relay/Blossom on cache miss

**Layer 4: Embedded Relay + Blossom**
- Purpose: Local event store (Nostr relay) + blob store (Blossom)
- Location: `myco-relay/src/`, `myco-blossom/src/`
- Contains: In-memory/disk-backed event store, HTTP/WebSocket servers on `:4870` and `:24243`
- Depends on: Axum (web framework), socket2 (IPv6-only binding)
- Used by: nsite-deck gateway, gossiper, peer sync

**Layer 5: myco-core (App Crate)**
- Purpose: Wires nsite-deck + relay + Blossom; defines device identity; manages content sync; bridges FIPS
- Location: `myco-core/src/`
- Contains: `AppRuntime` (identity + Tokio + FIPS node), `Content` (gateway orchestrator), gossiper, peer relay pool, DNS intercept, JNI bindings
- Depends on: nsite-deck, myco-relay, myco-blossom, fips (embedded mesh)
- Used by: JNI/FFI from Kotlin

**Layer 6: FIPS Core + Transports**
- Purpose: Mesh routing, peer discovery, transport multiplexing
- Location: `reference/fips` (upstream, not modified)
- Contains: Node identity, BLE (L2CAP), UDP (LAN discovery + WiFi Aware), TCP, Tor, Noise handshakes, peer state machine
- Depends on: Nothing in this repo
- Used by: myco-core; feeds peer identity/reachability to Content layer

## Data Flow

### Primary Request Path: Load an nsite

1. **UI Layer** — User taps "open app" (Kotlin).
   - Calls `dispatch(OpenNsite { link, holder })` across JNI.
   
2. **myco-core Reducer** (`runtime.rs:dispatch`) — Process action synchronously.
   - Parses link into `SiteAddr` (host + optional dTag).
   - Spawns `Content::open_site()` on Tokio.
   - Returns updated `AppState` with `SiteStatus::syncing`.

3. **Sync Engine** (`content.rs:open_site`, `nsite-deck/sync.rs`) — Drive blob/event sync.
   - Query local relay for manifest event.
   - On cache miss: dial mesh peers (or public relay if online fallback enabled).
   - Use `PeerSource` to fetch missing blobs from peer Blossom or pull from public Blossom (HTTPS).
   - Store fetched events/blobs locally.
   - Update `SiteStatus` to `ready` when all blobs present.

4. **Kotlin Poll** — `Tick` action triggers `state()` call.
   - Reads `SiteStatus::ready` from `AppState`.
   - Launches `NsiteActivity` (fullscreen WebView).

5. **WebView** — Navigates to `http://mycobiome.nsite` (localhost).
   - Kernel DNS intercepts `.nsite`, returns `127.0.0.1` (via `dns_intercept.rs`).
   - HTTP GET routed to local gateway `:4870` (loopback).

6. **Gateway** (`nsite-deck/gateway.rs:serve`) — Resolve + serve.
   - Parse hostname to pubkey + dTag.
   - Query relay for manifest event (now cached, hits in-memory store).
   - Parse manifest → find `/index.html` entry.
   - Look up blob SHA256 in Blossom (cached locally).
   - Return with appropriate `Content-Type`.

7. **WebView Renders** — Loads HTML, fetches subresources (CSS, JS, etc.) via gateway.
   - Each request: parse hostname → manifest → blob path → serve.

### Secondary Flow: Circle Peer Relay Subscriptions (Keepwarm)

1. **Runtime Init** (`runtime.rs:try_new`) — Spawn keepwarm task.
   - Tick every 8 seconds.
   - Read peer list from FIPS node's `ControlReadHandle`.
   - For each reachable Circle member (connected over mesh):
     - Dial their mesh relay (`ws://<npub>.fips:4870`).
     - Subscribe to their manifest events (kind 15128/35128).
     - Fan in-app send events to their relay (gossip).

2. **PeerRelayPool** (`peer_relay.rs`) — Persistent per-peer WebSocket.
   - One socket per peer, multiplexed via NIP-01 subscription IDs.
   - Handles `send` (fire-and-forget EVENT) and `request` (REQ + collect until EOSE).
   - Ping keepalive; detect half-open connection via timeout on pong.
   - Backoff on dial failure (8s → 180s cap).

3. **Gossiper** (`gossip.rs`) — Subscribe + forward.
   - Subscribe to local relay for user's own events (chat messages, etc.).
   - On new event: fan to all connected peers via `PeerRelayPool::send`.
   - Also subscribes to peer relays and stores events locally.

### Content Sync Flow: Staged Updates

1. **CheckNsiteUpdates Action** — User triggers "check for updates".
   - Spawn query to public relays (HTTPS via reqwest) for latest manifest.
   - Compare timestamps.
   - If newer: spawn `SyncOutcome::Staging` (download to separate bucket).

2. **Update Store State** — Write to `SiteStatus::update_available`.
   - UI shows "update ready" badge.
   - User taps "apply".
   - Move staged blobs to active location.
   - Bump manifest event in relay.

**State Management:**
- Mutable: `AppRuntime::content` (Arc), `SiteStatus` entries in `sites` map.
- Read-only: `FIPS::ControlReadHandle` (lock-free peer view).
- Shared: Relay store, Blossom store (both thread-safe).

## Key Abstractions

**RelayBackend** (trait in `nsite-deck::seams`)
- Purpose: Store/query Nostr events (manifest, chat, pairing handshake).
- Implementation: `myco-relay::RelayStore` (in-memory + disk persistence).
- Used by: nsite-deck gateway (fetch manifest), gossiper (subscribe + publish), pairing (store handshake events).

**BlobStore** (trait in `nsite-deck::seams`)
- Purpose: Content-addressed blob store (SHA256 → bytes).
- Implementation: `myco-blossom::FsBlobStore` (filesystem under app data dir).
- Used by: nsite-deck gateway (serve blobs), sync engine (store fetched files).

**PeerSource** (trait in `nsite-deck::seams`)
- Purpose: Fetch manifest + blobs from a peer over the mesh.
- Implementation: `IpPeerSource` in `myco-core` (HTTPS to public Blossom; async HTTP fallback for P2).
- Used by: Sync engine when local relay misses a manifest or blobs.

**FanoutSink** (trait in `nsite-deck::seams`)
- Purpose: Publish an event to connected peers (propagation plane).
- Implementation: `MeshGossiper` wraps `PeerRelayPool` + local relay (P3 feature, P2 is no-op stub).
- Used by: nsite-deck propagator (eager refresh of pinned sites).

## Entry Points

**JNI Entry** (`myco-core/src/jni_abi.rs`)
- Location: Java method `myco_core.AppRuntime.dispatch(String actionJson) -> String stateJson`
- Triggers: Kotlin UI dispatches action, receives serialized `AppState`.
- Responsibilities: Lock `AppRuntime`, call `dispatch()`, serialize state, unlock.

**FFI Contract** (`dispatch` method in `runtime.rs`)
- Deserializes action JSON.
- Processes `NativeAppAction` enum (GetState, StartNode, OpenNsite, etc.).
- Returns `AppState` as JSON (via serde).
- Never blocks: all async work spawned on Tokio.

**Tokio Runtime Tasks**
- `node.start()` spawned in `StartNode` — runs FIPS transport loops.
- `run_rx_loop()` spawned in `StartNode` — reads FIPS UDP/BLE packets, pumps peer state.
- `keepwarm_tick()` spawned in `try_new` — subscribes to Circle peers every 8s.
- `mesh relay server` spawned in `try_new` — serves `:4870` to peers.
- `loopback relay server` spawned in `try_new` — serves `127.0.0.1:4870` to WebView.
- `Blossom server` spawned in `try_new` — serves `:24243` to peers.

## Architectural Constraints

- **Threading:** Multi-threaded Tokio runtime; FIPS spawns its own tasks (BLE accept/scan, Noise handshakes). All long-lived work runs on `rt`'s worker threads; Kotlin JNI calls never block.
- **Global state:** 
  - `AppRuntime` wrapped in `Mutex` (JNI lock).
  - `Content` layer (`Arc<Content>`) shared across all spawned tasks; internal state is `Mutex<…>` (library, circle, sites map).
  - Peer relay pool holds `Arc<Mutex<HashMap<String, DialBackoff>>>` per-peer dial state.
  - FIPS node's peer state accessed via lock-free `ControlReadHandle`.
- **Circular imports:** None (Rust crates are acyclic: nsite-deck → seams, myco-core → nsite-deck/relay/blossom, fips is external).
- **Origin isolation:** Per-nsite WebView is a separate task/origin (`<host>.nsite`); WebView storage/cookies partition automatically.
- **Concurrency model:** All async via Tokio; no raw threads in myco-core (FIPS may spawn threads for transports).

## Anti-Patterns

### Connection Pooling Without Read Detection

**What happens:** Previous peer relay implementation held one socket per peer for send only, never reading inbound frames.
**Why it's wrong:** After a mesh flap, a pre-flap socket stays half-open. Writes buffer for minutes (OS retransmit horizon); fan-out vanishes into the black hole while the mesh looks healthy.
**Do this instead:** Always read the socket (use `select!` on read half + keepalive ping). A silent half-open is surfaced as a peer close/error or ping timeout; drop the task, reconnect on next send. See `myco-core/src/peer_relay.rs:run()`.

### Blocking the FFI Thread

**What happens:** An action handler waits synchronously on async work (e.g., relay query, peer dial).
**Why it's wrong:** Blocks the JNI caller; Kotlin hangs; UI freezes.
**Do this instead:** Spawn async work (`tokio::spawn`), return immediately with partial state, let Kotlin poll for progress via `Tick`. See `dispatch` in `runtime.rs`; all long operations are spawned, never awaited.

## Error Handling

**Strategy:** Fallback + user feedback.

**Patterns:**
- **Relay/peer unreachable:** Show "syncing" in UI; retry on next keepwarm tick or when peer reconnects.
- **Hash mismatch:** Reject blob in `sync_site`, mark site as unreachable (never serve corrupted content). See `nsite-deck/sync.rs:verify_and_store_event`.
- **Port collision:** Warn in `AppState::error` (e.g., "relay port 4870 unavailable"). UI shows banner to user.
- **No data dir:** Capture error at runtime startup in `AppState::error`; `Content` is `None`.
- **Pairing handshake timeout:** Recorded as outbound pair (waiting); retry on mesh reconnect.

## Cross-Cutting Concerns

**Logging:** Rust `tracing` crate; bridged to Android logcat via `paranoid-android` on Android; configurable via `RUST_LOG` env var (e.g., `RUST_LOG=myco=debug,fips=info`).

**Validation:**
- Event signatures: `nostr` crate handles NIP-01 Schnorr verification in relay store.
- Blob hashes: SHA256 computed during sync; compared against manifest hash; rejected if mismatch (offline correctness gate).
- Manifest complete-ness: Gateway checks all listed blobs present before serving; returns 503 "loading" page if incomplete.

**Authentication:**
- Device identity: One Nostr keypair (nsec) per device, persisted at startup.
- Pairing: Mutual handshake (pair-request + pair-accept events signed by device key); one-time secret from QR to prove scan.
- Circle access: Only paired peers can read/pull nsites from this device's relay/Blossom (CircleGate restricts mesh access).

---

*Architecture analysis: 2026-08-01*
