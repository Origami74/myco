# Offline propagation

> Status: DESIGN / proposal for a not-yet-built app. Voice is "the app will…".
> Open questions are marked **TBD / open**.

The vision is nak's "Pillars of Propagation": small relays and Blossom blobs
hopping over crappy links in all directions, surviving outages via local
propagation. The headline scenario: an nsite authored by Alice (with external
nsite tooling, off-device) enters the mesh; the device holding it goes offline;
Carl — who has never met that holder — still gets a verified copy of Alice's
site, because Ben carried it across in between. The app never authors, signs, or
publishes nsites; it only **stores, serves, and replicates** other people's
already-signed events and content-addressed blobs. See
[diagrams/03-offline-propagation.svg](diagrams/03-offline-propagation.svg).

This document specifies how that works. The single most important boundary it
draws: **FIPS transport does not do this.** Offline propagation is net-new and
lives at the relay/nsite layer.

## 1. The two-layer model

Propagation is split across two layers that must not be conflated.

### Layer A — FIPS transport: live-path routing only

`fips-core` gives multi-hop, end-to-end-encrypted delivery across the mesh
spanning tree, but only between nodes that have a *live* path at the moment of
sending. Intermediate nodes route on the destination `node_addr` and cannot read
the payload (Noise XK end-to-end over FSP, Noise IK hop-by-hop over FMP).
Crucially, routing is **live-path only — there is no store-and-forward in the
transport** ([../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md),
[fips-mesh-layer.md](../../reference/fips/docs/design/fips-mesh-layer.md),
[fips-spanning-tree.md](../../reference/fips/docs/design/fips-spanning-tree.md)).

If Alice is offline or out of range, FIPS cannot reach her, full stop. It will
not queue your request and deliver it when she reappears. By itself, Layer A
does not let data survive a partition.

### Layer B — nsite store-and-forward: net-new, survives partition

The behaviour "cache Alice's site and re-serve it to Carl later, offline" is
**net-new** and is implemented at the nsite/relay layer. The embedded relay and
Blossom server ([../../reference/site-deck](../../reference/site-deck)) retain
peers' signed events and content-addressed blobs and thereby **become a new
source**. When Ben's device caches Alice's site, Ben can serve it to Carl over a
fresh BLE hop even though Alice's original holder is nowhere in sight.
Re-emitting Alice's already-signed manifest relay-to-relay is ordinary relay
behaviour — Ben is a *holder*, not the author; his device never signs anything
on Alice's behalf.

This is the layer that survives offline / partition. It is patterned on
bitchat's offline mesh primitives (used as a *pattern reference only* — its
GATT transport and crypto are not used; see §5), applied to Nostr events and
Blossom blobs instead of bitchat packets.

