# System Architecture

The system structure of **Myco**: a six-layer stack on a single Android
device, the FFI boundary between Kotlin and Rust, the role of the kept TUN, and
a provenance breakdown of what is reused versus net-new. For terminology
(npub / node_addr / fd00::, `.fips` vs `.nsite`, nsite, the embedded services,
"Pillars of Propagation"), read [concepts.md](./concepts.md) first.

The companion diagram for this doc is
[diagrams/01-system-layering.svg](./diagrams/01-system-layering.svg). Related
flows: pairing in [diagrams/02-pairing-transitive-discovery.svg](./diagrams/02-pairing-transitive-discovery.svg),
offline propagation in [diagrams/03-offline-propagation.svg](./diagrams/03-offline-propagation.svg),
and the browse lifecycle in [diagrams/04-nsite-browse-flow.svg](./diagrams/04-nsite-browse-flow.svg).

> Design doc for a not-yet-built app, written in proposal voice. Open questions
> are marked **TBD / open**. Where this doc and the diagram disagree, this doc is
> authoritative (the diagram predates two locked decisions; see the notes below).

---

## Overview

Myco runs entirely on one phone: an Android **manager app** over a single Rust
native library (`libmyco_core.so`) built from a small **Cargo workspace** —
`myco-core` (the app crate) on top of three reusable crates: `nsite-deck` (the
transport-agnostic nsite host: gateway + sync), `myco-relay` (a generic embedded
Nostr relay), and `myco-blossom` (a generic embedded Blossom blob store). Kotlin
owns the UI, the per-nsite WebView, the radio (BLE), and the `VpnService`/TUN;
Rust owns identity, the mesh, and the nsite services. They talk across a JNI/JSON
reducer boundary.

Myco itself is the manager — Library / Pair / Discover / Settings, with its own
homescreen icon — and each nsite launches as **its own fullscreen "app"** in a
separate task (a `NsiteActivity` WebView, no Myco chrome). See
[app-shell.md](./app-shell.md) for the launch model.

The single most important structural fact: **a WebView only ever loads
`http://<host>.nsite` (IPv4 localhost).** All cross-device fetching happens in
native code over `.fips`. The browser never resolves `.fips`. Everything else in
the stack exists to make that local page load resolvable from peer content.

The stack, top to bottom:

```
  1  Android UI            (Jetpack Compose) — Myco manager: Library, Pair, Discover, Settings
  2  NsiteActivity         (fullscreen WebView, one task per nsite) — loads http://<host>.nsite, no chrome
  3  Local gateway + DNS   — *.nsite → 127.0.0.1; resolve manifest; serve files
  4  Embedded relay+Blossom (Rust) — myco-relay :4870 / myco-blossom :24243 + S&F cache
  5  myco-core / FFI      (Rust) — wires nsite-deck + relay + Blossom; binds into FIPS
  6  FIPS core + transports (upstream fips crate)  — mesh, crypto, BLE/UDP/TCP/Tor
     ── plus ── Android VpnService/TUN: routes fd00::/8, intercepts .fips/.nsite
```

(The diagram collapses UI + browser into one band and shows six bands; this doc
splits the per-nsite WebView out to make the "loads localhost only" boundary
explicit.)

---

## Crate workspace

The native library is one `.so` built from **four crates**, split along **reuse
boundaries** — so the genuinely reusable piece (the nsite know-how) stays free of
any particular relay, blob store, or transport:

| Crate | Role | Reusable? |
| --- | --- | --- |
| `myco-core` | app crate: FIPS endpoint, `AndroidBleIo`, JNI/JSON FFI, and the wiring that picks which backends to plug in | no — Myco-specific |
| `nsite-deck` | the nsite host: gateway (manifest → path → sha256 → serve) + sync engine + the **propagator** (a process that subscribes to local + peer relays and publishes accepted events onward, and eager-refreshes pinned sites); impl-agnostic | **yes** — the reusable core |
| `myco-relay` | a generic embedded Nostr relay (event store + `ws://…:4870` server) | yes — any Nostr app |
| `myco-blossom` | a generic embedded Blossom blob store (content-addressed store + `http://…:24243` server) | yes — any Blossom need |

