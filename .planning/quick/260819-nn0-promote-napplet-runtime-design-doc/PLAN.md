---
task: Promote the napplet runtime plan into docs/design/
quick_id: 260819-nn0
date: 2026-08-19
branch: next
---

# Promote the napplet runtime plan into `docs/design/`

Turn the working plan at `reference/myco-runtime/plan/napplet-runtime.md` (gitignored
scratch) into a checked-in design document that reads like the rest of `docs/design/`.

## Scope

1. Write `docs/design/napplet-runtime.md`, matching the house style of
   `nsite-layer.md` / `deep-links.md`: `# Title`, prose lead, related-doc links, `---`,
   numbered `## N.` sections. No frontmatter.
2. Carry over in full: the 12 decisions, the resolution pipeline, the architecture
   (crate layout, shell, transport, intent bridge, window model), the slices, and the
   open questions.
3. Drop the interview framing ("locked decisions", "gaps we may be missing") and the
   session-scratch tone. Written for someone who did not sit in the conversation.
4. Reconcile the internal contradictions the scratch doc accumulated as decisions
   landed:
   - §5.1's "not folded into nsite-deck" predates G1 — shared NIP-5A primitives now live
     in `nsite-deck`.
   - `napplet.localhost` survives in §5.5 and S1 after D12 made origins per-napplet.
   - S3 still lists an "offline degradation rule" for NAP-OUTBOX, which G5 replaced.
   - §5.4 lists `nostr:` as a live entry point, which G10 deferred.
5. Add a row to the design-doc table in `docs/README.md`.

## Out of scope

No code. No `.planning/` roadmap or phases — that is a separate decision.

## Done when

`docs/design/napplet-runtime.md` exists, is internally consistent, is linked from
`docs/README.md`, and is committed on `next`.
