# Roadmap

The phased plan for **Myco**, from scaffolding to the v1 two-device offline
BLE demo and beyond. Each phase has a one-line goal and an explicit **Exit
criterion** — the observable condition that says the phase is done. Phases are
roughly sequential, but P2 can begin once the scaffold (P0) lands.

> Design doc for a not-yet-built app, written in proposal voice. Phase
> boundaries and ordering are **proposed**; open questions inside each phase are
> tracked in the linked design docs and marked **TBD / open** there.

For orientation see [getting-started.md](./getting-started.md); for the doc map
see the [index](./README.md). **P1 is the first de-risk milestone** — two
devices forming a FIPS BLE link offline — and the v1 product headline is the
[two-device offline browse demo](./how-to/run-two-device-demo.md) at **P4**.

---

## Phase overview

| Phase | Goal | Exit criterion |
| --- | --- | --- |
| **P0** | Minimal scaffold + FIPS up | arm64 / minSdk 29 APK builds (plus the macOS core build); app persists an nsec and shows its npub; no relay/Blossom/nsite/TUN yet. |
| **P1** | BLE peering over FIPS + developer UI (the de-risk milestone) | Two devices (Android↔Android and/or Android↔Mac), offline, form a FIPS BLE link via universal per-peer PSM discovery; each shows the other connected in the developer UI. |
| **P2** | Relay + Blossom + gateway (serve-direct) | A side-loaded nsite launches as a fullscreen `NsiteActivity` (its own task), served direct from the local relay + Blossom over the localhost gateway. |
| **P3** | Handshake-mandatory pairing + sync + nsite-deck propagator | Two devices complete the mandatory scan-and-confirm handshake; B opens A's site as a fullscreen app over `.fips`; the propagator forwards manifests both ways. |
| **P4** | Full offline browse demo (the v1 headline) | Two Androids in airplane mode + BLE: B browses A's nsite offline. |
| **P5** | Propagation at scale (set-recon + transitive + eviction) | A cached site survives the origin going offline; reach goes transitive. |
| **P6** | Linux interop | An Android and a Linux peer form a FIPS BLE link via per-peer PSM discovery and sync an nsite. |
| **Later** | htdocs serving cache, home-screen pinning + app-shortcuts, WiFi-Direct, public-node peering, nsite capability API, open nsite links, relay read-auth, NAT46 | (see below — each is its own milestone) |

---

## P0 — Minimal scaffold + FIPS up

**Goal.** Stand up the Myco app shell (Kotlin/Compose) and the Rust workspace
(`myco-core` + `nsite-deck` + `myco-relay` + `myco-blossom`, one `.so`, one
JNI/JSON FFI surface), fork nostr-vpn's app scaffolding, and **strip the
nostr-vpn net layer** — exit-node, WireGuard upstream egress, roster/admin
membership (kind 30388), join-requests-as-membership, `.nvpn` MagicDNS, and
LAN-multicast pairing. Depend on the **canonical upstream `fips` crate** (single
crate, one `patch.crates-io.fips` override) and **embed FIPS via
`Node::new(Config)`**. Generate and persist the single Nostr identity in
`filesDir` and show its npub. **No relay, Blossom, nsite, or TUN yet** — those
arrive later (the app-owned TUN patch is first needed at P2). **Also stand up the
macOS core build** so the same Rust core compiles and runs on macOS for
development/test, alongside the Android target.

**Exit criterion.** `just build` (or `./gradlew assembleDebug`) produces a single
arm64 / minSdk 29 APK; the app launches, generates and persists an nsec, and
shows its own npub; the macOS core build also compiles; none of the stripped
net/roster/exit features remain.

**Design docs.** [architecture.md](./design/architecture.md) (reused-vs-net-new) ·
[concepts.md](./design/concepts.md) (what we dropped vs keep) ·
[identity-pairing.md](./design/identity-pairing.md) (identity storage) ·
[config.md](./reference/config.md) (what is stripped from the config) ·
[ffi-surface.md](./reference/ffi-surface.md) (the reducer + build path) ·
[build.md](./how-to/build.md) (toolchain, local-fips wiring).

## P1 — BLE peering over FIPS + developer UI (the de-risk milestone)

