<p align="center"><img src="myco-logo.png" alt="Myco" width="200"></p>

# Myco

> **Install apps from the people around you** — over Bluetooth, with no internet
> and no app store. Meet someone, bump phones, and their apps appear on your
> phone, ready to use offline. Anything you install you can pass on — so apps
> spread from phone to phone, on their own.

![Install apps from the people around you](design/diagrams/intro-01-what-it-is.svg)

![Get started in 3 steps](design/diagrams/intro-02-get-started.svg)

![How apps spread](design/diagrams/intro-03-how-it-spreads.svg)

![Apps you install live on your home screen](design/diagrams/intro-04-on-your-homescreen.svg)

![Every app gets its own window](design/diagrams/intro-05-each-its-own-app.svg)

---

## What it is

Myco is a different kind of app store. Instead of downloading from a
company's servers, you **install apps from the people around you** — over
Bluetooth, Wi-Fi Aware or the local network, with no internet connection.

Meet someone, **pair** with a bump or a QR scan, and their apps land in your
**Apps** grid, ready to use offline. Anything you install you can pass on to the
next person, so apps spread from phone to phone on their own — no servers, no
single point that has to stay online.

## Two kinds of app

- An **nsite** is a static website published on Nostr. Myco stores its signed
  files and serves them to a WebView. It is a document Myco *serves*.
- A **napplet** is a program: a sandboxed page with a permission model. It asks
  Myco for things — relays, the mesh, pictures — and Myco decides. It is a
  program Myco *hosts*.

Both arrive the same way (a signed manifest plus content-addressed files),
both live in the same grid, both open as their own full-screen app. What differs
is trust: an nsite gets nothing, a napplet gets exactly what you granted it.

## Pairing

Getting started takes one in-person hello:

1. Open **Circle**.
2. **Bump phones** (NFC), or show your code and let a friend scan it.
3. You're paired — both ways. They join your **Circle**, and apps can flow in
   either direction between you.

Your code carries a **memorable name** so the people you pair with remember
who you are. Pairing is always mutual: a one-time secret in the code proves the
scan happened, and the other phone confirms.

---

## For developers

Four layers, top to bottom. Each one only knows the one below it.

| Layer | What it is | Where |
| --- | --- | --- |
| **1. Apps** — nsites and napplets | The manifest and file model both share; the gateway that serves an nsite; the runtime that hosts a napplet and its capabilities (NAPs). | `nsite-deck`, `myco-napplet-runtime`, `NsiteActivity`, `NappletActivity` |
| **2. Circles and pairing** | Who you trust. A Circle is your list of paired people — a *virtual* mesh laid over the physical one, built on purpose, one mutual signed handshake at a time. It decides who may read your relay and store, whose apps you pull, and who a napplet's "everyone nearby" is. | `content.rs` (Circle, pairing, gate), `auth_service.rs`, `nfc/`, `share/` |
| **3. Relay and Blossom** | Your phone's own Nostr relay and blob store. Every app's data rests here; peers sync from here. Hop-limited gossip carries events between Circle members. | `myco-relay`, `myco-blossom`, `mesh_relay.rs`, `gossip.rs`, `peer_relay.rs` |
| **4. FIPS** | The mesh. Encrypted links between phones over BLE, Wi-Fi Aware, LAN or the internet; IPv6 addresses derived from device keys; `<npub>.fips` names. | `reference/fips`, `ble/`, `aware/`, `ap/`, the TUN |

**Pairing is not a FIPS peer.** Layer 4 will happily hold an encrypted link to
any phone running FIPS in range — that is a *peer*, and it says nothing about
trust. Layer 2's *pairing* is a Myco decision made by two people: it is what
lets a peer read your relay, pull your apps, and send you files. A phone can be
a connected FIPS peer and a stranger at the same time; the content ports refuse
it. A Circle member can be out of range; they are still in your Circle. The
docs use *peer* for layer 4 and *Circle member* (or *paired*) for layer 2, and
never swap them.

**Two keys, not one.** The **device key** is layer 4's identity: the mesh
address, the link authentication, the name of this phone's relay
(`<npub>.fips`). The **user key** is layer 1's: what a napplet publishes *as*,
generated the first time a napplet runs. Neither is ever an app author's key —
apps are authored elsewhere, and Myco only ever holds and re-serves their
signed events.

Rust owns layers 2–4 and the runtime half of layer 1, in one `libmyco_core.so`
behind a JSON reducer over JNI. Kotlin owns the UI, the WebViews, the radios
and the `VpnService`. Start with [concepts.md](./design/core/concepts.md), then
[architecture.md](./design/core/architecture.md).

---

## Documentation index

### Design

#### `core/` — the system and its identities

