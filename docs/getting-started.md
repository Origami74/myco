# Getting Started

Orientation for someone new to **Myco**: what it is, the four ideas to hold,
and where to read next. Ten minutes.

## What the app is

Myco lets someone who has a useful app hand it to the people they are with,
in person. They pair — a bump, a tap on someone Myco found nearby over
Bluetooth, or a QR scan — and share the app. It lands on the other phone,
opens full-screen like any app, and still opens with no internet. Whoever has
it can hand it on. Apps travel over Bluetooth, Wi-Fi Aware or the local
network, with no central app store in between.

Two kinds of app run in it:

- a **napplet** — a program in a sandbox, which asks Myco for what it needs
  (an identity, relays, the mesh, pictures) and gets exactly what you granted.
  Napplets are what Myco is built to run; and
- an **nsite** — a static website published on Nostr, which Myco serves to a
  WebView from its own store. A bonus: any nsite works offline once it is on
  the phone.

Myco itself is the manager: **Apps** (your grid), **Circle** (the people you
have paired with), **Settings**. New apps come from a share, a scan, or the
preinstalled **AppStore** app. Every app opens as its own full-screen task with
no Myco chrome.

The framing is nak's "Pillars of Propagation": small relays and Blossom blobs
hopping over bad links in every direction, surviving outages by local
propagation. Every phone runs its own Nostr relay and Blossom store, keeps the
signed events and content-addressed files of every app it has seen, and
re-serves them — so a phone that got an app yesterday is a source for it today,
whether or not the original holder is around.

## Four ideas to hold

**1. Four layers.** Apps (nsites and napplets) · the Circle · relay and
Blossom · FIPS. Each knows only the one below. [concepts.md](./design/core/concepts.md)
lays them out; [architecture.md](./design/core/architecture.md) maps them to
code.

**2. A peer is not a Circle member.** FIPS — the physical mesh — links to any
compatible phone in range and calls it a *peer*. The **Circle** is the list of
people you have paired with: a virtual mesh over the physical one, built on
purpose, one mutual handshake at a time. Only Circle members read your relay
and store, receive your gossip, or get your files. A peer can be a stranger; a
member can be out of range. [circle.md](./design/circle/circle.md).

**3. Three keys.** The **device key** is the phone's: mesh identity, link
authentication, `<npub>.fips`. The **user key** is yours as napplets see it:
what they publish as. An **author key** belongs to whoever made an app, and
Myco never holds it — apps are authored elsewhere and Myco only re-serves their
signed events. [concepts.md § Three keys](./design/core/concepts.md#three-keys).

**4. Trust the data, and the person — never the radio.** Every artifact is
self-authenticating (signed events, sha256 blobs), so any source is safe to
read from. What a *program* may do is a grant you made. What a *phone* may do
to yours is whether you paired with it. [security.md](./design/core/security.md).

## The demo in three sentences

Two phones, both in airplane mode with Bluetooth on, pair (a bump, or a tap
on the other under Nearby). One
long-presses an app and shares it; the other scans (or is bumped again) and
pulls it straight from the first phone over the mesh. It opens full-screen,
served from the second phone's own store — and that phone can now share it on.
Runbook: [run-two-device-demo.md](./how-to/run-two-device-demo.md).

## Next steps

- Build it: [how-to/build.md](./how-to/build.md).
- Read the vocabulary: [concepts.md](./design/core/concepts.md).
- See what's next: [roadmap.md](./roadmap.md).
- Look something up: [ports](./reference/ports.md) · [kinds](./reference/nostr-kinds.md)
  · [settings](./reference/settings.md) · [the FFI](./reference/ffi-surface.md).
