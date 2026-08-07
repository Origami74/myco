# Ship readiness — `gsd/phase-01-make-peering-observable` → `main`

Written 2026-08-07 from the Linux build host. Branch is **56 commits ahead of
`main`, 0 behind** — a clean fast-forward, no conflicts to resolve.

---

## 🔴 BLOCKER — merging now breaks CI and every fresh checkout

`myco-core/src/peer_diagnostics.rs:19` imports:

```rust
use fips::transport::ble::attempts::{BlePeerAttempts, MAX_ATTEMPTS_PER_PEER};
```

That module was created this session. `fips` is a **gitignored path dependency**
(`Cargo.toml:24` → `reference/fips`), so the myco repo does not carry it; CI
clones it (`.github/workflows/ci.yml`):

```yaml
FIPS_REPO: https://github.com/jmcorgan/fips.git
FIPS_REF:  feat/platform-peer-queue
```

**That ref does not have our commits.** Verified:

| | commit |
|---|---|
| `jmcorgan/fips` @ `feat/platform-peer-queue` (what CI clones) | `59028d6` |
| what myco now needs | `2120839` |

Our four fips commits (`4e3dfa8`, `5c49a44`, `cef3fc5`, `2120839`) live only on
the nostr fork:

```
nostr://npub1ymx0pnzc5sdmfgd4mf8netq8dyx6wzgc5g4asndpgpzqkefy5h2syxs3fp/relay.ngit.dev/fips
```

So a merge to `main` today gives a repo that **does not compile from a fresh
clone**, and CI fails on the first run. This is not a warning — it is certain.

### Three ways to clear it, in order of preference

1. **Push the fips branch to a GitHub remote the project controls**, then point
   `FIPS_REPO`/`FIPS_REF` at it. Keeps CI on the transport it already uses. Needs
   a GitHub repo you can write to — `jmcorgan/fips` is upstream and not ours.
2. **Get the four commits upstream into `jmcorgan/fips@feat/platform-peer-queue`.**
   Cleanest long-term and the commits were written to be upstream-extractable —
   `attempts.rs` carries no Myco vocabulary and the whole footprint is three files.
   Slowest, since it depends on someone else merging.
3. **Point CI at the nostr fork's git server** —
   `https://relay.ngit.dev/npub1ymx0pnz…/fips.git` is plain HTTPS and clonable
   unauthenticated. Fastest. Couples CI to a relay-hosted mirror, which is a real
   availability dependency to accept knowingly rather than by accident.

**Whichever is chosen, `FIPS_REF` should be pinned to a commit rather than a
branch name.** The current `feat/platform-peer-queue` float is exactly why this
blocker is invisible until CI runs.

---

## Green

| Gate | Result |
|---|---|
| fips full suite | 1406 passed, 0 failed |
| `cargo test` (myco workspace) | all crates pass; myco-core 65 passed |
| `cargo fmt --all --check` both trees | clean |
| `reference/clippy-gate.sh` | PASS vs recorded baseline |
| `just ndk-build` | 22 MB aarch64 `.so` |
| `:app:compileDebugKotlin` + `:app:testDebugUnitTest` | BUILD SUCCESSFUL, ThemeTest green |
| `assembleDebug` | 79 MB APK, installs and runs on two devices |
| Working tree | clean, both repos |

Note the clippy gate is the **local** substitute described in
`reference/clippy-gate.sh` — the plans' literal
`cargo clippy --all-targets -- -D warnings` cannot pass on this host and, worse,
aborts at the first failing crate so it silently skips myco-core. CI runs its own
clippy step; confirm what that step actually does before trusting it.

---

## What is being merged

**Phase 01 complete (4/4 plans).** DIAG-01/03/04/05/06/07 all satisfied.

Three release-gate requirements landed early and are device-verified:

- **PEER-05** — Wi-Fi Aware on by default
- **UX-01** — opening one app from Discover pins only that app
- **PEER-03** (MAC-rotation half) — node-identity keying so a rotating peer cannot
  occupy several pool slots

Six `reference/FIX-TODOS.md` items closed, all confirmed working on device.

Two findings Phase 2 depends on: **F-05** (the tiebreaker was absorbing address
rotation, not racing — fixed) and **F-06** (PEER-02 failing live: a device that
never probes outbound deadlocks the pair).

---

## Decide before merging

1. **The blocker above.** Nothing else matters until it is resolved.
2. **`reference/FIX-TODOS.md` is tracked inside a wholesale-gitignored path**
   (`.gitignore:3` excludes `/reference/`; the file was `git add -f`'d). It is now
   the only tracked file there and it carries Phase 2 input. Open since the first
   handoff. `git rm --cached` to undo, or leave deliberately — but decide, because
   a gitignored-but-tracked file surprises the next person.
3. **`reference/clippy-gate.sh` and `clippy-baseline.txt` are also under
   `/reference/`** and therefore untracked. They are machine-local by design (the
   baseline is arch- and toolchain-specific) — fine, but that means the gate does
   not travel with the branch and CI is not running it.
4. **`.planning/` commits are included** (`commit_docs: true`). If `main` should
   stay code-only, `gsd-pr-branch` filters them out.
5. **Phase 01's `gsd-verifier` goal-backward check has not been run.** The
   build+test half of the phase tail is green; the judgement half is not done.

---

## Not verified

- **D-4** (F-04 relay reachability correlation) and **D-5** (Wi-Fi Aware lane
  label showing `aware` on a live NDP) — both need a second session with two
  phones. See `phases/01-make-peering-observable/DEVICE-TEST-BATCH.md`.
- **F-06 defect 1** — why the DC-1 never probes outbound. Six causes ruled out;
  cause unknown. Does not block the merge, but it means one attached device
  currently cannot initiate BLE connections at all.
