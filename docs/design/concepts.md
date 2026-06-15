# Concepts and Glossary

This is the canonical concept and terminology doc for **Myco**. Other design
docs build on the vocabulary fixed here. For the system structure that realizes
these concepts, see [architecture.md](./architecture.md). For the underlying mesh,
see the upstream FIPS docs: [fips-concepts.md](../../reference/fips/docs/design/fips-concepts.md)
and [fips-architecture.md](../../reference/fips/docs/design/fips-architecture.md).

Myco is a peer-to-peer **app-sharing network**: a phone app for exchanging, browsing,
and propagating websites ("nsites") over a FIPS mesh — including fully offline
over Bluetooth. The visual companion to this doc is
[diagrams/01-system-layering.svg](./diagrams/01-system-layering.svg).

> These are design docs for a not-yet-built app. They are written in
> proposal voice. Open questions are marked **TBD / open**.

---

## The device identity — three derived forms

Myco combines several **device-level** roles into a **single Nostr
keypair**, the **device key**:

- your **mesh network identity** (who you are on the FIPS network),
- your **BLE link authentication** (who the radio link is talking to), and
- your **app/device identity** in Myco — the address of *this* device's
  embedded relay + Blossom (`<npub_device>.fips:4869` / `:24242`).

The device key is **never** used to author nsites. nsites are authored
elsewhere, by external tooling, under separate keys (see *nsite author identity*
below). The app holds and serves other people's already-signed events; it does
not sign on anyone's behalf.

There is one device keypair per device in v1 (multi-persona is later). From that
one keypair, three addressing forms are deterministically derived. All three
name the *same* device; they differ only in which layer consumes them.

| Form | Value | Who uses it |
| --- | --- | --- |
| **npub** | the bech32-encoded secp256k1 public key (`npub1…`) | the UI, QR pairing, BLE link auth, the relay/Blossom address |
| **node_addr** | `SHA256(npub)[0:16]` — a 16-byte routing identifier | the FIPS mesh routing layer (packet headers, spanning tree, bloom filters) |
| **fd00:: IPv6** | `fd` ‖ `node_addr[0:15]`, i.e. an address inside `fd00::/8` | unmodified IP applications, via the TUN |

The npub is the cryptographic identity used in Noise handshakes; it is never
exposed beyond the endpoints of an encrypted channel. The **node_addr** is a
one-way hash, so intermediate routers forward on it without learning the Nostr
identity of either endpoint — an observer who already knows a pubkey can verify
"does this node_addr belong to pubkey X?" but cannot enumerate identities from
traffic. The **fd00:: IPv6** address is a ULA overlay address that lets ordinary
IPv6 software reach a mesh node through the TUN.

(See [fips-architecture.md § Identity System](../../reference/fips/docs/design/fips-architecture.md)
and the upstream [fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md).)

---

## Device identity vs nsite author identity

These are **two different kinds of key** and the docs keep them strictly apart:

- The **device key** (above) is the one Nostr keypair this device holds. It is
  the mesh address, the BLE link identity, and the address of *this* device's
  relay+Blossom. The app holds its secret key.
- An **nsite author key** is an **external** key belonging to whoever authored a
  site, somewhere else, with external tooling. The app **never** holds an
  author's secret key and **never** signs on their behalf. An author key shows up
  in exactly two places: as the **URL host** (`<npub_author>.nsite`) and as the
  `authors` filter in a relay query (`{kinds:[15128,35128], authors:[<author>]}`).

The load-bearing consequence: **the site you want and the peer you fetch it from
are different keys.** A site is identified by its *author* npub; you fetch it from
a *holder* — any device that has cached the author's signed events and blobs and
re-serves them — reached on the mesh at `<npub_holder>.fips`. A holder is not the
author; re-serving an author-signed event relay-to-relay is normal relay
behaviour, not authorship. So to fetch a site, you query a holder's relay with
`{kinds:[15128 or 35128], authors:[<author_pubkey>]}` — the holder's own mesh
address has nothing to do with the author you are filtering on.

---

## `.fips` vs `.nsite`

Myco intercepts two TLDs at the device's DNS layer. They serve different
purposes and resolve to different address families. Keeping them distinct is
load-bearing.

### `.fips` — the mesh (IPv6-only)