| | Layer A — FIPS transport | Layer B — nsite store-and-forward |
| --- | --- | --- |
| Provenance | reused from `fips-core` | net-new in Myco |
| Unit | opaque encrypted datagrams | signed Nostr events + sha256 blobs |
| Reach | live end-to-end path only | any node that ever cached the content |
| Survives partition? | no | **yes** |
| Trust model | hop crypto (can't read payload) | self-authenticating data (§4) |

The boundary is load-bearing: every time this doc says "propagate", "announce",
or "pull", it means Layer B riding *on top of* whatever live links Layer A
happens to provide right now. The two BLE hops in
[diagrams/03-offline-propagation.svg](diagrams/03-offline-propagation.svg) (Alice→Ben
at t1, Ben→Carl at t2) need never overlap in time. The two layers are drawn
side by side in [diagrams/08-two-layer-propagation.svg](diagrams/08-two-layer-propagation.svg).

## 2. Hybrid announce / pull design

The proposed default (a [LOCKED DECISION], vetoable in its parameters) is
**HYBRID: announce widely, pull full content on demand.** This keeps the chatty,
fan-out part of propagation small (the author-signed manifest event) and the
heavy part (the referenced blobs) lazy and demand-driven.

### Announce (flood the author-signed manifest, TTL 5 hops)

The unit that propagates is the **author-signed nsite manifest event itself**
(nsite kinds `15128` root / `35128` named;
[../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)) —
small, self-authenticating, and keyed by the *author's* pubkey plus, for named
sites, the `d` tag. There is **no separate availability-announcement event and no
new event kind**: a holder that wants to "announce" a site simply re-emits the
author's manifest unchanged. Re-emitting an already-signed event relay-to-relay
is normal relay behaviour, not authoring, and requires no holder signature.

- Manifests flood outward with a hop budget. **Proposed default TTL = 5 hops**;
  each forwarder decrements, and TTL=0 is not forwarded (the decrement-and-drop
  discipline mirrors bitchat's `PacketRelayManager`,
  [../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/PacketRelayManager.kt](../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/PacketRelayManager.kt)).
- **Only manifests propagate multi-hop. Blobs stay pull-only** — there is no
  TTL-flood of blobs.

#### Mechanism: relay-mesh fanout

The flood is not a bespoke protocol — it is the embedded relay acting as a
**relay-mesh node**. When the relay accepts an event (pushed by a peer or pulled
in by sync), it **fans it out to every other connected peer** — an
`["EVENT", …]` to each `<peer>.fips:4869`, source excluded — so a manifest ripples
outward one relay-hop at a time. "Connected peers" are your live FIPS links
(directly paired peers and, transitively, their peers — §3). This is push-based:
a node need not ask for a site to learn it exists; a connected neighbour
volunteers the manifest. The relay store, the sync engine, and this fanout share
one event intake, so an event learned by any path is both stored locally and
re-broadcast once. Loop/duplicate suppression is in §5; the relay-side
behaviour is specified in [./nsite-layer.md §2.1](./nsite-layer.md). It is wanted
**early** (roadmap P2), with transitive discovery and scale-dedup layered on at
P4.

### Pull (fetch content on demand)

When a user opens a site (or the app decides to pre-fetch a pinned one), the app
*pulls* the actual content from a holder whose manifest it has: it already has
(or queries for) the manifest event, then fetches each referenced blob by sha256
from that holder's Blossom server (`<npub_holder>.fips:24242`), verifies, and
caches. The manifest identifies the site by the **author** pubkey; the holder it
is pulled from is a *different* key, reached at `<npub_holder>.fips`. A holder's
relay is queried with `{kinds:[15128 or 35128], authors:[<author_pubkey>]}`. The
browse lifecycle is detailed in
[diagrams/04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg); from
propagation's point of view the key facts are: pulls are point-to-point Layer-A
fetches against a holder, and every pulled object is verified before it is cached
(§4) — so the pull source need not be the original author or its first holder.

This split means a site can be *known to be available* across many hops cheaply
(the manifest has flooded), while the bytes only ever traverse the one path a
user actually needs, when they need it.

## 3. Transitive peer discovery

How does Alice come to receive manifests from nodes she never paired with? Via
transitive discovery, authorized by mutual QR pairing. See
[diagrams/02-pairing-transitive-discovery.svg](diagrams/02-pairing-transitive-discovery.svg).

- **Pairing.** Two users mutually scan QR codes. The payload carries only a
  *device* npub (+ memorable name): `myco://pair/<base64>`; no MAC/PSM — those
  are learned over BLE adverts. A device's npub *is* its FIPS address, so after a
  scan Alice can reach Ben's relay (`:4869`) and Blossom (`:24242`) over the mesh.
  (This device key is only ever a mesh/relay address; it is never an nsite author
  key.)
- **Mutual scan = authorization.** A mutual pairing authorizes Alice to **poll
  Ben's collected peer list**. Ben, in effect, introduces his peers to Alice.
- **Transitive reach.** Having learned Ben's peers (Carl, Dana, …), Alice can
  now receive the manifests they hold and pull from them multi-hop over FIPS —
  even though she paired only with Ben. Reach grows along the pairing graph, not
  just the radio neighbourhood.

This is the social analogue of bitchat's neighbour-gossip TLV
([../../reference/bitchat-android/docs/ANNOUNCEMENT_GOSSIP.md](../../reference/bitchat-android/docs/ANNOUNCEMENT_GOSSIP.md)),
where a node advertises which peers it is directly connected to. Myco adapts
the idea to *authorized* polling of a curated peer list rather than unsolicited
topology gossip — pairing is the consent gate.

**TBD / open:** the exact authorization mechanism. Is "Ben lets Alice poll his
list" enforced (e.g. Ben only answers a peer-list query from npubs he has
paired), advisory, or transitive-by-degree (can Alice reach Dana's peers too, or
does reach stop at one hop past a pairing)? How is a pairing revoked, and does
revocation prune transitively-learned peers?