`nsite-deck` never names a concrete relay, blob store, or radio. It reaches
everything outside itself through **four trait seams**, which `myco-core` wires up:

- **storage seams** (provided by `myco-relay` / `myco-blossom`) — **`RelayBackend`**
  (store/query manifest events) and **`BlobStore`** (`get`/`has`/`put` blobs by
  sha256). **Citrine-forward is just an alternate `RelayBackend`.**
- **transport seams** (provided by `myco-core` over FIPS) — **`PeerSource`** (pull a
  manifest + blobs from a reachable peer) and **`FanoutSink`** (publish an event onward
  to connected peers). The `FanoutSink` is driven by the nsite-deck **propagator**, which
  *subscribes* to the relevant relays (local + connected peers) and *publishes* (`EVENT`)
  to peer relays — `myco-relay` is a plain NIP-01 store + socket and does no forwarding
  itself. The same propagator runs eager-pinned-refresh (see layer 4 / store-and-forward).

All four cross-compile into the single `libmyco_core.so`. Relay and Blossom sitting
**outside** `nsite-deck` is deliberate: a Nostr relay and a Blossom server are
generic primitives, independently useful (there is, notably, no good Android
Blossom app) — so they are their own crates, and `nsite-deck` consumes them through
traits rather than embedding them. `myco-core` is the only crate that names FIPS
*or* a concrete relay/Blossom.

---

## The six layers, top to bottom

### 1. Android UI (Jetpack Compose) — **net-new**

**Responsibilities.** Myco is the **manager app**, with its own homescreen icon.
Its UI is four surfaces: the **Library** (your installed nsites/apps), **Pair**
(the QR pairing screen — scan + show), **Discover** (your circle — "nsites around
me" / transitively-discovered sites), and **Settings** (identity, storage,
relay backend). Alongside these four is a **Developer UI** — a diagnostic surface,
distinct from Library / Pair / Discover / Settings, that consumes the existing FFI
`ble` / `blePeers` state to render BLE adapter/scanning status and, per peer,
`node_addr` / `npub` (once Noise completes) / connected / `psm` / `rssi`. To *manage*
an nsite — info, remove, re-pair, add-to-home-screen — you go to Myco. Myco does
**not** host the nsite itself: launching an nsite hands
off to a separate fullscreen `NsiteActivity` (layer 2). There is no Myco-imposed
browser chrome, no bottom bar, no URL bar. See [app-shell.md](./app-shell.md) for
the launch model, Recents behaviour, intents (`myco://app/<host>`), and
homescreen-pinning via `ShortcutManager` (Library is the source of truth for
"installed"; homescreen presence is a soft hint).

**Provenance.** Net-new. nostr-vpn's UI is network/roster/exit-node management —
none of that survives the strip. The **QR pairing** sub-feature is reused
(see layer 5 / FFI), but the screens around it are new.

### 2. NsiteActivity — fullscreen WebView per nsite — **net-new (thin)**

**Responsibilities.** Render an nsite. Each nsite launches as its **own fullscreen
"app"** — a `NsiteActivity` whose `WebView` fills the screen, with **no toolbar,
no URL bar, and no Myco chrome**. It loads exactly one kind of URL,
`http://<host>.nsite`, where `<host>` is `npub1…` (root site) or
`<pubkeyB36><dTag>` (named site). The activity is `documentLaunchMode="always"`
and launched with `FLAG_ACTIVITY_NEW_DOCUMENT`, so each nsite is its own task /
instance and its own card in Android Recents (title + favicon + colour via
`ActivityManager.TaskDescription`). Per-nsite **origin isolation is automatic**:
each nsite is its own origin (`<host>.nsite`), so WebView storage/cookies
partition per nsite. Refresh, in-app navigation, and pull-to-refresh are the
**nsite developer's** responsibility, implemented inside the nsite — Myco adds no
reload button and no back bar; Android Back walks the WebView history then
finishes the task. In v1 the content is **pure static** (no privileged JS); a
capability API is a later milestone. See [app-shell.md](./app-shell.md).

**Provenance.** Net-new, but thin — it is a standard Android `WebView` pointed at
localhost. The "nsite-deck model" (browser loads localhost, native code does the
fetching) is the pattern ported from site-deck.

