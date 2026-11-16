<p align="center"><img src="myco-logo.png" alt="Myco" width="200"></p>

# Myco

> **Hand a useful app to the people around you.** Phone to phone, and it
> keeps opening when the internet is gone.

![Your apps live on your home screen and open like any app](design/diagrams/intro-01-your-apps.svg)

![Apps from the people you trust — over whatever mesh is around](design/diagrams/intro-02-what-it-is.svg)

---

## What it is

Someone in the room has an app that helps: a checklist, a game, a way to ring
each other. With Myco they invite you into a neighborhood and hand it
over, and the app is on
your phone too. It opens full-screen like any other app, and it still opens
with no signal. Once you have it, you can hand it on to the next person.

Apps travel over Bluetooth, Wi-Fi Aware or the local network, with no
internet needed. The [project README](../README.md) has the short version for
people joining a group, organizing one, or building apps.

## Napplets first, nsites as a bonus

- A **napplet** is a program: a sandboxed page with a permission model. It
  asks Myco for things — an identity, relays, neighborhoods, pictures — and gets
  exactly what you granted. It is a program Myco *hosts*, and what Myco is
  built to run.
- An **nsite** is a static website published on Nostr. Myco stores its signed
  files and serves them to a WebView, so it works offline once it is on the
  phone. It is a document Myco *serves*, and it gets no grants.

Both arrive the same way (a signed manifest plus content-addressed files),
both live in the same grid, both open as their own full-screen app.

## Neighborhoods

Getting started takes one in-person hello. Open **Neighbors**, tap
**Invite**, and pick a neighborhood ("Household", "Sports club") or make one.
The other person taps **Join** and:

- **bumps phones** (NFC), or
- **scans your code**, or
- opens the link you sent.

You now share that neighborhood, and a link between your two phones carries
it. Apps and updates can flow in either direction between you while the
neighborhood's **Install updates from people here** and **Send my updates
to people here** switches are on (the default). Your name travels in the invitation only, never over
Bluetooth. A neighborhood has no members list and no admin: posts travel link
to link, so it is not private.

---

## For developers

Four layers, top to bottom. Each one only knows the one below it.

| Layer | What it is | Where |
| --- | --- | --- |
| **1. Apps** — nsites and napplets | The manifest and file model both share; the gateway that serves an nsite; the runtime that hosts a napplet and its capabilities (NAPs). | `nsite-deck`, `myco-napplet-runtime`, `NsiteActivity`, `NappletActivity` |
| **2. Neighborhoods and links** | Who you share with. A link is set up on purpose from a one-time invitation, with its own link key; it carries neighborhoods — propagation channels with no members — and posts travel link to link. Apps and updates travel the same way, as *extensions* with a Get and a Share switch per neighborhood. The link service on `:4873` is the only port open to other phones. | `myco-core/src/neighbor/`, `auth_service.rs` (the link service), `nap/neighbor/`, Android `neighbor/`, `nfc/`, `share/` |
| **3. Relay and Blossom** | Your phone's own Nostr relay and blob store. Every app's data rests here. Loopback and in-process only: other phones never reach them. | `myco-relay`, `myco-blossom`, `myco-cache`, `tiered.rs` |
| **4. FIPS** | The mesh. Encrypted links between phones over BLE, Wi-Fi Aware, LAN or the internet; IPv6 addresses derived from device keys; `<npub>.fips` names. | `reference/fips`, `ble/`, `aware/`, `ap/`, the TUN |

**A link is not a FIPS peer.** Layer 4 will happily hold an encrypted
connection to any phone running FIPS in range — that is a *peer*, and it says
nothing about trust. Layer 2's *link* is a Myco decision made by two people:
it is what lets a peer reach the neighborhoods you share with it, and nothing
else on the phone. A phone can be a connected FIPS peer and a stranger at the
same time; the link service refuses it. A linked person can be out of range;
the link stays. The docs use *peer* for layer 4 and *link* for layer 2, and
never swap them.

**Two keys, plus one per link.** The **device key** is the FIPS mesh key, layer 4's
identity: the mesh address, the link authentication, the name of this phone's
relay (`<npub>.fips`). The **user key** is the Nostr key, the social one and
layer 1's: what a napplet publishes *as*. It is a guest key from the first
launch, or your own `nsec` or a signer app. Each link adds a **link key**,
used only for that link. None is ever an app author's key —
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
| [concepts.md](./design/core/concepts.md) | Glossary: the four layers, device key vs user key vs link key vs author key, peer vs link, `.fips` vs `.localhost`, nsite vs napplet. Read this first. |
| [architecture.md](./design/core/architecture.md) | The stack on one phone: the crates, the Kotlin↔Rust boundary, what each layer owns. |
| [app-shell.md](./design/core/app-shell.md) | The launch model: the manager app versus each nsite/napplet as its own full-screen task; intents, Recents, home-screen pins, origin isolation. |
| [deep-links.md](./design/core/deep-links.md) | `myco://app/<host>/<path>`: a link that names an app *and* a place inside it. |
| [identity.md](./design/core/identity.md) | The device identity: one key, its derived forms, where it is stored. |
| [security.md](./design/core/security.md) | Trust model: self-authenticating data, FIPS link crypto, links and what they authorize, the nsite sandbox, the napplet sandbox and its grants. |

#### `nap-neighbor/` — neighborhoods

