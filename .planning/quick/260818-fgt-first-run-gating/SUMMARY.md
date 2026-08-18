---
id: 260818-fgt
slug: first-run-gating
date: 2026-08-18
status: complete
branch: feat/status-pill-panel
---

# Quick: first-run gating + peer names people actually chose

## First-run gating

`MainActivity.onCreate` started every lane unconditionally, before `setContent`.
On a cold install that meant the LAN browse up, the Bluetooth and Wi-Fi Aware
permission dialogs stacked, and the system's "Myco wants to set up a VPN
connection" prompt on top — four system dialogs over a splash animation, before
the app had said what it is.

That block is now `startEnabledLanes()`, called from `onCreate` only when
`PREF_INTRO_SEEN` is already set, and from the intro's `onFinished` on the first
run. Returning launches are byte-for-byte what they were. A replayed intro takes
the same branch and is a no-op — `ApRadio.ensureStarted`, both services' `start`
and `startNode` are all idempotent.

No consent card was added: the ask was for the gate.

## Peer names

`peerLabel(state, npub)` resolves a peer to the name **they** chose, checking
the sources that actually carry one, most-recently-confirmed first: their Circle
entry, the pair request they sent us, the invite we sent them. The npub-derived
name is the floor, not the default. Replaces `DeviceName.generated(peer.npub)`
at every display site — Circle NEARBY bubbles and their sort key, invite
bubbles, the Dev peer list, the speedtest labels, and both sections of the
status sheet (where lane rows had been showing fips's `display_name`, which is
an abbreviated npub, not a chosen name).

### The gap, stated plainly

A peer we are merely connected to and have exchanged no pair traffic with has
told us nothing but an npub, so it still shows the generated name. There is no
channel to fix that with today:

- the BLE advert is 27 of its 31 legacy bytes already (flags + the 128-bit FIPS
  service UUID + the 2-byte PSM service data), so a name does not fit;
- fips's `display_name` is a local alias/hosts lookup, never sent by the far
  side.

Covering it needs a name exchange over the mesh on connect — which broadcasts a
possibly-real name (the new default is the phone's own name) to every unpaired
device in BLE range. That is a product decision, not a bug fix, and is left
open.

## Verification

`:app:testDebugUnitTest`, `:app:assembleDebug` — pass. Installed on both
attached devices. First-run behaviour not yet re-verified from a clean install.

## Follow-up: the first-run name question

The chips alone were not enough — they only lived in Settings and the Circle
rename dialog, so a clean install silently adopted the phone's own name without
ever showing it. `FirstRunNameDialog` now asks once, between the intro and the
radios: the phone name prefilled, both suggestions as chips, free text if
neither fits. Gated on its own `name_chosen` pref rather than `intro_seen`, so
replaying the intro doesn't re-ask and an upgrade from a build that never asked
still gets it once.

Order is deliberate — name first, lanes second. The permission dialogs would
otherwise stack on top of it, and the name is what every pair request carries,
so it should be settled before anything can send one.

### Verified on a clean install (tablet, DC_1)

- `Displayed` at 17:20:12, no services and no dialogs until the intro was
  tapped — the gate holds.
- Name dialog appeared with `DC-1` prefilled; answering it wrote
  `device_name=DC-1` and `name_chosen=true`.
- Only then: notifications + nearby-devices prompts, VPN consent, then
  `BleService` and `MycoVpnService`, 6 peers.
- `NEARBY_WIFI_DEVICES` came out `granted=false` — the Aware lane was left
  without its permission on this run. Not investigated.
