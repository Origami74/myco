# Project Research Summary

**Project:** fips-pop / Myco — Android P2P mesh app runtime
**Domain:** Android peer-to-peer mesh networking (BLE L2CAP + Wi-Fi Aware) with a Nostr-based nsite/napplet app runtime
**Researched:** 2026-08-04
**Confidence:** MEDIUM-HIGH

## Executive Summary

Myco is a mesh-networked Android app that already ships nsites (Nostr-hosted mini-sites resolved over a relay/Blossom pipeline) and is now extending in two directions: **Milestone A** hardens the BLE/Wi-Fi Aware peering layer that field testing shows is failing asymmetrically (some phones see everyone, most see nobody), and **Milestone B** adds a mesh-protocol extension (TTL moved out of event bodies into a relay-to-relay wire verb), pluggable external relay/Blossom backends, a browser-facing mesh API for nsites, and a napplet host implementing NIP-5D. Milestone A has a demo today and a release tomorrow; Milestone B follows once A ships.

Expert precedent is decisive on both fronts. For peering, the reported symptom matches known BLE failure modes exactly: a non-deterministic central/peripheral tiebreaker race, advertise/scan duty-cycle asymmetry, Android 12+ background restrictions, and Wi-Fi Aware MAC rotation breaking peer identity when sessions are keyed by MAC/IP instead of stable pubkey. None of these are chipset connection-limit issues. For the runtime work, the authoritative source is napplet.run/docs and the NIP-5D spec text directly — napplets are a browser-iframe-sandbox protocol (`srcdoc` + `sandbox="allow-scripts"`, `MessageEvent.source` identity), not a JS/WASM-embedding problem, so Android's existing WebView is the correct execution engine and no new Rust JS/WASM crate is needed. `jodobear/uzel` is a different, unrelated project (a fork of pablof7z's "nampplets," not napplet.run) and is used only as a loose architecture-pattern reference, never as an API/dependency source.

The v1 napplet surface is settled as "core + Nostr reach": NIP-5D envelope, sandbox, kind-35129 manifest resolve/verify, plus the `identity`, `storage`, and `relay`/`outbox` NAP domains so napplets can read and publish Nostr through the host — other NAP domains are deferred. `napplet.neighbours` is a Myco addition that will be proposed as a candidate NAP domain upstream, designed against the NAP registry's own conventions rather than shipped as a Myco-private API, mirroring the project's "fips changes stay upstreamable" constraint. The key risk across both milestones is building on unverified assumptions: for Milestone A, the tiebreaker-race and duty-cycle-asymmetry theories are inferred from code, not confirmed by device logs, so diagnostics must be instrumented before fixes — turning guesses into observable facts (role decisions, discovery latency, send-failure counts). For the fips rebase (19 commits against 232 commits of upstream drift), the mitigation is a strict theme-by-theme rebase with a per-theme timebox, dropping any theme that fights the refactor to a fallback branch rather than risking the whole cutoff. For Milestone B, the mesh TTL relocation is a deliberate clean break with no v0.4 back-compat, and must fail loudly on version mismatch.

## Key Findings

### Recommended Stack

No new Rust crates are required for the napplet runtime itself: manifest verification reuses `nsite-deck`'s existing sha2/nostr/serde_json path (napplet manifests are NIP-5A, one kind number over from nsites), and transport reuses the existing `axum` (mesh-facing) / `tokio-tungstenite` (backend-facing) split. The one genuinely new dependency is Android-side: `androidx.webkit:webkit` 1.16.0 for `WebViewCompat.addWebMessageListener`, replacing the reflection-based `addJavascriptInterface` footgun with an origin-scoped shell↔native bridge. `nostr` bumps from 0.44.3 to 0.44.7 (same minor line, stay off the `0.45.0-alpha.*` line). `negentropy` 0.5.0 is already correctly resolved and pairs cleanly with Citrine (the target external relay), which already implements NIP-77 server-side.

**Core technologies:**
- `nostr` 0.44.7 — event parsing/verification for napplet manifests (kinds 5129/15129/35129) — same-minor patch bump, zero API churn
- `negentropy` 0.5.0 (already present) — set-reconciliation sync, unmodified against Citrine's NIP-77 handler
- Android `WebView` + `iframe`/`srcdoc` (existing) — the napplet execution engine; no new JS/WASM crate needed
- `androidx.webkit:webkit` 1.16.0 — origin-scoped shell↔native bridge (`addWebMessageListener`), replacing `addJavascriptInterface`
- Hand-rolled `axum` WebSocket relay (existing `myco-relay`) — host for the new `MESH_EVENT` verb; do not migrate to `nostr-relay-builder` (no extension point for non-NIP-01 verbs)
- `tokio-tungstenite` / `reqwest` (existing) — pure NIP-01 / BUD-01 clients for external Citrine relay and Blossom backends