| Doc | Description |
| --- | --- |
| [concepts.md](./design/core/concepts.md) | Glossary: the four layers, device key vs user key vs author key, peer vs Circle member, `.fips` vs `.localhost`, nsite vs napplet. Read this first. |
| [architecture.md](./design/core/architecture.md) | The stack on one phone: the crates, the Kotlin↔Rust boundary, what each layer owns. |
| [app-shell.md](./design/core/app-shell.md) | The launch model: the manager app versus each nsite/napplet as its own full-screen task; intents, Recents, home-screen pins, origin isolation. |
| [deep-links.md](./design/core/deep-links.md) | `myco://app/<host>/<path>`: a link that names an app *and* a place inside it. |
| [identity-pairing.md](./design/core/identity-pairing.md) | The device identity and the pairing handshake: the `myco://pair/` payload, the auth service, NFC tap-to-pair, unpairing. (What a pairing *means* is [circle.md](./design/circle/circle.md).) |
| [event-gossip.md](./design/core/event-gossip.md) | Layer 3's push and pull planes: hop-limited flooding between Circle members, the `MESH` envelope, the seen-set. |
| [security.md](./design/core/security.md) | Trust model: self-authenticating data, FIPS link crypto, the Circle gate, the nsite sandbox, the napplet sandbox and its grants. |

#### `circle/` — who you trust

| Doc | Description |
| --- | --- |
| [circle.md](./design/circle/circle.md) | The Circle as a virtual mesh over the physical FIPS mesh: a web of trust built intentionally, what it decides (admission, sources, gossip, napplet reach, files), what it is not, and where the member channel is going. |

#### `nsite/` — apps that are documents

| Doc | Description |
| --- | --- |
| [nsite-layer.md](./design/nsite/nsite-layer.md) | The embedded relay, Blossom, and in-process gateway; the manifest/URL scheme; resolve→cache→serve; sync from a Circle member. |
| [propagation.md](./design/nsite/propagation.md) | How a site spreads: flood the signed manifest, pull blobs on demand, dedupe, retain. |
| [nsite-updates.md](./design/nsite/nsite-updates.md) | How a site gets a new version: discovery, staged download, activation, mesh propagation. |
| [nsite-permissions.md](./design/nsite/nsite-permissions.md) | Per-peer grants: what a Circle member may do to this phone. (Per-app capabilities live in the napplet runtime.) |

#### `napplet/` — apps that are programs

| Doc | Description |
| --- | --- |
| [napplet-runtime.md](./design/napplet/napplet-runtime.md) | NIP-5D manifests over the nsite shape, verified resolve into a sandboxed iframe, the NAP capability seam, grants and the review screen, the three relay lanes. |
| [NAP-MESH.md](./design/napplet/NAP-MESH.md) | Myco's own capability: hop-limited publish and subscribe over the mesh, in the registry's template so it can be proposed upstream. |

#### `fips/` — the transport lanes

| Doc | Description |
| --- | --- |
| [ble-interop.md](./design/fips/ble-interop.md) | BLE L2CAP over fips's `BleIo` seam: per-peer PSM discovery, MAC randomization, the foreground service. |
| [wifi-aware-interop.md](./design/fips/wifi-aware-interop.md) | Wi-Fi Aware as a bulk lane: Kotlin raises the data path, fips's UDP transport dials it. |
| [ap-lane.md](./design/fips/ap-lane.md) | The LAN lane: same-network peers over ordinary UDP, found by mDNS. |
| [usb-transport.md](./design/fips/usb-transport.md) | Proposed, not started: USB/AOA for seeding large sites. |

#### Shared assets

| Doc | Description |
| --- | --- |
| [diagrams/](./design/diagrams/README.md) | Design diagrams. |
| [mockups/](./design/mockups/README.md) | Early UI mockups — historical; the screens moved on. |

### Reference

| Doc | Description |
| --- | --- |
| [ports.md](./reference/ports.md) | Relay `4870`, Blossom `24243`, auth `4873`, and how each is or is not exposed over the mesh — the mesh ports are deprecated. |
| [nostr-kinds.md](./reference/nostr-kinds.md) | Every event kind Myco reads, stores, publishes or replicates. |
| [settings.md](./reference/settings.md) | What is persisted in `settings.json`, and what is not. |
| [ffi-surface.md](./reference/ffi-surface.md) | The Kotlin↔Rust contract: the JSON reducer, every action, the state snapshot, the napplet and radio entry points. |

### How-to

| Doc | Description |
| --- | --- |
| [build.md](./how-to/build.md) | Build the Rust core and the arm64 APK; the local `reference/fips` checkout. |
| [run-two-device-demo.md](./how-to/run-two-device-demo.md) | Two phones, airplane mode: pair, share an app, browse it offline. |
| [publish.md](./how-to/publish.md) | Release to GitHub Releases and Zapstore. |
| [exit-node-demo.md](./how-to/exit-node-demo.md) | Experimental: a BLE-only phone browsing the internet through a mesh exit node. |

---

## Where to start

- New here? [getting-started.md](./getting-started.md), then
  [concepts.md](./design/core/concepts.md).
- What's next? The [roadmap](./roadmap.md).
- Building it? [how-to/build.md](./how-to/build.md) →
  [how-to/run-two-device-demo.md](./how-to/run-two-device-demo.md).
- The mesh underneath: [FIPS docs](../reference/fips/docs/README.md).
