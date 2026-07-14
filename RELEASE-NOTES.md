# Myco v0.2.0

**Released**: 2026-07-14

v0.2.0 is about **reach and reliability**: chat and content now flow across your
whole Circle — including peers several hops away over the mesh — and survive the
link drops, reconnects, and Bluetooth flaps that used to silently stall them. On
top of that it ships an **experimental Wi-Fi Aware bulk lane** for fast transfers
between capable phones, and a redesigned Discover tab.

> **Update every device together.** v0.2.0 moves the embedded relay and Blossom
> ports (see below), so a v0.2.0 phone and a v0.1.0 phone **won't exchange chat
> or content over the mesh**. Pairing and discovery still work across versions —
> but sync won't — so update all the devices in a Circle.

It upgrades from v0.1.0 in place with no data loss; your identity (nsec), Circle,
and installed apps are preserved.

## At a glance

- **Whole-Circle, multi-hop chat** — messages reach every Circle member wherever
  they are on the mesh, not just direct neighbours.
- **Resilient relay links** — each device keeps a persistent, two-way relay
  connection to every Circle member, detects a dead link, and re-establishes it
  within seconds of a flap — both directions — pulling back anything missed.
- **BLE scanning recovers itself** — discovery no longer gets stuck at zero peers
  after a burst of connects/disconnects.
- **Wi-Fi Aware bulk lane (experimental)** — a second, much faster transport
  raised beside BLE when both phones support it. Off by default.
- **Redesigned Discover** — an app-icon grid with a **Suggested** row of starter
  apps; tapping one opens it just like opening a shared app.
- **Bigger uploads** — the in-app Blossom store now accepts up to 64 MiB.
- **Relay/Blossom ports moved** to 4870 / 24243 (see Behavior changes).

## What's new

### Wi-Fi Aware bulk lane (experimental)

BLE is reliable but slow — a real handset L2CAP link tops out around ~22 KB/s,
which makes a large nsite a long transfer. v0.2.0 adds an **experimental Wi-Fi
Aware (NAN) lane**: when two nearby phones both have the hardware, they form a
pairwise Wi-Fi data path and larger transfers ride it at Wi-Fi speed, while
pairing and discovery stay on the always-on BLE mesh. It runs as fips's native
UDP transport over the Aware interface — no new wire format — and is **off by
default**; enable it under the Wi-Fi Aware section in Settings. Aware-less phones
simply stay on BLE. A dev-menu **adaptive speedtest** measures the lane
end-to-end (up/down throughput to a paired peer), with sub-1-Mbps results shown
in kbps.

Wi-Fi Aware is experimental: hardware coverage is uneven, throughput is
unmeasured across the device matrix, and the BLE↔Aware cutover policy is still
being tuned. Treat it as a preview.

### Redesigned Discover

Discover is now an app-drawer icon grid (favicon or lettered tile), matching the
Apps screen. A curated **Suggested** row sits above the mesh-discovered results:
**bitchat** (also the bundled first-run default, so a user who wiped it can get
it back) and **ICS**, an Incident Command System app for disaster response.
Tapping any tile — Suggested or discovered — opens the app exactly like opening a
shared one: it starts syncing and shows its live page, pulling from a Circle peer
that has it or, for the public suggestions, from public relays/Blossom.

## Reliability: the mesh now holds

The bulk of v0.2.0 is a wave of mesh-comms fixes so chat and content keep flowing
as people move around and links come and go.

- **Chat reaches your whole Circle.** Messages used to fan out only to Circle
  members you were *directly* connected to; once two paired people drifted several
  hops apart, their messages stopped even though the mesh could still route
  between them. Chat now fans out to the whole Circle, so a paired peer keeps
  receiving your messages wherever they sit on the mesh.
- **Relay links restore fast and both ways.** When a device in the middle of a
  chain dropped and reconnected, the relay links between Circle members came back
  slowly and often one-way, stalling messages for up to a minute. Each device now
  proactively keeps a live relay connection to every Circle member, re-establishes
  it within seconds of a flap in **both** directions, and on reconnect recreates
  its open subscriptions against the returned peer to pull back anything missed.
- **No more silent stalls after a Bluetooth flap.** The reused write-only relay
  connection could go stale — a half-open socket the app never noticed — and
  quietly swallow every message while the mesh still looked healthy. Each peer now
  holds a single persistent, two-way connection that detects a dead link (read
  side + keepalive) and reconnects; manifest fetches share that one socket.
- **BLE discovery recovers on its own.** After a burst of connects/disconnects,
  Android's scan throttle (~5 starts per 30s) could leave discovery stuck at zero
  peers until you toggled the mesh off and on. The scanner now re-arms itself on a
  backoff, waits out the throttle window, and recovers on its own.

## Behavior changes worth flagging

- **Relay/Blossom ports moved.** The embedded Nostr relay is now on **4870** and
  Blossom on **24243** (up one from 4869 / 24242), so Myco stops squatting on the
  ports a developer's own localhost relay or Blossom may use. Because the mesh and
  localhost binds share a port number, both moved together. **Consequence:** a
  v0.2.0 device and a v0.1.0 device won't sync chat or content over the mesh —
  update all devices together. (Temporary until the ports become configurable.)
- **Wi-Fi Aware is off until you enable it.** No behavior change unless you turn
  the lane on in Settings.

## Notable bug fixes

- **Chat stalled across the mesh** — one-way and lost messages after a mid-chain
  peer flapped; see Reliability above.
- **Discovery stuck at zero peers** — BLE scan-throttle recovery.
- **Silent event-propagation stall** — stale half-open peer-relay socket.

## Getting it

- **Android**: install the v0.2.0 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.2.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.2.0 tag. See [CONTRIBUTING.md](CONTRIBUTING.md) for build prerequisites.

The full per-release change history lives in [CHANGELOG.md](CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed code, design, testing, or bug reports to this
release — and to [@Origami74](https://github.com/Origami74) for maintaining the
project.
