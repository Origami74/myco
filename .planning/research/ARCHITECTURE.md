# Architecture Research

**Domain:** Android P2P mesh app runtime — mesh protocol extension, pluggable backends, browser mesh API, napplet host
**Researched:** 2026-08-04
**Confidence:** HIGH on structure/boundaries (derived directly from existing seams and two directly-fetched prior-art precedents); MEDIUM on exact wire syntax for `MESH_EVENT` (a design choice, not yet ratified) and on the capability-URL token scheme (novel, not externally sourced)

## Standard Architecture

### System Overview

This extends the existing six-layer stack (`.planning/codebase/ARCHITECTURE.md`) at three points — the mesh-facing socket, the seam implementations behind `RelayBackend`/`BlobStore`, and a new sibling to Layer 3's gateway — without altering the JNI/Redux contract shape.

```
┌───────────────────────────────────────────────────────────────────────┐
│ Layer 1/2: Android UI + per-nsite WebView (existing)                  │
│  + NsiteActivity WebView gains a WebMessageListener("myco")           │
│    → window.myco.neighbours() / window.myco.meshRelayUrl              │
│  + NEW: NappletActivity — shell WebView hosting a sandboxed iframe    │
│    → shell has its own WebMessageListener("napplet"); iframe has NONE │
├───────────────────────────────────────────────────────────────────────┤
│ Layer 3: Local Gateway (existing) ── sibling: Napplet Shell Assembler │
│  nsite-deck::gateway          │  NEW napplet_host (in nsite-deck)     │
│  serves file bytes as page    │  assembles shell HTML, inlines        │
│  directly to top WebView      │  verified napplet bytes as             │
│                                │  <iframe sandbox="allow-scripts"      │
│                                │   srcdoc="...">  (no network fetch)   │
│  + NEW: mints per-session capability WS URL (mesh relay) at serve time│
├───────────────────────────────────────────────────────────────────────┤
│ Layer 4: RelayBackend / BlobStore seam (existing trait, new impls)    │
│  Embedded (existing) │ NEW: ExternalRelay (tokio-tungstenite → Citrine)│
│  Embedded (existing) │ NEW: ExternalBlob  (reqwest → BUD-01 endpoint) │
│  Selected per Settings mode: Embedded | External | Both               │
│  → speaks ONLY plain NIP-01 EVENT/REQ/CLOSE — never sees TTL          │
├───────────────────────────────────────────────────────────────────────┤
│ Layer 5: myco-core — MeshGossiper / PeerRelayPool (existing, extended)│
│  Mesh-facing socket only (ws://<npub>.fips:4870 peer↔peer) speaks a   │
│  second, sibling wire verb: ["MESH_EVENT", <event>, <ttl>]            │
│  MeshGossiper decrements ttl, absorbs at 0, writes plain EVENT into   │
│  whichever RelayBackend is configured (Layer 4) — TTL never persists │
├───────────────────────────────────────────────────────────────────────┤
│ Layer 6: FIPS Core + Transports (existing, unmodified)                │
└───────────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Where it lives |
|-----------|----------------|-----------------|
| `MeshGossiper` (existing, extended) | Speaks `MESH_EVENT` dialect on peer sockets; decrements/absorbs TTL; writes clean `EVENT` into `RelayBackend`; re-wraps for further fan-out | `myco-core/src/gossip.rs` |
| `PeerRelayPool` (existing, extended) | Frames outbound `MESH_EVENT` instead of `EVENT` on the mesh-facing leg only | `myco-core/src/peer_relay.rs` |
| `ExternalRelayBackend` (new) | `RelayBackend` impl: pure NIP-01 client (`tokio-tungstenite`) against a configured external relay (Citrine, default `127.0.0.1:4869`, or remote) | `myco-core` (new module beside `content.rs`) |
| `ExternalBlobStore` (new) | `BlobStore` impl: BUD-01 HTTP client (`reqwest`) against a configured external Blossom endpoint | `myco-core` (new module) |
| Backend mode switch (new) | Settings-driven selection of Embedded / External / Both per backend type; "Both" is itself a `RelayBackend`/`BlobStore` impl that fans writes and reads-with-fallback across two inner impls | `myco-core::content` (composition point) |
| Mesh capability-URL minter (new) | On serving an nsite's `index.html`, mints a short-lived origin-bound token, injects `window.myco.meshRelayUrl = "ws://127.0.0.1:4870/mesh?tok=…"` | `nsite-deck::gateway` |
| Mesh WS upgrade authorizer (new) | Validates the token/Origin pair on WS upgrade before proxying frames to the real relay; rejects mismatched origin | `nsite-deck::gateway` (co-located with existing Host-header routing) |
| `window.myco` / `napplet.neighbours` bridge (new) | Shared native handler (`NativeAppAction::MeshNeighbours` or similar) exposed to both surfaces via `WebViewCompat.addWebMessageListener` | `myco-core` reducer + two thin JS shims |
| Napplet shell assembler (new) | Resolves kind `5129`/`15129`/`35129` manifest via the *same* `nsite-deck` manifest/blob resolution path; generates shell HTML with `<iframe sandbox srcdoc>` inlining verified bytes | `nsite-deck` (new `napplet_host` module, sibling to `gateway.rs`) |

## Recommended Structure (additions only)

```
nsite-deck/src/
├── gateway.rs              # existing — nsite HTTP serve, Host-header routing
├── napplet_host.rs         # NEW — shell assembly + srcdoc inlining, reuses gateway's
│                           #   manifest/blob resolution fns, forks only at serve step
├── seams.rs                # existing traits — UNCHANGED signatures
myco-core/src/
├── gossip.rs                # existing — extend with MESH_EVENT framing + ttl decrement
├── peer_relay.rs             # existing — extend outbound framing on mesh leg only
├── content.rs                 # existing — add backend-mode composition (Embedded/External/Both)
├── external_relay.rs        # NEW — RelayBackend impl, tokio-tungstenite, pure NIP-01
├── external_blob.rs         # NEW — BlobStore impl, reqwest, BUD-01
├── mesh_bridge.rs             # NEW — shared MeshNeighbours native handler, called from
│                              #   both the nsite WebMessageListener and the napplet shell bridge
```

### Structure Rationale

- `napplet_host.rs` sits beside `gateway.rs`, not inside it — same crate, same manifest/blob machinery, different serve-time output (page bytes vs shell+srcdoc). This is a fork at the last mile, not a parallel sync engine.
- New `RelayBackend`/`BlobStore` implementations live in `myco-core`, not `nsite-deck` — `nsite-deck::seams` defines the trait; concrete backends are an application-layer wiring concern, matching where `IpPeerSource` and `MeshGossiper`'s `FanoutSink` impl already live today.
- `mesh_bridge.rs` is deliberately one module shared by two callers (nsite top-level WebView bridge, napplet shell bridge) — this is the direct answer to "how does `napplet.neighbours` route to the same plane without duplicating it."

## Architectural Patterns

### Pattern 1: Sibling wire verb on the mesh-facing socket only

**What:** `MESH_EVENT` rides the *same* WebSocket as standard NIP-01 frames, but only on the peer↔peer mesh leg (`PeerRelayPool` dialing `ws://<npub>.fips:4870`). The backend-facing leg (embedded relay's own storage path, or the client connection to external Citrine) never sees it.

