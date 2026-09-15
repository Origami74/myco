---
quick_id: 260915-lwj
status: complete
date: 2026-09-15
branch: feat/napplet-runtime
commits:
  - fa777e31e69bbf470582b775397d6e08bfce7126
  - db66ff592a5555008fe6a9a858e4d525d483f8d2
  - b638d3927a5d682b2b2b5944c94d67c9e75b2dcc
  - 013a84f8e92cf3d865236469b15cb44ddfb7cd04
---

# Summary

Rewritten: docs/README.md, getting-started, concepts, architecture, roadmap,
build, run-two-device-demo, ffi-surface (from source), settings (new; config.md
removed), circle/circle.md (new). Patched to the code: app-shell, nsite-layer,
ports, nostr-kinds, propagation, security, nsite-permissions, identity-pairing,
napplet-runtime, event-gossip, ble-interop, wifi-aware, ap-lane, exit-node,
diagrams and mockups READMEs, README.md. nostr-vpn / site-deck / bitchat
reference links removed (checkouts do not exist). Mesh ports (relay 4870 and
Blossom 24243 over .fips) marked deprecated in ports, nsite-layer, concepts,
circle, event-gossip. Roadmap: N1 login (nsec + Amber), N2 drop mesh from
nsites, N3 notifications, N5 release.

Left as is: ~50 "TBD / open" markers in nsite-layer, propagation, nsite-updates,
identity-pairing, ble-interop, security — each needs a judgement call. The
technical SVG diagrams are not redrawn (README says what moved).
