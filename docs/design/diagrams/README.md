# Design diagrams

Visuals for the Myco design. The **intro** set is current but for
intro-02's wording. The **technical**
set was drawn from the first plan and has not been redrawn since; each row
says what has moved. Treat the docs as authoritative where they disagree.
Structure mirrors `reference/fips/docs/design/diagrams/`.

### Friendly intro (for the README / non-technical readers)

Two images, redrawn around the ideas a newcomer has to get.

| File | What it shows |
| --- | --- |
| [intro-01-your-apps.svg](intro-01-your-apps.svg) | Your home screen in the middle; tap an icon and the app opens full-screen as its own Recents card — a map on the left, a chat on the right — no browser, no tabs, no Myco chrome. Works offline; one window per app. |
| [intro-02-what-it-is.svg](intro-02-what-it-is.svg) | One room: the FIPS mesh forms out of whoever is in range (grey); on top of it, the people you share a neighborhood with (indigo); apps and what you do in them travel through them. Ben is reached *through a stranger's phone* — the mesh carries the packets, your links decide who they are for; Dan is out of range and still linked. **Stale wording:** the picture says "Circle", which neighborhoods replaced ([nap-neighbor/](../nap-neighbor/README.md)). |

### Technical (design docs)

| File | What it shows |
| --- | --- |
| [01-system-layering.svg](01-system-layering.svg) | The single-device stack from the first plan. **Stale:** no napplet band, no neighborhoods band, the provenance tags ("reuse (nostr-vpn)") no longer mean anything; the current four-layer picture is the ASCII one in [architecture.md](../core/architecture.md). |
| [02-pairing-transitive-discovery.svg](02-pairing-transitive-discovery.svg) | **Obsolete:** pairing and the Circle are gone; links from one-time invitations replace them ([nap-neighbor/](../nap-neighbor/README.md)). Was: mutual pairing — **invite-pairing** (echo a one-time long-random secret over Noise + confirm; v1, handshake-mandatory, **always mutual**) — authorizes polling a peer's collected list, so Alice transitively reaches Ben + Ben's peers. (There is no one-way fetch-only scan; scanning initiates the mandatory mutual handshake.) |
| [03-offline-propagation.svg](03-offline-propagation.svg) | "Pillars of Propagation": an nsite hopping device→device over BLE with no internet; the split between FIPS live-routing and the net-new nsite store-and-forward layer that survives partition. |
| [04-nsite-browse-flow.svg](04-nsite-browse-flow.svg) | The browse request lifecycle: `NsiteActivity` → gateway → blobs present? serve **direct from local Blossom**; else sync manifest+blobs from a neighborhood over FIPS (or the internet), verify, retain → serve. **Stale detail:** the gateway is in-process and the host is `.localhost`, not a localhost port and `.nsite`. |
| [05-nsite-layer-architecture.svg](05-nsite-layer-architecture.svg) | Component view of the four crates (five now — `myco-napplet-runtime` is missing): `nsite-deck` (gateway + sync) consuming `myco-relay` + `myco-blossom` via `RelayBackend`/`BlobStore`, with `myco-core`'s FIPS endpoint providing `PeerSource`/`FanoutSink` — and what fans out vs pulls. **Moved:** only the link service (`:4873`) is on the mesh; relay and Blossom are loopback / in-process only. |
| [06-nsite-data-model.svg](06-nsite-data-model.svg) | What an nsite *is*: an author-signed manifest event (kind 15128/35128) mapping paths → sha256, plus the content-addressed Blossom blobs. |
| [07-relay-mesh-fanout.svg](07-relay-mesh-fanout.svg) | Event fanout: each accepted event is re-broadcast to all *other* peers (source-excluded, deduped, hop-budgeted); events fan out, blobs stay pull-only. **Obsolete:** the relay no longer fans anything out to other phones. Everything, app manifests included, travels in neighborhoods, passed on link to link ([nap-neighbor/](../nap-neighbor/README.md), [extensions.md](../nap-neighbor/extensions.md)). |
| [08-two-layer-propagation.svg](08-two-layer-propagation.svg) | The load-bearing split: Layer A (FIPS live-path routing) vs Layer B (nsite store-and-forward that survives partition); announce-manifest vs pull-blob. |
| [09-identity-model.svg](09-identity-model.svg) | The two identities un-conflated: the device key (mesh/BLE/link service address, three derived forms) vs the external nsite-author key; holder ≠ author. **Stale:** the napplet **user key** is a third; and each link has its own **link key**; see [concepts.md § Keys](../core/concepts.md#keys). |

**What the technical set still gets right,** and what has moved since:

- Device key ≠ author key; holder ≠ author. *Now also:* device key ≠ user key
  ≠ link key.
- FIPS BLE is **L2CAP CoC**; the Rust core is one `.so` from a crate workspace
  (five crates now, not four); `nsite-deck` reaches relay and Blossom through
  `RelayBackend` / `BlobStore`.
- Each app launches as its own fullscreen task with no chrome; the gateway
  serves direct from Blossom. *Moved:* the host is `<host>.localhost` and the
  gateway is in-process; `.nsite` is not used.
- The app-owned TUN stays, scoped to Myco's uid, for `.fips` only. *Moved:* no
  `*.nsite` interception.
- Propagation is hybrid: forward the signed manifest, pull blobs on demand.
  *Moved:* the edges are **neighborhoods** with an update extension on, not
  "connected peers" — see
  [nap-neighbor/extensions.md](../nap-neighbor/extensions.md).
- The two-device demo is the check every mesh change still has to pass.
