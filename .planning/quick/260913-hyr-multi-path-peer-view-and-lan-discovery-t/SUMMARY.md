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
