# Getting Started

Orientation for someone new to **Myco**'s code and design: what it is, the
four ideas to hold, and where to read next. Ten minutes.

Just want to use the app? Start with
[Join a group](../README.md#join-a-group) or
[Organize a group](../README.md#organize-a-group) instead.

## What the app is

Myco lets someone who has a useful app hand it to the people they are with,
in person. One invites the other into a neighborhood — a QR scan, a share
link or an NFC tap — and shares the app. It lands on the other phone,
opens full-screen like any app, and still opens with no internet. Whoever has
it can hand it on. Apps travel over Bluetooth, Wi-Fi Aware or the local
network, straight from one phone to the next.

Two kinds of app run in it:

- a **napplet** — a program in a sandbox, which asks Myco for what it needs
  (an identity, relays, neighborhoods, pictures) and gets exactly what you granted.
  Napplets are what Myco is built to run; and
- an **nsite** — a static website published on Nostr, which Myco serves to a
  WebView from its own store. A bonus: any nsite works offline once it is on
  the phone.

Myco itself is the manager: **Apps** (your grid), **Neighbors** (your
neighborhoods and the people you share them with), **Settings**. New apps come from a share, a scan, or the
preinstalled **AppStore** app. Every app opens as its own full-screen task with
no Myco chrome.

The framing is nak's "Pillars of Propagation": small relays and Blossom blobs
hopping over bad links in every direction, surviving outages by local
propagation. Every phone runs its own Nostr relay and Blossom store, keeps the
signed events and content-addressed files of every app it has seen, and
re-serves them — so a phone that got an app yesterday is a source for it today,
whether or not the original holder is around.

## Four ideas to hold

**1. Four layers.** Apps (nsites and napplets) · neighborhoods · relay
and Blossom · FIPS. Each knows only the one below. [concepts.md](./design/core/concepts.md)
lays them out; [architecture.md](./design/core/architecture.md) maps them to
code.

**2. A mesh peer is not a link.** FIPS — the physical mesh — links to any
compatible phone in range and calls it a *peer*. A **link** is set up on
purpose, from a one-time invitation, and carries **neighborhoods**: channels
("Household", "Sports club") that posts travel along, link to link. Apps and
updates travel the same way, as extensions. Your relay and store are never
reachable from another phone; linked peers reach only the neighborhoods you
share with them. A mesh peer can be a stranger; a linked
person can be out of range. [nap-neighbor/](design/nap-neighbor/README.md).

**3. Two keys, plus one per link.** The **device key** is the phone's FIPS mesh key: mesh
identity, link authentication, `<npub>.fips`. The **user key** is your Nostr
key, the social one: what napplets publish as. It is a guest key from the first
launch, or your own `nsec` or a signer app. Each link also gets its own
**link key**, used only to set it up and sign its requests. Myco never holds an app author's
key — apps are authored elsewhere and Myco only re-serves their signed events.
[concepts.md § Keys](./design/core/concepts.md#keys).

**4. Trust the data, and the person — never the radio.** Every artifact is
self-authenticating (signed events, sha256 blobs), so any source is safe to
read from. What a *program* may do is a grant you made. What a *phone* may do
to yours depends on the link you set up with it. [security.md](./design/core/security.md).

## The demo in three sentences

Two phones, both in airplane mode with Bluetooth on: one invites the other
into a neighborhood (a QR scan or an NFC tap). One long-presses an app and shares it; the other scans (or is bumped again) and
pulls it straight from the first phone over the mesh. It opens full-screen,
served from the second phone's own store — and that phone can now share it on.
Runbook: [run-two-device-demo.md](./how-to/run-two-device-demo.md).

## Next steps

- Build it: [how-to/build.md](./how-to/build.md).
- Read the vocabulary: [concepts.md](./design/core/concepts.md).
- See what's next: [roadmap.md](./roadmap.md).
- Look something up: [ports](./reference/ports.md) · [kinds](./reference/nostr-kinds.md)
  · [settings](./reference/settings.md) · [the FFI](./reference/ffi-surface.md).
