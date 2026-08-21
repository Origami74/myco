---
id: 260821-cib
status: complete
branch: chore/ci-fips-platform-ble
commit: 67832eca209b6af9c48c7ad280293a9d197fb90b
date: 2026-08-21
---

# CI: track the fips `fix/platform-ble` branch

## What changed

`.github/workflows/ci.yml` — `FIPS_REF: integration/platform` → `fix/platform-ble`,
and the `env:` comment now explains that the branch is the mobile-seam work
rebased onto fips master rather than a long-lived integration line.

Both jobs (`rust`, `android`) pick it up; they share the one `FIPS_REF`.

## Verification

- `git ls-remote` resolves `fix/platform-ble` at `4e6f1841` — same head the local
  `reference/fips` checkout is on.
- YAML parses; `env.FIPS_REF` reads `fix/platform-ble`.
- Host `cargo build` clean against the branch. The seam rename
  (`expose the UDP transport's socket fd` → `hand the embedder each UDP listen
  socket, labelled by instance`) did not break myco-core.
- `./gradlew assembleDebug` — BUILD SUCCESSFUL, `Cargo.lock` undisturbed.
- APK installed on both devices: SM-A528B (`R5CR916CDCF`) and Pixel 7 Pro
  (`29131FDH3007HW`).

## Left open

`.github/workflows/release.yml:22` still pins `integration/platform`. Pointing the
release pipeline at a branch that is still being rebased is a separate decision
and was not made here — so CI and release currently build different fips code.

## Note on the branch shape

`fix/platform-ble` is **not** ahead of `integration/platform`: they diverge 16/17
from merge-base `23ec0a7b` (fips master). `integration/platform` still carries
`6580a806 fix(node): compare a candidate address against the instance the peer is
on`, which has no counterpart on `fix/platform-ble`. Worth checking whether that
fix needs porting.
