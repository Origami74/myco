# FFI Surface (Kotlin ↔ Rust)

The contract between the Kotlin shell and `libmyco_core.so`. It is a **JNI +
JSON-over-strings reducer**: Kotlin dispatches an action, Rust returns the whole
state. Beside the reducer are a few per-purpose entry points that need bytes or
blocking. Source of truth: [`action.rs`](../../myco-core/src/action.rs),
[`state.rs`](../../myco-core/src/state.rs), [`jni_abi.rs`](../../myco-core/src/jni_abi.rs)
(Android-only — host `cargo test` never compiles it), and
[`NativeCore.kt`](../../android/app/src/main/java/app/myco/core/NativeCore.kt).
This page is a map, not the contract; when they disagree, the code is right.

---

## Opaque handle

```kotlin
object NativeCore {
    external fun initializeAndroidContext(context: Context)   // once, before appNew
    external fun appNew(dataDir: String, appVersion: String): Long
    external fun appFree(handle: Long)
    external fun stateJson(handle: Long): String              // read (no rev bump)
    external fun refreshJson(handle: Long): String            // == dispatch Tick
    external fun dispatchJson(handle: Long, actionJson: String): String
}
```

`appNew` builds the `AppRuntime` — identity, content layer, relay and Blossom
servers, a Tokio runtime — and never panics: a failure lands in `state.error`.
One handle per process, held by `MycoCore`. JNI symbols are
`Java_app_myco_core_NativeCore_<name>`.

## The reducer

```
dispatch(actionJson) → stateJson
```

- Actions are internally tagged: `{"type": "snake_case", ...camelCaseFields}`.
- The state carries a monotonic **`rev`**; Kotlin skips a redraw when it has not
  moved. `GetState` does not bump it; everything else does.
- **Spawn, never block.** Anything that waits on the network or a peer is
  spawned on Tokio inside the reducer and its result lands in a later snapshot.
  Kotlin polls `stateJson` at 1 Hz and after each dispatch.

### Actions

