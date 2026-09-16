# System Architecture

The structure of **Myco** on one phone: four layers, five Rust crates in one
`.so`, a Kotlin shell, and the boundary between them. Vocabulary is in
[concepts.md](./concepts.md); the visual companion is
[diagrams/01-system-layering.svg](../diagrams/01-system-layering.svg).

---

## Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│ Kotlin                                                               │
│  MainActivity (Compose: Apps · Circle · Discover · Settings · Dev)   │
│  NsiteActivity ──── one WebView per nsite, its own task              │
│  NappletActivity ── one WebView per napplet: shell page + sandboxed  │
│                     srcdoc iframe, capability channel to Rust        │
│  radios: ble/ · aware/ · ap/ · nfc/     vpn/ (the TUN)     share/    │
├──────────────────────── JNI · JSON reducer ──────────────────────────┤
│ Rust — libmyco_core.so                                               │
│                                                                      │
│  1 Apps        nsite-deck (gateway, sync)  myco-napplet-runtime      │
│                                            (resolve, sandbox, NAPs)  │
│  2 Circle      content.rs: Circle, pairing, the gate; auth_service   │
│  3 Relay/Blob  myco-relay · myco-blossom · mesh_relay (proxy, hub)   │
│                gossip · peer_relay (pool) · outbox lanes             │
│  4 FIPS        the fips node · lanes: BLE bridge, Aware/LAN UDP,     │
│                TCP · TUN bridge · .fips DNS · control socket         │
└──────────────────────────────────────────────────────────────────────┘
```

Each layer talks only to the one below it. Layer 1 never names a radio; layer
4 never knows what an event is.

---

## Crate workspace

| Crate | Layer | Role | Reaches the world through |
| --- | --- | --- | --- |
| `myco-core` | all | the app crate and only cdylib: wires everything, owns identity, embeds the fips node, the JNI surface | — (it *is* the wiring) |
| `myco-napplet-runtime` | 1 | the napplet host: NIP-5D manifest, verified resolve, the `srcdoc` artifact, the session and grants, one module per NAP | `Signer`, `EventSink`, `MeshSink`, `OutboxResolver`, `LaneTransport`, `BlobFetcher`, `NapTransport` |
| `nsite-deck` | 1 | the nsite host: gateway (manifest → path → sha256 → serve), sync/import, propagator; the NIP-5A primitives napplets reuse | `RelayBackend`, `BlobStore`, `PeerSource`, `FanoutSink` |
| `myco-relay` | 3 | embedded NIP-01 relay store (durable events in LMDB via `nostr-lmdb`; expiring chat memory-only) | implements `RelayBackend` |
| `myco-blossom` | 3 | embedded Blossom store: sha256-named files, hash verified on write | implements `BlobStore` |

The two layer-1 crates are **Android-free and transport-agnostic**. Every
concrete thing — the relay in use, the radio, the WebView — arrives through a
trait implemented in `myco-core`, which is why host `cargo test` exercises them
with in-memory seams and no phone. The storage seams are swappable by the user:
a custom relay or Blossom (Settings › Storage) is an alternate `RelayBackend` /
`BlobStore` reached over plain NIP-01 / HTTP (`remote_backend.rs`,
`remote_blobs.rs`).

---

## The layers

### 1 — Apps

**nsite.** `NsiteActivity` loads `http://<host>.localhost/` and answers every
request from `shouldInterceptRequest` → `gatewayGet` → the gateway in
`nsite-deck`, which resolves host → manifest → path → sha256 → bytes from the
local stores. No socket, no DNS, no TUN. A missing or incomplete site is synced
from a holder first (layer 2 says which) and served when whole.

**napplet.** `NappletActivity` loads a trusted shell page from the APK, which
mounts the verified napplet in a `sandbox="allow-scripts"` `srcdoc` iframe. The
napplet's only way out is `postMessage` to the shell, which relays each frame to
Rust over `addWebMessageListener` scoped to the shell's origin. Rust holds one
`Session` per window — identity, grants, subscriptions — and `dispatch` routes
each `domain.action` envelope to a NAP handler after checking the grant on
every call. Deliveries the other way (subscription events, relaunch) queue per
window and are drained by a long poll. Design:
[../napplet/napplet-runtime.md](../napplet/napplet-runtime.md).