### 3. Local gateway + name resolution — **port (site-deck)**

**Responsibilities.** This is the localhost HTTP gateway and the `.nsite` name
resolution, ported from nsite-deck. In v0 it serves **directly from the local
relay + Blossom** — no derived serving cache. It:

1. answers `*.nsite` DNS with `127.0.0.1` (an A record),
2. receives the WebView's HTTP request, parses the left-most label into a pubkey
   (and optional `dTag`),
3. looks up the site manifest event from the **local relay** (layer 4),
4. on a cache miss, triggers a sync (layer 5 fetches over `.fips`), showing a
   loading page that polls until ready; before serving a site it checks that
   **all referenced blobs are present** (else it is still syncing),
5. resolves the requested path to a blob hash via the manifest's `path` tags
   (falling back to `index.html`, then `/404.html`), fetches that blob from the
   **local Blossom server**, verifies it against its sha256, and serves it with a
   content-type inferred from the path extension.

So v0 is per-request: manifest → `sha256` → blob → serve, with **no version
dirs, no atomic swap, and no sha→name file writing**. The htdocs serving cache
(writing blobs out as path-named files under a `current/` dir for fast static
serving) is **deferred to the roadmap** — a speed optimization, not needed for
v0. See [nsite-layer.md](./nsite-layer.md).

Because `.nsite` is IPv4/localhost, this works in any browser, including
Chromium. Only the relay+Blossom **sync** in step 4 ever touches `.fips`.

**Provenance.** Port from site-deck. The upstream gateway is Go; here it is
**re-implemented in Rust** inside `nsite-deck` (one language, one `.so`).
The HTTP listener may live in Kotlin or Rust — **TBD / open**; either way the
resolution logic ports from site-deck.

### 4. Embedded Nostr relay + Blossom server — **port (site-deck) → Rust**

**Responsibilities.** Hold the nsite data and serve it both to the local gateway
and to peers. **Both are embedded from day one** — they are simple, and there is
no good Android app to forward to:

- the **relay** (`ws://localhost:4870`) stores/serves signed manifest events
  (kinds 15128/35128). The relay *backend* is a **pluggable seam**: the default
  is the embedded relay; optionally it can **forward to a local relay app (e.g.
  Citrine)** for devs who already run one. Embedding is the default and earliest
  path,
- **Blossom** (`http://localhost:24243`) is **always embedded** — a small
  content-addressed HTTP store (`GET /<sha256>`, `PUT /upload`, `HEAD`). There is
  no good Android Blossom app to forward to, so embedding is the only sensible
  path.

This layer is also the **store-and-forward cache** that makes offline
propagation work: it retains *peers'* signed events and blobs and re-serves
them, so the device becomes a new source (a "relay-of-relays"). The
content-addressed **Blossom blob store** is what we *retain* — the
store-and-forward source, governed by an **LRU cache, default cap 2 GB**; sites
added to the Library are **pinned** (exempt). (The derived, path-named **htdocs**
serving cache of layer 3 is a separate, deferred store on top.)

**Provenance.** Port from site-deck (which embeds Khatru, a Go relay/Blossom).
Myco re-implements them in **Rust** as **two generic, reusable crates** —
`myco-relay` (event store + WS server, implementing `RelayBackend`) and
`myco-blossom` (content-addressed store + HTTP server, implementing `BlobStore`).
They sit **outside** the `nsite-deck` crate, which consumes them only through
those traits, so each is independently reusable (Citrine-forward is just an
alternate `RelayBackend`; there is no good Android Blossom app, which is exactly
why a standalone `myco-blossom` is worth having). See
[nsite-layer.md § The crate workspace](./nsite-layer.md). (The diagram tags this
band "PORT · lang TBD"; the language is now **locked to Rust** — treat that label
as stale.)

### 5. myco-core + the FFI boundary (Rust) — **reuse (nostr-vpn) + net-new glue**