**Precedent:** This is exactly the shape of NIP-77 (Negentropy syncing) as shipped by `strfry` and adopted into the Nostr ecosystem — `NEG-OPEN`/`NEG-MSG`/`NEG-ERR` are non-standard top-level array verbs riding the same socket as `EVENT`/`REQ`, and NIP-01 explicitly requires relays/clients to tolerate unrecognized message types rather than erroring. `MESH_EVENT` is architecturally identical: a peer-to-peer-only sibling verb, never sent toward a backend that only speaks plain NIP-01.

**When to use:** Any time hop/propagation metadata must not leak into the persisted event or reach a third-party backend. This is the mechanism that makes "backend only ever sees clean EVENTs" true, not just a policy.

**Where TTL decrement belongs:** In the relaying node's gossip layer (`MeshGossiper`), analogous to Meshtastic's `hop_limit` field — a byte in the *packet header*, decremented by every relaying radio, structurally separate from payload, and never written to any application-level store. BitChat's BLE mesh header follows the same pattern (TTL byte, decremented per hop, independent of the encrypted payload). Neither Scuttlebutt (log-replication, not flood — no TTL concept applies) nor libp2p gossipsub (uses peer-scoring + seen-message dedup instead of a decrementing hop counter, because gossipsub already bounds fan-out via mesh degree) are close analogues for Myco's small, hop-bounded, no-scoring mesh — Meshtastic/BitChat's simple decrementing-counter-in-the-header model is the right fit here, not gossipsub's scoring model.

