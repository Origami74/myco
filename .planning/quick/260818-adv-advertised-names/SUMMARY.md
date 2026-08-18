---
id: 260818-adv
slug: advertised-names
date: 2026-08-18
status: complete
branch: feat/status-pill-panel
---

# Quick: broadcast the chosen name so Nearby can show it

## The channel

The primary BLE advert is full — 27 of its 31 legacy bytes carry flags, the
128-bit FIPS service UUID and the 2-byte PSM service data — and the PSM must
stay there, because a scan response needs an active-scan round-trip that drops
asymmetrically across chipsets. So the name goes in the **scan response's own
31 bytes**, under a new 16-bit service-data UUID (`0x9C91`, the PSM key's
neighbour). The trade is deliberate: a dropped scan response is fatal for a PSM
and merely cosmetic for a name, which just falls back to the generated one.

`BleRadio.localName` is process-global rather than per-radio, because the app
asserts the name on every resume and that can precede the radio's existence.
Setting it while advertising re-issues the advert, since the scan response is
fixed at start time.

## The path

Received names land in `advert_names` — a Myco-owned, BLE-address-keyed map
with its own JNI entry point (`bleDeliverAdvertName`), deliberately *not* the
fips bridge's `deliver_scan`: the name has no bearing on routing and fips must
stay clean. Same shape as `lane_observation`. `merge_peers` joins it onto rows
by BLE address; `peerLabel` places it below every name learned from signed pair
traffic and above the npub-derived floor.

**The name is unauthenticated** — a plaintext broadcast anyone in range can
forge, and now readable by any scanner near you, which matters more since the
default is the phone's own name. The ordering in `peerLabel` is what keeps it
from ever displacing a signed name; that ordering is load-bearing, not
cosmetic.

## The bug this exposed

The first build advertised and received correctly but nothing showed, because
`merge_peers` could only attribute an advert to a peer row when the
connect-attempt log had learned that BLE address — and that log only records
dials *we* made. Both live BLE peers had connected inbound, so their rows had
no `ble_addr`, no RSSI, and no advertised name.

fips was already reporting the answer and Myco was discarding it:
`show_peers`'s `transport_addr` is the peer's live link address, formatted for
BLE as exactly the `adapter/AA:BB:CC:DD:EE:FF` key the adverts arrive under. It
now rides `PeerView` and sets `ble_addr` on BLE rows directly. This fixes RSSI
attribution for inbound peers too, which was silently missing before.

## Verification

- New unit tests: advert-name join by address (and no borrowing across rows);
  inbound-BLE peer keyed by its link address, with RSSI and name both landing
  and a socket address correctly keying into nothing.
- `cargo test -p myco-core` — 91 pass.
- On device: tablet logs `advertising PSM 196 …, name 'DC-1' (in scan
  response)` and `ble0/77:B5:98:5E:D1:E6 advertises name 'orchid eero'` — both
  directions confirmed on the wire.

## Not verified

The rendered label. Both test devices' chosen names are indistinguishable from
what the generator produces for their npubs (the A52's *is* `orchid eero`), so
the Nearby bubble looks identical whether the advertised name was used or not.
Proving it on screen needs one device renamed to something the generator cannot
emit — `DC-1` qualifies, and the tablet already advertises it, but the A52 was
locked. Check the A52's Nearby list for `DC-1`: if it appears, the whole path
renders.

Also unchanged: this is BLE-only. A peer found over Wi-Fi Aware or the LAN
still carries no advertised name.