| `type` | fields | what |
|---|---|---|
| `get_state` | — | Pure read; does not bump `rev`. |
| `tick` | — | Advance time-based work; bumps `rev`. |
| `start_node` | — | Start the embedded FIPS node (spawns its transport loops). |
| `stop_node` | — | Stop the embedded FIPS node. |
| `set_ble_enabled` | `enabled`: bool | Master switch for the BLE L2CAP transport. |
| `set_wifi_aware_enabled` | `enabled`: bool | Master switch for the Wi-Fi Aware bulk lane. |
| `open_nsite` | `link`: String, `holder`: Option<String> | Resolve a pasted nsite link / `<host>` and drive its sync to readiness (author-signed manifest + its blobs). |
| `import_nsite` | `dir`: String | DEV-ONLY side-load: import an already-signed manifest + blobs from a bundle directory (`<dir>/manifest.json` + `<dir>/blobs/<sha256>`). |
| `add_to_library` | `link`: String | Pin a site to the Library (exempt from eviction; eviction itself is P5). |
| `remove_from_library` | `link`: String | Unpin a site from the Library. |
| `forget_nsite` | `link`: String | Forget a single nsite: remove it from the Library and the Apps grid. |
| `fetch_napplet` | `pointer`: String, `holder`: Option<String> | Fetch a napplet by `naddr` (or `<npub>:<dtag>`), verify it, and store it locally — D9's acquisition path, online once and mesh-replicable after. |
| `install_napplet` | `pointer`: String, `granted`: Vec<String> | Record what install review granted, and pin the napplet to the Library. |
| `forget_napplet` | `pointer`: String | Unpin a napplet and drop its grants. |
| `set_napplet_grant` | `pointer`: String, `domain`: String, `allowed`: bool | Allow or withdraw one capability for an installed napplet, from its sheet. |
| `set_napplet_mesh_reach` | `publishTtl`: u8, `subscribeTtl`: u8 | Cap how far a napplet may reach over the mesh (NAP-MESH): the most hops a `mesh.publish` and a `mesh.subscribe` backlog pull may ask for. |
| `dismiss_napplet_review` | — | Close the install-review screen without installing. |
| `check_nsite_updates` | — | Check online relays for newer versions of installed nsites and stage/apply them (`docs/design/nsite/nsite-updates.md`). |
| `search_nsites` | `query`: Option<String> | Discover nsites on connected Circle peers' relays ("nsites around me"): query each reachable member's mesh relay for kind 15128/35128 manifests. |
| `wipe_stores` | — | Clear the local relay + Blossom + Library + site status (dev/test reset). |
| `wipe_cache` | — | Clear cached relay events + Blossom blobs **except** those backing pinned nsites (Settings → Storage → "Delete cache"). |
| `add_to_circle` | `npub`: String, `name`: String | Add a paired peer to the **Circle**: the contact list of devices we pull nsites from over the mesh. |
| `remove_from_circle` | `npub`: String | Forget a peer (remove from the Circle). |
| `send_pair_request` | `npub`: String, `name`: String, `secret`: String | Scanned a peer's pairing QR: send them a signed pair request over the mesh (to their relay). |
| `accept_pair_request` | `npub`: String, `name`: String | Accept an incoming pair request: add the requester to the Circle and signal them (a pair-accept) so they add us back. |
| `decline_pair_request` | `npub`: String | Dismiss an incoming pair request without pairing. |
| `cancel_pair_invite` | `npub`: String | Withdraw an invite we sent that is still waiting, so it can be sent again. |
| `set_offline_only` | `enabled`: bool | Toggle "mesh-only": when enabled, never use the public IP relay/Blossom fallback — pull only over the mesh. |
| `set_custom_relay` | `url`: String | Point the event store at a **custom relay**, or back at the built-in one with an empty `url`. |
| `set_custom_blossom` | `url`: String | Point the blob store at a **custom Blossom server**, or back at the built-in one with an empty `url`. |
| `set_aware_data_paths` | `count`: u8 | Report how many concurrent Wi-Fi Aware data paths this chipset supports (`Characteristics.getNumberOfSupportedDataPaths()`), which is what the Aware UDP socket pool is sized to. |
| `set_device_name` | `name`: String | Set this device's human label (memorable name). |
| `speedtest_peer` | `npub`: String | Dev-menu speedtest against a mesh peer: PUT a fresh payload to the peer's Blossom and GET it back, timing each leg. |
| `share_file` | `path`: String, `name`: String, `mime`: String, `peerNpub`: String | Encrypt a local file and send a private offer to a Circle peer. |
| `accept_file_transfer` | `transferId`: String | Accept an incoming encrypted file offer. |
| `decline_file_transfer` | `transferId`: String | Decline an incoming encrypted file offer. |
| `cancel_file_transfer` | `transferId`: String | Cancel a transfer that is still in flight and tell the other phone, so neither side is left waiting on a message that is no longer coming. |
| `forget_file_transfer` | `transferId`: String | Forget a finished transfer after the Android side has safely published the received file (or after a terminal sender-side outcome). |

`Option<String>` fields are omitted when absent. `link` for an nsite is a
pasted link or a bare `<host>` label; `pointer` for a napplet is an `naddr…` or
`<npub>:<dtag>`.

### State

`AppState` in [`state.rs`](../../myco-core/src/state.rs), `camelCase`. The
big fields, by layer:

| Field | Layer | What |
| --- | --- | --- |
| `rev`, `error`, `appVersion`, `multipathCore` | — | bookkeeping; `error` is empty when healthy |
| `identity` | 4 | `ownNpub`, `ownPubkeyHex`, `nodeAddrHex`, `fipsAddr`, the mesh ULA |
| `node`, `ble`, `bleAdverts`, `blePeers`, `wifiAware`, `peers` | 4 | node status; per-lane radio status; the merged per-peer diagnostics rows (state, transport, every multi-path link, RTT, attempts) |
| `circle`, `reachableNpubs`, `pendingPairRequests`, `outboundPairs` | 2 | the Circle; members with a live relay connection right now; incoming requests awaiting an answer; invites waiting |
| `sites`, `library`, `discovered`, `updateCheck` | 1 | per-nsite sync state (`syncing` / `ready` / `unreachable` / `incomplete`, files pulled/total, staged update); every installed app with `kind`, `granted`, `pointer`; "around me" results |
| `nappletReview`, `nappletDomains`, `nappletMeshReach` | 1 | a fetched napplet awaiting install review (loading / requires / grants / error / holder); every grantable NAP; the user's mesh caps |
| `cache`, `relayBackend`, `blobBackend`, `pendingRelayUrl`, `pendingBlossomUrl`, `offlineOnly` | 3 | store counts; custom backends and their health |
| `fileTransfers`, `speedtest` | 2 / dev | native file sharing; the Dev speedtest |

