---
id: 260913-hyr
status: in-progress
branch: feat/multi-path-peer-view
---

# Multi-path peer view + LAN discovery toggle

## Why

fips `feat/multi-path-switchover` lets one peer hold several paths at once
(BLE + Aware + LAN) and switch between them. `show_peers` rows now carry a
`paths` array. Myco still shows one transport per peer and infers the Aware/LAN
split from a Kotlin-side "who observed it" record. The status sheet should show
what fips actually has: every path, active one lit, the rest greyed.

Two side asks: unnamed peers show a shortened npub instead of the generated
two-word name, and the LAN mDNS browse/advert gets an on/off switch in Settings.

## Tasks

1. **myco-core** — `control_client.rs`: parse `paths[]` into `PeerView.paths`
   (`lane`, `state`, `active`). Lane from `transport_type`, except UDP where the
   instance name decides: `aware<N>` → `aware`, `lan` → `udp`. `state.rs`: add
   `paths` to `PeerDiagnosticView`. `peer_diagnostics.rs`: copy paths through,
   take `transport` from the active path when there is one, fill
   `also_reachable_via` from the other non-dead paths.
2. **Android sheet** — `AppCoreClient.kt` parses `paths`. `MeshStatusSheet.kt`:
   each peer line shows one icon per path, inactive at reduced alpha; lane blocks
   list every peer with a path on that lane. Unnamed peers: `npub1abcd…wxyz`.
3. **Settings** — "Network" toggle under Wi-Fi Aware. `ApRadio.setEnabled()`
   stops/starts browse + advert; `PREF_LAN` in `myco_prefs`, default on.

## Verification

- `cargo test -p myco-core` (path parse + merge tests).
- `./gradlew assembleDebug`.
- On device: two phones with BLE + Wi-Fi up show two icons per peer, one lit;
  toggling Network off drops the LAN lane's "scanning" to "off".