`<npub>.fips` and `<alias>.fips` resolve to the node's `fd00::` IPv6 address
(an **AAAA** record only). This is how mesh traffic addresses a remote node.
`<npub>.fips` is purely *derivational* (the address is `fd` + SHA256(npub)[0:15],
computable by anyone), whereas `<alias>.fips` depends on a **device-local
alias→npub mapping** the interceptor holds — so aliases are local nicknames, not
globally resolvable names.
Queries that are *not* `.fips` are **REFUSED**, so the system falls through to
normal DNS for everything else. Myco only ever speaks `.fips` for
relay/blossom **sync** traffic — never for page loads.

(See [fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md).)

### `.nsite` — the local gateway (IPv4/localhost)

`*.nsite` resolves to `127.0.0.1` (an **A** record). This is deliberately
IPv4 and on localhost, and that choice matters: because it is an IPv4 loopback
address, `.nsite` works in **any** browser, including Chromium. (Chromium
suppresses AAAA/ULA answers, which only ever affected the IPv6-only `.fips`
namespace; it cannot break an IPv4 loopback target.) An nsite is always loaded
from `http://<host>.nsite`, never over `.fips`.

The `<host>` label follows the nsite URL convention:

- **root site** → `npub1…` (the author's npub as the single DNS label),
- **named site** → `<pubkeyB36><dTag>` where `pubkeyB36` is the 50-character
  base36 encoding of the raw 32-byte pubkey and `dTag` is the site identifier
  appended directly after it (no separator).

(See [reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md).)

**Why the split exists in one sentence:** `.fips` is the transport namespace
(IPv6, mesh, sync-only); `.nsite` is the presentation namespace (IPv4,
localhost, what the WebView loads). The browser never resolves `.fips`.

---

## What an nsite is — and "nsites as apps"

An **nsite** is a static website published on Nostr **by its author, using
external nsite tooling** — Myco never authors one. It is not files on a
server; it is a signed Nostr event plus content-addressed blobs:

- a **manifest event** whose tags map absolute paths to blob hashes, e.g.
  `["path","/index.html","<sha256>"]`. The manifest is a Nostr event, so it is
  signed by the author and self-authenticating. Myco stores and re-emits this
  event **unmodified**, so the author's signature stays valid.
- each referenced file is a **blob**, stored and retrieved by its sha256 hash
  (Blossom BUD-01, content-addressed).

There are two manifest kinds:

| Kind | Meaning | `d` tag | URL host |
| --- | --- | --- | --- |
| **15128** | root site (one replaceable event per pubkey) | none | `npub1…` |
| **35128** | named site (parameterized-replaceable) | required | `<pubkeyB36><dTag>` |

(Kind `34128`, legacy per-file events, may be supported for backward
compatibility. See [nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md).)

**Nsites as apps.** Each nsite is presented as its **own fullscreen "app"**,
launched *by* Myco, not as a web page in a tabbed browser. **Myco itself is the
manager app** — its home is the **Library** (your installed nsites/apps), with
Pair, Discover, and Settings alongside. Tapping a Library entry launches that
nsite as a **separate fullscreen task** (its own Android Recents card), a WebView
with **no URL bar and no Myco chrome at all** — there is no fixed bottom bar, no
Back · Reload · Library, and no long-press-in-the-Library browser. Refresh and
in-app navigation are the **nsite developer's** responsibility, implemented
inside the nsite; Android Back and Recents handle task-level navigation, and you
return to Myco to *manage* a site (info, remove, re-pair, add-to-home-screen). In
v1, nsite content is **pure static** (HTML/CSS/JS with no privileged access); a
capability API (query peers, manage the cache) is a later milestone. The full
shell and launch model — separate-task launch, deep links (`myco://app/<npub>`),
home-screen pinning, per-nsite origin isolation — is in
[app-shell.md](./app-shell.md); the browse lifecycle is in
[diagrams/04-nsite-browse-flow.svg](./diagrams/04-nsite-browse-flow.svg).

---

## The embedded relay + Blossom server

Every Myco device runs **its own** Nostr relay and Blossom server in-process.
No external services are required.

- **Embedded Nostr relay** — default `ws://localhost:4869`. Stores and serves the
  signed manifest events (kinds 15128/35128), and **fans them out** to connected
  peers (a relay-mesh multiplex — events only, source-excluded; see
  [propagation.md](./propagation.md)).
- **Embedded Blossom server** — default `http://localhost:24242`. Stores and
  serves the sha256-addressed blobs (Blossom BUD-01).

Both are implemented in **Rust**, unified with the FIPS endpoint into a single
`myco-core` crate (one `.so`, one FFI surface). They cache **other people's**
nsites: when you browse someone's nsite, your relay retains the author's signed
events and your Blossom store retains their blobs, so your device **becomes a new
holder** — a fresh source for that site. Re-serving those author-signed events is
ordinary relay behaviour, not authorship. This is the mechanism behind offline
propagation (below).

These services are exposed to peers over the mesh by FIPS **FSP port
multiplexing**, which delivers mesh datagrams to localhost ports. A reachable
peer reaches your relay at `<npub_device>.fips:4869` and your Blossom at
`<npub_device>.fips:24242` — no separate gateway is needed on a reachable path.
That address is *this device's* (the holder's) address; the nsites it serves are
filtered by their own author keys, which are unrelated to it.

