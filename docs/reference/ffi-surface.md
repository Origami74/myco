# FFI Surface (Kotlin ↔ Rust)

This is the **proposed** Kotlin ↔ Rust FFI contract for Myco. It is modeled
directly on nostr-vpn's JNI/JSON reducer
([`../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs),
[`actions.rs`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/actions.rs),
[`state.rs`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/state.rs),
[`ffi.rs`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/ffi.rs)) and the Kotlin side
([`NativeCore.kt`](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/core/NativeCore.kt),
[`AppCoreClient.kt`](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/core/AppCoreClient.kt)).

> Design doc, not-yet-built app. The contract below is **proposed**; action /
> state field names are provisional. Open questions are marked **TBD / open**.

The model, taken from nostr-vpn: **JNI + JSON-over-strings, not UniFFI.** Kotlin
owns the UI and the BLE radio; Rust owns the relay + blossom + FIPS endpoint.
State flows one way (Rust → Kotlin as a JSON state snapshot); intent flows the
other way as a single `dispatch(actionJson) -> stateJson` reducer. A monotonic
`rev` counter lets the UI skip no-op redraws.

For the data model behind these fields see [concepts.md](../design/concepts.md);
for the BLE plumbing the radio actions drive, see
[identity-pairing.md](../design/identity-pairing.md) and
[propagation.md](../design/propagation.md).

---

## Opaque-handle lifecycle

nostr-vpn wraps a Tokio-runtime-backed app in a `Box`, hands Kotlin the raw
pointer as a `jlong`, and frees it on `close()`
([`c_abi.rs` `appNew`/`appFree`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs)). Myco
keeps this exactly.

```
NativeCore.appNew(dataDir, appVersion) : Long    // Box::into_raw -> opaque handle
NativeCore.stateJson(handle)           : String  // current snapshot, no side effects
NativeCore.refreshJson(handle)         : String  // == dispatch(Tick)
NativeCore.dispatchJson(handle, json)  : String  // reduce one action, return new snapshot
NativeCore.appFree(handle)                        // Box::from_raw -> drop
```

- `appNew` builds the runtime (relay + blossom + FIPS endpoint live behind it),
  returns `Box::into_raw(Box::new(handle)) as jlong`. Returns `0` on startup
  failure; the first `stateJson` then carries a non-empty `error` (mirrors
  nostr-vpn's `error_state`).
- The handle is **opaque**: Kotlin never dereferences it, only passes it back.
- A `Mutex<Runtime>` inside `FfiApp` serializes all calls
  ([`ffi.rs` `with_runtime`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/ffi.rs)); a poisoned lock
  is recovered and surfaced as an `error` string rather than aborting.
- `appFree` is `Box::from_raw` + drop; the Kotlin wrapper guards against
  double-free by zeroing its stored handle (`AppCoreClient.close()` pattern).
- All returned `*mut c_char` strings are freed with a `stringFree`
  export on the C-ABI path; on the JNI path the `jstring` is owned by the JVM.

### `initializeAndroidContext`

Before `appNew`, Kotlin passes the Android `Context` once so Rust can reach the
JavaVM (needed for the BLE bridge and ndk-context). This is nostr-vpn's
`initializeAndroidContext` JNI export, kept verbatim
([`c_abi.rs`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs)).

---

## The reducer: `dispatch(actionJson) -> stateJson + rev`

One entry point reduces all intent. Kotlin builds a JSON action object, calls
`dispatchJson`, and parses the returned state snapshot.

```kotlin
// Proposed AppCoreClient (cf. nostr-vpn AppCoreClient.kt)
class AppCoreClient(dataDir: String, appVersion: String) : AutoCloseable {
    private var handle = NativeCore.appNew(dataDir, appVersion)
    fun state(): AppState   = parse(NativeCore.stateJson(handle))
    fun refresh(): AppState = parse(NativeCore.refreshJson(handle))      // == dispatch(Tick)
    fun dispatch(a: JSONObject): AppState = parse(NativeCore.dispatchJson(handle, a.toString()))
    override fun close() { if (handle != 0L) { NativeCore.appFree(handle); handle = 0 } }
}
```

On the Rust side, `dispatchJson` deserializes the string into the action enum
and falls back to an `error`-stamped state on bad JSON (nostr-vpn's
`invalid native action JSON` path), so a malformed action never crashes the
runtime.

### `rev` (monotonic revision)

Every state snapshot carries a `rev: u64` that increments whenever the runtime
mutates. The UI keeps the last seen `rev` and skips recomposition when the new
snapshot's `rev` is unchanged. `GetState`/`stateJson` is pure (no mutation, no
`rev` bump); `Tick` and all mutating actions bump it. (nostr-vpn keeps `rev` on
`NativeAppRuntime`; the wire field is the JSON `rev`.)

### Polling

There is no Rust→Kotlin callback channel. Kotlin drives a periodic
`refresh()` (== `dispatch(Tick)`) on a UI-side timer to advance time-based
work (BLE scan results, manifest flood/replication, peer liveness) and pick up
the new snapshot — same as nostr-vpn's `Tick`. A push channel is **TBD/open**.

---

## Proposed action enum

Serialized internally tagged: `{"type": "snake_case", ...camelCaseFields}`,
matching nostr-vpn's `#[serde(tag = "type", rename_all = "snake_case",
rename_all_fields = "camelCase")]` on `NativeAppAction`. **Proposed; provisional.**

```rust
// Myco NativeAppAction — PROPOSED
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum NativeAppAction {
    // --- lifecycle / polling ---
    GetState,                               // pure read; no rev bump
    Tick,                                   // advance time-based work; == refresh()

    // --- node (relay + blossom + FIPS endpoint) ---
    StartNode,                              // start embedded relay/blossom/endpoint
    StopNode,

    // --- site entry / Library ---
    OpenNsite { author: String, dTag: Option<String> },      // resolve + sync a site (author-signed
                                                             // kind 15128/35128 + its blobs) from peers/relays
    AddNsite  { author: String, dTag: Option<String> },      // aka register a site of interest, triggers a sync
    ImportNsite { manifest: String },        // DEV-ONLY: side-load an already-signed manifest + blobs created elsewhere
    AddToLibrary    { author: String, dTag: Option<String> },   // pin a site to the Library
    RemoveFromLibrary { author: String, dTag: Option<String> },

    // --- peers ---
    Pair    { npub: String, name: Option<String> },   // name = memorable handle (colour + name)
    Unpair { npub: String },

    // --- discovery ---
    SearchNsites { query: Option<String> }, // query reachable relays for kind 15128/35128

    // --- BLE radio ---
    SetBleEnabled { enabled: bool },        // master switch for the L2CAP transport

    // --- settings (single patch action, cf. UpdateSettings) ---
    UpdateSettings { patch: SettingsPatch },
}
```

Notes:

- `dTag` distinguishes a **named** site (kind 35128, parameterized-replaceable)
  from a **root** site (kind 15128, `dTag = None`). Library identity is
  `author + dTag` (matches the search dedup key). See
  [concepts.md](../design/concepts.md).
- The app **never authors nsites** — it never signs or publishes events on an
  author's behalf. A site enters a device only by **syncing** it (`OpenNsite` /
  `AddNsite` pull the author-signed manifest + blobs from peers/relays) or, for
  development, by **side-loading** externally-created artifacts (`ImportNsite`).
- `ImportNsite.manifest` is **TBD/open** and **dev-only** — it takes an
  already-signed manifest event (kind 15128/35128) plus its content-addressed
  blobs, produced by external nsite tooling; the app only stores and re-serves
  them. It is **not** an authoring path (no key, no signing).
- BLE **role is fixed to central** on Android (see
  [config.md § `[ble]`](./config.md#ble)); there is no role action in v1.
- A Kotlin `NativeActions` helper object builds these JSON objects (cf.
  nostr-vpn `NativeActions`), e.g.
  `NativeActions.pair(npub, name) = action("pair", "npub" to npub, "name" to name)`.

### Settings patch

`UpdateSettings { patch }` carries an all-`Option` struct so the UI sends only
changed fields (cf. nostr-vpn `SettingsPatch`). Stripped to Myco scope; each
field maps to a key in [config.md](./config.md). **Proposed; provisional.**

```rust
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub alias:            Option<String>,   // <alias>.fips label
    pub relay_port:       Option<u16>,
    pub blossom_port:     Option<u16>,
    pub autostart:        Option<bool>,
    pub cache_cap_bytes:  Option<u64>,
    pub eviction:         Option<String>,   // "lru" (only value in v1)
    pub pin_library_items:   Option<bool>,
    pub ble_enabled:      Option<bool>,
    pub announce_ttl:     Option<u8>,
    pub tun_enabled:      Option<bool>,
    pub intercept_fips:   Option<bool>,
    pub intercept_nsite:  Option<bool>,
}
```

(`SetBleEnabled` is kept as a discrete action because the radio toggle is a
hot-path UI control; it is equivalent to `UpdateSettings { ble_enabled }`.)

---

## Proposed state shape

One JSON object per snapshot, `#[serde(rename_all = "camelCase")]` (cf.
nostr-vpn `UiState`). Includes `rev` and an `error` string (empty when healthy).
**Proposed; provisional.**

