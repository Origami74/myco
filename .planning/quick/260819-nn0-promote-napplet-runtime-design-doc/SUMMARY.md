---
task: Promote the napplet runtime plan into docs/design/
quick_id: 260819-nn0
date: 2026-08-19
status: complete
commit: 7458e2bb783950f6996ac948cf4e887325afaab9
---

# Summary

`docs/design/napplet-runtime.md` (524 lines) is checked in and linked from the design-doc
table in `docs/README.md`.

## What changed from the scratch plan

Beyond the rewrite for a reader who was not in the conversation, four internal
contradictions the scratch doc had accumulated were reconciled:

- the crate-layout section still argued against sharing with `nsite-deck`, which the
  NIP-5A/NIP-5D finding had already overturned — shared aggregate primitives now live in
  `nsite-deck`,
- `napplet.localhost` survived in the window-model and delivery sections after the
  decision to give each napplet its own origin,
- `NAP-OUTBOX` still carried an "offline degradation rule", which the `<npub>.fips`
  relay-URL approach replaced entirely,
- `nostr:` was still listed as a live intent entry point after it was deferred.

Gap numbering became section numbering (`G1`–`G12` → §4.1 and §7.1–§7.10), since resolved
gaps belonged in the body rather than in a hazards list.

## Not done

No `.planning/` roadmap or phases — turning the delivery stages into tracked phases is a
separate decision. The working plan stays at
`reference/myco-runtime/plan/napplet-runtime.md` (gitignored) as the conversational
record.