**Trade-off:** Requires a dedup/seen-check independent of TTL (already implicitly available: `RelayBackend` query for existing event id before re-fanning) to prevent redundant re-broadcast storms when TTL alone hasn't hit zero but the event has already looped back.

**Example (conceptual wire frame, mesh leg only):**
```json
["MESH_EVENT", { "id": "...", "kind": 1, ... }, 3]
```
On receipt: write `["EVENT", subId, event]`-shaped record into whichever `RelayBackend` is configured (no ttl field persisted); if `ttl > 0`, re-send `["MESH_EVENT", event, ttl - 1]` to other connected peers via `FanoutSink`; if `ttl == 0`, absorb.

### Pattern 2: Dumb, pluggable backend behind an unchanged seam

**What:** `RelayBackend` and `BlobStore` (existing traits, `nsite-deck::seams`) gain second implementations that are thin protocol clients (`tokio-tungstenite` NIP-01 client; `reqwest` BUD-01 client) against an external process. No new trait, no mesh-awareness inside these implementations.

**When to use:** Whenever mesh-specific behavior (TTL, Circle gating, fan-out policy) is tempted to leak into a backend implementation — it shouldn't. Pattern 1 already keeps TTL out of the wire body; this pattern keeps it out of the storage layer too, by construction: `ExternalRelayBackend`/`ExternalBlobStore` are *incapable* of understanding mesh dialect, because they only ever originate and receive plain NIP-01/BUD-01 traffic.

**Where mesh-specific behavior lives instead:** `MeshGossiper`, `PeerRelayPool`, and the existing `CircleGate` — all *above* the `RelayBackend`/`BlobStore` seam, calling into it, never inside it. This is unchanged from today's embedded-only world; adding external backends doesn't move this boundary, it just proves the boundary was drawn in the right place originally.

**Composition for "Both" mode:** Implement `DualRelayBackend`/`DualBlobStore` as *additional* implementations of the same traits (fan writes to both inner backends, read from embedded first with external fallback) rather than special-casing "both" in every caller. Consumers (`gateway`, `gossiper`, sync engine) stay unaware of which mode is active.

**Trade-off:** External Citrine defaults to port `4869` (one below Myco's embedded `4870`); the port must be a Settings value, never hardcoded, since both embedded and external may need to coexist during a transition or in "Both" mode.

### Pattern 3: Capability URL, not a well-known endpoint, for browser-facing mesh access

**What:** The mesh relay WebSocket that nsite JS dials is *not* a fixed, guessable `ws://127.0.0.1:4870`. The gateway (Layer 3), at the moment it serves an nsite's `index.html`, mints a short-lived, origin-bound token and injects `window.myco.meshRelayUrl` carrying it. The WS upgrade handler (co-located with the gateway's existing Host-header routing) validates the token against the requesting Origin before proxying frames through to whichever `RelayBackend` is active.

**Why this is the authorization boundary, not the relay itself:** All `.nsite` hostnames resolve to the same loopback process; a bare `ws://127.0.0.1:4870` would be dialable identically by *every* nsite's JS with no way to tell them apart — WebSocket has no CORS-equivalent connect-time restriction the way `fetch` does. The gateway is the only component that already parses per-request Host/Origin (it does this for HTTP today), so it — not `myco-relay` itself — is where per-origin scoping must be enforced. This directly prevents one nsite's JS from opening the same raw URL and impersonating or eavesdropping on another's mesh traffic.

**Why publish/subscribe stays explicit:** Because the URL is a capability an nsite must actively read from `window.myco.meshRelayUrl` and pass to `nostr-tools`/applesauce itself — it is never injected as a default relay in any pool, never monkey-patches `window.WebSocket`, and never auto-configures a signer. An nsite that never reads `window.myco` never touches the mesh, by construction.