```rust
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub rev: u64,
    pub error: String,                 // empty when healthy
    pub app_version: String,

    // --- identity (derived forms shown for the UI) ---
    pub identity: Identity,            // ownNpub, ownPubkeyHex, nodeAddrHex, fipsAddr, alias

    // --- node status ---
    pub node: NodeStatus,              // running, relayPort/relayStatus, blossomPort/blossomStatus,
                                       // meshReady, fipsAddr ("<npub>.fips")
    // --- content ---
    pub library:             Vec<LibraryItem>,        // pinned sites
    pub discovered_nsites: Vec<DiscoveredNsite>, // search results

    // --- peers ---
    pub paired_peers: Vec<PairedPeer>, // npub + memorable name + reachability
    pub ble_peers:    Vec<BlePeer>,    // peers seen/connected over the radio
    pub ble: BleStatus,                // enabled, role, scanning, adapterName
    pub cache: CacheStatus,            // capBytes, usedBytes, itemCount, pinnedCount
}

#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub own_npub: String,
    pub own_pubkey_hex: String,
    pub node_addr_hex: String,   // SHA256(npub)[0:16]
    pub fips_addr: String,       // <npub>.fips
    pub alias: String,           // this device's own <alias>.fips label (NOT a peer's memorable name)
}

#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub running: bool,
    pub relay_port: u16,
    pub relay_status: String,    // "stopped" | "listening" | "error"
    pub blossom_port: u16,
    pub blossom_status: String,
    pub mesh_ready: bool,
    pub status_text: String,
}

#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub author: String,          // npub of the site author
    pub d_tag: Option<String>,   // None = root site (15128); Some = named (35128)
    pub title: String,
    pub url_host: String,        // npub1… (root) or <pubkeyB36><dTag> (named)
    pub pinned: bool,
    pub last_updated: u64,
}

#[serde(rename_all = "camelCase")]
pub struct DiscoveredNsite {
    pub author: String,
    pub d_tag: Option<String>,
    pub title: String,
    pub created_at: u64,
    pub source: String,          // "relay" | "ble" | "cache"
    pub in_library: bool,
}

#[serde(rename_all = "camelCase")]
pub struct PairedPeer {
    pub npub: String,
    pub name: String,            // memorable name (colour + name)
    pub reachable: bool,         // currently reachable over mesh/BLE
    pub last_seen_text: String,
}

#[serde(rename_all = "camelCase")]
pub struct BlePeer {
    pub node_addr_hex: String,   // identity from the in-band pubkey exchange, NOT MAC
    pub npub: String,            // resolved once the Noise/pubkey handshake completes
    pub connected: bool,
    pub psm: u16,                // learned from adverts (addr->PSM map)
    pub rssi: Option<i32>,
}

#[serde(rename_all = "camelCase")]
pub struct BleStatus {
    pub enabled: bool,
    pub role: String,            // "central" (fixed on Android in v1)
    pub scanning: bool,
    pub adapter_name: String,
}

#[serde(rename_all = "camelCase")]
pub struct CacheStatus {
    pub cap_bytes: u64,
    pub used_bytes: u64,
    pub item_count: u64,
    pub pinned_count: u64,
}
```