**Grants.** What a napplet may do is decided at install review and adjustable
on its Manage permissions sheet; both write `granted` on the library entry, and
nothing else does. A live window is told to relaunch when its grants change.

### 2 — Circles and pairing

`content.rs` holds the Circle (`circle.json`), the outbound-invite ledger, the
issued-secret ledger and the per-peer permissions. `auth_service.rs` is the
one port open to strangers (`:4873`): it takes signed pair requests, accepts and
removals. The **Circle gate** (`PeerGate`) is consulted by the relay proxy and
the Blossom server on every mesh connection; loopback bypasses it. Design:
[identity-pairing.md](./identity-pairing.md), [security.md](./security.md).

This layer also decides *where content comes from*: the holder named in a share,
then any reachable Circle member, then the internet — and, for napplets, which
relays an author's events are read from (NIP-65 lists over the three lanes).

### 3 — Relay and Blossom

`mesh_relay.rs` is a NIP-01 proxy in front of whatever `RelayBackend` is in
use. It owns the mesh behaviour the store must not know about: the `MESH`
envelope with its hop budget, the seen-set that makes flooding terminate, the
query-id set that makes pulls answer once, the live bus that wakes this phone's
subscriptions, and the gate. The same `RelayHub` serves two sockets — loopback
`127.0.0.1:4870` for WebViews and `[::]:4870` for the mesh — so a WebView and a
Circle member see one relay. `gossip.rs` implements the push plane (fan an
accepted event to Circle members with a decremented budget) and the pull plane
(forward a REQ with hops left). `peer_relay.rs` keeps one pooled WebSocket per
Circle member at `ws://<npub>.fips:4870`. Blossom is served on `[::]:24243`
behind the same gate. Design: [event-gossip.md](./event-gossip.md),
[../nsite/nsite-layer.md](../nsite/nsite-layer.md).

### 4 — FIPS

`myco-core` embeds a fips `Node` on a Tokio multi-thread runtime
(`runtime.rs`). Transports:

- **BLE** — Kotlin owns the radio (`BleService`, L2CAP CoC, per-peer PSM
  discovery); Rust owns the protocol through fips's `BleIo` seam, bridged by
  `ble_bridge_jni.rs` as byte channels.
- **Wi-Fi Aware** and **LAN** — Kotlin raises the data path or finds the mDNS
  advert and pushes `(npub, addr)` into the platform peer queue; fips's UDP
  transport dials from its own socket.
- **TCP** — when the phone has internet.

Peers can hold several paths at once (multi-path core); fips sends on one and
keeps the others as standbys. The IPv6 side is an app-owned `VpnService` TUN
scoped to Myco's uid, routing `fd00::/8`; `.fips` DNS is answered by the node's
own resolver. Rust reads peer state through the node's control socket
(`control_client.rs`). Design: [../fips/](../fips/).

---

## The Kotlin ↔ Rust boundary

One opaque handle, one reducer: `dispatch(actionJson) → stateJson` with a
monotonic `rev`, over JNI as strings. Kotlin polls `stateJson` at 1 Hz and
after each dispatch. Long-running work is **spawned, never awaited** in the
reducer; results land in the next snapshot. Beside the reducer are the
per-purpose entry points that need bytes or blocking: the napplet channel
(`nappletOpen` / `nappletFrame` / `nappletNextFrames`), the gateway
(`gatewayGet`), the BLE byte bridge, the UDP socket hand-off, and the TUN packet
pump. All listed in [../../reference/ffi-surface.md](../../reference/ffi-surface.md).

Kotlin owns: the UI, the WebViews, the radios, NFC, the `VpnService`, notifications,
the share sheet. Rust owns: both identities, the Circle, the stores, the node,
the gossip, every capability a napplet gets.

---

## What is deliberately not here

- **No app authoring.** Myco never signs a manifest. Apps are built and
  published elsewhere; Myco holds and re-serves signed events.
- **No membership authority.** A Circle is a local list on each phone; there
  is no roster, admin, or join event. Two people pair; nobody approves it.
- **No exit node, no tunnel-all.** The TUN routes `fd00::/8` only, for Myco's
  uid only. (An experimental exit-node demo exists as a how-to; it is not in
  the product.)
- **No store-and-forward in FIPS.** Survival across partition is layer 3's,
  by every phone being a holder.
