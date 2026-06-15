# Roadmap

The phased plan for **Myco**, from scaffolding to the v1 two-device offline
BLE demo and beyond. Each phase has a one-line goal and an explicit **Exit
criterion** — the observable condition that says the phase is done. Phases are
roughly sequential, but P1 and P2 can overlap once the scaffold (P0) lands.

> Design doc for a not-yet-built app, written in proposal voice. Phase
> boundaries and ordering are **proposed**; open questions inside each phase are
> tracked in the linked design docs and marked **TBD / open** there.

For orientation see [getting-started.md](./getting-started.md); for the doc map
see the [index](./README.md). The v1 target (P3) is the
[two-device demo](./how-to/run-two-device-demo.md).

---

## Phase overview

| Phase | Goal | Exit criterion |
| --- | --- | --- |
| **P0** | Scaffold & strip | App builds; nostr-vpn's private-network layer is gone; identity persists. |
| **P1** | Embed relay + Blossom + gateway (serve-direct) | A locally-seeded nsite launches as a fullscreen `NsiteActivity` (its own task), served direct from the local relay + Blossom. |
| **P2** | QR pairing + sync + relay-mesh fanout | Two devices pair by QR; B opens A's site as a fullscreen app over `.fips`; manifests fan out both ways. |
| **P3** | AndroidBleIo (the v1 demo) | Two Androids, airplane mode + BLE: B browses A's nsite offline. |
| **P4** | Propagation at scale (set-recon + transitive + eviction) | A cached site survives the origin going offline; reach goes transitive. |
| **P5** | Linux interop | An Android dials a Linux `BluerIo` peer and syncs an nsite. |
| **Later** | htdocs serving cache, home-screen pinning + app-shortcuts, WiFi-Direct, public-node peering, nsite capability API, NAT46 | (see below — each is its own milestone) |

---

## P0 — Scaffold & strip

**Goal.** Stand up the Myco workspace (Kotlin/Compose app + the four-crate Rust
workspace `myco-core` + `nsite-deck` + `myco-relay` + `myco-blossom`, one `.so`,
one JNI/JSON FFI surface), fork nostr-vpn's app scaffolding, and **strip the
private-network layer** — exit-node, WireGuard upstream egress, roster/admin
membership (kind 30388), join-requests-as-membership, `.nvpn` MagicDNS, and
LAN-multicast pairing. Depend on the **canonical upstream `fips` crate** (single
crate, one `patch.crates-io.fips` override) embedded via `Node::new(Config)`, and
land the **two Android-enabling seams upstream `fips` lacks** — an **app-owned TUN**
(the `VpnService` owns the fd; FIPS exchanges packet bytes over a channel) and
**custom `BleIo` injection** (for `AndroidBleIo`) — as upstream contributions, with
nostr-vpn's fork as the reference (see [build.md § 4c](./how-to/build.md)). Keep the
`VpnService`/TUN, narrowed to route only `fd00::/8` and DNS-intercept
`*.fips`/`*.nsite`. Generate and persist the single Nostr identity in `filesDir`.

**Exit criterion.** `just build` (or `./gradlew assembleDebug`) produces a single
arm64 / minSdk 29 APK; the app launches, generates and persists an nsec, and
shows its own npub; none of the stripped network/roster/exit features remain.