**Example:**
```js
// nsite JS, explicit opt-in — never automatic
import { SimplePool } from 'nostr-tools'
const pool = new SimplePool()
pool.ensureRelay(window.myco.meshRelayUrl)   // capability URL, not a well-known default
```

### Pattern 4: Shared native bridge for `window.myco.neighbours` and `napplet.neighbours`

**What:** One `NativeAppAction` variant (mesh-neighbours query against FIPS's existing lock-free `ControlReadHandle`, filtered to Circle-visible peers) is exposed through `androidx.webkit.WebViewCompat.addWebMessageListener` on *two* independent surfaces: the nsite's own top-level WebView (`window.myco.neighbours`) and the napplet shell's top-level WebView (which itself relays a `postMessage` from the sandboxed iframe's `napplet.neighbours.*` NAP envelope). Both adapters call the same Rust handler; only the request/response envelope shape differs (bare promise vs NIP-5D `{type, ...}` JSON).

**When to use:** Any capability that both nsites and napplets need identically. Prevents the mesh-neighbours logic (and any future shared capability) from existing twice with two chances to drift.

**Origin-scoping detail (napplet-specific):** The shell's `addWebMessageListener` is scoped via `allowedOriginRules` to the shell's *own* origin only. The nested napplet `iframe`'s `srcdoc` content has an opaque origin by web-platform definition — it structurally cannot match any `allowedOriginRules` pattern, so the bridge is unreachable from inside the sandbox even if the napplet's JS is fully compromised. The iframe can only reach the bridge indirectly, via `postMessage` to the shell's own JS, which is the sole holder of native access. This is the concrete resolution to "WebView has no direct iframe-sandbox parity": the DOM-level `sandbox="allow-scripts"` attribute *is* honored by Chromium-in-WebView (it's a rendering-engine primitive, not an Android-specific one) — what WebView lacks is OS-level per-iframe process isolation, which is compensated for by origin-scoping the bridge so a compromised iframe has no reachable native surface regardless of process boundaries.

## Data Flow

### Mesh TTL hop flow (Addition 1)

```
Device A publishes event locally
    ↓
MeshGossiper reads from local RelayBackend, wraps: ["MESH_EVENT", event, N]
    ↓ (mesh-facing socket only, ws://<npub_B>.fips:4870)
Device B's PeerRelayPool receives MESH_EVENT
    ↓
MeshGossiper: dedup-check event id against local RelayBackend
    ↓ (new) write plain ["EVENT", event] into configured RelayBackend (embedded or external)
    ↓ (N > 0) re-wrap ["MESH_EVENT", event, N-1] → fan to Device B's other connected peers
    ↓ (N == 0) absorb, no further fan-out
```
Backend (embedded store or external Citrine) sees only the plain `EVENT` write — never the verb, never the ttl.

### Browser-facing mesh flow (Addition 3)

```
Gateway serves <host>.nsite/index.html
    ↓ mints per-session token, injects window.myco.meshRelayUrl
nsite JS (nostr-tools/applesauce) dials window.myco.meshRelayUrl explicitly
    ↓ WS upgrade hits gateway's authorizer (checks token + Origin)
    ↓ authorized → frames proxied to active RelayBackend (embedded or external, per settings)
Standard NIP-01 EVENT/REQ/CLOSE only — no MESH_EVENT dialect ever reaches the browser
```

### Napplet render flow (Addition 4)

```
Library lists a napplet (kind 5129/15129/35129) alongside nsites — same sync/Discover/Circle path
    ↓ user opens it
napplet_host resolves manifest + blobs via nsite-deck's existing resolver (one kind number over)
    ↓ verify signature, verify NIP-5A aggregate hash — reject on mismatch (existing pattern)
    ↓ assemble shell HTML: <iframe sandbox="allow-scripts" srcdoc="<verified bytes>">
NappletActivity WebView loads shell (top-level, native bridge attached here only)
    ↓ iframe executes napplet JS, no network access, no src=, everything pre-inlined
    ↓ napplet.* NAP-domain calls → postMessage → shell's own JS → WebMessageListener → mesh_bridge.rs
```

## Scaling Considerations

Not applicable in the traditional sense — this is a single-device, small-mesh (same-room peer count) system, not a multi-tenant service. The relevant "scale" axis is mesh size and event volume, not user count:

