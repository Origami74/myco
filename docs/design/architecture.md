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

Myco runs entirely on one phone: an Android UI shell over a single Rust
native library (`myco-core`) that bundles the FIPS endpoint, the embedded
Nostr relay, and the embedded Blossom server. Kotlin owns the UI, the WebView,
the radio (BLE), and the `VpnService`/TUN; Rust owns identity, the mesh, and the
nsite services. They talk across a JNI/JSON reducer boundary.

The single most important structural fact: **the WebView only ever loads
`http://<host>.nsite` (IPv4 localhost).** All cross-device fetching happens in
native code over `.fips`. The browser never resolves `.fips`. Everything else in
the stack exists to make that local page load resolvable from peer content.

The stack, top to bottom:

```
  1  Android UI            (Jetpack Compose) — Library, browser chrome, QR pair
  2  Embedded browser      (WebView) — loads http://<host>.nsite, no URL bar
  3  Local gateway + DNS   — *.nsite → 127.0.0.1; resolve manifest; serve files
  4  Embedded relay+Blossom (Rust) — ws://…:4869 / http://…:24242 + S&F cache
  5  myco-core / FFI      (Rust) — JNI/JSON reducer; binds services into FIPS
  6  FIPS core + transports (fips-core/endpoint) — mesh, crypto, BLE/UDP/TCP/Tor
     ── plus ── Android VpnService/TUN: routes fd00::/8, intercepts .fips/.nsite
```

(The diagram collapses UI + browser into one band and shows six bands; this doc
splits the WebView out to make the "loads localhost only" boundary explicit.)

---

## The six layers, top to bottom

### 1. Android UI (Jetpack Compose) — **net-new**

**Responsibilities.** The Library (home grid of nsites-as-apps), the browser chrome
(fixed bottom bar: Back · Reload · Library), the QR pairing screen (scan + show),
"nsites around me" search, and storage/settings. Navigation is deliberately
minimal: no URL bar; site info on long-press in the Library.

**Provenance.** Net-new. nostr-vpn's UI is network/roster/exit-node management —
none of that survives the strip. The **QR pairing** sub-feature is reused
(see layer 5 / FFI), but the screens around it are new.

### 2. Embedded browser (WebView) — **net-new (thin)**

**Responsibilities.** Render an nsite. The WebView loads exactly one kind of URL,
`http://<host>.nsite`, where `<host>` is `npub1…` (root site) or
`<pubkeyB36><dTag>` (named site). It has no address bar and no cross-origin
navigation affordance; the only controls are the bottom bar. In v1 the content
is **pure static** (no privileged JS); a capability API is a later milestone.

**Provenance.** Net-new, but thin — it is a standard Android `WebView` pointed at
localhost. The "nsite-deck model" (browser loads localhost, native code does the
fetching) is the pattern ported from site-deck.

### 3. Local gateway + name resolution — **port (site-deck)**

**Responsibilities.** This is the localhost HTTP gateway and the `.nsite` name
resolution, ported from nsite-deck. It:

1. answers `*.nsite` DNS with `127.0.0.1` (an A record),
2. receives the WebView's HTTP request, parses the left-most label into a pubkey
   (and optional `dTag`),
3. looks up the site manifest event from the **local relay** (layer 4),
4. on a cache miss, triggers a sync (layer 5 fetches over `.fips`), showing a
   loading page that polls until ready,
5. resolves the requested path to a blob hash via the manifest's `path` tags
   (falling back to `index.html`, then `/404.html`), fetches the blob from the
   **local Blossom server**, verifies it against its sha256, and serves it.

Because `.nsite` is IPv4/localhost, this works in any browser, including
Chromium. Only the relay+Blossom **sync** in step 4 ever touches `.fips`.

**Provenance.** Port from site-deck. The upstream gateway is Go; here it is
**re-implemented in Rust** inside `myco-core` (one language, one `.so`).
The HTTP listener may live in Kotlin or Rust — **TBD / open**; either way the
resolution logic ports from site-deck.

### 4. Embedded Nostr relay + Blossom server — **port (site-deck) → Rust**

**Responsibilities.** Hold the nsite data and serve it both to the local gateway
and to peers:

- the **relay** (`ws://localhost:4869`) stores/serves signed manifest events
  (kinds 15128/35128),
- **Blossom** (`http://localhost:24242`) stores/serves sha256-addressed blobs.

This layer is also the **store-and-forward cache** that makes offline
propagation work: it retains *peers'* signed events and blobs and re-serves
them, so the device becomes a new source (a "relay-of-relays"). Eviction is an
LRU cache, default cap 2 GB; sites added to the Library are **pinned** (exempt).

**Provenance.** Port from site-deck (which embeds Khatru, a Go relay/Blossom).
Myco re-implements the relay + Blossom in **Rust** inside a **separate, reusable
crate, `nsite-deck`** — gateway + relay + Blossom + sync + cache, deliberately
knowing nothing about FIPS/BLE/Android so other apps can reuse it. It depends on
two host-provided transport seams (`PeerSource` to pull, `FanoutSink` to push);
`myco-core` supplies the FIPS-backed implementations. See
[nsite-layer.md § A reusable crate](./nsite-layer.md). (The diagram tags this
band "PORT · lang TBD"; the language is now **locked to Rust** — treat that label
as stale.)

### 5. myco-core + the FFI boundary (Rust) — **reuse (nostr-vpn) + net-new glue**