**Explicitly rejected:** `wasmtime`/`deno_core`/`boa`/`quickjs-rs` (duplicates WebView's own engine, steps outside its trust model), and `jodobear/uzel`/`nampplets` as a Cargo dependency (pre-alpha, Linux/Tauri-only, divergent domain vocabulary from the ratified NAP registry).

### Expected Features

**Milestone A — must have (table stakes):**
- Per-peer connection-state list (connected / relayed-reachable / offline) — FFI fields (`PairedPeer.reachable`, `BlePeer.connected`) already exist; this is a UI-surfacing job
- Pending-pairing indicator (`PairedPeer.pairing`)
- Wi-Fi Aware default-on + radio/scanning status visible
- Last-seen timestamp per peer
- Transport indicator (which radio reached this peer)

**Milestone A — differentiator:**
- Per-peer "why isn't this connected" reason code (`no_shared_transport` / `handshake_pending` / `handshake_failed` / `out_of_range` / `circle_not_paired`) — no surveyed comparable app (Briar, bitchat, Meshtastic, Berty, Manyverse) does this well; this is the direct diagnostic answer to the field-reported tiebreaker bug

**Milestone A — defer:** RSSI/signal bars, mesh topology graph sheet, share-logs-via-NIP-17-DM — v1.x, triggered by field feedback volume.

**Milestone B — v1 surface (settled): "core + Nostr reach"**
- NIP-5D-conformant sandbox (WebView iframe `srcdoc`, `sandbox="allow-scripts"`, no `allow-same-origin`, no injected `window.nostr`)
- JSON envelope wire format matching `@napplet/sdk`/`@napplet/shim` exactly
- Kind-35129 manifest ingestion via the existing nsite manifest+Blossom pipeline
- `identity` (read-only), `storage` (scoped/quota'd), `relay`/`outbox` (query/subscribe/publish via existing relay pool) NAP domains — enough for napplets to actually read and publish Nostr through the host
- `napplet.neighbours` — Myco's own mesh pub/sub addition, to be proposed upstream as a candidate NAP domain designed against the NAP registry's conventions (a design deliverable, not just implementation)
- Settings toggle for embedded vs. external (Citrine) relay/Blossom backend

**Milestone B — defer to v2+:** `inc`, `config`, `intent`, `ble`, `webrtc`, `serial`, `media`, and the remaining NAP domains — add per actual napplet demand, not speculatively.

**Anti-features to actively avoid:** full interactive topology viz, silent auto-retry with no visible state change, manual peer entry as a discovery fallback, implicit/ambient mesh publish on `outbox.publish` (must stay explicit, opt-in), granting `allow-same-origin` "just for one napplet," injecting `window.nostr` into the sandbox.

### Architecture Approach

The mesh/napplet work extends the existing six-layer stack at three points without changing the JNI/Redux contract shape: a sibling wire verb (`MESH_EVENT`) on the mesh-facing socket only (never touching the backend leg), second implementations of the existing `RelayBackend`/`BlobStore` traits for external Citrine/Blossom, and a new `napplet_host` sibling to the existing nsite gateway. TTL decrement lives in `MeshGossiper`, structurally separate from event bodies — precedented by NIP-77's `NEG-OPEN`/`NEG-MSG` sibling verbs and Meshtastic's `hop_limit` packet-header field, not by libp2p gossipsub's scoring model. Browser-facing mesh access uses a capability URL (short-lived, origin-bound token minted per nsite at serve time), not a fixed well-known WebSocket endpoint — this is the actual authorization boundary, since Circle-gating governs remote-peer access but says nothing about which local nsite origin may use the mesh. One shared native bridge module (`mesh_bridge.rs`) serves both `window.myco.neighbours` (nsite top-level WebView) and `napplet.neighbours` (napplet shell WebView relaying from a sandboxed iframe with zero direct bridge access) — one capability, two conformant callers, no drift, and the concrete design basis for the eventual upstream NAP-domain proposal.

**Major components:**
1. `MeshGossiper`/`PeerRelayPool` (existing, extended) — speaks `MESH_EVENT` on the peer-to-peer leg only; decrements/absorbs TTL; writes clean `EVENT` into the configured backend
2. `ExternalRelayBackend`/`ExternalBlobStore` (new) — pure NIP-01/BUD-01 clients against Citrine/any BUD-01 endpoint, incapable of understanding mesh dialect by construction
3. Mesh capability-URL minter + WS upgrade authorizer (new, in `nsite-deck::gateway`) — per-origin token validation for browser-facing mesh access
4. `napplet_host.rs` (new, sibling to `gateway.rs`) — resolves/verifies napplet manifests via the same NIP-5A machinery, assembles shell HTML with inlined `srcdoc` iframe bytes
5. `mesh_bridge.rs` (new, shared) — single native handler exposed via `WebViewCompat.addWebMessageListener` to both the nsite WebView and the napplet shell WebView

**Build order (load-bearing for phase slicing):**
- **Addition 1 → Addition 2, strict.** Mesh TTL relocation must land before pluggable external backends — otherwise the first "unmodified Citrine" integration still leaks TTL into event bodies and has to be redone.
- **{Addition 1, 2} before {Addition 3, 4}, soft.** No direct code dependency, but both the nsite mesh API (3) and napplet host (4) expose a public capability surface on top of a mesh plane that 1 and 2 are actively reshaping — stabilize the plane before exposing it externally.
- **Addition 3 before Addition 4.** Both need the shared native bridge; Addition 3's surface (top-level WebView, no nested iframe) is strictly simpler than Addition 4's (shell WebView + origin-scoped bridge + nested sandboxed iframe + postMessage relay). Prove the bridge against the simpler case first, then reuse it.

### Critical Pitfalls

1. **BLE connect-role tiebreaker race** — two phones can independently compute different central/peripheral role winners with no platform-level arbitration, producing exactly the "zero-to-few peers connect" symptom. Fix: pure-function tiebreaker over stable pubkeys (never ephemeral MAC), with role-flip retry after N failures.
2. **Advertise/scan duty-cycle asymmetry** — asymmetric discovery times compound in a room-scale mesh. Fix: verify foregrounded high-duty-cycle scan/advertise with correct Android 12+ permissions; measure discovery latency per peer.
3. **Wi-Fi Aware MAC rotation breaks peer identity** — sessions keyed by MAC/IP desync silently on reconnect (this is FIPS#130 directly). Fix: key all peer/Circle-gate state by stable pubkey, force re-resolution on every transport-level reconnect signal.
4. **Fire-and-forget delivery masquerading as "connected"** — transport-connected peers can have every message silently dropped with no user-visible signal. Fix: surface send-failure/drop counts per peer; persist undelivered pair requests and retry on reconnect.
5. **Mechanical rebase of the 19-commit fips branch loses PR-extractability** — a single head-on rebase against 232 commits of upstream refactor risks silently duplicating logic master already covers. Fix: theme-by-theme rebase with a per-theme timebox; drop any theme that fights the refactor to a fallback branch rather than risking the whole cutoff.

Two moderate pitfalls carry into Milestone B: conflating `napplet.run`/NIP-5D with the unrelated `jodobear/uzel`/pablof7z "nampplets" lineage, and a clean-break `MESH_EVENT` wire format that must reject mismatched-version frames outright rather than silently misinterpreting missing TTL as "forward forever."

## Implications for Roadmap

### Phase 1: Peering Diagnostics Instrumentation
**Rationale:** Per PITFALLS, the tiebreaker-race and duty-cycle-asymmetry root causes are currently inferred from code, not confirmed by device logs. Instrumenting first turns guesses into observable facts before any fix is attempted — fixing blind risks solving the wrong problem on a one-day timebox. This is the single most load-bearing sequencing decision in Milestone A.
**Delivers:** Per-peer diagnostics surfacing role decisions, discovery latency, connect-attempt outcomes, and send-failure/drop counts — this is also the Milestone A UI table-stakes/differentiator feature set (connection-state list, pending-pairing indicator, reason codes), so instrumentation and user-facing diagnostics UI are the same deliverable.
**Addresses:** Per-peer connection-state list, pending-pairing indicator, "why not connected" reason code (FEATURES.md P1s)
**Avoids:** Pitfall 5 (fire-and-forget masking); makes Pitfalls 1 and 2 observable rather than assumed

### Phase 2: BLE/Wi-Fi Aware Peering Fixes
**Rationale:** With role decisions and discovery latency now visible, apply the deterministic tiebreaker, duty-cycle correction, and pubkey-keyed session identity fixes, using instrumentation to confirm each fix actually changes observed behavior.
**Delivers:** Deterministic central/peripheral role assignment (pubkey-based, role-flip retry), foregrounded high-duty-cycle scan/advertise config, pubkey-keyed (not MAC/IP-keyed) peer/Circle-gate state surviving reconnect, Wi-Fi Aware default-on
**Avoids:** Pitfalls 1, 2, 3, 4 directly; reproduces and closes FIPS#130 explicitly

### Phase 3: fips Rebase (theme-by-theme)
**Rationale:** A same-day, hard-timeboxed activity gating the release. PITFALLS names the ordering discipline explicitly: rebase theme-by-theme (BLE backend+PSM discovery → peer queue → TUN/DNS seams → transport-preference roaming → UDP fix), re-verifying compilability/tests after each theme, with a per-theme timebox — drop any theme that fights the refactor to the fallback branch rather than risking the whole rebase.
**Delivers:** 19 commits rebased, each remaining an extractable upstream PR
**Avoids:** Pitfall 6 (mechanical rebase losing PR-extractability)

### Phase 4: Mesh TTL Relocation (`MESH_EVENT`)
**Rationale:** Per ARCHITECTURE's strict build order, this must land before pluggable external backends — it's what makes "an unmodified third-party relay never sees mesh internals" actually true.
**Delivers:** `MESH_EVENT` sibling wire verb on the mesh-facing leg only, TTL decrement in `MeshGossiper`, explicit protocol-version field for clean-break rejection of mismatched v0.4/v0.5 frames (no back-compat by design)
**Uses:** Existing `axum`/`tokio-tungstenite` split; NIP-77 sibling-verb precedent
**Avoids:** Pitfall 9 (silent misparse on version mismatch); Anti-Pattern 1 (TTL leaking into backend leg)

### Phase 5: Pluggable External Relay/Blossom Backends
**Rationale:** Soft-depends on Phase 4 (same justification chain — "unmodified relay never sees mesh internals" only holds once TTL is off the wire body).
**Delivers:** `ExternalRelayBackend` (tokio-tungstenite, pure NIP-01, default port 4869 configurable) and `ExternalBlobStore` (reqwest, BUD-01) as new `RelayBackend`/`BlobStore` implementations; Settings-driven Embedded/External/Both mode
**Implements:** Pattern 2 (dumb pluggable backend behind an unchanged seam)
**Avoids:** Anti-Pattern 1 (TTL leak); hardcoding the external port

### Phase 6: Browser-Facing Mesh API for Nsites
**Rationale:** Needs the shared native bridge; simpler surface (top-level WebView, no nested iframe) than the napplet host — build and prove the bridge here first, per ARCHITECTURE's "3 before 4" ordering.
**Delivers:** Capability-URL minting at gateway serve time, WS upgrade origin authorizer, `window.myco.meshRelayUrl`/`window.myco.neighbours` exposed to nsite JS
**Implements:** Pattern 3 (capability URL, not well-known endpoint), Pattern 4 (shared native bridge, first caller)
**Avoids:** Anti-Pattern 2 (single relay endpoint with no origin check)

### Phase 7: Napplet Runtime Host
**Rationale:** Hard-depends on Phase 6's bridge infrastructure (reused, not redesigned); soft-depends on Phases 4-5 for `napplet.neighbours` to have a stable mesh plane to sit on. Highest-risk phase — sandboxing correctness on Android where WebView has no native iframe-sandbox process-isolation parity.
**Delivers:** NIP-5D-conformant sandbox, JSON envelope wire format, kind-35129 manifest ingestion, the settled v1 NAP domain set (`identity`, `storage`, `relay`/`outbox`), and the **`napplet.neighbours` NAP domain design proposal** — drafted against the NAP registry's own conventions for eventual upstream submission
**Uses:** `androidx.webkit:webkit` 1.16.0, existing `nsite-deck` manifest/blob resolution
**Avoids:** Anti-Pattern 3 (second JS/WASM engine); Pitfall 8 (uzel/nampplets conflation); Pitfall 10 (incomplete sandboxing — scope `neighbours` grants per-napplet, not all-or-nothing)

### Phase Ordering Rationale

- Milestone A phases (1-3) are sequenced diagnostics-before-fixes specifically because PITFALLS flags the root causes as inferred, not confirmed — the demo-today/release-tomorrow timeline makes this the highest-leverage ordering, not a nice-to-have.
- The fips rebase (Phase 3) is time-boxed independently within the same one-day window; its internal ordering (theme-by-theme, timeboxed, drop-don't-force) is non-negotiable regardless of where it sits relative to Phases 1-2.
- Milestone B phases (4-7) follow ARCHITECTURE's build-order table exactly: 4→5 strict, {4,5} before {6,7} soft, 6 before 7 — a direct translation of "Additions 1-4" numbering into phase order.
- Clean-break TTL protocol (Phase 4) intentionally excludes any v0.4 compatibility work — the correct scope is "reject mismatched versions loudly," not "interoperate," per the settled clean-break decision.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 7 (Napplet Runtime Host):** NIP-5D/NAP-domain surface is explicitly alpha and "a moving target"; the `napplet.neighbours` upstream proposal requires engagement with the live NAP registry conventions, not just implementation
- **Phase 1 (Diagnostics Instrumentation):** Wi-Fi Aware status-struct shape is new and needs device-log-confirmed root causes before fix design in Phase 2

Phases with standard patterns (skip research-phase):
- **Phase 4 (Mesh TTL Relocation):** direct precedent from NIP-77 sibling verbs and Meshtastic's `hop_limit`, well-documented
- **Phase 5 (Pluggable Backends):** existing `RelayBackend`/`BlobStore` seams, thin protocol clients only, no new trait design
- **Phase 6 (Browser-Facing Mesh API):** capability-URL pattern and existing gateway Host-header routing are directly analogous, low novelty

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | MEDIUM-HIGH | Protocol sources (napplet.run/docs, Kehto's RUNTIME-SPEC.md, Citrine source) fetched directly and HIGH confidence; "standard stack" conclusion is build-it-yourself rather than off-the-shelf |
| Features | HIGH (Milestone B protocol surface) / MEDIUM (Milestone A competitor UX) | NIP-5D/NAP surface and uzel's functional floor inspected directly; comparable-app peer-diagnostics UX is search-summarized for all but bitchat |
| Architecture | HIGH (structure/boundaries) / MEDIUM (exact wire syntax) | Structure derived directly from existing seams; `MESH_EVENT` wire syntax and capability-URL token scheme are design choices, not yet ratified externally |
| Pitfalls | HIGH (Android platform behavior) / MEDIUM (Myco-specific root causes) | BLE/NAN platform docs are official AOSP/Android sources; tiebreaker-race and duty-cycle theories are inferred from code, not yet confirmed by device logs — this is why Phase 1 instrumentation is load-bearing |

**Overall confidence:** MEDIUM-HIGH

### Gaps to Address

- **Myco-specific peering root causes are unconfirmed by device logs.** Phase 1 (diagnostics instrumentation) exists specifically to close this gap before Phase 2 commits to fixes — do not skip straight to fixes on the strength of this research alone.
- **NIP-5D/NAP domain spec is self-described as alpha and a moving target.** Pin the `napplet/naps` registry commit SHA used during Phase 7 implementation; re-check before the `napplet.neighbours` upstream proposal is drafted.
- **No mature Android-native Blossom-server reference exists** for testing external-Blossom backend mode (Phase 5) — plan to point at any BUD-01-conformant endpoint (local or remote) for validation.
- **Exact NIP list supported by Citrine is MEDIUM confidence** (NIP-77 confirmed via source inspection, not NIP-11 introspection) — worth a live handshake check early in Phase 5.

## Sources

### Primary (HIGH confidence)
- https://napplet.run/docs and https://napplet.run/docs/naps/ — NAP domain table, sandbox model, envelope format, fetched directly
- github.com/kehto/web, `RUNTIME-SPEC.md` — reference conformant-host behavior, fetched directly
- github.com/nostr-protocol/nips/pull/2303 (NIP-5D) — canonical spec text, fetched directly
- github.com/greenart7c3/Citrine — relay backend source inspection (NIP-77, NIP-42, NIP-86, default port)
- crates.io API — `nostr`, `nostr-relay-builder`, `negentropy` version data
- developer.android.com / source.android.com — Wi-Fi Aware, MAC randomization, official AOSP docs
- Project-internal: `.planning/PROJECT.md`, `.planning/codebase/CONCERNS.md`, `.planning/codebase/ARCHITECTURE.md`, `fips-pop-portbump/docs/reference/ffi-surface.md`

### Secondary (MEDIUM confidence)
- github.com/jodobear/uzel and github.com/jodobear/nampplets — inspected directly for scope/status, confirmed unrelated pablof7z fork, not napplet.run; architecture-pattern reference only
- github.com/permissionlesstech/bitchat — `MeshPeerList.swift`/`MeshTopologyView.swift` read directly for diagnostics UX pattern
- androidx.webkit / `WebViewCompat.addWebMessageListener` vs `addJavascriptInterface` — Android docs + WebSearch, not Context7-verified
- ST Community / Android support forum threads on BLE connection limits — vendor forum, cross-checked

### Tertiary (LOW confidence)
- BitChat's BLE-mesh TTL-byte header and libp2p gossipsub scoring contrast — general knowledge, not independently re-fetched this session
- Berty/Manyverse/Meshtastic diagnostics UX — search-summarized, not source-inspected

---
*Research completed: 2026-08-04*
*Ready for roadmap: yes*