## 4. Why any source is trustworthy

Transitive, multi-source propagation is only safe because **the data is
self-authenticating** and integrity is independent of the path it took:

- **Signed events.** The nsite manifest event is signed by the *author's*
  external key (held off-device by whoever authored the site). Carl checks
  Alice's signature on Alice's manifest; Ben — a holder, not the author — cannot
  forge or tamper with it in transit, and never possesses Alice's secret key.
- **Content-addressed blobs.** Blossom blobs are addressed by sha256 (BUD-01).
  Carl recomputes the hash of every blob he receives and matches it against the
  path→hash entry in the signed manifest.

So Carl can verify Alice's site is authentic *even though it arrived via Ben* —
exactly the property the bottom band of
[diagrams/03-offline-propagation.svg](diagrams/03-offline-propagation.svg)
states: "the data carries its own integrity, independent of the path it took."
This is what makes "become a new source" sound: any cache is as trustworthy as
the origin, because trust is in the keys and the hashes, not in the relay.

It also bounds the threat model: a malicious relay can *withhold* or *delay*
content, but cannot *corrupt* it without detection. Verification is mandatory on
the receive path — content that fails signature or hash checks is dropped, never
cached, never re-served.

## 5. Dedup and anti-loop

Multi-hop manifest flooding needs loop suppression and duplicate suppression, or
a small mesh will melt under re-broadcast storms. The pattern is adapted directly
from bitchat (reference only).

### TTL flood with bounded relay

Each flooded manifest carries a hop budget (TTL=5, §2). Forwarders decrement and
drop at zero. As in bitchat's `PacketRelayManager`, relay can be *probabilistic*
on larger meshes (relay with probability < 1 to thin out redundant copies) while
small meshes always relay to preserve connectivity
([../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/PacketRelayManager.kt](../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/PacketRelayManager.kt)).
A node never relays its own packets and never relays a packet already addressed
to it.

### Set-reconciliation sync (negentropy / NIP-77), not blind reflooding

For steady-state convergence between neighbours, Myco uses **negentropy
([NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md)) set
reconciliation** — the Nostr-native, range-based protocol (`NEG-OPEN` / `NEG-MSG` /
`NEG-CLOSE`): two connected relays exchange compact range fingerprints and converge
on exactly "which **events** does each side hold that the other lacks" in ~log
bandwidth, then each pulls only its missing manifests. It is strictly local
(neighbour-to-neighbour, not relayed), so it converges content between directly
connected nodes without wide-area flooding. We pick the Nostr-native NIP-77 over
bitchat's `REQUEST_SYNC` **Golomb-Coded Set** digest (the conceptual reference,
[../../reference/bitchat-android/docs/sync.md](../../reference/bitchat-android/docs/sync.md))
because Myco's units are Nostr events and the `negentropy` Rust crate already ships
in the rust-nostr stack — we are not bitchat-wire-compatible anyway. **Blobs are not
reconciled**: they stay content-addressed pull-by-sha256.

- **Dedup / seen-set IDs: 16-byte (128-bit) SHA-256 prefixes**, mirroring
  bitchat's `PacketIdUtil` (ID = first 16 bytes of SHA-256 over the canonical
  bytes). These drive the TTL-flood **seen-set** (loop/dup suppression for the
  fanout), *not* the negentropy filter — negentropy reconciles manifest events
  only (above):
  - manifests → first 16 bytes of the Nostr event `id` (itself a SHA-256 of the
    canonical serialization); this 16-byte prefix is the fanout seen-set element,
    while negentropy itself reconciles over the full event ids.
  - blobs → the blob's sha256 (already content-addressed) — used to **pull** and
    to track what's held, **never** as a reconciliation/filter element (blobs are
    not reconciled; see above).
