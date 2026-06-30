# Contributing to Myco

<!-- markdownlint-disable MD013 -->

Myco is an offline-first, peer-to-peer Android client for nsites —
self-contained web apps served from a local relay + Blossom store and
shared with people nearby over a Bluetooth LE mesh. The codebase spans
an Android app and a Rust core, top to bottom:

- **Android app** (`android/`) — the Kotlin / Jetpack Compose shell:
  Apps, Circle, Discover, and Settings; it pairs with peers (QR, NFC
  tap, nearby) and renders each nsite in its own chrome-less WebView.
- **`myco-core`** (Rust) — the brains, driven from the app over a JNI
  bridge: device identity, the pairing/Circle state machine, content
  sync, and the embedded gateway. It hosts an embedded FIPS mesh node.
- **`myco-relay` / `myco-blossom`** (Rust) — the embedded NIP-01 relay
  and Blossom blob store that serve an nsite's signed manifest and
  blobs over the mesh and in-app loopback.
- **`nsite-deck`** (Rust) — nsite manifest / deck handling.
- **`reference/fips`** — the embedded FIPS mesh node (BLE L2CAP CoC
  transport, IPv6 ULA addressing), consumed as a path dependency.

Most non-trivial changes affect behavior that only shows up on a real
device — or across two paired phones: pairing, BLE/NFC discovery, mesh
sync. A host `cargo test` run is necessary but not sufficient for that
class of change; testing on a physical device (and a second one for
anything pairing-related) is where regressions actually surface.
Protocol and design depth lives in [docs/design/](docs/design/).

## Quick start

```bash
git clone https://github.com/Origami74/myco.git
cd myco

# Rust core + crates (host build/tests use the in-memory mock transports)
cargo build
cargo test

# Android app — also cross-compiles the Rust core into the app's jniLibs
cd android
./gradlew assembleDebug
```

The Android build needs the Android SDK + NDK and the Rust Android
targets (e.g. `aarch64-linux-android`); the Gradle build invokes cargo
to produce the native library. `minSdk` is 29 (the L2CAP CoC floor).
Install a debug build on a connected device with
`./gradlew installDebug`.

## Choosing a branch to target

Myco is single-trunk: branch off the latest **`main`**, and open your
PR back into `main`. Keep your branch rebased on a recent `main` so the
diff stays reviewable.

## Reporting bugs

Search [open issues](https://github.com/Origami74/myco/issues) before
filing a new one — duplicates are common in a young project.

When you open a bug report, please include:

- **Myco version** (Settings shows the app version).
- **Android version + device model** (e.g. "Android 15, Pixel 7 Pro").
- **What you expected to happen** — your mental model of the behavior.
- **What actually happened** — the observed behavior, including the
  surprise.
- **Reproduction steps** — minimal and deterministic if you can.
  Pairing/mesh bugs should note how many devices were involved and what
  each was doing (which tab, NFC vs QR, online/offline).
- **Evidence** — relevant `adb logcat` excerpts, and a screen recording
  or screenshots for UI issues.

One issue per bug. Don't bundle unrelated symptoms even if you suspect
they share a root cause — they'll be linked if they turn out to be
related.

## Submitting pull requests

### Scope discipline

Every PR should make one logical change. The reviewer should be able to
read the whole diff and trace every line back to the PR's stated
purpose.

- No drive-by reformatting of unrelated files.
- No unrelated refactors folded into a bug fix or a feature PR.
- No "while I was in there" cleanups in files outside the change's
  natural footprint. Send them as separate PRs; they'll usually land
  faster on their own.
- Pre-existing lint warnings in files you didn't touch are not yours to
  fix in this PR.

### Required before opening any PR

Run the checks for whatever you touched and confirm they pass.

Rust (`myco-core`, `myco-relay`, `myco-blossom`, `nsite-deck`):

```bash
cargo fmt --check
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

Android (`android/`):

```bash
cd android
./gradlew assembleDebug
```

Formatting drift and new clippy warnings are the cheapest things to
catch locally — fix them before opening rather than in review.

### Self-review against the project review checklist

The 13-criteria checklist used on every incoming PR is published at
[PR-REVIEW.md](PR-REVIEW.md). Run your own change through it before
opening — or hand the document to your coding agent with "review my
branch against this checklist" and let it do the pass. The checklist
covers PR hygiene (body, commit shape, base freshness), diff content
(does the change do what the description says, does it fit the codebase
as a natural extension), and cross-cutting concerns (tests, docs,
dependencies, security, idiomatic Rust + Kotlin/Android patterns).

Running it yourself saves a review round trip.

### Additional requirements for feature PRs

- **Test coverage where practical.** Add `cargo test` coverage for new
  core logic. Behavior that can only be exercised on-device (pairing,
  BLE/NFC, WebView) should be described in the PR with the manual test
  you ran (which devices, what you observed).
- **Documentation updated alongside the code.** Design-affecting
  changes update the relevant [docs/design/](docs/design/) page;
  user-visible changes update [README.md](README.md); and every release
  note belongs in [CHANGELOG.md](CHANGELOG.md) under `[Unreleased]`.

### Additional requirements for bug-fix PRs

- **A regression test** where practical. If one isn't tractable (some
  bugs only surface across two devices or under timing that's hard to
  encode), say so in the PR description with a one-paragraph
  explanation.
- **Commit message references the bug**: the symptom, the root cause in
  one sentence, and the fix shape.

### Merge mechanics

PRs land on `main`. Keep your branch a clean set of commits — squash
intermediate "WIP" / "fix typo" / "address review" commits — so the
landed history stays bisectable.

## AI coding assistant policy

Use of AI coding assistants (Claude Code, Copilot, Cursor, Aider, and
similar) in preparing a contribution is welcome. These tools are force
multipliers and there's no objection in principle to their use in
writing code, tests, documentation, or PR descriptions.

What's required is that the contributor does a thorough manual review
and editorial pass over the output before submission. Concretely:

- Verify that the code does what it claims, not just that it compiles.
- Verify that any tests the agent wrote actually test something useful,
  not just that they pass.
- Verify that any documentation matches the behavior.
- Spot-check the diff for nothing-surprising: no unrelated files
  modified, no fabricated APIs, no references to symbols that don't
  exist, no version bumps you didn't intend, no churn outside the
  change's natural footprint.
- Be ready to discuss the design choices in the PR as if you wrote
  every line, because for the purposes of accountability you did.

The coding agent is a tool. The contributor is the author of record and
is accountable for whatever they submit. PRs are reviewed on what they
contain, not on who or what wrote them.

**Review effort scales with submission effort.** A submission that
shows signs of being unreviewed agent output — irrelevant edits
scattered across the tree, hallucinated function names, mismatched
test/behavior pairs, fabricated API references, summary-prose comments —
will get a correspondingly low-effort reply. If you want a careful human
review, do the editorial pass yourself first.

## Where the conversation happens

- **GitHub issues** — bugs, feature requests, and design discussions
  that don't fit on a specific PR.
- **GitHub PRs** — design discussion specific to a change in flight.
  Comment threads on the diff are the right place to push back on a
  decision.

For implementation questions specific to your PR, ask in the PR itself.
For design or roadmap questions that don't have a clear PR home yet,
file a GitHub issue.

## Further reading

- [PR-REVIEW.md](PR-REVIEW.md) — the 13-criteria PR review checklist;
  run it yourself before opening to save a round trip.
- [docs/design/](docs/design/) — architecture and protocol design.
- [CHANGELOG.md](CHANGELOG.md) — the per-release change history.
- [README.md](README.md) — project overview.
