Phase 01 turns peering from something you infer into something you read off the screen — and then uses that instrumentation to find and fix three real faults, two of which were never in the plan.

The Dev tab now opens on a radio self-check, and every peer row expands in place onto the BLE role this device chose, how long discovery took, how the attempt resolved and how many sends failed — with a bounded, crash-surviving history so a race that resolved thirty seconds ago still leaves evidence.

That instrument was then read, and it changed what we thought was wrong.

---

## User Stories & Acceptance Criteria

**Phase 01 — Make Peering Observable. All six DIAG requirements satisfied.**

| | | Verified |
|---|---|---|
| DIAG-01 | Every known peer with its current connection state | device |
| DIAG-03 | How long ago each peer was last heard from | device |
| DIAG-04 | Which transport is carrying each connected peer | device |
| DIAG-05 | Whether each radio is enabled and actively scanning | device |
| DIAG-06 | Pending pair requests | device |
| DIAG-07 | Own identity and the Circle name peers see | device |

**Three release-gate requirements landed early**, while working the field TODO list:

- **PEER-05** — Wi-Fi Aware on by default on a fresh install. Observed coming up unprompted: `Aware attached → publish started → subscribe started`, no toggle touched.
- **UX-01** — opening one app from Discover pins only that app. The report was that clicking any app pinned all of them; the cause was worse — merely *opening* the tab did it. `DiscoverTile`'s favicon probe hit `gatewayGet`, which spawned `open_site()` on a 503, which called `add_to_library(pinned: true)`. Verified fixed on device: three tiles rendered, library stayed at one entry, zero sync log lines.
- **PEER-03** (MAC-rotation half) — a peer rotating BLE addresses can no longer occupy several connection-pool slots. See F-05 below.

**Six `reference/FIX-TODOS.md` items closed**, all confirmed working on device: Discover over-pinning, Wi-Fi Aware default, camera autofocus, nsite status-bar inset, pending-pairing visibility, own-identity visibility, plus the home-screen offer when a peer-shared app finishes downloading.

---

## Risks & Dependencies

### Requires a matching `fips`

`myco-core` now imports `fips::transport::ble::attempts`, which is new. `fips` is a gitignored path dependency (`Cargo.toml:24`), so CI clones it — `FIPS_REF` must point at a revision that carries these four commits:

| | |
|---|---|
| `4e3dfa8` | per-peer BLE connect-attempt log |
| `5c49a44` | recording at every outcome site, both sides of the tiebreaker |
| `cef3fc5` | recognise a peer by node identity, not its rotating link address |
| `2120839` | stop re-probing an address already resolved to a live peer |

**Pinning `FIPS_REF` to a commit rather than a branch is worth doing here** — the current float is what made this dependency invisible until CI ran.

The fips diff is deliberately upstream-extractable: three files, no Myco vocabulary anywhere in them (`grep -c 'myco\|Myco\|android' attempts.rs` → 0).

### Dependency surface

One addition, test-scope only: `testImplementation("junit:junit:4.13.2")`, which
backs `ThemeTest`. No new runtime crates, no new system dependencies, no new
Gradle runtime artifacts. The `Cargo.lock` delta is the `fips` version moving
`0.4.0-dev` → `0.5.0-dev`, not a new package.

### Behavioural risk

The two fips fixes change BLE admission control. A peer already connected is now recognised across a rotated link address and its duplicate declined; the tiebreaker itself is untouched. Both are field-verified (below), but they are the highest-risk change in this PR — everything else is additive UI and instrumentation.

### Not verified

- **D-4** (relay-reachability correlation) and **D-5** (Wi-Fi Aware lane label on a live NDP) need a second two-phone session — see `.planning/phases/01-make-peering-observable/DEVICE-TEST-BATCH.md`.
- **F-06 defect 1** — why the DC-1 never probes outbound. Six causes ruled out, cause unknown. Does not block this PR; it is instrumentation *finding* the problem, not this PR causing it.

### Nyquist gap, stated plainly

The Android module has one JVM unit test and no Compose UI-test harness, so nothing automated pins the rendered layout or the expansion behaviour. This is not theoretical: a `rememberSaveable` crash that killed the Dev tab on every cold open passed both `compileDebugKotlin` and `testDebugUnitTest`, and was caught only by running it on a phone (`41ab89b`).

---

## Success Metrics & Release Criteria

### This does not meet the release gate, by design