(See [fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md),
[fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md),
and [reference/site-deck](../../reference/site-deck/).)

---

## The FIPS mesh, in one paragraph

**FIPS** is a self-organizing mesh network with no central authority. Nodes use
Nostr identities as addresses, authenticate each other, and route traffic for
each other across heterogeneous transports (UDP, TCP, Tor, BLE, …) without any
node knowing the full topology. A **spanning tree** forms by distributed parent
selection (root = the lexicographically smallest node_addr), giving every node a
coordinate; routing is **coordinate-based greedy**, decided locally at each hop.
Security is two layers of Noise: **IK hop-by-hop** (every link is encrypted) and
**XK end-to-end** (the payload is encrypted source-to-destination), so
intermediate nodes route on the destination node_addr but cannot read the
payload. Crucially, FIPS routing is **live-path only** — it is a best-effort
datagram service with no store-and-forward in the transport. (See
[fips-spanning-tree.md](../../reference/fips/docs/design/fips-spanning-tree.md),
[fips-mesh-layer.md](../../reference/fips/docs/design/fips-mesh-layer.md),
[fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md).)

---

## What we dropped vs what we keep

Myco is a fork of **nostr-vpn** (mmalmi), a Tailscale-style private mesh VPN
on a FIPS data plane. Myco strips the **private-network layer** and keeps
the FIPS transport plus the Android TUN.

### Dropped — "the private network"

The nostr-vpn product layer that turned the mesh into a managed private network
is removed:

- **exit-node selection** and **WireGuard upstream egress** (tunnel-all-internet),
- **roster/admin membership** (the signed roster, kind 30388) and
  **join-requests-as-membership**,
- **`.nvpn` MagicDNS**,
- **LAN-multicast pairing**.

Myco is not a VPN that carries all your internet traffic and not a
membership-gated network. There is no admin and no roster.

### Kept

- **The Android `VpnService`/TUN.** It is retained, but narrowed: it routes only
  `fd00::/8` (the mesh ULA) and DNS-intercepts `*.fips` and `*.nsite`,
  system-wide for every app on the phone. It does **not** capture `0.0.0.0/0` —
  there is no tunnel-all-internet. The TUN is what gives every app on the device
  `.fips`/`.nsite` resolution.
- **The FIPS mesh** (fips-core / fips-endpoint): identity, routing, two-layer
  Noise crypto, transports.
- **The embedding pattern** from nostr-vpn: link FIPS in-process via
  `FipsEndpoint::builder()` with `.without_system_tun()`, so the app owns the TUN
  and hands FIPS only packet bytes.
- **QR pairing** (CameraX + ML Kit), reused and re-pointed at the Myco
  payload (`myco://pair/<base64>` carrying npub + memorable name).