**Design docs.** [architecture.md](./design/architecture.md) (reused-vs-net-new,
the TUN's role) · [concepts.md](./design/concepts.md) (what we dropped vs keep) ·
[identity-pairing.md](./design/identity-pairing.md) (identity storage) ·
[config.md](./reference/config.md) (what is stripped from the config) ·
[ffi-surface.md](./reference/ffi-surface.md) (the reducer + build path) ·
[build.md](./how-to/build.md) (toolchain, local-fips wiring).

## P1 — Relay + Blossom + gateway

**Goal.** Embed, in Rust inside `myco-core`, both the relay and Blossom from day
one — they are simple. **Blossom is always embedded** (`http://localhost:24242`,
a small content-addressed HTTP store: `GET /<sha256>`, `PUT /upload`, `HEAD`);
there is no good Android Blossom app to forward to. **The relay is embedded too**
(`ws://localhost:4869`), but the relay *backend* is a pluggable seam: default =
embedded; optional = forward to a local relay app (e.g. Citrine) for devs who
already run one. Add the localhost HTTP gateway, **serving direct** from the
local relay + Blossom: per request for `<host>.nsite/<path>`, look up the
manifest event (kinds 15128/35128) on the local relay, map `<path> → sha256`,
fetch that blob from local Blossom, verify, and serve with a content-type
inferred from the path extension — plus an "are all referenced blobs present?"
check before serving (else the site is still syncing). No version dirs, no atomic
swap, no sha→name file writing in v0. Open an nsite by launching a fullscreen
`NsiteActivity` — a `WebView` filling the screen, no Myco chrome — in its own
task. Add a minimal site-entry path — a one-time side-load/import of an
externally-authored nsite (its already-signed manifest event + blobs) into the
local stores — so there is something to load. The app never authors, signs, or
publishes nsites; it only stores and serves events authored elsewhere.

**Exit criterion.** A side-loaded nsite (authored by external tooling) launches
as a fullscreen `NsiteActivity` (its own task) and loads end-to-end from
`127.0.0.1`, served **direct** from the local relay + Blossom with hash
verification — no htdocs cache, no network involved.

**Design docs.** [nsite-layer.md](./design/nsite-layer.md) (the whole content
layer) · [nostr-kinds.md](./reference/nostr-kinds.md) (manifest kinds + tags) ·
[ports.md](./reference/ports.md) (localhost ports) ·
[diagrams/04-nsite-browse-flow.svg](./design/diagrams/04-nsite-browse-flow.svg).

## P2 — QR pairing + sync + relay-mesh fanout

**Goal.** Reuse nostr-vpn's QR machinery (CameraX + ML Kit, deep-link intent),
re-pointed at the `myco://pair/<base64>` payload (npub + memorable name). Wire
the sync engine to pull a peer's manifest + blobs from `<npub>.fips:4869` /
`:24242` over FIPS, verify, and mirror into the local stores. Prove the sync path
**over IP first** (two devices on a LAN, or the UDP/TCP transport) before BLE
exists. **Stand up relay-mesh fanout early:** once two relays are connected over
FIPS, each re-broadcasts events it accepts to every other connected peer (source
excluded), so manifests gossip **both ways**, not just A→B pull — with the
minimal loop/dup guard (seen-set on the 16-byte SHA-256 event id + TTL 5) that
fanout requires. Blobs stay pull-only. **Pull a basic negentropy (NIP-77) reconcile
into this phase too**, so a (re)connecting peer efficiently catches up on missing
manifests (`NEG-OPEN` on a manifest filter → fetch the diff), not by live fanout
alone — events only; blobs stay pull-only. See
[nsite-layer.md §2.1, §2.4](./design/nsite-layer.md) and
[propagation.md §2, §5](./design/propagation.md).

**Exit criterion.** Device B QR-pairs to A and, over an IP-based FIPS path,
syncs and opens A's nsite as a fullscreen app (its own task) on B; B's local
relay + Blossom now hold A's events and blobs; **a manifest accepted by either
relay appears on the other via fanout** (source-excluded, not re-echoed); **and a
peer that missed events catches up via a basic negentropy (NIP-77) reconcile**
rather than re-pulling the full set.

**Design docs.** [identity-pairing.md](./design/identity-pairing.md) (QR payload,
peer-as-source) · [nsite-layer.md](./design/nsite-layer.md) (§5 sync over FIPS) ·
[ports.md](./reference/ports.md) (`.fips` vs `.nsite`, FSP port-mux) ·
[security.md](./design/security.md) (TOFU pairing, self-authenticating data) ·
[diagrams/02-pairing-transitive-discovery.svg](./design/diagrams/02-pairing-transitive-discovery.svg).

## P3 — AndroidBleIo (the v1 demo)

**Goal.** The v1 target. Implement native `AndroidBleIo` against fips-core's
`BleIo` trait over Android L2CAP CoC: Kotlin owns the radio and hands raw bytes
to Rust; FIPS keeps the pool, the cross-probe tiebreaker, the pubkey exchange,
and Noise. Solve the PSM problem with the Android-side addr→PSM advert map
(Android is the central/dialer). Run the relay/Blossom/sync of P1–P2 over the BLE
transport with **both devices in airplane mode, BLE on**.

**Exit criterion.** Two Android phones, fully offline (airplane mode), form a
one-hop BLE link; B opens A's nsite as a fullscreen app (its own task), A's
events/blobs are cached on B, and `<npubA>.fips` resolves on B over the mesh DNS
interceptor. This is the
[run-two-device-demo.md](./how-to/run-two-device-demo.md) success condition.

**Design docs.** [ble-interop.md](./design/ble-interop.md) (option A, the PSM
problem, foreground service) · [ble-wire.md](./reference/ble-wire.md) (on-wire
constants, addr→PSM scheme) · [ffi-surface.md](./reference/ffi-surface.md) (the
BLE byte-bridge) · [run-two-device-demo.md](./how-to/run-two-device-demo.md) ·
[diagrams/01-system-layering.svg](./design/diagrams/01-system-layering.svg).

## P4 — Propagation at scale (set-reconciliation + transitive discovery)

**Goal.** Make a node a durable **new source** that survives the origin going
offline, and scale the P2 relay-mesh fanout across the pairing graph. The
per-link manifest fanout and a basic negentropy reconcile already exist (P2); P4
adds the harder pieces: **negentropy (NIP-77) set-reconciliation at scale** (harden
the P2 reconcile across the pairing graph — efficiently catching up on "what events
do you have that I don't" rather than relying on live fanout alone),
the mutual-scan **transitive peer-list poll** so reach grows past directly paired
peers, and **LRU eviction** (default 2 GB) with Library-pinned sites exempt. Blobs
remain **pull-only**, now **pull-from-many** (any holder, verified by sig/hash).

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

## P5 — Linux interop

**Goal.** Validate wire compatibility against the reference `BluerIo`: an Android
peer dials a Linux peer listening on the fixed default PSM `0x0085`, runs the
33-byte pubkey pre-handshake and Noise IK, and syncs an nsite. This proves
`AndroidBleIo` matches the reference byte-for-byte. Linux-dials-Android (the hard
direction) is explicitly deferred — its fix (a fips-core per-peer-PSM patch
against the local checkout) is scoped here but not required to exit.

**Exit criterion.** An Android phone and a Linux machine running stock `BluerIo`
form a FIPS BLE link (Android dials `0x0085`), and the Android browses an nsite
hosted on the Linux peer (or vice-versa via Android-initiated pull).

**Design docs.** [ble-interop.md](./design/ble-interop.md) (scenario 1 Android→Linux,
scenario 3 deferred) · [ble-wire.md](./reference/ble-wire.md) (the constants the
backend must match) · [build.md](./how-to/build.md) (§4 local-fips wiring for any
needed patch).

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
- **NAT46 for external browsers.** Let a browser *outside* the app (system
  Chrome, etc.) reach mesh-hosted content, bridging the IPv4/IPv6 split beyond
  the localhost gateway each `NsiteActivity` WebView uses —
  [ports.md](./reference/ports.md),
  [concepts.md](./design/concepts.md) (`.fips` vs `.nsite`).
- **Multi-persona identity.** More than one keypair per device (independent
  node_addr / ULA / Library) — [identity-pairing.md](./design/identity-pairing.md)
  (§3).
- **Re-surfacing the FIPS peer ACL** as a "block this peer" control —
  [security.md](./design/security.md) (§3).

---

## See also

- [getting-started.md](./getting-started.md) — orientation and the v1 demo in
  three sentences.
- [README.md](./README.md) — the full documentation index.
- [how-to/build.md](./how-to/build.md) · [how-to/run-two-device-demo.md](./how-to/run-two-device-demo.md)
  — the build and demo runbooks.
