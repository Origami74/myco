# Design diagrams — DRAFT (under review)

Working visuals for the Myco design conversation. These encode current
*proposed* design and are expected to change as open questions get answered.
Structure mirrors `reference/fips/docs/design/diagrams/`.

### Friendly intro (for the README / non-technical readers)

| File | What it shows |
| --- | --- |
| [intro-01-what-it-is.svg](intro-01-what-it-is.svg) | Plain-language hero: install apps from the people around you over Bluetooth, works offline. |
| [intro-02-get-started.svg](intro-02-get-started.svg) | The 3 steps: scan a friend's QR → install their apps → open one (works offline). |
| [intro-03-how-it-spreads.svg](intro-03-how-it-spreads.svg) | "Like passing a note": an app hops Alice→Ben→Carl; it keeps spreading even after Alice leaves. |

### Technical (design docs)

| File | What it shows |
| --- | --- |
| [01-system-layering.svg](01-system-layering.svg) | The single-device stack: Android UI → embedded browser → local gateway → embedded relay+Blossom → myco-core (Rust/FFI) → fips-core mesh → transports. Each band tagged with reuse provenance (nostr-vpn / site-deck / fips-core / new). |
| [02-pairing-transitive-discovery.svg](02-pairing-transitive-discovery.svg) | Mutual QR pairing (npub exchange) and how a mutual scan authorizes polling a peer's collected list, so Alice transitively reaches Ben + Ben's peers. |
| [03-offline-propagation.svg](03-offline-propagation.svg) | "Pillars of Propagation": an nsite hopping device→device over BLE with no internet; the split between FIPS live-routing and the net-new nsite store-and-forward layer that survives partition. |
| [04-nsite-browse-flow.svg](04-nsite-browse-flow.svg) | The browse request lifecycle: WebView → localhost gateway → cache hit/miss → fetch manifest+blobs from a reachable peer over FIPS → verify + atomic cache → serve. |
| [05-nsite-layer-architecture.svg](05-nsite-layer-architecture.svg) | Component view of the content layer inside `myco-core`: gateway, embedded relay, embedded Blossom, sync engine, FIPS endpoint — and what fans out vs pulls. |
| [06-nsite-data-model.svg](06-nsite-data-model.svg) | What an nsite *is*: an author-signed manifest event (kind 15128/35128) mapping paths → sha256, plus the content-addressed Blossom blobs. |
| [07-relay-mesh-fanout.svg](07-relay-mesh-fanout.svg) | Relay-mesh fanout: the relay re-broadcasts each accepted event to all *other* connected peers (source-excluded, deduped); events fan out, blobs stay pull-only. |
| [08-two-layer-propagation.svg](08-two-layer-propagation.svg) | The load-bearing split: Layer A (FIPS live-path routing) vs Layer B (nsite store-and-forward that survives partition); announce-manifest vs pull-blob. |
| [09-identity-model.svg](09-identity-model.svg) | The two identities un-conflated: the device key (mesh/BLE/relay address, three derived forms) vs the external nsite-author key; holder ≠ author. |

**Key established facts baked into these (verified from source):**

- Two distinct identities: the **device** key (one per device) = FIPS mesh address (`node_addr` = SHA256(npub)[:16]; `fd00::` = fd ‖ node_addr[:15]) = BLE link auth = the address of this device's relay+Blossom; it **never** authors nsites. The **nsite author** is a separate, external key, appearing only as the `<npub_author>.nsite` URL host and the `authors` query filter. The site you want (author npub) and the peer/holder you fetch it from (`<npub_holder>.fips`) are different keys.
- FIPS BLE = **L2CAP CoC** (incompatible with bitchat's GATT); Linux interop ⇒ native Android L2CAP.
- Backend language = **Rust** (relay + Blossom unified into `myco-core`).
- **The VpnService/TUN stays** — it gives system-wide `.fips` resolution to every app (the "vpn-like" goal). The DNS interceptor maps `*.fips → fd00::` (IPv6 mesh) and `*.nsite → 127.0.0.1` (IPv4 gateway). What's dropped is the private-network layer on top: exit-node / tunnel-all-internet / roster-admin membership / `.nvpn` MagicDNS.
- `npub.nsite` loads exactly like nsite-deck (IPv4 localhost → works in any browser, incl. Chromium); only relay+blossom **sync** is reached over `.fips`.
- "Offline propagation" lives at the **nsite/relay layer** (store-and-forward), not in FIPS transport. Default = **hybrid: flood the author-signed manifest events widely (re-emitted unmodified, TTL 5 hops), pull the large blobs on demand**.
- v1 target = **two-device Android demo** (Android↔Android over BLE; one browses the other's nsite).
