# Deferred Items — Phase 01 (Make Peering Observable)

Out-of-scope discoveries found during execution, logged per the executor's scope-boundary
rule rather than fixed inline.

## 01-01: pre-existing `fips` clippy errors block the workspace-wide `-D warnings` gate

**Found during:** Task 2, running `cargo clippy -p myco-core --all-targets -- -D warnings`.

**Issue:** `cargo clippy -p myco-core` builds `reference/fips` (a local path dependency) with
full clippy lints, not just plain `rustc`. Two pre-existing lint errors in files this plan
never touches fail the `-D warnings` gate:

1. `reference/fips/src/transport/udp/darwin_sockopts.rs:13` — `clippy::duplicated_attributes`:
   `#![cfg(target_os = "macos")]` duplicates the `#[cfg(target_os = "macos")]` already on the
   `mod darwin_sockopts;` declaration in `reference/fips/src/transport/udp/mod.rs:11`.
2. `reference/fips/src/node/lifecycle.rs:628` — `clippy::collapsible_if`: a nested `if let ... {
   if ... { continue; } }` that clippy wants collapsed into one `if let ... && ... { continue; }`.

**Confirmed pre-existing:** `git log --oneline -- src/transport/udp/darwin_sockopts.rs
src/node/lifecycle.rs` shows both files were last touched by `feat/platform-peer-queue` commits
that predate this phase (`006c331`, `5275146`, `0aed417`) — unrelated to the `read_handle.rs`
change this plan makes.

**Why not fixed here:** Out of scope per the executor's scope-boundary rule — these files are
untouched by 01-01's diff, and fixing them here would mix an unrelated fips cleanup into a diff
this plan's own acceptance criteria requires to "stand alone as an upstream PR" (the three
additive `PeerView` fields only). Both are genuinely trivial (a duplicate `cfg` attribute; a
collapsible `if`) and safe for a future targeted fips cleanup pass — likely as part of Phase 4
(fips rebase) or a dedicated small PR before then.

**Verification performed instead:** `cargo clippy -p myco-core --all-targets -- -D warnings
2>&1 | grep 'peer_diagnostics\|myco-core/src/runtime.rs\|myco-core/src/state.rs'` produces no
output — this plan's own new/changed myco-core files carry zero clippy warnings. `cargo test -p
myco-core` (48 passed) and `cargo fmt --all --check` both pass clean.

**Status:** open — recommend a 2-line fips-only fix (delete the duplicate `cfg` line; collapse
the nested `if`) as its own tiny upstream-extractable commit, whenever fips is next touched.