`ble_peers` is identified by `node_addr` from the in-band pubkey exchange, never
by MAC — Android MAC randomization is therefore harmless (see
[identity-pairing.md](../design/identity-pairing.md) and fips-core's BLE
discovery [`../../reference/fips/src/transport/ble/discovery.rs`](../../reference/fips/src/transport/ble/discovery.rs)).

---

## The BLE bridge (Kotlin owns the radio)

Android BLE APIs are Java-only, so Kotlin owns the radio and hands raw bytes to
Rust — symmetric to how nostr-vpn's `MobileTunnel` exchanges raw packet bytes
across the FFI (`mobileTunnelSendPacket` / `mobileTunnelNextPacket` in
[`c_abi.rs`](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs)). For Myco the bytes are
L2CAP CoC stream/datagram payloads instead of TUN packets, wiring Kotlin's
radio into the native `AndroidBleIo` that implements fips-core's `BleIo` trait
([`../../reference/fips/src/transport/ble/io.rs`](../../reference/fips/src/transport/ble/io.rs)).

This byte-bridge is **separate** from the reducer above and is **TBD/open** in
detail (exact JNI signatures for accept/connect/send/recv, advert callbacks, the
`addr->PSM` learn path). FIPS owns all connection tracking, the pool, the
cross-probe tiebreaker, Noise, and reconnect; Kotlin only moves bytes and
surfaces adverts. The reducer's `SetBleEnabled` and the `ble`/`blePeers` state
are the **control/observation** plane over that byte plane.