The provenance of every layer is tabulated in
[architecture.md](./architecture.md#reused-vs-net-new).

---

## Pillars of Propagation — live routing vs store-and-forward

The project's framing comes from nak's **"Pillars of Propagation"**: small
relays and Blossom blobs hopping over crappy links in all directions, surviving
outages via local propagation. Realizing that vision requires distinguishing two
mechanisms that Myco deliberately keeps separate:

- **Live-path routing (FIPS transport).** Multi-hop delivery between two nodes
  that are *currently* connected by some path through the mesh. This is what
  fips-core provides. It is best-effort and has **no store-and-forward** — if no
  live path exists, the datagram does not get delivered.
- **Store-and-forward (the nsite/relay layer).** The "cache an author's site
  now, re-serve it to a third device tomorrow when the original holder is gone"
  behaviour is **net-new** and lives *above* FIPS, at the relay+Blossom layer.
  Because each device's relay retains the author's signed events and each Blossom
  store retains their blobs, any device that has seen a site becomes an
  independent holder for it. The data is self-authenticating (signed events,
  content-addressed blobs), so **any source is trustworthy** regardless of who
  relays it.

The **proposed default propagation mode is hybrid**: *announce availability
widely, pull content on demand.* What floods is the author-signed **manifest
event** itself (kind 15128 / 35128) — small, self-authenticating, and re-emitted
**unmodified** by relays so the author's signature stays valid. There is no new
"announcement" kind and no holder signature: re-emitting an author's manifest is
plain relay behaviour. "Announce widely" = flood those manifest events with a TTL
of 5 hops; "pull on demand" = fetch the large **blobs** only when a site is
opened. Discovery ("nsites around me") is just the manifests you have received via
flood or queried from reachable relays. This split — FIPS for the live hop, the
nsite layer for survival across partition — is drawn in
[diagrams/03-offline-propagation.svg](./diagrams/03-offline-propagation.svg).

---

## Glossary (quick reference)

| Term | Meaning |
| --- | --- |
| **npub** | bech32 Nostr public key; for the **device** it is mesh/UI/BLE identity, for an **author** it is the nsite URL host + query filter |
| **device key** | this device's one Nostr keypair: mesh address, BLE link auth, relay/Blossom address; never authors nsites |
| **nsite author key** | an *external* key that signed a site; appears only as `<npub_author>.nsite` and the `authors` query filter; its secret key is never held by the app |
| **holder** | a device that has cached an author's signed events + blobs and re-serves them; reached at `<npub_holder>.fips`; distinct from the author |
| **node_addr** | `SHA256(npub)[0:16]`; the FIPS routing identifier |
| **fd00:: IPv6** | `fd ‖ node_addr[0:15]`; the ULA overlay address used via the TUN |
| **`.fips`** | DNS namespace → `fd00::` (AAAA); mesh sync traffic only; non-`.fips` queries refused |
| **`.nsite`** | DNS namespace → `127.0.0.1` (A); what the WebView loads; works in any browser |
| **nsite** | a static website authored on Nostr by external tooling: a signed manifest event + sha256 blobs |
| **manifest** | the Nostr event whose `path` tags map paths → blob hashes (kind 15128 root / 35128 named) |
| **blob** | a content-addressed file, retrieved by sha256 (Blossom BUD-01) |
| **the Library** | the Myco home grid of nsites-as-apps |
| **embedded relay** | in-process Nostr relay, `ws://localhost:4869` |
| **embedded Blossom** | in-process blob server, `http://localhost:24242` |
| **FIPS mesh** | self-organizing, transport-agnostic mesh; live-path-only routing |
| **FMP / FSP** | FIPS Mesh Protocol (hop-by-hop, Noise IK) / FIPS Session Protocol (end-to-end, Noise XK) |
| **FSP port-mux** | delivery of mesh datagrams to localhost ports, exposing `<npub>.fips:4869`/`:24242` |
| **TUN** | the kept `VpnService`; routes `fd00::/8`, intercepts `.fips`/`.nsite`; not tunnel-all |
| **store-and-forward** | net-new nsite-layer caching that lets a device re-serve a site offline |
| **myco-core** | the app crate that wires `nsite-deck` + `myco-relay` + `myco-blossom` behind one FFI `.so`; the only crate that names FIPS or a concrete relay/Blossom |
| **nsite-deck** | the reusable nsite host (gateway + sync); impl-agnostic — consumes the storage + transport seams, knows nothing about FIPS/BLE/Android |
| **myco-relay / myco-blossom** | the generic, independently-reusable embedded relay / blob-store crates (impl `RelayBackend` / `BlobStore`) |
| **RelayBackend / BlobStore** | nsite-deck's **storage** seams (provided by myco-relay / myco-blossom; Citrine-forward = an alternate `RelayBackend`) |
| **PeerSource / FanoutSink** | nsite-deck's **transport** seams — pull from a peer / re-broadcast events — provided by `myco-core` over FIPS |
| **the private network we dropped** | nostr-vpn's exit-node / WireGuard / roster-admin / `.nvpn` / LAN-multicast layer |