---

## Beside the reducer

### The gateway

```kotlin
external fun gatewayGet(handle, host, path, range, allowSync): ByteArray
```

Called from `NsiteActivity`'s `shouldInterceptRequest` for every request a
site's WebView makes. Returns `[u32 BE header-len][header JSON][body]`; the
header is `{status, contentType, headers}`. Blocks while the in-process
gateway serves from the local relay and Blossom — it runs on the WebView's
worker thread, never the UI thread.

### The napplet channel

```kotlin
external fun nappletShellPage(): String        // the trusted shell HTML, from the APK
external fun nappletRuntimeObject(): String    // the name the shell's channel object is injected as
external fun nappletOpen(handle, pointer): String              // resolve + verify → {ok, sessionId, shellHost, title, error}
external fun nappletFrame(handle, sessionId, frameJson): String     // one frame in, JSON array of frames out
external fun nappletNextFrames(handle, sessionId, timeoutMs): String // long poll for pushed frames; BLOCKS
external fun nappletClose(handle, sessionId)
```

`nappletOpen` takes no grant list: grants are read from the library on the Rust
side, so an intent that starts `NappletActivity` cannot hand a napplet
capabilities the user never approved. Frames are `{channel: "shell" | "napplet"
| "relaunch", ...}`; `NappletActivity` drives them off the main thread — one at
a time until `shell.init` has answered, concurrently after — and posts replies
to the shell. A `relaunch` frame recreates the activity.

### The BLE byte bridge

Kotlin owns the radio (`BleService`: scanning, advertising, L2CAP CoC
sockets); Rust owns the protocol. `bleBridgeNew(appHandle, radio)` hands Rust
a callback object; Rust asks it to connect/listen and Kotlin delivers bytes back
with `bleDeliverInbound`, `bleDeliverConnectResult`, `bleChannelDeliverRecv`,
`bleChannelClosed`, and scan/advert state with `bleDeliverScan`,
`bleDeliverAdvertName`, `bleDeliverScanningState`,
`bleDeliverAdvertisingState`. Outbound bytes are pulled with
`bleChannelNextSend` (blocking, per channel). Design:
[ble-interop.md](../design/fips/ble-interop.md).

### Platform peers (Wi-Fi Aware, LAN)

```kotlin
external fun awarePeerFound(npub, addr, lane)   // "aware" or "udp"
external fun awarePeerLost(npub, lane)
external fun awareSetDiscovering(on)
external fun nextUdpTransportFd(lane, sinceVersion, timeoutMs): Long  // the node's socket, for Kotlin to bind to the Aware network
```

No byte bridge: fips's own UDP transport dials the address Kotlin pushes.
Design: [wifi-aware-interop.md](../design/fips/wifi-aware-interop.md),
[ap-lane.md](../design/fips/ap-lane.md).

### The TUN

```kotlin
external fun tunSendPacket(packet, len): Boolean   // device → mesh
external fun tunNextPacket(out, timeoutMs): Int    // mesh → device; BLOCKS
external fun setUpstreamDns(servers)               // where non-.fips queries go
```

The `VpnService` (scoped to Myco's uid, routing `fd00::/8`) pumps packets both
ways on its own threads. Design: [ports.md](./ports.md) §4.

---

## Rules that hold everywhere

- Kotlin never waits on the mesh inside a reducer call.
- Anything that **blocks** says so in its Kotlin doc and is called off the main
  thread: `nappletNextFrames`, `bleChannelNextSend`, `tunNextPacket`,
  `nextUdpTransportFd`, `gatewayGet`.
- Grants cross the boundary in exactly one direction: from the user, through
  `install_napplet` / `set_napplet_grant`, into the library. Nothing Kotlin
  passes at open time can widen them.