**Goal.** The de-risk milestone: prove two devices can form a FIPS BLE link
offline. Implement native `AndroidBleIo` (Kotlin owns the radio and hands raw
bytes to Rust; FIPS keeps the pool, the cross-probe tiebreaker, the pubkey
exchange, and Noise) and reuse the **macOS `BluestIo`** backend (the `bluest`
CoreBluetooth crate; the fips branch `macos-ble-rebased` commit `0ae9e01`;
`ble-macos` cargo feature; 2-byte length-prefix L2CAP framing) as the dev/test
backend. Land **custom `BleIo` injection** (upstream `fips` hardwires `BluerIo`
in `Node::new`; the seam is `BleTransport::new(.., io, ..)`). Solve the PSM
problem with **universal per-peer PSM discovery**: every node advertises its
OS-assigned listener PSM (service-data and/or a readable GATT characteristic)
and every dialer reads the peer's PSM before `connect()` — symmetric, no fixed
well-known PSM, and the smaller-`node_addr` tiebreaker works normally (no
"Android must be central" constraint). Build a **debug Compose screen** — a
diagnostic surface distinct from Library/Pair/Discover/Settings — that renders
`BleStatus`/`BlePeer` from the existing FFI `ble` + `blePeers` state (adapter /
scanning; per peer: `node_addr`, `npub` once Noise completes, connected, `psm`,
`rssi`). No relay/Blossom/nsite/sync yet — this phase is purely the BLE link and
its diagnostics.

**Exit criterion.** Two devices (Android↔Android and/or Android↔Mac), fully
offline, form a FIPS BLE link via universal per-peer PSM discovery, and each
shows the other connected in the developer UI (the peer's `node_addr`, then its
`npub` once Noise completes).

**Design docs.** [ble-interop.md](./design/ble-interop.md) (the BLE backends, the
PSM problem, per-peer PSM discovery) · [ble-wire.md](./reference/ble-wire.md)
(on-wire constants, PSM advertise/read scheme) ·
[ffi-surface.md](./reference/ffi-surface.md) (the BLE byte-bridge, `ble` /
`blePeers` state, the developer UI) ·
[build.md § 4c](./how-to/build.md) (custom `BleIo` injection, per-peer PSM patch,
reused macOS `BleIo`) ·
[diagrams/01-system-layering.svg](./design/diagrams/01-system-layering.svg).

## P2 — Relay + Blossom + gateway (serve-direct)

**Goal.** Embed, in Rust inside `myco-core`, both the relay and Blossom from day
one — they are simple. **Blossom is always embedded** (`http://localhost:24242`,
a small content-addressed HTTP store: `GET /<sha256>`, `PUT /upload`, `HEAD`);
there is no good Android Blossom app to forward to. **The relay (`myco-relay`) is
embedded too** (`ws://localhost:4869`) — a plain NIP-01 store + socket, with no
forwarding behavior of its own — but the relay *backend* is a pluggable seam:
default = embedded; optional = forward to a local relay app (e.g. Citrine) for
devs who already run one. Add the localhost HTTP gateway on `127.0.0.1:80`,
**serving direct** from the local relay + Blossom: per request for
`http://<host>.nsite/<path>`, look up the manifest event (kinds 15128/35128) on
the local relay, map `<path> → sha256`, fetch that blob from local Blossom,
verify, and serve with a content-type inferred from the path extension — plus an
"are all referenced blobs present?" check before serving (else the site is still
syncing). The `*.nsite → 127.0.0.1` DNS interceptor plus binding loopback `:80`
let `http://<host>.nsite` (no port) resolve in any browser on the device; the
fallback is a high port for the in-app WebView only. No version dirs, no atomic
swap, no sha→name file writing in v0. Open an nsite by launching a fullscreen
`NsiteActivity` — a `WebView` filling the screen, no Myco chrome — in its own
task. Add a minimal site-entry path — a one-time side-load/import of an
externally-authored nsite (its already-signed manifest event + blobs) into the
local stores — so there is something to load. The app never authors, signs, or
publishes nsites; it only stores and serves events authored elsewhere. **The
app-owned TUN patch** (the `VpnService` owns the fd; FIPS exchanges packet bytes
over a channel; route only `fd00::/8`, DNS-intercept `*.fips`/`*.nsite`) is
**first needed here** — sync over `.fips` arrives in P3 — landed as an upstream
`fips` contribution with nostr-vpn's fork as the reference
([build.md § 4c](./how-to/build.md)).