| Scale | Consideration |
|-------|----------------|
| Few peers, low event rate (typical room) | TTL decrement + dedup-by-id is sufficient; no need for gossipsub-style scoring |
| Many peers or bursty event volume | Dedup-check cost (`RelayBackend` existence query per inbound `MESH_EVENT`) becomes the bottleneck before TTL logic does — same existing concern noted in `.planning/codebase/CONCERNS.md` re: unbounded concurrent sync tasks swamping a BLE link applies equally to mesh fan-out |

## Anti-Patterns

### Anti-Pattern 1: Letting TTL leak into the backend leg

**What people would do:** Have `ExternalRelayBackend` or the embedded store accept/persist a `ttl`/`hop` tag on the event itself, "just to keep things simple."
**Why it's wrong:** Reintroduces exactly the coupling Addition 1 exists to remove — an unmodified Citrine (or any third-party relay) would then need to tolerate or strip a Myco-specific tag, and the "clean break, no v0.4 interop" constraint becomes leaky in the other direction (new format now also touches bodies).
**Do this instead:** TTL exists only in the `MESH_EVENT` wire envelope on the peer-to-peer leg (Pattern 1); every backend write is a plain `EVENT`.

### Anti-Pattern 2: A single relay endpoint serving every nsite with no origin check

**What people would do:** Expose the embedded relay's existing `:4870` WebSocket directly to nsite JS, assuming Circle gating already covers "who can talk to the mesh."
**Why it's wrong:** Circle gating governs *remote peer* access to this device's relay/Blossom; it says nothing about which *local nsite origin* is allowed to publish/subscribe through it. Every nsite resolves through the same loopback process, so a bare shared URL lets any nsite impersonate or eavesdrop on any other.
**Do this instead:** Pattern 3 — capability URL minted per-origin at serve time, validated at WS upgrade.

### Anti-Pattern 3: A second JS/WASM engine for napplets

**What people would do:** Reach for `wasmtime`/`deno_core`/`boa` to "sandbox" napplet code natively in Rust.
**Why it's wrong:** Napplets are a browser-iframe-sandbox protocol (`srcdoc`, `sandbox="allow-scripts"`, `MessageEvent.source` identity) by specification (STACK.md, confirmed against `RUNTIME-SPEC.md` and NIP-5D directly). A second engine duplicates WebView's own JS engine, runs *outside* the trust model the spec assumes, and adds attack surface rather than removing it.
**Do this instead:** Pattern 4 — inline verified bytes into a real sandboxed iframe inside the existing WebView; origin-scope the native bridge so the iframe can never reach it directly.

## Integration Points

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| `PeerRelayPool`/`MeshGossiper` ↔ `RelayBackend` (embedded or external) | Plain NIP-01 `EVENT` writes only | TTL/hop metadata never crosses this boundary (Pattern 1) |
| `nsite-deck::gateway` ↔ mesh relay backend | WS proxy, token-authorized per origin | New authorization hop; existing Host-header routing logic is reused, not replaced |
| Nsite top-level WebView ↔ `myco-core` reducer | `WebViewCompat.addWebMessageListener` → `NativeAppAction::MeshNeighbours` | Reuses existing JNI `dispatch`/`state` contract; WebMessageListener is a new *transport* into the same reducer, not a new mechanism |
| Napplet shell WebView ↔ `myco-core` reducer | Same `addWebMessageListener` → same `MeshNeighbours` action, different envelope | Iframe has zero direct bridge access (Pattern 4) |
| `nsite-deck::gateway` (nsite serve) ↔ `nsite-deck::napplet_host` (napplet serve) | Shared manifest/blob resolution functions, forked only at final serve step | Kind number is the sole branch condition |

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| Citrine (external relay) | `ExternalRelayBackend` speaks pure NIP-01 over `tokio-tungstenite`, default port `4869` (Settings-configurable) | Already NIP-77-capable server-side (confirmed via `NegentropyHandler.kt`), so existing negentropy sync code needs no changes when pointed at it |
| BUD-01 Blossom endpoint (any conformant server) | `ExternalBlobStore` speaks BUD-01 over `reqwest` | No Android-native reference server found; treat as "any BUD-01 HTTP endpoint," local or remote |

## Build Order & Dependencies

