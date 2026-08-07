# Roadmap: Myco — Milestone A (Rock-solid peering)

## Overview

The mesh already works in principle; in the field it converges unevenly and gives the user
nothing to reason about. Milestone A fixes that in two moves and then cleans up behind
itself. First, make peering **observable** — a peer list that shows real state, real
transports, real radio status, and (internally) role decisions, discovery latency and
send-failure counts. That turns the inferred root causes (BLE role-tiebreaker race,
advertise/scan duty-cycle asymmetry, fire-and-forget fan-out) into facts. Second, use those
facts to make peering **converge and recover** — deterministic roles, pubkey-keyed sessions
that survive MAC rotation, durable delivery — and only then put plain-language reason codes
on the peers that still aren't connected. Those two phases plus a short field-fix pass are
the release cut. The fips rebase and the core-health work land *after* the release, because
neither is two-day work and neither changes what a user sees tomorrow.

## Release Boundary

**Demo: 2026-08-04 (today). Release: 2026-08-05 (tomorrow).**
The release ships only if mesh quality is measurably better than the current release.
Solo developer, phases execute sequentially.

| | Phases | Why |
|---|---|---|
| **Before the release** | 1, 2, 3 | These are what "mesh quality is measurably better" means. Phase 1 makes failures explainable at the demo; Phase 2 is the actual quality delta; Phase 3 is a short pass of small field-reported fixes sized to whatever hours remain. |
| **After the release** | 4, 5 | Real work, not release-blocking. The fips rebase is 19 commits across 232 commits of upstream drift — a multi-day, theme-timeboxed activity that the release date must not depend on. The `content.rs` breakup, lock hierarchy and panic purge change nothing a user sees tomorrow. |

If Phase 3 doesn't fit before the cut, it slips past the release without blocking it.
Phases 1 and 2 do not have that option — they *are* the release.

## Sequencing Constraints

These are load-bearing, from `.planning/research/PITFALLS.md`. Violating them costs the
release, not just some rework.

1. **Instrumentation before fixes.** The root causes are inferred from code, not confirmed
   by device logs. Phase 1 exists so Phase 2 is not spent guessing. Do not reorder.

2. **Reason codes come last inside Phase 2.** A plain-language reason on top of flaky
   reconnect logic is noise, not diagnosis — DIAG-02 lands after the churn/reconnect fixes
   in the same phase, not before them.

3. **The fips rebase is theme-by-theme with a per-theme timebox.** Not one mechanical
   rebase. Re-verify build and tests after each theme; drop any theme that fights the
   refactor to the fallback branch rather than risking the whole thing. Themes master
   already covers get dropped, not re-applied.

## Phases

**Phase Numbering:**

- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 1: Make Peering Observable** - [BEFORE RELEASE] Peer list showing real state, transport, radio status and pending pairings — plus the role/latency/drop instrumentation Phase 2 needs
- [ ] **Phase 2: Peering That Converges and Recovers** - [BEFORE RELEASE] Everyone in the room connects within 60s and stays connected across churn; unconnected peers say why
- [ ] **Phase 3: The Release Cut Behaves Itself** - [BEFORE RELEASE] Field-reported bugs that would embarrass the demo: Discover over-pinning, UI freeze during sync, silently emptied Circle
- [ ] **Phase 4: fips Rebased, Theme by Theme** - [AFTER RELEASE] Myco builds against current fips master, every retained theme standing alone as an upstream PR
- [ ] **Phase 5: A Core That Fails Loudly** - [AFTER RELEASE] Errors reach the user instead of killing the app; `content.rs` becomes concern-shaped modules with a documented lock order

## Phase Details

### Phase 1: Make Peering Observable

**Goal**: A user opening the peer view sees the true state of every peer and both radios, and a developer at the demo can say why a connection failed without attaching a debugger — turning the inferred peering root causes into observed facts before any fix is attempted. **Inside the deadline: must be usable at the 2026-08-04 demo.**
**Mode:** mvp
**Depends on**: Nothing (first phase)
**Requirements**: DIAG-01, DIAG-03, DIAG-04, DIAG-05, DIAG-06, DIAG-07
**Success Criteria** (what must be TRUE):

  1. User can open a peer list showing every known peer as connected, reachable via relay, or offline, and the list updates as peers come and go.
  2. For each peer, user can see how long ago it was last heard from and which transport (BLE, Wi-Fi Aware, TCP/Tor) is currently carrying it.
  3. User can see, for BLE and for Wi-Fi Aware separately, whether the radio is enabled and whether it is actively scanning right now.
  4. User can see their own npub and the Circle name other peers see them as, plus every pending pair request marked waiting, complete, or failed.
  5. After a failed connection, the diagnostics show — in the app, not a debugger — which BLE role this device chose for that peer, how long discovery took, and how many sends to it were dropped.

**Plans**: 2/4 plans executed
**UI hint**: yes

Plans:
**Wave 1**

- [x] 01-01-PLAN.md — Merge every known peer into one npub-keyed FFI array with true state, last-seen and transport

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 01-02-PLAN.md — Both radios report observed scanning/advertising state instead of a computed proxy

