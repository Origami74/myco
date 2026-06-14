# Getting Started

A short orientation to **Myco**. For the full conceptual model read
[design/concepts.md](./design/concepts.md); for the documentation map see the
[index](./README.md).

> Design doc for a not-yet-built app, written in proposal voice. Open questions
> are marked **TBD / open**.

## What the app is

Myco is a peer-to-peer **app-sharing network**: an Android app for exchanging,
browsing, and propagating **nsites** — static websites published on Nostr — over
a [FIPS](../reference/fips/docs/README.md) mesh, including fully **offline over
Bluetooth**. Each nsite is presented as an "app" inside an embedded browser:
there is no URL bar, just a fixed bottom bar (Back · Reload · Library). The **Library**
is your home grid of nsites; tapping one opens it in a WebView served entirely
from your phone's localhost. Two phones in a pocket — no Wi-Fi, no cell — can
exchange and browse each other's sites over BLE. The framing is nak's "Pillars
of Propagation": small relays and Blossom blobs hopping over crappy links in all
directions, surviving outages via local propagation.

Under the hood, every device runs its **own** embedded Nostr relay
(`ws://localhost:4869`) and Blossom blob server (`http://localhost:24242`),
unified with the FIPS endpoint into one Rust `myco-core` library. The app never
authors nsites — it only stores, serves, and replicates ones authored elsewhere.
When you browse a peer's site, your relay retains the author's signed events and
your Blossom store retains their content-addressed blobs — so **your device
becomes a new holder** for that site and can re-serve it to others later, even
fully offline (re-emitting author-signed events relay-to-relay is normal relay
behaviour, not authorship). The data is self-authenticating (signed events,
sha256 blobs), so any source is trustworthy regardless of who relayed it. See
[design/nsite-layer.md](./design/nsite-layer.md) and
[design/propagation.md](./design/propagation.md).

## The device identity model

Myco combines several **device-level** roles into a **single Nostr
keypair** — the **device key** — generated on first launch and held in
app-private storage:

- your **mesh network identity** (who you are on the FIPS mesh),
- your **BLE link authentication**, and
- your **app/device identity** in Myco — the address of this device's
  embedded relay + Blossom.

The device key is **never** used to author nsites. nsites are authored elsewhere
by external tooling under separate **author** keys; the app only ever holds and
serves other people's already-signed events, never their secret keys. An author
key shows up only as the `<npub_author>.nsite` URL host and as the `authors`
filter in a relay query.

From the device keypair, three addressing forms are deterministically derived:
the **npub** (the bech32 public key, used in the UI, QR pairing, BLE link auth,
and as this device's relay/Blossom address), the **node_addr**
(`SHA256(npub)[0:16]`, the FIPS routing identifier), and the **`fd00::/8` ULA**
(`fd ‖ node_addr[0:15]`, what `<npub_device>.fips` resolves to). The load-bearing
consequence: once you have a peer's device npub, you can compute where they live
on the mesh with **no further exchange** — which is why a QR code carrying only
an npub is enough to pair. Note this device npub is a *holder* address: the site
you fetch from a peer is identified by its own *author* npub, a different key.
There is one device identity per device in v1; multi-persona is a later
milestone. See [design/concepts.md](./design/concepts.md) and
[design/identity-pairing.md](./design/identity-pairing.md).

## The v1 demo in three sentences

The v1 target is a **two-device Android demo over BLE, fully offline**: phone A
**seeds** a known public nsite once (by syncing it while online, or side-loading
it — A never authors it), the two phones QR-pair (the code carries only an npub),
and then both go into airplane mode with Bluetooth on. Over a native L2CAP BLE
link, phone B reaches A's embedded relay and Blossom at `<npubA>.fips:4869` /
`:24242` (A's device/holder address), pulls the author's signed manifest event
and its blobs on demand, caches them locally, and serves the site to its in-app
WebView from `127.0.0.1` — with no internet, Wi-Fi, or cell anywhere in the path.
Afterward B has become a new holder for that site and could re-serve it to a third
device even if A is gone.

## Next steps

- **Build it:** [how-to/build.md](./how-to/build.md) — cross-compile the Rust
  backend and assemble the arm64 / minSdk 29 APK.
- **Run the demo:** [how-to/run-two-device-demo.md](./how-to/run-two-device-demo.md)
  — the full two-phone, offline, BLE browse runbook.
- **The phased plan:** [roadmap.md](./roadmap.md) — phases P0–P5 and what comes
  later.
- **Go deeper:** [design/concepts.md](./design/concepts.md) (terminology) and
  [design/architecture.md](./design/architecture.md) (the stack).
