# Myco v0.1.0

**Released**: 2026-06-30

v0.1.0 makes the Bluetooth mesh **reliable**. The L2CAP reader and writer had a
fundamental framing mismatch with the embedded core that caused data corruption
on almost every BLE link — fragmented socket reads were shipped up as truncated
packets and coalesced reads were silently dropped. That's now fixed: the radio is
a transparent byte pipe, and the core recovers packet boundaries itself using the
same length-prefixed framer the IP transport uses. If a chunk ever does go
missing the link resets cleanly instead of silently corrupting the rest of the
session.

On top of the reliability fix, v0.1.0 ships NFC sharing, a storage settings
page, peer speedtest diagnostics, and a cleaned-up settings layout. It upgrades
from v0.0.3 in-place with no data loss.

It interoperates with v0.0.3 devices on the mesh; older peers aren't aware of
the NFC sharing path or the speedtest, but the mesh still routes and pairs over
QR as before.

## At a glance

- **BLE reliability fix** — the headline change; mesh links no longer corrupt
  data or thrash when the socket read doesn't align to a packet boundary.
- NFC app sharing — bump phones to hand off a `myco://share` link; the recipient
  pairs and pulls the app from you over the mesh.
- Storage settings — usage gauge, delete-cache, and delete-all-data.
- Peer speedtest — round-trip a payload through a paired peer and read up/down
  throughput in the Dev tab.
- Settings reorganised into focused pages; mesh controls and raw identity fields
  behind a developer page.
- Circle *Nearby* is always shown, sorted by name — no reshuffling as signal
  fluctuates.
- App locked to portrait everywhere.

## What's new

### BLE reliability fix

The Kotlin L2CAP radio was applying its own 2-byte length-prefix framing on top
of `BluetoothSocket` — a byte *stream* with no packet boundaries — and
reconstructing packets in the reader with a `readFully()` that assumed each
socket `read()` returned exactly one whole mesh packet. It didn't: a fragmented
read produced a runt packet; a coalesced read was truncated to the first expected
length. Either way data was corrupted and the link would eventually thrash or
stall.

The fix removes the per-packet framing from Kotlin entirely. The radio is now a
**transparent, in-order byte pipe**: the reader forwards whatever `read()` returns
directly to the core via `deliver_recv`, and the writer writes whatever the core
hands it verbatim. The embedded core's `BleStreamRead` framer — the same
`FMP` length-prefixed framer used on the IP transport — recovers packet
boundaries from the raw stream. If `deliver_recv` ever refuses a chunk (inbound
queue full or channel gone), the reader now treats that as fatal and resets the
link, rather than dropping the chunk silently and desyncing the framer for the
rest of the connection.

### NFC app sharing

The "Share this app" surface is now a bottom sheet: a larger QR code, a
prominent "tap phones together" prompt, and an auto-close once the recipient
pairs. On the receiving side, *Add an app* is a bottom sheet with a live camera
scanner, a paste-a-link button, and a tap-a-phone option — and tapping opens the
share sheet in emulation mode so a bump hands off the `myco://share` code over
NFC the same way pairing does.

### Storage settings

A new **Storage** page in Settings shows a local storage gauge (relay + Blossom)
and two delete actions: *Delete cache* reclaims space while keeping pinned apps
working offline, and *Delete all data, including apps* wipes the relay and
Blossom entirely. Your identity (nsec) and Circle survive both.

### Peer speedtest

The Dev diagnostics tab now has a **speedtest** that round-trips a fixed payload
through a connected, paired peer's mesh Blossom and reports up and down
throughput. Results under 1 Mbps display in kbps so a slow BLE link reads as
e.g. "220 kbps" rather than "0.2 Mbps."

### Settings reorganisation

Settings is now a list of focused pages. Everyday controls — device name,
storage, and the mesh (with Bluetooth as a sub-toggle) — stay on the first
screen. The mesh-only switch and raw identity fields (npub, node\_addr, `.fips`
address, mesh ULA) move behind a **Developer** page, so everyday users don't
land on a screen full of hex strings.

## Behavior changes worth flagging

- **BLE links will reset on framer desync.** Under the old code a corrupted link
  would stall silently; now it resets and reconnects. You may see reconnect
  events in the Dev tab where before you saw a stalled link.
- **NFC sharing auto-closes the sheet.** Once the recipient pairs, the "Share
  this app" bottom sheet closes itself — the same pattern as the pairing flow.

## Notable bug fixes

- **Bluetooth data corruption** — fragmented or coalesced L2CAP reads no longer
  produce runt or truncated packets; see above.
- **Portrait lock** — the app no longer flips to landscape on a slight tilt.

## Getting it

- **Android**: install the v0.1.0 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.1.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.1.0 tag. See [CONTRIBUTING.md](CONTRIBUTING.md) for build prerequisites.

The full per-release change history lives in [CHANGELOG.md](CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed code, design, testing, or bug reports to this
release — and to [@Origami74](https://github.com/Origami74) for maintaining the
project.