**Exit criterion.** A side-loaded nsite (authored by external tooling) launches
as a fullscreen `NsiteActivity` (its own task) and loads end-to-end from
`http://<host>.nsite` over the localhost gateway, served **direct** from the
local relay + Blossom with hash verification — no htdocs cache, no network
involved.

**Design docs.** [nsite-layer.md](./design/nsite-layer.md) (the whole content
layer) · [nostr-kinds.md](./reference/nostr-kinds.md) (manifest kinds + tags) ·
[ports.md](./reference/ports.md) (localhost ports, the `:80` gateway,
`*.nsite → 127.0.0.1`) · [build.md § 4c](./how-to/build.md) (the app-owned TUN
patch) ·
[diagrams/04-nsite-browse-flow.svg](./design/diagrams/04-nsite-browse-flow.svg).

## P3 — Handshake-mandatory pairing + sync + nsite-deck propagator

**Goal.** Reuse nostr-vpn's QR machinery (CameraX + ML Kit, deep-link intent),
re-pointed at the `myco://pair/<base64>` payload, which now carries JSON
`{ npub, name, pairSecret }`. **Pairing is handshake-mandatory and always
mutual:** scanning initiates the **invite-pairing handshake**
([identity-pairing.md § 6.1](./design/identity-pairing.md)) against the inviter's
on-device `<npub>.fips` endpoint — the scanner echoes `pairSecret` back over that
already Noise-encrypted channel, the inviter matches it and the user taps OK.
`pairSecret` is a long, single-use random string that proves the peer scanned this
invite (but grants no membership/admin authority); completion makes each device a
mutual source. There
is no one-way fetch-only scan. Wire the sync engine to pull a peer's manifest +
blobs from `<npub>.fips:4869` / `:24242`, verify, and mirror into the local
stores — running **over the BLE link from P1, or over IP**. **Stand up the
nsite-deck propagator:** a separate **propagator** process inside `nsite-deck`
(not the relay — `myco-relay` stays a plain store + socket) does all forwarding
by **subscribing** to the relevant relays (local + connected peers) and
**publishing** (`EVENT`) to peer relays, so manifests gossip **both ways**, not
just A→B pull — with the minimal loop/dup guard (seen-set on the 16-byte
SHA-256 event id + TTL 5). The same propagator keeps an internal subscription for
kinds 15128/35128 and runs **eager pinned-refresh**: when a newer manifest for a
Library-pinned `(author, dTag)` arrives, it auto-runs the blob pull in the
background so pinned apps stay current and offline-ready before next open. Blobs
stay pull-only. **Pull a basic negentropy (NIP-77) reconcile into this phase
too**, so a (re)connecting peer efficiently catches up on missing manifests
(`NEG-OPEN` on a manifest filter → fetch the diff), not by live forwarding alone
— events only. See [nsite-layer.md § 2.1, § 2.4](./design/nsite-layer.md) and
[propagation.md § 2, § 5](./design/propagation.md).

**Exit criterion.** Device B scans A's invite and completes the mandatory
secret-echo handshake (both now mutual sources); over the BLE link from P1 (or an IP-based
FIPS path), B syncs and opens A's nsite as a fullscreen app (its own task); B's
local relay + Blossom now hold A's events and blobs; **a manifest accepted by
either relay appears on the other via the propagator** (source-excluded, not
re-echoed); **a peer that missed events catches up via a basic negentropy
(NIP-77) reconcile** rather than re-pulling the full set; **and a newer manifest
for a Library-pinned site triggers an automatic background blob pull**.

**Design docs.** [identity-pairing.md § 6.1](./design/identity-pairing.md) (the
invite-pairing handshake, `pairSecret`, peer-as-source) ·
[nsite-layer.md](./design/nsite-layer.md) (§5 sync over FIPS) ·
[propagation.md](./design/propagation.md) (the propagator, fanout, eager
pinned-refresh) · [ports.md](./reference/ports.md) (`.fips` vs `.nsite`, FSP
port-mux) · [security.md](./design/security.md) (scan-and-confirm pairing,
self-authenticating data) ·
[diagrams/02-pairing-transitive-discovery.svg](./design/diagrams/02-pairing-transitive-discovery.svg).