The roadmap is explicit: *"Phase 2 is the release gate — mesh quality measurably better than the current release means this."* Phase 1 makes failures explainable; it does not fix them. Phase 2 has not started. Three of its requirements are met early (above); PEER-01, PEER-02, PEER-04, PEER-06, CORE-04 and DIAG-02 are not.

### What the instrument found

**F-05 — the tiebreaker was never racing.** The hypothesis Phase 2 was going to act on was a BLE role-tiebreaker race. The first real reading of the attempt log does not support it: against one peer, 6 `central`/`connected` and 28 `peripheral`/`lost-tiebreaker` — the convention applied correctly on *both* paths. What churned was 28 *distinct link addresses* belonging to one node, rotating resolvable private addresses.

A code read then showed why that was worse than it looked: every identity check keyed on the link address, and the pool holds 7. Those rotations were harmless only because the tiebreaker happened to reject them — had the two node addresses sorted the other way, they would have been admitted and evicted real peers roughly four times over.

Fixed, and **field-verified against the same peer that produced the finding**:

```
BLE probe: peer already connected on another address, dropping duplicate
    addr=ble0/6B:69:40:AE:45:EA  existing=ble0/60:6B:C1:8B:3C:44
```

The fix then caused a smaller problem — a declined address never enters the pool, so the cooldown guard never saw it and the loop re-probed every ~30s forever. Also fixed, also verified: zero recurrences across a 9-minute window against a prior ~2/min, while legitimate connections continued to be promoted.

**F-06 — PEER-02 is failing in the field.** Across the DC-1's entire recorded attempt history, 61 records, *every one is `peripheral`*. It has never initiated an outbound BLE connection. Its tiebreaker still defers to an outbound that never comes, so the peer dialling it is dropped and retries at ~1 Hz indefinitely and neither side connects. That is PEER-02's own wording — *"a failed attempt flips role rather than retrying the same one forever"* — reproducing on attached hardware.

Phase 2 has been re-scoped around both findings rather than planned from the original assumption.

### Verification

| Gate | Result |
|---|---|
| fips full suite | 1406 passed, 0 failed |
| `cargo test` (myco workspace) | all crates pass; myco-core 65 |
| `cargo fmt --all --check`, both trees | clean |
| `just ndk-build` | 22 MB aarch64 `.so` |
| `:app:compileDebugKotlin` + `:app:testDebugUnitTest` | BUILD SUCCESSFUL, `ThemeTest` green |
| `assembleDebug` | 79 MB APK, installed and exercised on two devices |

Device verification ran on a Samsung SM-A528B and a Daylight DC-1, always via `adb install -r` so app data, identity and Circle survived every install.

---

## Self-review against PR-REVIEW.md

Run per CONTRIBUTING before opening. Notes where it is worth a reviewer's time:

- **Commit hygiene** — 58 commits, none WIP/fixup/typo; each is a task-scoped
  change with a message stating symptom, cause and fix shape. Base is fresh:
  0 commits behind `main`, so this fast-forwards.
- **Coherent whole** — two clusters, deliberately. The Phase 01 plans (01-01 …
  01-04) are the instrumentation; the rest are fixes that instrumentation found
  or that came off the field TODO list. If that is too broad for one PR, the
  natural split is the `.planning/` + Dev-tab work from the BLE transport fixes.
- **Test coverage** — new core logic is covered (`attempts.rs` 9 tests,
  `attempt_store.rs` 8, `pool.rs` +4 including a regression test for the
  rotation case, `peer_diagnostics.rs` +4). What is *not* covered is the Compose
  layer — see the Nyquist gap above; that is stated rather than papered over.
- **Documentation** — `CHANGELOG.md` updated under `[Unreleased]` with Added /
  Changed / Fixed entries in user-facing language.
- **Security** — the attempt log persists BLE addresses and timestamps, i.e. a
  record of which devices were physically near this phone. Bounded to 20 entries
  per peer and evicted after 24 hours, kept in app-private storage, with no
  export path. Records carry no peer-supplied free text. Threat-modelled in
  `01-03-PLAN.md`.

## Review notes

- **`.planning/` is included** (`commit_docs: true`). The substantive reading is `.planning/phases/01-make-peering-observable/01-FIELD-FINDINGS.md` — F-05 and F-06 are what Phase 2 depends on.
- **`reference/FIX-TODOS.md` is tracked inside a wholesale-gitignored path** (`.gitignore:3`), force-added in an earlier session. It is the only tracked file there and carries Phase 2 input. Worth an explicit decision — `git rm --cached`, or keep deliberately.
- The four fips commits are in a **separate repository** and are not part of this diff.
