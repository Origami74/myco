# Requirements

**Project:** Myco
**v1 milestone:** Milestone A — Rock-solid peering (demo 2026-08-04, release 2026-08-05)
**v2 milestone:** Milestone B — Mesh protocol + napplet runtime

v1 below is the scope of the current roadmap. v2 is captured so it isn't lost, and
becomes a roadmap of its own via `/gsd-new-milestone` once v1 ships.

---

## v1 Requirements — Milestone A

### Peering reliability

- [ ] **PEER-01**: Every phone in a room connects to every other reachable phone within 60 seconds of the app starting, with no user action
- [ ] **PEER-02**: Two devices that discover each other simultaneously always agree on their BLE roles — no device pair can deadlock by both choosing the same role
- [ ] **PEER-03**: Peering recovers on its own after a Wi-Fi reconnect or MAC rotation, without the user toggling mesh off and on
- [ ] **PEER-04**: Peering recovers on its own after a BLE link drop or after the app has been backgrounded
- [ ] **PEER-05**: Wi-Fi Aware is enabled by default on a fresh install
- [ ] **PEER-06**: When a peer's send queue fills, the event is retried or reported as failed rather than silently dropped

### Peer diagnostics

- [x] **DIAG-01**: User can see every known peer with its current connection state — connected, reachable via relay, or offline
- [ ] **DIAG-02**: For any peer that is not connected, user can see a plain-language reason: no shared transport, handshake pending, handshake failed, out of range, or not paired
- [x] **DIAG-03**: User can see how long ago each peer was last heard from
- [x] **DIAG-04**: User can see which transport is currently carrying each connected peer
- [x] **DIAG-05**: User can see whether each radio is enabled and actively scanning, for both BLE and Wi-Fi Aware
- [ ] **DIAG-06**: User can see pending pair requests and whether each is waiting, complete, or failed
- [ ] **DIAG-07**: User can see their own identity and the Circle name other peers see them as

### Upstream mesh core

- [ ] **FIPS-01**: The Myco fips branch is rebased onto current fips master and Myco builds and runs against it
- [ ] **FIPS-02**: Each retained change theme is a self-contained commit series that applies to fips master on its own, ready to open as a focused pull request
- [ ] **FIPS-03**: Themes already present on master are dropped rather than re-applied
- [ ] **FIPS-04**: The fips tree contains no Myco-specific names, types, or assumptions

### Code health

- [ ] **CORE-01**: `content.rs` is split into concern-shaped modules, each independently readable
- [ ] **CORE-02**: The Content layer's lock ordering is documented and the code acquires locks in that documented order
- [ ] **CORE-03**: A corrupted `circle.json` or `library.json` surfaces an error and preserves the existing file instead of silently loading as empty
- [ ] **CORE-04**: A pair request survives an app restart and is retried when the mesh reconnects
- [ ] **CORE-05**: Panics on critical paths are replaced with handled errors that reach the user as a message

### Field-reported fixes

- [ ] **UX-01**: Opening one app from Discover pins that app only, not every app in the list
- [ ] **UX-02**: The UI stays responsive while the mesh syncs — state polling never blocks the UI thread

---

## User Stories

- As someone at a meetup, I open Myco and my phone finds everyone else running it, so I can browse what they're sharing without asking anyone for anything.
- As someone whose phone isn't connecting, I open the peer list and read why — so I know whether to move closer, turn a radio on, or pair first, instead of guessing.
- As someone who walked out of Wi-Fi range and came back, my mesh reconnects by itself, so I never learn the toggle-it-off-and-on trick.
- As someone browsing Discover, I tap one app and get one app, so my Library stays mine.
- As a fips maintainer receiving Myco's changes, each pull request is scoped to one concern and carries nothing Myco-specific, so I can review and merge it on its own merits.

## Definition of Done — v1

- Room-scale convergence and churn recovery both demonstrated on at least three device vendors
- Every unconnected peer in the diagnostics list shows a reason, and the reasons are accurate against device logs
- The fips branch applies to master, Myco builds green against it, and each theme is PR-ready
- Mesh quality is measurably better than the previous release — this is the gate on shipping at all

---

## v2 Requirements — Milestone B (deferred)

### Mesh event protocol

- [ ] **MESH-01**: Mesh hop TTL travels in the relay-to-relay protocol, not inside Nostr event bodies
- [ ] **MESH-02**: A backend relay only ever receives clean NIP-01 `EVENT` frames with no mesh-specific fields
- [ ] **MESH-03**: TTL is decremented by Myco's own gossip layer, never by the backend relay
- [ ] **MESH-04**: Devices running the old TTL-in-body format are not supported — clean break

### Pluggable backends

- [ ] **BACK-01**: An unmodified third-party Nostr relay (Citrine) can serve as Myco's relay backend
- [ ] **BACK-02**: An unmodified BUD-01 Blossom endpoint can serve as Myco's blob backend
- [ ] **BACK-03**: User can set custom relay and Blossom URLs in settings
- [ ] **BACK-04**: User can choose embedded, external, or both for each backend
- [ ] **BACK-05**: Circle gating and mesh fan-out keep working when the backend relay is third-party and dumb

