---
quick_id: 260915-l2e
status: complete
date: 2026-09-15
commit: 49b0824ac51ab12924a39b5b85777c4fc30f5bd5
branch: feat/napplet-runtime
---

# Summary

Logcat: `refused: capability not granted napplet=boxedmappy domain=resource
action=bytes`, sent as `resource.bytes.result {error}` → shim resolved
`undefined` blob → `arrayBuffer` on undefined.

- Dispatch refusals in the `resource` domain are `.error` envelopes
  (`blocked-by-policy`).
- `NappletHost::open` widens an installed napplet's stored grants to
  `effective_grants(manifest.requires)` (as this build implements it) and the
  runtime persists the result; `napplet_grants` returns `None` for an
  uninstalled napplet, which gets nothing.
- `resource` added to `DEFAULT_GRANTS`.

Verified: cargo test (379), clippy. Flashed to both phones.