**Strict ordering: Addition 1 → Addition 2.** Both touch only `PeerRelayPool`/`MeshGossiper` (Addition 1) and the `RelayBackend`/`BlobStore` seam (Addition 2), with no code dependency between them — but Addition 2's entire justification ("an *unmodified* third-party relay never sees mesh internals") is only actually true once Addition 1 has moved TTL off the wire body. Building 2 before 1 would mean the first "unmodified Citrine" integration still leaks TTL into event bodies, then has to be redone. Build 1 first.

**Additions {1, 2} vs {3, 4}: no hard code dependency, but sequence 1,2 before 3,4 anyway.** Neither the nsite mesh API (3) nor the napplet host (4) touches `MESH_EVENT` framing or backend selection directly — they consume the mesh plane as an ordinary Nostr relay from the browser/iframe side. But both expose a *public capability surface* (`window.myco.meshRelayUrl`, `napplet.neighbours`) on top of a mesh plane that 1 and 2 are actively reshaping. Stabilize the plane before exposing it externally.

**Addition 3 before Addition 4.** Both need the shared native bridge (`androidx.webkit.WebViewCompat.addWebMessageListener` → `NativeAppAction::MeshNeighbours`, Pattern 4). Addition 3's case (top-level nsite WebView, no nested iframe) is strictly simpler than Addition 4's (shell WebView + origin-scoped bridge + nested sandboxed iframe + `postMessage` relay). Build and prove the bridge against the simpler surface first, then reuse it — don't design the bridge for the harder case first with nothing running against it.

**Summary for phase slicing:**

| Order | Addition | Depends on | Touches |
|-------|----------|------------|---------|
| 1 | Mesh TTL relocation (`MESH_EVENT`) | Nothing new | `PeerRelayPool`, `MeshGossiper` |
| 2 | Pluggable relay + Blossom backends | Addition 1 (for the "unmodified relay" property to hold; no code dependency) | `RelayBackend`/`BlobStore` new impls, Settings |
| 3 | Browser-facing mesh API for nsites | Additions 1–2 stable (soft); introduces the shared native bridge | `nsite-deck::gateway` (capability URL + WS authorizer), new `mesh_bridge.rs`, `NsiteActivity` WebView |
| 4 | Napplet runtime host | Addition 3's bridge infra (hard, for reuse); Additions 1–2 stable (soft, for `napplet.neighbours`) | New `napplet_host.rs`, new `NappletActivity`, `mesh_bridge.rs` (second caller) |

Additions 1 and 2 can, in principle, run as one phase or two sequential phases for a solo developer (per `PROJECT.md`'s "phases execute sequentially" constraint) — they are small and tightly coupled in purpose even though the code touches disjoint files. Additions 3 and 4 are large enough, and different enough in risk profile (network-facing auth surface vs. sandboxing correctness), to warrant separate phases.

## Sources

- `.planning/codebase/ARCHITECTURE.md`, `.planning/codebase/INTEGRATIONS.md`, `.planning/research/STACK.md`, `.planning/PROJECT.md` — primary inputs, given as fact per task framing.
- NIP-77 (Negentropy syncing) and `strfry`'s `NEG-OPEN`/`NEG-MSG`/`NEG-ERR` verbs — cited as direct prior art for a non-standard sibling verb riding a standard NIP-01 socket. [NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md), [hoytech/strfry](https://github.com/hoytech/strfry), [NIP-77 PR #1494](https://github.com/nostr-protocol/nips/pull/1494)
- Meshtastic `hop_limit` packet-header field (decremented per relaying node, separate from payload) — cited as direct prior art for where TTL decrement belongs. [Meshtastic mesh broadcast algorithm](https://meshtastic.org/docs/overview/mesh-algo/), [Meshtastic overview](https://meshtastic.org/docs/overview/)
- BitChat's BLE-mesh TTL-byte header (same decrement-per-hop pattern) and libp2p gossipsub's scoring-based alternative (contrasted, not adopted) — general knowledge, not independently re-fetched this session; treat as MEDIUM confidence pending a dedicated check if the roadmap wants a citation-grade source.

---
*Architecture research for: Android P2P mesh app runtime — mesh protocol, pluggable backends, browser mesh API, napplet host*
*Researched: 2026-08-04*
