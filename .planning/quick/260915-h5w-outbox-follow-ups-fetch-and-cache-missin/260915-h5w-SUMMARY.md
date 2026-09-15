---
quick_id: 260915-h5w
status: complete
date: 2026-09-15
branch: feat/napplet-runtime
commits:
  - 775487566e8a9865456afbdbd768dde767039554 (core: fetch/cache NIP-65 lists)
  - 31fb3b1a14ed1774fe0b6845ee112d55e11243df (runtime: relay reads through the pool; docs)
---

# Summary: outbox follow-ups

- Missing NIP-65 list → fetched from the pool (configured relays unless
  offline-only + Circle mesh relays, 4s bound), stored in the local relay
  (the cache), miss remembered 10 min. List older than 24h → `source: cache`
  now, one background refresh per author.
- `relay.query` → whole pool (policy plan), bounded 5s, deduped; error only
  when no lane answered. `relay.subscribe` → local backlog + EOSE, then pool
  pulled into the local relay; `options.relay` → that relay only, validated,
  no local backlog.
- Tests: 3 new in core (`with_configured_relays` keeps them off the
  internet), 2 new in the runtime. 363 pass, clippy clean. Not run on device.

Not done: NIP-66 relay intelligence — a MAY in the spec, and the offline
case has no monitors to consult; would only reorder candidates we already
try in parallel.