- **Dedup.** A node maintains a rolling "seen" set of these IDs and never
  re-processes or re-floods an ID already in the set — the same role bitchat's
  seen-set plays, which is what makes TTL flood terminate.
- **Anti-loop.** TTL bounds the worst case; the seen-set kills cycles before TTL
  runs out. Source-route loops (a packet whose recorded path revisits a node)
  are dropped, mirroring the duplicate-hop check in `PacketRelayManager`.
- **Replaceable-event dedup.** nsite kind `35128` is parameterized-replaceable
  (`d` tag). As bitchat keeps only the latest ANNOUNCE per peer, Myco keeps
  only the newest manifest per `(author, dTag)` in the sync candidate set and
  supersedes older ones.

**Original events are forwarded unmodified** so author signatures stay valid —
the same rule bitchat applies to ANNOUNCE. Re-serializing would break the
signature and defeat §4.

## 6. Cache retention and LRU eviction

Caching is what turns a node into a new source, so retention policy *is*
propagation policy. Proposed defaults (vetoable):

- **LRU cache, default cap 2 GB** of blobs + retained events.
- **Pinned sites are exempt from eviction.** A site the user adds to their Library
  is pinned and kept indefinitely; the user's Library is their guaranteed
  serve-set.
- **Eviction.** When the cache exceeds cap, evict least-recently-used *unpinned*
  content. An evicted site is simply no longer served (or re-emitted) by this
  node; it can be re-pulled later from any other source that still has it.
- **Retention horizon for forwarded metadata.** bitchat's store-and-forward uses
  a 12h cache for relayed messages and ages out stale records
  ([../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/StoreForwardManager.kt](../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/StoreForwardManager.kt)).
  Myco's analogue: a node stops re-emitting a manifest it no longer holds,
  and re-flood of a given manifest is soft-state; cached *content* lifetime is
  governed by LRU + pinning above, not by a fixed timeout.

**TBD / open:** does pinning a site also pin everything its manifest references
transitively (e.g. blobs referenced by the manifest are obviously pinned, but
what about sub-resources fetched lazily)? Per-site cap vs global cap? Whether the
2 GB default holds on low-storage devices.

## 7. Boundary restated

To be unambiguous, because this is the most common point of confusion:

- **FIPS / `fips-core` provides:** encrypted, multi-hop, live-path delivery
  between online nodes. Nothing in this document's announce/pull/cache/sync
  behaviour comes from FIPS.
- **Myco's relay + nsite layer provides:** everything in §§2–6 — the
  manifest flood, the on-demand blob pull, transitive discovery, dedup/anti-loop,
  and the cache that makes a node a re-serving source. This is the layer that
  "survives a partition."

If a future reader is tempted to "just add store-and-forward to FIPS," the
answer is no: it belongs here, at the layer where the data is self-authenticating
and a cache is a first-class source.

## Open questions

- **Privacy of "which manifests do I replicate."** Re-emitting an author's
  manifest reveals which sites a device is willing to serve, which leaks
  reading/caching interests to anyone within the 5-hop TTL radius. **TBD /
  open:** can a user propagate without making the set of manifests they replicate
  observable under their own (device) identity? Options to weigh: flood only to
  paired peers, anonymous/aggregate hints, or opt-in "I am a public re-server"
  mode. This trades propagation reach against metadata privacy and is unresolved.
  Note this is purely about *which already-signed manifests a holder chooses to
  replicate* — no holder-authored event is involved.
- **Transitive authorization depth & revocation.** See §3 — how far past a single
  pairing does reach extend, and how is a pairing (and its transitively-learned
  peers) revoked?
- **Sync scope & cost on BLE.** Negentropy (NIP-77) reconcile cadence and
  `NEG-MSG` round size may need different parameters over FIPS L2CAP than over
  bitchat's GATT links; **TBD / open** pending the two-device demo.