**Wave 3** *(blocked on Wave 2 completion)*

- [ ] 01-03-PLAN.md — Per-peer BLE role, discovery latency, attempt outcomes and send failures, persisted crash-tolerantly

**Wave 4** *(blocked on Wave 3 completion)*

- [ ] 01-04-PLAN.md — The Dev tab rebuilt around peering: radio self-check, expanding peer rows, pending pairings, identity

### Phase 2: Peering That Converges and Recovers

**Goal**: Phones in a room all find each other and connect without anyone touching anything, stay connected through Wi-Fi reconnects, MAC rotation, BLE flaps and backgrounding, and when a peer still isn't connected the list says why in plain language. **This phase is the release gate — "mesh quality measurably better than the current release" means this. Must land before 2026-08-05.**
**Mode:** mvp
**Depends on**: Phase 1 (fixes are chosen and confirmed against Phase 1's instrumentation, not against inference)
**Requirements**: PEER-01, PEER-02, PEER-03, PEER-04, PEER-05, PEER-06, CORE-04, DIAG-02
**Success Criteria** (what must be TRUE):

  1. Starting the app on every phone in a room results in every reachable phone showing connected to every other within 60 seconds, with no user action and no setting to find first — Wi-Fi Aware is already on out of the box on a fresh install.
  2. Two phones that discover each other at the same instant always end up in complementary BLE roles; no pair sits in a stalled handshake, and a failed attempt flips role rather than retrying the same one forever.
  3. After a Wi-Fi reconnect or MAC rotation, a BLE link drop, or the app being backgrounded and reopened, peering comes back on its own — the user never learns the toggle-mesh-off-and-on trick.
  4. A peer never shows connected while silently dropping everything: a send into a full queue is retried or reported as failed, and a pair request made while a peer is away survives an app restart and completes when that peer returns.
  5. Every peer in the list that is not connected shows a plain-language reason — no shared transport, handshake pending, handshake failed, out of range, not paired — and the reason matches what the device logs actually did.

**Plans**: 4 plans (3 original, re-scoped; +1 investigation)
**UI hint**: yes

Plans:

> **Re-scoped 2026-08-07 against work that landed early.** Phase 1's instrumentation
> was used to fix three things ahead of this phase (at the user's direction), and
> two field findings changed this phase's premise. Read
> `.planning/phases/01-make-peering-observable/01-FIELD-FINDINGS.md` F-05 and F-06
> before planning — the original 02-01 wording assumes a tiebreaker race that the
> field data does **not** show.
>
> **Already landed, do not re-plan:**
>
> | Was scoped as | Status |
> |---|---|
> | Wi-Fi Aware default-on (02-02) | ✅ done, device-verified — PEER-05 |
> | Pubkey-keyed peer state, BLE pool half (02-02) | ✅ done, field-verified — fips `cef3fc5` + `2120839`; a rotating peer can no longer occupy several pool slots |
> | Deterministic tiebreaker over stable pubkeys (02-01) | ✅ already correct — F-05 shows both sides applying the convention consistently; it was never racing |
>
> **What F-06 adds that was not scoped at all:** a device that never probes
> outbound deadlocks the pair, because the tiebreaker defers to an outbound that
> never comes and the losing side retries the same role at ~1 Hz forever. This is
> PEER-02's own wording and is currently reproducible on the DC-1.

- [ ] 02-01: **Role-flip retry** — a side that yields the tiebreaker notices the outbound it deferred to never materialised and flips role (F-06 defect 2, PEER-02). Generic; does not require diagnosing why a given device fails to probe. Plus foregrounded high-duty-cycle scan/advertise with jittered retry intervals. *(The tiebreaker convention itself is already correct — do not rewrite it.)*
- [ ] 02-02: Churn recovery — pubkey-keyed **Circle/peer state at the myco layer** (the fips BLE pool half is done), forced re-resolution on reconnect (FIPS#130), backgrounding survival. *(Wi-Fi Aware default-on is done.)*
- [ ] 02-03: Durable delivery (queue-full retry/report, persisted pair requests) then reason codes surfaced per peer — reason codes land last, once reconnect is sane
- [ ] 02-04 (new, investigation): **Why does the DC-1 never probe outbound?** F-06 defect 1. Six causes already ruled out on-device; what remains is whether `ScanFilter.setServiceUuid(FIPS_PARCEL_UUID)` matches what that stack delivers. Sized as a spike, not a fix — and explicitly *not* blocking 02-01, which is what makes the deadlock survivable regardless of the answer.

### Phase 3: The Release Cut Behaves Itself

**Goal**: The field-reported bugs that would embarrass the demo are gone — one tap pins one app, the UI keeps moving while the mesh syncs, and a corrupted Circle or Library file says so instead of quietly wiping itself. **Inside the deadline, but deliberately small: bounded fixes sized to the hours left before the 2026-08-05 release. If they don't fit, they slip without blocking the release.**
**Mode:** mvp
**Depends on**: Phase 2 (the sync-load and peer-state behaviour these fixes are verified against is the post-fix behaviour)
**Requirements**: UX-01, UX-02, CORE-03
**Success Criteria** (what must be TRUE):

  1. Opening one app from Discover adds exactly that app to the Library; every other app in the list is left alone.
  2. The UI scrolls and responds while a room-scale sync is running — the peer list and Library stay usable while peers are exchanging data.
  3. When `circle.json` or `library.json` is corrupt, the user sees an error naming the file, and the existing file is preserved on disk rather than replaced with an empty one.

**Plans**: 1 plan
**UI hint**: yes

Plans:

- [ ] 03-01: Discover single-app pinning, non-blocking state polling off the UI thread, corrupt Circle/Library surfaced instead of silently emptied

### Phase 4: fips Rebased, Theme by Theme

**Goal**: Myco builds and runs against current fips master, with each retained change theme standing alone as a focused upstream pull request and nothing Myco-specific left in the fips tree. **After the 2026-08-05 release, by design: 19 commits against 232 commits of upstream refactor is not two-day work, and the release must not depend on it landing.**
**Mode:** mvp
**Depends on**: Phase 3 (release is cut first; the rebase starts from a shipped, known-good baseline)
**Requirements**: FIPS-01, FIPS-02, FIPS-03, FIPS-04
**Success Criteria** (what must be TRUE):

  1. Myco builds green and the mesh runs on a device against current fips master, with no path dependency left on the old `feat/platform-peer-queue` branch.
  2. Each retained theme — Android BLE L2CAP backend + PSM discovery, platform-pushed peer queue, app-owned TUN/DNS seams, transport-preference roaming, UDP `sin6_scope_id` — applies to fips master on its own and is ready to open as a pull request.
  3. Every theme master already covers, or that fought the refactor past its timebox, is recorded as dropped with a one-line reason rather than re-applied or forced in.
  4. A fips maintainer reading the resulting tree finds no Myco names, types, or assumptions in it, and no theme's diff touches files outside its stated seam.

**Plans**: 2 plans

Plans:

- [ ] 04-01: Re-diff all 19 commits against master per theme; classify keep / dissolved / at-risk; rebase the first themes (BLE backend + PSM discovery, peer queue) with a per-theme timebox and build+test after each
- [ ] 04-02: Remaining themes (TUN/DNS seams, transport-preference roaming, UDP fix); Myco-specificity sweep; per-theme PR-readiness check against master

### Phase 5: A Core That Fails Loudly

**Goal**: The app tells the user when something breaks instead of vanishing, and the Content layer underneath it is shaped so the next round of peering fixes takes hours instead of days. **After the release: none of this changes what a user sees tomorrow, and all of it makes the milestone-B work cheaper.**
**Mode:** mvp
**Depends on**: Phase 4 (refactoring the Content layer against a settled fips seam avoids redoing it)
**Requirements**: CORE-01, CORE-02, CORE-05
**Success Criteria** (what must be TRUE):

  1. A failure on a critical path — storage, sync, pairing — reaches the user as a message they can act on instead of killing the app.
  2. A developer opening the Content layer finds concern-shaped modules, each readable on its own, instead of one 2,500-line file with 14 mutex fields.
  3. The lock acquisition order is written down, the code acquires locks in that order, and a reviewer can check a new call site against the document without reading the whole layer.

**Plans**: 2 plans

Plans:

- [ ] 05-01: Split `content.rs` into concern-shaped modules; document the lock hierarchy and align acquisition order to it
- [ ] 05-02: Replace panic-prone unwrap/expect on critical paths with handled errors that surface to the user

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5

Phases 1-3 are inside the 2026-08-05 release deadline. Phases 4-5 land after it.

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Make Peering Observable | 2/4 | In Progress|  |
| 2. Peering That Converges and Recovers | 0/3 | Not started | - |
| 3. The Release Cut Behaves Itself | 0/1 | Not started | - |
| 4. fips Rebased, Theme by Theme | 0/2 | Not started | - |
| 5. A Core That Fails Loudly | 0/2 | Not started | - |

## Coverage

All 24 v1 requirements are mapped to exactly one phase. No orphans, no duplicates.

| Phase | Requirements | Count |
|-------|--------------|-------|
| 1 | DIAG-01, DIAG-03, DIAG-04, DIAG-05, DIAG-06, DIAG-07 | 6 |
| 2 | PEER-01, PEER-02, PEER-03, PEER-04, PEER-05, PEER-06, CORE-04, DIAG-02 | 8 |
| 3 | UX-01, UX-02, CORE-03 | 3 |
| 4 | FIPS-01, FIPS-02, FIPS-03, FIPS-04 | 4 |
| 5 | CORE-01, CORE-02, CORE-05 | 3 |
| **Total** | | **24** |

v2 requirements (MESH, BACK, WEB, NAP — Milestone B) are deliberately out of this roadmap
and become their own via `/gsd-new-milestone` once Milestone A ships.