---

## Build path

Same toolchain as nostr-vpn:

- Crate type `cdylib`, cross-compiled with **cargo-ndk** for `arm64-v8a`
  (arm64-only, minSdk 29 for L2CAP) into
  `android/app/src/main/jniLibs/arm64-v8a/lib*.so`.
- Loaded with `System.loadLibrary(...)` in a `NativeCore` object that declares
  the `external fun` JNI bindings (cf.
  [`NativeCore.kt`](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/core/NativeCore.kt)).
  Proposed library name `fips_pop_core` → `libfips_pop_core.so`.
- The single `.so` is the whole native surface: relay + blossom + FIPS endpoint
  (the `myco-core` Rust crate, one FFI surface).

```
cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release
# -> android/app/src/main/jniLibs/arm64-v8a/libfips_pop_core.so
```

JNI export naming follows the `Java_<pkg>_core_NativeCore_<fn>` convention;
with package id `app.myco` (placeholder) and a `core` subpackage the prefix
is `Java_to_fips_pop_core_NativeCore_…` (cf. nostr-vpn's
`Java_org_nostrvpn_app_core_NativeCore_…`).

---

## Open questions

- **Push vs. poll.** Whether to add a Rust→Kotlin event channel (e.g. a blocking
  `nextEvent(handle, timeoutMs)` like `mobileTunnelNextPacket`) or stay
  poll-only via `Tick`. **TBD/open.**
- **`OpenNsite`/`ImportNsite` shape.** Exact inputs for triggering a sync vs.
  side-loading already-signed artifacts (dev-only). The app stores and re-serves
  externally-authored events/blobs; it never signs. **TBD/open.**
- **Blocking actions.** `SearchNsites` and `OpenNsite` (a sync) may be slow;
  whether they return immediately with a "pending" status that later `Tick`s
  resolve, or block the reducer. **TBD/open** (nostr-vpn's reducer is
  synchronous under the `Mutex`).
- **BLE byte-bridge signatures.** Exact JNI shape for the radio bridge
  (accept/connect/send/recv/advertise/scan) and how the `addr->PSM` map crosses
  the FFI. **TBD/open.**
- **State diffing.** Whether `rev` alone suffices or the UI needs per-section
  revisions to avoid re-parsing the whole snapshot on every `Tick`. **TBD/open.**