## P4 — Full offline browse demo (the v1 headline)

**Goal.** The v1 product headline. With the BLE link (P1), serve-direct gateway
(P2), and handshake-mandatory pairing + sync (P3) all in place, run the whole
relay/Blossom/sync stack over the BLE transport with **both devices in airplane
mode, BLE on** — no IP path available. B pairs to A offline, syncs A's nsite
over BLE, and browses it as a fullscreen app while both radios are dark.

**Exit criterion.** Two Android phones, fully offline (airplane mode), form a
one-hop BLE link; B opens A's nsite as a fullscreen app (its own task), A's
events/blobs are cached on B, and `<npubA>.fips` resolves on B over the mesh DNS
interceptor. This is the
[run-two-device-demo.md](./how-to/run-two-device-demo.md) success condition.

**Design docs.** [run-two-device-demo.md](./how-to/run-two-device-demo.md) (the
runbook) · [ble-interop.md](./design/ble-interop.md) (the BLE transport under
load) · [nsite-layer.md](./design/nsite-layer.md) (sync + serve over BLE) ·
[ffi-surface.md](./reference/ffi-surface.md) (`siteStatus` / sync-state copy) ·
[diagrams/01-system-layering.svg](./design/diagrams/01-system-layering.svg).

## P5 — Propagation at scale (set-reconciliation + transitive discovery)

**Goal.** Make a node a durable **new source** that survives the origin going
offline, and scale the P3 propagator's per-link manifest forwarding across the
pairing graph. The per-link manifest fanout and a basic negentropy reconcile
already exist (P3); P5 adds the harder pieces: **negentropy (NIP-77)
set-reconciliation at scale** (harden the P3 reconcile across the pairing graph
— efficiently catching up on "what events do you have that I don't" rather than
relying on live forwarding alone), the pairing-gated **transitive peer-list poll**
so reach grows past directly paired peers, and **LRU eviction** (default 2 GB)
with Library-pinned sites exempt. Blobs remain **pull-only**, now
**pull-from-many** (any holder, verified by sig/hash).

**Exit criterion.** With the origin (A) gone, a third device pulls A's site from
a node (B) that only cached it earlier — verified by signature/hash — and a
node receives flooded manifests for, and pulls from, a peer it never directly
paired with.

**Design docs.** [propagation.md](./design/propagation.md) (the whole phase) ·
[nostr-kinds.md](./reference/nostr-kinds.md) (manifest kinds 15128/35128) ·
[identity-pairing.md](./design/identity-pairing.md) (§6 transitive authorization) ·
[security.md](./design/security.md) (why any source is trustworthy; propagation
privacy) ·
[diagrams/03-offline-propagation.svg](./design/diagrams/03-offline-propagation.svg).

## P6 — Linux interop

**Goal.** Validate wire compatibility against the reference `BluerIo`: an Android
peer and a Linux peer form a FIPS BLE link, run the 33-byte pubkey pre-handshake
and Noise IK, and sync an nsite — proving the Android and Linux backends match
byte-for-byte. **Both directions work via universal per-peer PSM discovery:** the
Linux node advertises its OS-assigned listener PSM the same as everyone else, and
each dialer reads the peer's PSM before `connect()`. BlueZ is the outlier that
*can* bind a fixed PSM, but Myco does not rely on it — **fixed-`0x0085` wire
compat is intentionally dropped** (`0x0085` is only a legacy default), so there is
no "Android dials `0x0085`" constraint and no deferred Linux-dials-Android special
case. The per-peer PSM patch (advertise own PSM + read peer's PSM into
`connect()`, across all backends) already landed at P1.

**Exit criterion.** An Android phone and a Linux machine running `BluerIo` with
per-peer PSM advertising form a FIPS BLE link (each reads the other's advertised
PSM), and the Android browses an nsite hosted on the Linux peer (or vice-versa).