### Browser-facing mesh API

- [ ] **WEB-01**: An nsite can reach the mesh through a relay URL that applesauce and nostr-tools dial unmodified
- [ ] **WEB-02**: That URL is scoped per nsite session so one nsite cannot act as another
- [ ] **WEB-03**: `window.myco.neighbours` offers publish and subscribe without requiring a Nostr library
- [ ] **WEB-04**: Publishing to the mesh is always an explicit call — an ordinary local relay publish never reaches neighbours as a side effect

### Napplet runtime

- [ ] **NAP-01**: Myco resolves, verifies, and runs a NIP-5D napplet from a kind-35129 manifest
- [ ] **NAP-02**: Napplets execute inside the spec's sandbox boundary — no same-origin access, no direct network, no injected `window.nostr`
- [ ] **NAP-03**: Napplets can read identity through the `identity` NAP domain
- [ ] **NAP-04**: Napplets can persist data through the `storage` NAP domain, scoped per napplet build
- [ ] **NAP-05**: Napplets can read and publish Nostr events through the `outbox` and `relay` NAP domains
- [ ] **NAP-06**: Napplets reach the mesh through `napplet.neighbours`, on the same mesh plane as the nsite API
- [ ] **NAP-07**: `neighbours` is written up as a candidate NAP domain against the registry's conventions and proposed upstream
- [ ] **NAP-08**: Napplets ship over the existing manifest and Blossom pipeline under their own event kind, and appear in the same Library as nsites with a type badge

---

## Out of Scope

- **Desktop and OpenWrt targets** — Android-first this cycle; the core stays mobile-shaped
- **Interop with v0.4 devices carrying TTL in the event body** — clean break, small user base, a shim costs more than it saves
- **`jodobear/uzel` as an implementation dependency** — it forks pablof7z's unrelated nampplets/NMP project, is Linux/Tauri-only, and uses a domain vocabulary that diverges from the ratified NAP registry
- **An embedded JS or WASM engine for napplets** — the spec runs napplets as plain JS in the host's own engine; Android WebView already is that engine
- **NAP domains beyond identity, storage, outbox, and relay in napplet v1** — domain presence is feature-detected, so a subset is conformant; the rest follow demand
- **Interactive force-directed mesh topology graph** — the reason-code list answers the actual field complaint; a graph is demo candy
- **Manual peer entry by address** — regresses QR-based mutual pairing and reintroduces spoofing risk
- **Overfitting fips to Myco** — fips changes stay minimal and upstreamable, or they don't land
- **Parallel-team phase structure** — solo developer; phases execute sequentially

---

## Traceability

v1 (Milestone A) only. All 24 v1 requirements map to exactly one phase — no orphans, no
duplicates. Phases 1-3 are inside the 2026-08-05 release deadline; Phases 4-5 land after it.

| Requirement | Phase | Status |
|-------------|-------|--------|
| PEER-01 | Phase 2 | Pending |
| PEER-02 | Phase 2 | Pending |
| PEER-03 | Phase 2 | Pending |
| PEER-04 | Phase 2 | Pending |
| PEER-05 | Phase 2 | Pending |
| PEER-06 | Phase 2 | Pending |
| DIAG-01 | Phase 1 | Complete |
| DIAG-02 | Phase 2 | Pending |
| DIAG-03 | Phase 1 | Complete |
| DIAG-04 | Phase 1 | Complete |
| DIAG-05 | Phase 1 | Complete |
| DIAG-06 | Phase 1 | Pending |
| DIAG-07 | Phase 1 | Pending |
| FIPS-01 | Phase 4 | Pending |
| FIPS-02 | Phase 4 | Pending |
| FIPS-03 | Phase 4 | Pending |
| FIPS-04 | Phase 4 | Pending |
| CORE-01 | Phase 5 | Pending |
| CORE-02 | Phase 5 | Pending |
| CORE-03 | Phase 3 | Pending |
| CORE-04 | Phase 2 | Pending |
| CORE-05 | Phase 5 | Pending |
| UX-01 | Phase 3 | Pending |
| UX-02 | Phase 3 | Pending |

**Coverage:** 24/24 v1 requirements mapped.

**Notes on non-obvious placements:**

- **DIAG-02** (reason codes) sits in Phase 2, not Phase 1, and lands last within it — a
  plain-language reason on top of flaky reconnect logic is noise, not diagnosis.

- **CORE-04** (durable pair requests) sits in Phase 2 with PEER-06 — both are the same
  fire-and-forget-delivery defect, and both are needed for "connected" to mean "working".

- **CORE-03** (corrupt Circle/Library) sits in Phase 3 with the field-reported UX fixes,
  not with the Phase 5 code-health work — silently emptying a Circle destroys pairings, so
  it belongs in the release cut.

v2 requirements (MESH, BACK, WEB, NAP) are not mapped here. They become Milestone B's own
roadmap via `/gsd-new-milestone` once v1 ships.
