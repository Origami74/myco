---
id: 260913-hyr
status: complete
branch: feat/multi-path-peer-view
---

# Multi-path peer view + LAN discovery toggle

## What shipped

- `myco-core`: `show_peers` rows' `paths[]` parsed into `PeerView.paths` /
  `PeerDiagnosticView.paths` (`lane`, `state`, `active`). Lane from
  `transport_type`, UDP split by instance name (`aware<N>` vs `lan`). Row
  `transport` comes from the active path; `also_reachable_via` is the other
  non-dead lanes. Cores/daemons without `paths` behave as before (lane record
  still applies).
- Sheet: `PathIcons` draws one icon per non-dead lane, standbys at 0.3 alpha;
  lane blocks list a peer under every lane it has a path on. Unnamed peers:
  `npub1abcde…wxyz` (sheet only — `peerLabel` elsewhere unchanged).
- Settings → Mesh → "Network": `ApRadio.setEnabled()` stops/starts browse +
  advert; `lan_discovery_enabled` pref, default on.

## Verified

- `cargo test -p myco-core`: 160 passed (4 new).
- `./gradlew assembleDebug` + `testDebugUnitTest`: green.
- Not yet on device — needs two phones with BLE + Wi-Fi both up to see two
  icons per peer.

## Commits

- 182b9442 feat(core): surface every fips path per peer
- 807778fc feat(ui): show every path per peer in the mesh status sheet
- fd6cc4c5 feat(settings): Network switch for the LAN mDNS browse and advert

## Follow-ups on the same branch (from on-device testing, 2026-09-13/14)

- d8172fd fix(ui): fade the whole peer row on a standby lane instead of icons
- f53e83c feat(dev): per-path selection numbers on the Dev tab
- 93505ca fix(core): make BLE a backup path (`role: backup`) so Aware/LAN carry traffic
- 08eddce fix(ap): re-resolve LAN peers; space dials past the handshake timeout
- a1b46be fix(vpn): restart the mesh tunnel after the VPN slot comes back
- 3fe65d2 / c8536c3 fix(ble): dial peers Aware carries on a multi-path core; keep
  the coexistence gate on a single-path core (`multipath_core` in AppState)
- 9399585 build: `fips-multipath` Cargo feature, auto-detected by Gradle from
  `MYCO_FIPS_REPO_PATH`; the branch builds for Android against fips master too

fips-side fixes found along the way live in the fips repo:
`fix/ble-link-arbitration` (off master) and the rebased
`feat/multi-path-switchover` (two `fix(path)` commits on top).