**Responsibilities.** The **app crate** and the single native library. It is the
thin Myco-specific layer that wires everything together: it depends on
`nsite-deck` (the content layer) and on `myco-relay` + `myco-blossom` (the backend
impls it plugs into `nsite-deck`'s storage seams). `myco-core`:

- exposes the **JNI/JSON reducer** FFI to Kotlin (below),
- holds the device **identity** (the `nsec`) in `filesDir`,
- instantiates `myco-relay` + `myco-blossom` (or a Citrine-forward `RelayBackend`),
  wires them into `nsite-deck` as the `RelayBackend` / `BlobStore`, and **binds
  them into FIPS** so peers reach them at `<npub>.fips:4870` / `:24243` via FSP
  port multiplexing,
- drives the FIPS endpoint (layer 6) and implements `AndroidBleIo`,
- implements the **transport seams** the content + relay crates consume over FIPS:
  the **`PeerSource`** (fetch a manifest + blobs from a holder at `<npub>.fips`,
  verify each against its sha256, retain in the local relay + Blossom) and the
  **`FanoutSink`** (`EVENT` to each connected `<peer>.fips:4870`).

`myco-core`, `nsite-deck`, `myco-relay`, and `myco-blossom` cross-compile into one
`libmyco_core.so`.

**Provenance.** Reuse the nostr-vpn embedding *pattern* and FFI scaffolding;
net-new glue to wire `nsite-deck` to FIPS. Myco depends on the **canonical upstream
`fips` crate**, embedded in-process via `Node::new(Config)` — *not* nostr-vpn's
`FipsEndpoint::builder().without_system_tun()`, which is a nostr-vpn abstraction that
does not exist upstream. The "app owns the TUN, FIPS gets only packet bytes" contract
and custom-`BleIo` injection are two capabilities **not yet in upstream `fips`**; Myco
contributes them upstream (nostr-vpn's fork is the reference for what they do) and
carries them as a minimal local patch until merged. See
[build.md § 4c](../how-to/build.md) for the two gaps and the local-FIPS wiring.

#### The FFI contract (JNI / JSON reducer)

The boundary is **JNI + JSON-over-strings, not UniFFI** (carried over from
nostr-vpn's Android path):

- Rust is built as a **`cdylib`**, cross-compiled with **cargo-ndk** into
  `jniLibs/arm64-v8a/lib*.so`, loaded with `System.loadLibrary`.
- The contract is a **Redux-style reducer**: Kotlin calls
  `dispatch(actionJson) -> stateJson`, with a monotonic **rev** counter so the
  UI can tell stale state from fresh.
- An opaque `jlong` handle wraps the Rust-side **Tokio runtime**.

(See nostr-vpn `crates/nostr-vpn-app-core/src/c_abi.rs`, and the Kotlin side
`NativeCore.kt` / `AppCoreClient.kt` under
[reference/nostr-vpn/android](../../reference/nostr-vpn/android/).)

Two things cross the boundary *outside* the JSON reducer, because they are
byte/­radio paths Kotlin must own:

- **TUN packets** — Kotlin reads/writes the `VpnService` fd; raw IPv6 packet
  bytes pass to/from Rust (the **app-owned-TUN** contract — an upstream-`fips`
  capability Myco adds; see [build.md § 4c](../how-to/build.md)).
- **BLE bytes** — Android's BLE APIs are Java-only, so Kotlin owns the radio and
  hands raw L2CAP bytes to Rust's `AndroidBleIo` (see layer 6 / BLE).

### 6. FIPS core mesh + transports — **reuse (fips-core)**, BLE I/O **net-new**

**Responsibilities.** The mesh itself: identity derivation
(npub → node_addr → `fd00::/8`), `.fips` DNS, spanning-tree formation,
coordinate-based greedy routing, and two-layer Noise crypto (XK end-to-end +
IK hop-by-hop). FSP **port multiplexing** delivers mesh datagrams to the
localhost relay/Blossom ports, which is what exposes `<npub>.fips:4870`/`:24243`.

**Transports.** UDP / TCP / Tor are reused from fips-core unchanged. **BLE** is
the v1 transport and the one net-new transport piece:

- FIPS BLE is **L2CAP CoC** (SeqPacket), with a UUID-only advert (no identity
  material), a pre-handshake pubkey exchange of `[0x00][pubkey:32]` over the stream,
  then Noise IK. The cross-probe tiebreaker: the smaller node_addr's outbound
  connection wins. There is **no fixed well-known PSM**: PSM assignment is per-peer.
  `0x0085` (133) is only a **legacy default**, and fixed-`0x0085` Linux wire compat is
  **intentionally dropped**.
- The platform surface is upstream `fips`'s **`BleIo` trait**
  (`fips::transport::ble::io::BleIo`: `listen`/`connect`/`start_advertising`/
  `start_scanning`/`local_addr`/…), and the transport is generic over it
  (`BleTransport<I: BleIo>`). There are **three backends**: **`Bluer`** (Linux,
  upstream), **`AndroidBleIo`** (native L2CAP CoC), and **`BluestIo`** (macOS — the
  "bluest" CoreBluetooth crate, reusing the fips `macos-ble-rebased` branch under a
  `ble-macos` cargo feature; framed as the **dev/test** backend). Caveat: upstream
  `Node::new` currently **hardwires `BluerIo`**, so injecting a custom `BleIo` is one
  of the upstream-`fips` changes Myco needs (see [build.md § 4c](../how-to/build.md)).
  FIPS owns all connection tracking, the pool, the tiebreaker, Noise, and reconnect —
  **bitchat is not used as a transport.**
- **Universal per-peer PSM discovery.** Dynamic listener-PSM assignment is the general
  case, not an Android quirk: Android's `listenUsingInsecureL2capChannel()` and Apple's
  `CBPeripheralManager.publishL2CAPChannel` (→ an OS-assigned `CBL2CAPPSM`) both let the
  OS pick the listener PSM; BlueZ is the **outlier** that can bind a fixed PSM. So every
  node **advertises its own OS-assigned listener PSM** (via service-data and/or a readable
  GATT characteristic), and every dialer **reads the peer's PSM before `connect()`**. This
  is symmetric — there is no "Android must be central" constraint; the smaller-`node_addr`
  tiebreaker works normally. FIPS identifies peers by the pubkey exchange, not by MAC, so
  Android MAC randomization is harmless. Android requires **API 29+** (minSdk 29).

**Provenance.** Reuse fips-core for everything except the platform BLE backends:
`AndroidBleIo` is net-new, `BluestIo` (macOS, dev/test) reuses the fips
`macos-ble-rebased` work, and per-peer PSM advertise/discover is added across all
backends (it breaks the fixed `0x0085` default). bitchat-android is a
**pattern reference only** for the propagation layer — its transport (GATT-only,
wire-incompatible with FIPS L2CAP) and crypto are *not* used.

(See [reference/fips/src/transport/ble/](../../reference/fips/src/transport/ble/)
— `mod.rs`, `io.rs`, `discovery.rs`, `addr.rs` — and the upstream
[fips-architecture.md](../../reference/fips/docs/design/fips-architecture.md).)

> Diagram note: band 6 of
> [01-system-layering.svg](./diagrams/01-system-layering.svg) says "lift
> bitchat's GATT/connection/power managers as the Android BLE substrate." That
> reflects an earlier option and is **superseded**: the locked decision is
> option A (native `AndroidBleIo` over L2CAP), with bitchat as pattern reference
> only. Treat that diagram label as stale.

---

## The TUN's role

Myco keeps the Android **`VpnService`/TUN** from nostr-vpn but narrows it
sharply. It is **not** a tunnel-all-internet VPN.

- It routes only **`fd00::/8`** — the mesh ULA. It does **not** capture
  `0.0.0.0/0`; ordinary internet traffic is untouched.
- It DNS-intercepts **`*.fips`** (→ `fd00::` IPv6, AAAA-only) and **`*.nsite`**
  (→ `127.0.0.1`, A record), **system-wide** for every app on the phone.
  Non-`.fips`/non-`.nsite` queries are refused so the OS falls through to normal
  DNS.
- The app **owns** the TUN and hands FIPS only packet bytes; the TUN fd is
  read/written in Kotlin. This **app-owned-TUN** mode is a capability Myco adds to
  upstream `fips` (it always creates its own system TUN today) — nostr-vpn's
  `.without_system_tun()` is the reference; see [build.md § 4c](../how-to/build.md).

System-wide `.fips`/`.nsite` resolution is the entire reason the TUN survives the
strip: it is what makes "every app on the phone can resolve a mesh name" true,
which is the residual "VPN-like" property of Myco. What we dropped from
nostr-vpn's TUN usage is the *policy* on top — exit-node, WireGuard egress,
roster-gated routing, `.nvpn` MagicDNS — not the TUN itself.

---

## Reused vs net-new

Provenance for each band, matching the legend in
[01-system-layering.svg](./diagrams/01-system-layering.svg).

| Layer / component | Provenance | Notes |
| --- | --- | --- |
| Myco manager UI (Library, Pair, Discover, Settings) | **net-new** | nostr-vpn's network/roster/exit UI does not survive the strip; Library = source of truth for "installed" |
| QR pairing (CameraX + ML Kit, deep-link intent) | **reuse (nostr-vpn)** | re-pointed at `myco://pair/<base64>` (npub + memorable name) |
| `NsiteActivity` (fullscreen WebView per nsite, no chrome) | **net-new (thin)** | own task/instance; standard WebView; nsite-deck "loads localhost" pattern |
| Local gateway + `*.nsite` resolution | **port (site-deck)** | Go → **Rust** in `nsite-deck`; HTTP-listener placement TBD/open |
| Embedded Nostr relay (`:4870`) | **port (site-deck) → Rust** | was Khatru/Go; now the **`myco-relay`** crate (impl `RelayBackend`); embedded default, optional Citrine-forward backend |
| Embedded Blossom (`:24243`) | **port (site-deck) → Rust** | now the **`myco-blossom`** crate (impl `BlobStore`); always embedded; content-addressed blobs, BUD-01 |
| Store-and-forward cache (re-serve peers' events/blobs) | **net-new** | the offline-propagation mechanism; Blossom blob store, LRU 2 GB, Library = pinned |
| nsite sync (fetch + verify + retain over `.fips`) | **net-new** | runs in **`nsite-deck`**; v0 serves direct from relay + Blossom |
| htdocs serving cache (path-named files under `current/`) | **deferred** | nsite-deck speed optimization; not needed for v0 |
| FFI: JNI/JSON reducer, cdylib via cargo-ndk | **reuse (nostr-vpn)** | `dispatch→state(rev)`; opaque `jlong` over Tokio |
| FIPS embedding (`Node::new(Config)` on upstream `fips`) | **reuse + upstream work** | app-owned TUN + custom `BleIo` injection are upstream gaps Myco adds (nostr-vpn's `.without_system_tun()` fork is the reference); see [§ 4c](../how-to/build.md) |
| `VpnService`/TUN | **reuse (nostr-vpn), narrowed** | routes `fd00::/8` only; intercepts `.fips`/`.nsite`; no tunnel-all |
| FIPS core (identity, routing, Noise IK/XK, FSP port-mux) | **reuse (fips-core)** | unchanged |
| Transports UDP / TCP / Tor | **reuse (fips-core)** | unchanged |
| BLE transport core (L2CAP CoC, PSM, tiebreaker, Noise) | **reuse (fips-core)** | `BleIo` trait; Linux interop target |
| `AndroidBleIo` (L2CAP over Android, addr→PSM map) | **net-new** | option A; Kotlin owns radio, hands bytes to Rust |
| **Dropped:** exit-node, WireGuard egress, roster/admin (kind 30388), join-as-membership, `.nvpn` MagicDNS, LAN-multicast pairing | **removed** | the "private network we dropped" |

---

## Related docs

- [concepts.md](./concepts.md) — terminology and the conceptual model.
- [diagrams/01-system-layering.svg](./diagrams/01-system-layering.svg) — this stack, visually.
- [diagrams/02-pairing-transitive-discovery.svg](./diagrams/02-pairing-transitive-discovery.svg) — QR pairing and transitive discovery.
- [diagrams/03-offline-propagation.svg](./diagrams/03-offline-propagation.svg) — live-routing vs store-and-forward.
- [diagrams/04-nsite-browse-flow.svg](./diagrams/04-nsite-browse-flow.svg) — the browse request lifecycle.
- Upstream FIPS: [fips-concepts.md](../../reference/fips/docs/design/fips-concepts.md), [fips-architecture.md](../../reference/fips/docs/design/fips-architecture.md).
