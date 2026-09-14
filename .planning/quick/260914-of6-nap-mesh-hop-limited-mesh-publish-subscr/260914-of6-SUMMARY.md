---
quick_id: 260914-of6
status: complete
date: 2026-09-14
branch: feat/napplet-runtime
commits:
  - 861001bd726d3a63cbe18ea3472b7cb6bd160429 (runtime: seam, handler, spec)
  - c1f8f321852b69309297d5e887aba324febd9c6e (runtime: prelude supplement)
  - f90f1939fc099d58b07dff103abf644b915a0247 (core: caps, ttl accept, sink)
  - d983883ec9f22d52e089463701ed7a90f89dbbce (android: settings, wording)
  - 8420c6281e0ea4175721af891c495e3ce93899a6 (docs)
---

# Summary: NAP-MESH

A new napplet capability domain `mesh`: hop-limited publish and subscribe over
the FIPS mesh, specified in `docs/design/napplet/NAP-MESH.md` in the
napplet/naps NAP-WORD template so it can be proposed upstream.

- `mesh.info` → online, reachable peer count, the user's caps.
- `mesh.publish {event, ttl?}` → signed as the user (NAP-RELAY's rules, shared
  parser), stored, flooded to the Circle at `min(ttl, cap)`; effective ttl
  echoed.
- `mesh.subscribe {subId, filters, ttl?}` → local backlog as `mesh.event`,
  peers asked `min(ttl, cap)` rings out (spawned, results accepted into the
  hub so they arrive live), then `mesh.eose {ttl}`; `mesh.close`/`mesh.closed`.
- Caps: `settings.json` `napplet_mesh_publish_ttl` (default 3 = `EVENT_TTL`)
  and `napplet_mesh_subscribe_ttl` (default 2 = `MAX_REQ_TTL`), never above
  those; `SetNappletMeshReach`; Settings › App reach steppers; live per call.
- Web projection: the vendored `@napplet/shim` filters unknown domains and has
  no `shell` domain, so `assets/myco-prelude.js` installs `napplet.mesh` and
  `napplet.shell.{supports,services}` after it.
- Grant wording on the install review sheet.

Verified: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`,
`cargo test` (342 passed), `./gradlew :app:compileDebugKotlin`. Not run:
`assembleDebug` (NDK cross-compile) and any on-device test — the two-phone
flood path (publish on A, `mesh.event` on B; subscribe on B pulls A's backlog)
needs physical devices.

Not done here, deliberately:
- `relay.publish` still floods the mesh at ttl 3; making it spec-conformant
  (relays only) is deferred to the inbox/outbox work.
- README.md untouched — it does not yet describe napplets on this branch.