**Design docs.** [ble-interop.md](./design/ble-interop.md) (Android↔Linux via
per-peer PSM discovery) · [ble-wire.md](./reference/ble-wire.md) (the constants
the backend must match; PSM advertise/read scheme) ·
[build.md § 4c](./how-to/build.md) (the per-peer PSM patch, all backends).

---

## Later

Out of scope for v1; each is its own milestone with its own design pass.

- **htdocs serving cache.** A speed optimization on top of v0's serve-direct
  gateway (nsite-deck's approach): write each referenced blob out as a path-named
  file under a `current/` dir for fast static serving, rather than resolving
  `<path> → sha256 →` blob per request. The content-addressed Blossom blob store
  stays the retained store-and-forward source (and what the LRU 2 GB cap governs);
  htdocs is a derived, path-named cache layered on top, not needed for v0 —
  [nsite-layer.md](./design/nsite-layer.md).
- **Home-screen pinning + app-shortcuts.** Offer "Add to home screen" for an
  nsite via `ShortcutManager.requestPinShortcut()` (always user-confirmed — Android
  shows a system dialog per pin; Myco cannot silently pin), plus dynamic
  app-shortcuts. Knowing whether an nsite is pinned is best-effort only
  (`getPinnedShortcuts()` is a soft hint; removal is under-reported and has no
  callback), so the Library stays the source of truth for "installed" —
  [nsite-layer.md](./design/nsite-layer.md),
  [identity-pairing.md](./design/identity-pairing.md).
- **WiFi-Direct transport.** A higher-throughput offline transport alongside BLE,
  for larger nsites where L2CAP MTU is the bottleneck. (BLE throughput is an open
  question in [run-two-device-demo.md](./how-to/run-two-device-demo.md).)
- **Public-node peering via Nostr discovery.** The online path: find and
  rendezvous with peers over the internet using FIPS discovery kinds (37195
  overlay advert, 21059 traversal signaling, 10050 inbox relays) and NAT
  traversal. Documented but unused in v1 —
  [nostr-kinds.md](./reference/nostr-kinds.md) (FIPS discovery kinds),
  [config.md](./reference/config.md) (online relay set / bootstrap, TBD).
- **nsite capability API.** Give nsite JavaScript a scoped capability surface
  (e.g. query peers) beyond v1's pure-static content. A large trust escalation
  needing a per-capability permission model — see
  [security.md](./design/security.md) (§5) and
  [nsite-layer.md](./design/nsite-layer.md) (§7).
- **Open `*.nsite` / `*.nsite.lol` links in Myco.** Register intent filters for
  nsite hostnames (the `.nsite` TLD and public gateways like `nsite.lol`) so tapping
  such a link anywhere opens it in Myco — **downloading the nsite if not already
  held** (source order in [nsite-layer.md](./design/nsite-layer.md) §5) — instead of
  a browser. Cross-nsite links open each site as its own task
  ([app-shell.md](./design/app-shell.md) §4).
- **NAT46 for external browsers.** Let a browser *outside* the app (system
  Chrome, etc.) reach mesh-hosted content, bridging the IPv4/IPv6 split beyond
  the localhost gateway each `NsiteActivity` WebView uses —
  [ports.md](./reference/ports.md),
  [concepts.md](./design/concepts.md) (`.fips` vs `.nsite`).
- **Multi-persona identity.** More than one keypair per device (independent
  node_addr / ULA / Library) — [identity-pairing.md](./design/identity-pairing.md)
  (§3).
- **Relay / Blossom read-auth.** v0 is **open-read** — any connected peer can
  `REQ` your relay and `GET` any blob, which lets a peer enumerate your manifest
  set (what you hold / installed). Restrict it with NIP-42 `AUTH` on the relay,
  per-peer read-scoping, and **unlisted/private nsites** + selective replication.
  All additive (NIP-42 is non-breaking on the wire) —
  [security.md](./design/security.md) (§3).
- **Re-surfacing the FIPS peer ACL** as a "block this peer" control —
  [security.md](./design/security.md) (§3).

---

## See also

- [getting-started.md](./getting-started.md) — orientation and the v1 demo in
  three sentences.
- [README.md](./README.md) — the full documentation index.
- [how-to/build.md](./how-to/build.md) · [how-to/run-two-device-demo.md](./how-to/run-two-device-demo.md)
  — the build and demo runbooks.
