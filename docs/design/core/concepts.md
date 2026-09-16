# Concepts and Glossary

The canonical vocabulary for **Myco**. Other design docs build on the terms
fixed here. For the system structure see [architecture.md](./architecture.md);
for the mesh underneath, the upstream FIPS docs:
[fips-concepts.md](../../../reference/fips/docs/design/fips-concepts.md) and
[fips-architecture.md](../../../reference/fips/docs/design/fips-architecture.md).

Myco is a peer-to-peer **app-sharing network**: a phone app for exchanging,
running and propagating apps published on Nostr over a FIPS mesh — including
fully offline over Bluetooth.

---

## The four layers

Everything in Myco sits in one of four layers. Each knows only the one below it.

| # | Layer | Question it answers | Unit |
| --- | --- | --- | --- |
| 1 | **Apps** — nsites and napplets | What runs, and what may it do? | a manifest + its files |
| 2 | **Circles and pairing** | Whom do I trust? | a Circle member |
| 3 | **Relay and Blossom** | Where does data rest, and how does it spread? | an event, a blob |
| 4 | **FIPS** | How do bytes get to another phone? | a peer, a link |

Two confusions the layering exists to prevent:

- **A peer is not a Circle member.** Layer 4 links to any FIPS node in range
  and calls it a *peer*. Layer 2 *pairs* two people, by a signed handshake, and
  calls the result a *Circle member*. Only Circle members may read your relay
  and store. A peer can be connected and a stranger; a Circle member can be out
  of range. See [Circles and pairing](#layer-2--circles-and-pairing).
- **The device key is not the user key.** Layer 4's identity is the phone's;
  layer 1's is the person's, as seen by napplets. Neither is an app author's
  key. See [Three keys](#three-keys).

---

## Layer 1 — Apps: nsites and napplets

Both kinds arrive the same way: a **manifest** — a signed Nostr event whose
`path` tags map paths to sha256 hashes — plus the files those hashes name,
stored in Blossom. Both sit in the same **Apps** grid and open as their own
full-screen task. What differs is what Myco does with the bytes.

### nsite — a document Myco serves

A static website published on Nostr by its author with external tooling. Myco
never authors one. The manifest kinds are `15128` (root site, one per author)
and `35128` (named site, with a `d` tag). Myco stores the manifest and blobs and
serves them to a WebView through an in-process gateway; the page gets no
privileges beyond a browser's. Its identity is its **author key**.

### napplet — a program Myco hosts

A NIP-5D manifest (kinds `5129` / `15129` / `35129`) over the same shape, whose
content is a page Myco runs in a sandboxed iframe. A napplet has no network: it
asks Myco for things through a capability seam (the **NAPs** — `relay`,
`outbox`, `mesh`, `resource`, `identity`), and Myco does them on its behalf,
only where the user **granted** it. Its identity is its **aggregate hash** —
every build is a different napplet. Design: [../napplet/napplet-runtime.md](../napplet/napplet-runtime.md).

### The URL host

The WebView loads an app at `http://<host>.localhost/` and every request is
answered in-process by the gateway (`shouldInterceptRequest`) — no socket, no
DNS, no TUN needed. `.localhost` because Chromium treats it as loopback and a
secure context, which is what lets a page open `ws://localhost:4870` to the
embedded relay. The `<host>` label is the nsite convention:

- **root site** → `npub1…` (the author's npub),
- **named site** → `<pubkeyB36><dTag>` (50-character base36 pubkey, then the
  `d` tag, no separator).

A napplet's window is `<label>.napplet.localhost`. The `.nsite` TLD is *not*
used by the app; it survives only as the public gateways' suffix (`nsite.lol`).

---

## Layer 2 — Circles and pairing

A **Circle** is this phone's list of paired people, persisted locally
(`circle.json`). A **pairing** is a mutual, signed handshake between two Myco
installs: one presents a one-time secret (NFC tag or QR), the other posts a
signed **pair request** (kind `9101`) carrying it to the presenter's auth
service at `<npub>.fips:4873`, and the presenter answers with a signed
**pair accept** (`9102`). Forgetting a peer posts a **pair remove** (`9103`).
Design: [identity-pairing.md](./identity-pairing.md).

Pairing is what admits a phone to the content ports. The relay and Blossom
servers gate every mesh connection on Circle membership (the **Circle gate**),
so a FIPS peer that is not paired can reach exactly one thing: the auth
service, to ask to pair. A Circle member is also who Myco *pulls from* — apps,
backlog, blobs — and who receives gossip.

**Not the same as a FIPS peer.** FIPS forms links with whatever compatible
node is in range; that is transport, and it carries no trust. The Circle is a
Myco-level decision, made by two people, stored on both phones — a *virtual*
mesh over the physical one, built intentionally. The docs say *peer* for layer
4 and *Circle member* or *paired* for layer 2. Design:
[../circle/circle.md](../circle/circle.md).

---

## Layer 3 — Relay and Blossom

Every phone runs **its own** Nostr relay (`ws://localhost:4870`) and Blossom
store (`http://localhost:24243`), in Rust, in-process. No external service is
required. Everything an app reads or writes rests here: manifests, blobs, chat,
napplet events, relay lists.

Both are reachable by Circle members over the mesh at `<npub>.fips:4870` and
`<npub>.fips:24243` today — the same numbers, so a peer dialling your `.fips`
name lands on your loopback service. These **mesh ports are deprecated**: a
Circle-owned channel will replace them ([../circle/circle.md](../circle/circle.md) §6).
What does not change is that a phone is a **holder**: a device that has an
author's signed events and blobs and re-serves them. A holder is not the
author; re-serving signed events is ordinary relay behaviour.

Between Circle members, events also travel by **gossip**: a hop-limited flood
(push plane, default 3 hops) and a hop-limited backlog pull (pull plane, default
2 hops), carried in a `MESH` envelope beside plain NIP-01. Design:
[event-gossip.md](./event-gossip.md).

---

## Layer 4 — FIPS

**FIPS** is a self-organizing mesh with no central authority. Nodes use Nostr
keys as identities, authenticate each other with Noise (IK hop-by-hop, XK
end-to-end), form a spanning tree, and route greedily on coordinates across
whatever transports they have — BLE L2CAP, Wi-Fi Aware, LAN UDP, TCP, Tor.
Routing is **live-path only**: a datagram is delivered now or not at all; there
is no store-and-forward in the transport. Store-and-forward is layer 3's job.

Myco embeds a FIPS node and gives it four lanes: BLE (Kotlin owns the radio,
fips owns the protocol), Wi-Fi Aware, the LAN, and — when a phone has internet
— TCP. The node's IPv6 side is an app-owned `VpnService` TUN that routes only
`fd00::/8` and answers `.fips` DNS. Design: [../fips/](../fips/).

---

## Three keys

| Key | Layer | What it is | Where it appears |
| --- | --- | --- | --- |
| **device key** | 4 | this phone's Nostr keypair; generated on first launch (`identity.nsec`) | the mesh identity, link authentication, the relay/Blossom address `<npub>.fips`, pairing |
| **user key** | 1 | the person's Nostr keypair as napplets see it; generated the first time a napplet runs, with a guest profile (kind `0`) and relay list (kind `10002`) | what a napplet publishes *as*; `identity.getPublicKey()` |
| **author key** | 1 | an app author's key, held elsewhere by external tooling | the nsite URL host, the `authors` filter in a query; never its secret |

The device key never authors an app. The app never holds an author's secret
and never signs for them. The user key is separate from the device key so that
a napplet learns who someone is socially, never which hardware they are on.

### The device key's three forms

From one keypair, three addresses derive, naming the same phone at different
layers:

| Form | Value | Who uses it |
| --- | --- | --- |
| **npub** | bech32 secp256k1 public key | the UI, pairing payloads, Noise handshakes, the relay/Blossom address |
| **node_addr** | `SHA256(npub)[0:16]` | FIPS routing (headers, spanning tree, bloom filters) |
| **fd00:: IPv6** | `fd` ‖ `node_addr[0:15]` | ordinary IPv6 software, via the TUN |

The node_addr is a one-way hash, so routers forward without learning the Nostr
identity of either endpoint. (See
[fips-architecture.md § Identity](../../../reference/fips/docs/design/fips-architecture.md).)

---

## `.fips` vs `.localhost`

- **`.fips`** is the transport namespace: `<npub>.fips` → that phone's
  `fd00::` address (AAAA), answered by the node's own resolver through the TUN.
  IPv6 only, mesh only. This is what **sync** talks to — relay and Blossom
  connections to Circle members. The WebView never resolves it.
- **`.localhost`** is the presentation namespace: what the WebView loads, served
  in-process by the gateway. It is not DNS at all.

**The site you want and the peer you fetch it from are different keys.** A site
is named by its *author*; you fetch it from a *holder*, at `<npub_holder>.fips`,
with a query filtered on the author: `{kinds:[35128], authors:[<author>]}`.

---

## Pillars of Propagation — live routing vs store-and-forward

The framing is nak's "Pillars of Propagation": small relays and Blossom blobs
hopping over bad links in every direction, surviving outages by local
propagation. Two mechanisms, kept apart:

- **Live-path routing (layer 4).** Multi-hop delivery between phones connected
  *now*. Best-effort; no store-and-forward.
- **Store-and-forward (layer 3).** Cache an author's site today, re-serve it to
  a third phone tomorrow when the original holder is gone. Every phone that has
  seen a site becomes an independent holder. The data is self-authenticating,
  so any source is trustworthy regardless of who relays it.

Propagation is **hybrid**: what floods is the author-signed manifest, small and
re-emitted unmodified (3 hops); the large blobs are pulled on demand when a site
is opened. Design: [../nsite/propagation.md](../nsite/propagation.md).

---

## Glossary

| Term | Meaning |
| --- | --- |
| **nsite** | a static site published on Nostr by external tooling: a signed manifest (`15128`/`35128`) + sha256 blobs; served to a WebView |
| **napplet** | a NIP-5D program (`5129`/`15129`/`35129`): the same shape, run in a sandboxed iframe with granted capabilities |
| **manifest** | the signed event whose `path` tags map paths to blob hashes |
| **blob** | a content-addressed file, by sha256 (Blossom BUD-01) |
| **aggregate hash** | the hash over a manifest's `path` entries; an nsite's integrity check, a napplet's identity |
| **NAP** | a napplet capability domain (`relay`, `outbox`, `mesh`, `resource`, `identity`, `shell`) from the [napplet registry](https://github.com/napplet/naps) |
| **grant** | the user's permission for a napplet to use a NAP; written at install review or on the app's Manage permissions sheet |
| **Apps** | the home grid of installed nsites and napplets (in code: the Library) |
| **Circle** | this phone's list of paired people, persisted locally |
| **Circle member / paired** | someone in the Circle; admitted to the relay and Blossom over the mesh |
| **peer** | a FIPS node this phone has a link to; says nothing about trust |
| **pairing** | the mutual signed handshake (kinds `9101`/`9102`/`9103`) over the auth service on `:4873` |
| **Circle gate** | the check on every mesh connection to the relay or Blossom: paired, or refused |
| **holder** | a phone that has an author's events and blobs and re-serves them; reached at `<npub_holder>.fips` |
| **device key** | this phone's keypair: mesh identity, link auth, `<npub>.fips`, pairing |
| **user key** | the person's keypair for napplets: what they publish as |
| **author key** | an app author's external key: the URL host and the `authors` filter; never held |
| **node_addr** | `SHA256(npub)[0:16]`; the FIPS routing id |
| **fd00:: IPv6** | `fd ‖ node_addr[0:15]`; the ULA overlay address via the TUN |
| **`.fips`** | DNS → `fd00::` (AAAA); mesh sync only |
| **`.localhost`** | what the WebView loads; answered in-process, not DNS |
| **embedded relay** | in-process NIP-01 relay, `ws://localhost:4870`; `<npub>.fips:4870` to Circle members |
| **embedded Blossom** | in-process blob store, `http://localhost:24243`; `<npub>.fips:24243` to Circle members |
| **gateway** | the in-process resolver that answers a WebView's requests from relay + Blossom |
| **gossip** | hop-limited flood (push) and backlog pull between Circle members, in a `MESH` envelope |
| **lane** | one transport under FIPS: `ble`, `aware`, `udp` (LAN), `tcp` |
| **FIPS mesh** | self-organizing, transport-agnostic, live-path-only routing |
| **FMP / FSP** | FIPS Mesh Protocol (hop-by-hop, Noise IK) / FIPS Session Protocol (end-to-end, Noise XK) |
| **TUN** | the app-owned `VpnService`, scoped to Myco's uid; routes `fd00::/8`, answers `.fips` |
| **myco-core** | the app crate: wires everything behind one `libmyco_core.so` and a JSON reducer |
| **nsite-deck** | the reusable nsite host (gateway + sync); reaches the world only through four seams |
| **myco-napplet-runtime** | the napplet host: resolve, sandbox, NAPs; reaches the world only through seams |
| **myco-relay / myco-blossom** | the embedded relay / blob-store crates |
| **RelayBackend / BlobStore** | the storage seams (implemented by myco-relay / myco-blossom, or a custom relay/Blossom the user points at) |
| **PeerSource / FanoutSink** | nsite-deck's transport seams — pull from a holder / re-broadcast — provided by myco-core over FIPS |
