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

## Correction: the address key was the wrong key

The first working build advertised and received fine and still showed nothing,
because the join ran on the BLE address. That only ever attaches a name to a
peer *currently carried over BLE* — and a device is routinely discovered by
advert and then connected over the LAN lane, which is exactly what both test
devices were doing. Its peer row is keyed by node address, not by MAC.

So the advertiser now says who it is: the scan response carries a 6-byte
`node_addr` prefix ahead of the name, and `advert_names` is keyed on that.
48 bits is ample against accidental collision in a room and leaves 21 bytes for
the name. No address mapping is needed at all, and the join works whatever
transport ends up carrying the peer.

The `transport_addr` work stays — it is a genuine fix for RSSI attribution on
inbound BLE peers, which was silently missing.

Confirmed on device: tablet logs `name 'DC-1' as 6B+name in scan response` and
`ble0/7E:80:31:62:69:92 (a16d353c9f25…) advertises name 'orchid eero'`, and the
A52's Dev peer row on the tablet now reads `advert name  orchid eero` while
every other peer (older builds) reads `—`.

## Not verified

The rendered Nearby bubble. On the tablet the A52's chosen name is identical to
what the generator produces for its npub (`orchid eero`), so the bubble looks
the same either way — the `advert name` forensic row is what proves the join.
The unambiguous direction is the A52 showing the tablet as `DC-1`, which the
generator cannot emit; the A52 was locked and needs a relaunch on this build,
since it pushes its name and node address from `onResume`.

Also unchanged: this is BLE-only. A peer found over Wi-Fi Aware or the LAN
still carries no advertised name.

## Follow-up: a rename didn't reach the radio

Renaming from Settings or the Circle dialog wrote the preference and told the
core, but never touched `BleRadio` — only `onResume` and the first-run dialog
pushed the name in. So a rename took effect on the next app foreground, not on
save, and the old name kept going out over the air. That is the surface a
rename is usually aimed at.

`applyDeviceName()` is now the single way to change the name and moves all
three publish points together: the preference, the core (which stamps pair
events), and the radio (which broadcasts it). All four call sites go through
it — Settings chips, Settings save, the Circle rename dialog, and the first-run
dialog — and `onResume` re-asserts through the same function.

### Rendering confirmed

The tablet's Nearby list showed `riley` and `DC-1`. Neither is producible by
the generator, which only ever emits lowercase `colour name`, so both can only
have arrived over the air.

One peer advertised `DC-1` while its Circle name, learned from its signed pair
event, was `frank`. That divergence is this exact bug seen from the other side:
a device renamed on a build predating the fix keeps broadcasting the old name
until it next foregrounds. It should resolve once that device runs this build.
Worth re-checking — if a peer still shows a stale advertised name after
restarting on this build, the join is attributing to the wrong row and that is
a different problem.