**Responsibilities.** The **app crate** and the single native library. It is the
thin Myco-specific layer that wires everything together; the content layer lives
in `nsite-deck` (layer 4), which it depends on. `myco-core`:

- exposes the **JNI/JSON reducer** FFI to Kotlin (below),
- holds the device **identity** (the `nsec`) in `filesDir`,
- runs the `nsite-deck` relay + Blossom and **binds them into FIPS** so peers
  reach them at `<npub>.fips:4869` / `:24242` via FSP port multiplexing,
- drives the FIPS endpoint (layer 6) and implements `AndroidBleIo`,
- provides `nsite-deck`'s transport seams over FIPS: the **`PeerSource`** (fetch
  a manifest + blobs from a holder at `<npub>.fips`, verify, atomically cache)
  and the **`FanoutSink`** (`EVENT` to each connected `<peer>.fips:4869`).

`myco-core` + `nsite-deck` cross-compile into one `libmyco_core.so`.

**Provenance.** Reuse the nostr-vpn embedding pattern and FFI scaffolding;
net-new glue to wire `nsite-deck` to FIPS. nostr-vpn embeds FIPS in-process via
`FipsEndpoint::builder()` with `.without_system_tun()` — the app owns the TUN and
hands FIPS only packet bytes, while FIPS moves opaque encrypted datagrams between
npubs. Myco keeps exactly this and adds the nsite services on top.

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
  bytes pass to/from Rust (the `.without_system_tun()` contract).
- **BLE bytes** — Android's BLE APIs are Java-only, so Kotlin owns the radio and
  hands raw L2CAP bytes to Rust's `AndroidBleIo` (see layer 6 / BLE).

### 6. FIPS core mesh + transports — **reuse (fips-core)**, BLE I/O **net-new**

**Responsibilities.** The mesh itself: identity derivation
(npub → node_addr → `fd00::/8`), `.fips` DNS, spanning-tree formation,
coordinate-based greedy routing, and two-layer Noise crypto (XK end-to-end +
IK hop-by-hop). FSP **port multiplexing** delivers mesh datagrams to the
localhost relay/Blossom ports, which is what exposes `<npub>.fips:4869`/`:24242`.

**Transports.** UDP / TCP / Tor are reused from fips-core unchanged. **BLE** is
the v1 transport and the one net-new transport piece:

- FIPS BLE is **L2CAP CoC** (SeqPacket), fixed default PSM `0x0085` (133),
  with a UUID-only advert (no identity material), a pre-handshake pubkey exchange
  of `[0x00][pubkey:32]` over the stream, then Noise IK. The cross-probe
  tiebreaker: the smaller node_addr's outbound connection wins.
- The platform surface is fips-core's **`BleIo` trait**
  (`listen`/`connect`/`start_advertising`/`start_scanning`/`local_addr`/…).
  Linux backs it with `BluerIo`; Myco adds **`AndroidBleIo`** (option A:
  native L2CAP CoC), targeting **Linux interop**. FIPS owns all connection
  tracking, the pool, the tiebreaker, Noise, and reconnect — **bitchat is not
  used as a transport.**
- Android L2CAP specifics force a dial direction: `listenUsingInsecureL2cap­Channel()`
  returns a *dynamically* assigned PSM (you cannot bind the fixed default PSM 133 as a
  listener), while `createL2capChannel(psm)` can dial any PSM. So **Android is the
  central/dialer**; `AndroidBleIo` keeps an `addr→PSM` map learned from adverts
  and uses it in `connect()`. FIPS identifies peers by the pubkey exchange, not by
  MAC, so Android MAC randomization is harmless. Requires **API 29+** (minSdk 29).

**Provenance.** Reuse fips-core for everything except `AndroidBleIo`, which is
net-new (the only platform-specific code in this layer). bitchat-android is a
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
- The app **owns** the TUN; FIPS is embedded with `.without_system_tun()` and is
  handed only packet bytes. The TUN fd is read/written in Kotlin.

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
| Android UI (Library, browser chrome, search, settings) | **net-new** | nostr-vpn's network/roster/exit UI does not survive the strip |
| QR pairing (CameraX + ML Kit, deep-link intent) | **reuse (nostr-vpn)** | re-pointed at `myco://pair/<base64>` (npub + memorable name) |
| Embedded browser (WebView, no URL bar) | **net-new (thin)** | standard WebView; nsite-deck "loads localhost" pattern |
| Local gateway + `*.nsite` resolution | **port (site-deck)** | Go → **Rust**; HTTP-listener placement TBD/open |
| Embedded Nostr relay (`:4869`) | **port (site-deck) → Rust** | was Khatru/Go; now Rust in `myco-core` |
| Embedded Blossom (`:24242`) | **port (site-deck) → Rust** | content-addressed blobs, BUD-01 |
| Store-and-forward cache (re-serve peers' events/blobs) | **net-new** | the offline-propagation mechanism; LRU 2 GB, Library = pinned |
| nsite sync (fetch + verify + atomic cache over `.fips`) | **net-new** | runs in `myco-core` |
| FFI: JNI/JSON reducer, cdylib via cargo-ndk | **reuse (nostr-vpn)** | `dispatch→state(rev)`; opaque `jlong` over Tokio |
| FIPS embedding (`FipsEndpoint::builder().without_system_tun()`) | **reuse (nostr-vpn)** | app owns TUN; FIPS moves opaque datagrams |
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
