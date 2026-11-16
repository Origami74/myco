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
| 2 | **Neighborhoods and links** | Whom do I share with? | a link, a neighborhood |
| 3 | **Relay and Blossom** | Where does data rest, and how does it spread? | an event, a blob |
| 4 | **FIPS** | How do bytes get to another phone? | a peer, a link |

Two confusions the layering exists to prevent:

- **A FIPS peer is not a link.** Layer 4 connects to any FIPS node in range
  and calls it a *peer*. Layer 2 sets up a *link* between two people, from a
  one-time invitation. Only linked devices sharing an extension with us
  (*app peers*) may read your relay and Blossom over the mesh. A FIPS peer can
  be connected and a stranger; a linked person can be out of range. See
  [Neighborhoods and links](#layer-2--neighborhoods-and-links).
- **The device key is not the user key.** Layer 4's identity is the phone's;
  layer 1's is the person's, as seen by napplets. Each link has a key of its
  own. None is an app author's key. See [Keys](#keys).

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
`outbox`, `neighbor`, `resource`, `identity`), and Myco does them on its behalf,
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

## Layer 2 — Neighborhoods and links

A **neighborhood** is a propagation channel ("Household", "Sports club"). It
has no members and no admin. It is carried on **links**.

A **link** is a pairwise relationship between two devices, set up once from a
one-time **invitation** (`myco://link/…`: QR, share link or NFC tap). Each
side makes a **link key** for it. Each link carries the neighborhoods both
sides agreed to; an event arriving on one link is passed on to the device's
other links in that neighborhood, within local limits.

The link service runs on the auth plane, `<npub>.fips:4873/nh/…`. It is the
only port a stranger can reach.

An **app peer** is a linked device sharing an extension with us ([extensions.md](extensions.md)) (the
default). App peers are who Myco fetches apps, updates and the release APK
from, and the only devices the mesh relay and Blossom admit (the **app peer
gate**).

**Not the same as a FIPS peer.** FIPS connects to whatever compatible node is
in range; that is transport, and it carries no trust. A link is a decision two
people make. The docs say *peer* for layer 4 and *link* or *app peer* for
layer 2. Design: [nap-neighbor/](../nap-neighbor/README.md).

---

## Layer 3 — Relay and Blossom

Every phone runs **its own** Nostr relay (`ws://localhost:4870`) and Blossom
store (`http://localhost:24243`), in Rust, in-process. No external service is
required. Everything an app reads or writes rests here: manifests, blobs, chat,
napplet events, relay lists.

Both are reachable by app peers over the mesh at `<npub>.fips:4870` and
`<npub>.fips:24243` — the same numbers, so a peer dialling your `.fips` name
lands on your loopback service. Neighborhood traffic does not use them: it has
its own scoped store, reached through the link service on `:4873`
([nap-neighbor/data-plane.md](../nap-neighbor/data-plane.md)). What does not change is that a phone is a **holder**: a device that has an
author's signed events and blobs and re-serves them. A holder is not the
author; re-serving signed events is ordinary relay behaviour.

Between app peers, nsite and napplet manifests are forwarded so updates
spread ([../nsite/nsite-updates.md](../nsite/nsite-updates.md) §4). Other events
travel only in neighborhoods.

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

<a id="three-keys"></a>

## Keys

| Key | Layer | What it is | Where it appears |
| --- | --- | --- | --- |
| **device key** | 4 | this phone's Nostr keypair; generated on first launch (`identity.nsec`) | the mesh identity, FIPS link authentication, the relay/Blossom address `<npub>.fips` |
| **user key** | 1 | the person's Nostr keypair as napplets see it — the account in Settings; a guest generated on first launch, with a guest profile (kind `0`) and relay lists (kinds `10002` and `10050`), or their own `nsec` or a signer app | what a napplet publishes *as*; `identity.getPublicKey()` |
| **author key** | 1 | an app author's key, held elsewhere by external tooling | the nsite URL host, the `authors` filter in a query; never its secret |
| **link key** | 2 | one keypair per link, generated at setup (`links.json`) | link setup, NIP-98 auth on the link service, signing offers |

The device key never authors an app. The app never holds an author's secret
and never signs for them. The user key is separate from the device key so that
a napplet learns who someone is socially, never which hardware they are on.

### The device key's three forms

From one keypair, three addresses derive, naming the same phone at different
layers:

| Form | Value | Who uses it |
| --- | --- | --- |
| **npub** | bech32 secp256k1 public key | the UI, share payloads, Noise handshakes, the relay/Blossom address |
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
  connections to app peers, and the link service. The WebView never resolves it.
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
| **NAP** | a napplet capability domain (`relay`, `outbox`, `neighbor`, `resource`, `identity`, `shell`) from the [napplet registry](https://github.com/napplet/naps) |
| **grant** | the user's permission for a napplet to use a NAP; written at install review or on the app's Manage permissions sheet |
| **Apps** | the home grid of installed nsites and napplets (in code: the Library) |
| **neighborhood** | a propagation channel carried on links; no members, no admin |
| **link** | a pairwise relationship between two devices, set up from an invitation; carries neighborhoods |
| **link key** | a keypair made for one link; neither the device key nor the user key |
| **invitation** | a one-time `myco://link/…` payload (QR, share link, NFC) that sets up a link |
| **offer** | a link-key-signed event (kind `37516`) listing what a device serves on a link |
| **app peer** | a linked device sharing an extension with us; Myco fetches apps and updates from it |
| **app peer gate** | the check on every mesh connection to the relay or Blossom: app peer, or refused |
| **peer** | a FIPS node this phone has a transport link to; says nothing about trust |
| **Circle, pairing** | removed; replaced by neighborhoods and links |
| **holder** | a phone that has an author's events and blobs and re-serves them; reached at `<npub_holder>.fips` |
| **device key** | this phone's keypair: mesh identity, FIPS link auth, `<npub>.fips` |
| **user key** | the person's keypair for napplets: what they publish as |
| **author key** | an app author's external key: the URL host and the `authors` filter; never held |
| **node_addr** | `SHA256(npub)[0:16]`; the FIPS routing id |
| **fd00:: IPv6** | `fd ‖ node_addr[0:15]`; the ULA overlay address via the TUN |
| **`.fips`** | DNS → `fd00::` (AAAA); mesh sync only |
| **`.localhost`** | what the WebView loads; answered in-process, not DNS |
| **embedded relay** | in-process NIP-01 relay, `ws://localhost:4870`; `<npub>.fips:4870` to app peers |
| **embedded Blossom** | in-process blob store, `http://localhost:24243`; `<npub>.fips:24243` to app peers |
| **gateway** | the in-process resolver that answers a WebView's requests from relay + Blossom |
| **link service** | the neighborhood protocol over HTTP, `/nh/{pk}/…` on `:4873` |
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