| Doc | Description |
| --- | --- |
| [README.md](./design/nap-neighbor/README.md) | Overview: what a neighborhood is, the three keys, files on the device, what the Circle's removal took with it. |
| [links.md](./design/nap-neighbor/links.md) | Invitations (`myco://link/`), the setup proof, link keys, offers (kind 37516), NIP-98, the link service on `:4873`, presence. |
| [data-plane.md](./design/nap-neighbor/data-plane.md) | The scoped store and blobs, the scoped relay, publish and subscribe, hops and dedup. |
| [extensions.md](./design/nap-neighbor/extensions.md) | How apps and updates travel: `napplet-updates`, `nsite-updates`, `myco-release` with Get and Share per neighborhood; where a missing app is fetched from. |
| [policy.md](./design/nap-neighbor/policy.md) | Per-neighborhood settings: reach presets, blob push hops, extensions. |
| [napplet-api.md](./design/nap-neighbor/napplet-api.md) | What NAP-NEIGHBOR gives napplets: `neighborhood://` addresses and how relay, outbox, resource and upload carry them. |
| [ui.md](./design/nap-neighbor/ui.md) | The Neighbors tab, the neighborhood page, invite and join. |

#### `nsite/` — apps that are documents

| Doc | Description |
| --- | --- |
| [nsite-layer.md](./design/nsite/nsite-layer.md) | The embedded relay, Blossom, and in-process gateway; the manifest/URL scheme; resolve→cache→serve; fetching a missing app. |
| [propagation.md](./design/nsite/propagation.md) | How a site spreads: share the signed manifest into neighborhoods, pull blobs on demand, dedupe, retain. |
| [nsite-updates.md](./design/nsite/nsite-updates.md) | How a site gets a new version: discovery, staged download, activation, mesh propagation. |
| [nsite-permissions.md](./design/nsite/nsite-permissions.md) | What another phone may do to this one (only the link service), and where per-app capabilities live. |

#### `napplet/` — apps that are programs

| Doc | Description |
| --- | --- |
| [napplet-runtime.md](./design/napplet/napplet-runtime.md) | NIP-5D manifests over the nsite shape, verified resolve into a sandboxed iframe, the NAP capability seam, grants and the review screen, the three relay lanes. |
| [NAP-NEIGHBOR](https://github.com/napplet/naps/pull/146) | Neighborhoods for napplets: list, peers, `neighborhood://` addresses for relay, outbox, resource and upload. Specified upstream; Myco's side is in [nap-neighbor/](./design/nap-neighbor/README.md). |
| [NAP-UPLOAD.md](./design/napplet/NAP-UPLOAD.md) | Uploading a napplet's file to the user's Blossom servers, signed as the user: servers, auth, the shell's byte hop, caps, consent. |

#### `fips/` — the transport lanes

| Doc | Description |
| --- | --- |
| [ble-interop.md](./design/fips/ble-interop.md) | BLE L2CAP over fips's `BleIo` seam: per-peer PSM discovery, MAC randomization, the foreground service. |
| [wifi-aware-interop.md](./design/fips/wifi-aware-interop.md) | Wi-Fi Aware as a bulk lane: Kotlin raises the data path, fips's UDP transport dials it. |
| [ap-lane.md](./design/fips/ap-lane.md) | The LAN lane: same-network peers over ordinary UDP, found by mDNS. |
| [public-mesh-nodes.md](./design/fips/public-mesh-nodes.md) | The internet lane: public fips nodes found on Nostr, join.fips.network's recommended first, opt-in. |
| [usb-transport.md](./design/fips/usb-transport.md) | The USB lane: Android Open Accessory between a phone and a desktop or two phones. |

#### Shared assets

| Doc | Description |
| --- | --- |
| [diagrams/](./design/diagrams/README.md) | Design diagrams. |
| [mockups/](./design/mockups/README.md) | Early UI mockups — historical; the screens moved on. |

### Reference

| Doc | Description |
| --- | --- |
| [ports.md](./reference/ports.md) | Relay `4870` (loopback only), link service `4873` (the only port open to other phones), Blossom (no port). |
| [nostr-kinds.md](./reference/nostr-kinds.md) | Every event kind Myco reads, stores, publishes or replicates. |
| [settings.md](./reference/settings.md) | What is persisted (`settings.json`, `neighborhoods.json`, `links.json`, …), and what is not. |
| [ffi-surface.md](./reference/ffi-surface.md) | The Kotlin↔Rust contract: the JSON reducer, every action, the state snapshot, the napplet and radio entry points. |

### How-to

| Doc | Description |
| --- | --- |
| [build.md](./how-to/build.md) | Build the Rust core and the arm64 APK; the local `reference/fips` checkout. |
| [run-two-device-demo.md](./how-to/run-two-device-demo.md) | Two phones, airplane mode: invite to a neighborhood, share an app, browse it offline. |
| [publish.md](./how-to/publish.md) | Release to GitHub Releases and Zapstore. |
| [exit-node-demo.md](./how-to/exit-node-demo.md) | Experimental: a BLE-only phone browsing the internet through a mesh exit node. |
| [connect-over-usb.md](./how-to/connect-over-usb.md) | Link a phone to a desktop running fips, or two phones, over a USB cable. |

---

## Where to start

- Using the app? [Join a group](../README.md#join-a-group) or
  [organize one](../README.md#organize-a-group).
- New to the code? [getting-started.md](./getting-started.md), then
  [concepts.md](./design/core/concepts.md).
- What's next? The [roadmap](./roadmap.md).
- Building it? [how-to/build.md](./how-to/build.md) →
  [how-to/run-two-device-demo.md](./how-to/run-two-device-demo.md).
- The mesh underneath: [FIPS docs](../reference/fips/docs/README.md).
