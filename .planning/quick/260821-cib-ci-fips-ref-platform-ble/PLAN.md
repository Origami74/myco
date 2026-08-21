---
id: 260821-cib
status: in-progress
branch: chore/ci-fips-platform-ble
---

# CI: track the fips `fix/platform-ble` branch

## Why

`reference/fips` locally moved from `integration/platform` to `fix/platform-ble`.
That branch is `integration/platform`'s mobile-seam work rebased onto current
fips master, plus BLE fixes (BlueZ PSM advertise/read, per-address dial backoff).
CI still clones `integration/platform`, so it no longer builds what developers build.

## Verified before planning

- Host `cargo build` against `fix/platform-ble`: clean, no API breakage.
- `./gradlew assembleDebug`: BUILD SUCCESSFUL, APK produced, `Cargo.lock` undisturbed.

## Task

Edit `.github/workflows/ci.yml`:

1. `FIPS_REF: integration/platform` → `fix/platform-ble`.
2. Update the `env:` comment above it — it describes what the branch carries, and
   the branch is now rebased-on-master rather than a long-lived integration line.

## Out of scope

`.github/workflows/release.yml` pins the same ref. Pointing the release pipeline
at a moving branch is a separate call — left for the user to decide.

## Verification

`FIPS_REF` reads `fix/platform-ble`; the branch resolves on the remote
(`git ls-remote https://github.com/jmcorgan/fips.git fix/platform-ble`).
