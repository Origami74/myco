# Feature Research

**Domain:** Peer-to-peer mesh app runtime for Android (BLE L2CAP / Wi-Fi Aware transports; Nostr nsites + napplets)
**Researched:** 2026-08-04
**Confidence:** HIGH for the napplet.run/NIP-5D protocol surface and the uzel reference implementation (primary sources, fetched and inspected directly). MEDIUM for comparable-app peer-diagnostics UX (a mix of official docs/manuals and direct source inspection for bitchat; search-summarized for Briar/Berty/Manyverse, cross-checked against project field reports).

This research is scoped to two upcoming milestones only, not the full nsite/mesh feature set (which already ships in Myco v0.4).

---

## MILESTONE A — Rock-solid peering (ships 2026-08-05, 3-day window)

### Table Stakes (Users Expect These)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Per-peer connection-state list (connected / reachable-via-relay / offline) | Every comparable app in the category has *some* peer list with state — Meshtastic (online dot + hop badge), Manyverse/SSB Connections screen (yellow=connecting/green=connected dot), bitchat `MeshPeerList` (per-peer glyph: radio=direct, dotted-path=relayed-reachable, globe=Nostr-only, slashed-radio=offline). Absence reads as broken. | LOW | Myco's FFI state already carries `PairedPeer.reachable` and `BlePeer.connected` (`ffi-surface.md`) — this is a UI-surfacing job, not new plumbing. |
| Pending-pairing status visible | Explicit field complaint: FIX-TODOS.md "Show pending status peering request." PROJECT.md Milestone A requirement: "pending peering requests visible in the UI." | LOW | `PairedPeer.pairing: "pending"\|"complete"\|"failed"` already exists in the proposed FFI state — just needs a badge/row treatment. |
| Wi-Fi Aware on by default + radio/scanning state visible | Explicit PROJECT.md requirement ("Wi-Fi Aware defaults on"). Users can't diagnose a dead radio they can't see the state of. | LOW–MED | `BleStatus` (`enabled`, `scanning`, `adapterName`) already models this shape for BLE; Wi-Fi Aware needs an equivalent status struct — new but small. |
| Last-seen / last-heard timestamp per peer | Meshtastic and Manyverse both show recency; it's the cheapest signal a user can use to judge "is this actually stale." | LOW | `PairedPeer.lastSeenText` field already exists in the FFI surface. |
| Transport indicator per peer (which radio reached this peer) | bitchat encodes this as the leading glyph on every row; Meshtastic badges MQTT vs. direct LoRa. Users with multiple transports (BLE + Wi-Fi Aware + exit-node) need to know which one is carrying a given peer. | LOW–MED | Maps directly onto Myco's existing `BlePeer` vs. future Wi-Fi Aware peer set; needs a unified "reached via" field in state. |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Per-peer "why isn't this connected" reason code | This is the actual field-reported bug ("not always connecting with peers, think handshake fails, tiebreaker-related" — FIX-TODOS.md) made diagnosable. **No surveyed app does this well**: Briar's manual documents no per-contact transport/troubleshooting UI at all; Berty has an internal BLE testing guide but no user-facing diagnostic; Meshtastic has Traceroute/Neighbor Info but those describe multi-hop LoRa path, not "why did the handshake not complete." bitchat's topology sheet shows *what* is connected, not *why something isn't*. | MEDIUM | Needs a new failure-reason enum in the FFI contract (`no_shared_transport` / `handshake_pending` / `handshake_failed` / `out_of_range` / `circle_not_paired`) — this is genuinely new state, not just UI. Directly targets the suspected tiebreaker bug. |
| RSSI / signal-strength display | Meshtastic shows SNR/RSSI as signal bars per node; cheap trust-builder ("my phone can actually see this peer"). | LOW | `BlePeer.rssi: Option<i32>` already exists in the FFI surface — nearly free to surface once the state plumbing is confirmed live. |
| Mesh topology / neighbor graph sheet | bitchat ships `MeshTopologyView` — a self-centered circular graph of gossiped `directNeighbors` claims, drawn with `Canvas`, manually refreshable. Strong demo credibility ("show me the mesh"), but built as a "minimal diagnostics sheet," not core UX. | MEDIUM–HIGH | Given the 3-day window, treat as stretch — the field bug (zero/few peers connecting) is better addressed by the reason-code list than a graph. Candidate for v1.x. |
| Share logs to developer via NIP-17 DM | Explicit field request (FIX-TODOS.md: "share logs with developer = nip17 DM"). High leverage during active field testing — turns silent failures into debuggable reports without asking users to plug in a laptop. | MEDIUM | The device's FIPS identity key *is* its Nostr key, so signing a DM to the developer's npub is in-scope (this is not "authoring an nsite on someone's behalf" — it's the device's own identity acting for itself). |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|------------------|-------------|
| Full interactive network topology visualization (force-directed graph, pan/zoom, live physics) | "Looks impressive," demo appeal | 3-day timeline; every comparable app that has one (bitchat) treats it as a minor circular-layout sheet, not a flagship feature — the value is in the reason codes, not the graph | Ship the flat per-peer reason-code list first; consider bitchat's simple circular layout later if time allows |
| Silent auto-retry/reconnect with no visible state change | "Just make it work" | PROJECT.md explicitly requires churn survival "without toggling mesh off/on" — but silent retries with zero visibility recreate the exact "why is nothing happening" confusion this milestone exists to fix | Pair every retry/backoff transition with a state change the diagnostics UI reflects (e.g., `handshake_pending` → `handshake_failed` → next attempt) |
| Manual peer entry (typing MAC/address) as a fallback | "Give me a way around broken discovery" | Regresses Myco's QR-based mutual-pairing model (`identity-pairing.md § 6.1`) and reintroduces spoofing risk the Noise-based handshake exists to prevent | Fix the underlying discovery/handshake reliability instead; surface *why* discovery is failing |
| Custom RF/BLE spectrum analyzer tooling (à la MeshTenna) | "Power users want raw signal data" | Out of category — Myco is a consumer mesh app, not a radio test tool; this is scope creep dressed as diagnostics | Signal bars (coarse RSSI) is the ceiling for v1; anything finer belongs in a separate dev-only build |

---

## MILESTONE B — Mesh protocol + napplet runtime

### The authoritative surface: napplet.run / NIP-5D

napplet.run/docs (VitePress site, fetched directly — see Sources) documents **NIP-5D** ("Nostr Web Applets," canonical text at `github.com/nostr-protocol/nips/pull/2303`, status: **alpha, explicitly a moving target**) plus the **NAPs track** (`github.com/napplet/naps`) that defines each capability domain. The `@napplet/*` packages (`core`, `shim`, `sdk`, `nap`, `vite-plugin`, `cli`, `conformance*`) are the reference SDK; **Kehto** (`github.com/kehto/web`) is the reference shell/runtime.

**Core wire protocol (normative, NIP-5D):**
- Every message: `{ type: "<domain>.<action>", ...payload }` posted over `postMessage`.
- Napplet → shell: `window.parent.postMessage(msg, '*')`. Shell → napplet: `iframeWindow.postMessage(msg, '*')`.
- Request/response pairs correlate on an `id` field (e.g. `outbox.query` → `outbox.query.result`).
- Unrecognized `type` values **MUST be silently ignored** (forward-compat: an old napplet can talk to a newer/smaller shell and degrade).
- Iframe **MUST** use `sandbox="allow-scripts"` with **no** `allow-same-origin` — no real origin, no service worker, no same-origin storage, no direct WebSocket/fetch.
- Shell **MUST NOT** inject `window.nostr` (NIP-07) — all signing/encryption is brokered.
- Identity is assigned at iframe creation: the shell maps the iframe's `Window` reference to a `(dTag, aggregateHash)` tuple read from the napplet's manifest. **No handshake.** `MessageEvent.source` (unforgeable) is the sender-identity check on every inbound message; messages from unmapped windows are **silently dropped**.
- ACL is keyed on `(dTag, aggregateHash)` — a different build of the same napplet type is a distinct subject; any file change re-triggers consent.

**Manifest:** NIP-5D kind **35129** ("named-napplet" event, always parameterized-replaceable — every napplet has a `d` tag identifying its type). Adopts NIP-5A's `path` + aggregate `x` tag schema (SHA-256 per file, one aggregate hash over all of them). Declares required capabilities as `["requires", "<nap-name>"]` tags. `@napplet/vite-plugin` generates this at build time (`nip5aManifest({ nappletType, requires, title, description, configSchema })`); `@napplet/cli` handles signing + Blossom/relay publication.

**The 23 active NAP capability domains** (from `@napplet/nap`'s `NapDomain` union, confirmed against the live NAP domain reference page):

| Domain | Purpose | Key surface |
|---|---|---|
| `relay` | Low-level explicit relay proxy (escape hatch — group relays, diagnostics) | `subscribe(filters, onEvent, onEose, opts)` |
| `outbox` | **Default** event read/publish boundary; shell owns NIP-65 discovery, fallback, dedup, sig validation, signing, fanout | `getEvent`, `query`, `subscribe`, `publish`, `resolveRelays` |
| `storage` | Scoped key-value store, isolated per `dTag:aggregateHash`, ~512 KB quota | `getItem`, `setItem`, `removeItem`, `keys`, `storage.instance.*` |
| `identity` | **Read-only** — never signs/encrypts/decrypts | `getPublicKey`, `getProfile`, `onChanged` |
| `inc` | Inter-napplet communication, topic pub/sub, convention-URI (`napplet:<archetype>/<intent>`) transposition | `emit(topic, payload?)`, `on(topic, cb)` |
| `intent` | Archetype-based intent dispatch/invocation (installed-handler resolution) | `invoke(request)`, `open(archetype, payload?, opts?)`, `available`, `handlers`, `onChanged` |
| `resource` | The **only** network-fetch primitive inside the sandbox — `data:`/`https:`/`blossom:sha256:<hex>`/`nostr:<bech32>` | `info()`, `bytes(url)`, `bytesMany(urls)`, `bytesAsObjectURL` |
| `upload` | Shell-mediated NIP-96 + Blossom upload; shell signs auth, returns NIP-94 metadata | `info()`, `upload({data, filename})` |
| `config` | Declarative, JSON-Schema-driven per-napplet settings; shell renders UI, validates, persists, is sole writer | `get`, `subscribe`, `openSettings`, `registerSchema`, `schema` |
| `theme` | Read-only shell theme tokens | `get()`, `onChanged` |
| `keys` | Keyboard bindings/action registration | `registerAction`, `unregisterAction`, `onAction` |
| `media` | Ownership-aware media sessions | `createSession`, `reportState`, `onCommand` |
| `notify` | Shell-rendered notifications/badges | `send`, `badge`, `onAction` |
| `cvm` | ContextVM (MCP-over-Nostr) bridge; shell owns transport/registry/policy | `discover`, `listTools`, `callTool`, `listResources`, `readResource`, `registry.*` |
| `common` | Public social actions (profile, follows, reactions, reports, NIP-19 helpers); shell owns consent/signing | `follows()`, `react()` |
| `lists` | NIP-51 list mutation; shell owns lookup/merge/encryption/signing/publishing | `supported`, `add`, `remove` |
| `count` | Event counts; shell owns relay COUNT support, aggregation, refusal policy | `query(filters, opts)` |
| `dm` | Shell-mediated encrypted DM helpers | (not detailed on the public page — draft) |
| `ble` | Runtime-mediated BLE/GATT sessions; shell owns chooser UI, device handles, GATT lifecycle, policy | `open`, `services` |
| `webrtc` | Runtime-mediated WebRTC data sessions; shell owns signaling/SDP/ICE | (session-scoped, shell-owned lifecycle) |
| `serial` | Runtime-mediated serial device access; shell owns permissions/raw handles | `open`, `write`, `close`, `onEvent` |
| `fs` | Shell-mediated virtual filesystem; shell owns host paths/mounts/policy, bytes are base64 on the wire | `pickFile(s)`, `pickDirectory`, `pickSaveFile`, `stat`, `list`, `read`, `write`, `mkdir`, `remove`, `move`, `watch`/`unwatch`, `onChanged` |
| `link` | Shell-mediated external navigation (user-visible, not fetching) | `open(url, opts?)` |

**Domain presence, not negotiation:** a napplet feature-gates at runtime with `if (window.napplet?.domain)`; absent domain ⇒ unavailable, must degrade gracefully. This is the mechanism that lets Myco ship a subset of the 23 domains in v1 without breaking conformant napplets.

**What napplet.run explicitly does NOT define:** it delegates every domain-specific message shape to the NAPs track, and states plainly it is alpha with "drift" expected between packages and spec. Treat the live GitHub PRs, not this research, as ground truth at implementation time.

### What uzel (`jodobear/uzel`) functionally implements

uzel is a **Linux-only, Tauri-based, Rust-native napplet runtime proof of concept** (not a product) — the closest existing reference for "a native Rust host implementing NIP-5D," which is exactly Myco's Milestone B shape, but the POC explicitly excludes Android, FIPS, and mobile from its scope (`uzel-poc-validated-pack/README.md`: *"Do not add FIPS, extensions, wallets, media focus, native napplets, Android, Plasma widgets, Lua, or host-WM integration to this POC."*). Its own daemon/engine is called **NMP**; two demo napplets (`follow-list`, `profile-card`) exercise it.

| NAP domain | uzel status | Evidence |
|---|---|---|
| `identity` | **Implemented** (read-only pubkey selection/restore) | STATUS.md: "restoring the key through NMP's parser after restart" |
| `outbox` | **Implemented**, partially — query/subscribe proven at scale (batched author filters, adaptive retry/circuit-breaker); publish path exists but is lightly exercised | STATUS.md: "Initial NAP-OUTBOX batches contain at most eight author-bound filters..." |
| `resource` | **Implemented** — `bytes()` proven; `bytesMany()` whole-operation bounding is an open upstream issue | STATUS.md: "Nampplets issue #9 tracks a whole-operation bound for sequential resource.bytesMany; current proof covers individual resource.bytes." |
| `inc` | **Partially implemented** — `emit`/convention-URI routing (`napplet:profile/open`) works; `inc.channel.opened` explicitly unsupported | STATUS.md "Accepted provisional risks": "inc.channel.opened and NAP-INTENT delivery remain unsupported and unused." |
| `storage` | **Minimally implemented** ("minimal persistence" in slice 04) | README.md slice list |
| `intent` | **Not implemented / unsupported** | STATUS.md, same line as above |
| `relay`, `common`, `lists`, `count`, `dm`, `keys`, `theme`, `media`, `notify`, `config`, `cvm`, `upload`, `ble`, `webrtc`, `serial`, `fs`, `link` | **Not implemented** — out of POC scope entirely | Not referenced anywhere in the pack; scope rule explicitly defers "anything not required by the acceptance criteria" |

**Reading for Myco:** uzel validates that `identity` + `storage` + `outbox` (read/subscribe) + `resource` + partial `inc` is a viable, demo-able minimum slice for a native runtime — that's the practical v1 floor to match uzel's functional bar. `intent` and full `inc` (channel semantics) are the two things even the reference native implementation punted on; Myco can punt on them too without falling behind the state of the art.

### Table Stakes (Milestone B)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| NIP-5D-conformant sandbox (iframe `allow-scripts` only, no `allow-same-origin`, no injected `window.nostr`) | Non-negotiable per spec ("MUST"); this *is* the security model | MEDIUM | Android WebView equivalent of "sandboxed iframe" needs care — WebView doesn't have the same iframe sandbox primitive; likely needs its own isolation strategy (separate WebView instance, no shared cookie jar/storage, JS bridge instead of postMessage-over-window). |
| `{ type: "domain.action", ...payload }` JSON envelope over the shell↔napplet channel | Wire-format compatibility with any napplet built against `@napplet/sdk`/`@napplet/shim` | LOW–MED | On Android this likely maps to a `WebMessagePort`/`addJavascriptInterface` bridge rather than literal `postMessage`, but the JSON envelope shape must match exactly for napplets to "just work." |
| `identity` NAP (read-only) | Table stakes per uzel's own floor; nearly every useful napplet needs "who is the user" | LOW | Straightforward — expose the device's own npub/pubkey, read-only, never signs. |
| `storage` NAP (scoped, quota'd) | Table stakes per uzel's own floor | LOW–MED | Scope key is `dTag:aggregateHash` — needs the manifest/aggregate-hash machinery first. |
| `outbox` NAP (query/subscribe/publish, shell owns relay selection/signing/fanout) | Table stakes; this is the *default* boundary the spec expects most napplets to use instead of raw `relay` | MEDIUM | Reuses Myco's existing relay pool and gossip fan-out logic — this is largely wiring, not new protocol work. |
| `resource` NAP (`bytes`/`bytesMany` over `https`/`blossom:sha256:`/`nostr:`/`data:`) | The *only* fetch primitive inside the sandbox — without it, napplets can't load a single image | LOW–MED | `blossom:sha256:<hex>` scheme maps directly onto Myco's embedded Blossom store — cheap. |
| Manifest ingestion (kind 35129, NIP-5A tag schema, `requires` tags) | Table stakes for distribution — this is how a napplet declares what it needs and how the shell verifies build identity | LOW–MED | PROJECT.md already commits to reusing the nsite manifest+Blossom pipeline under a new event kind — directly aligned. |
| Domain-presence feature-gating (`window.napplet?.domain`) | Mandatory per spec; the only way to ship a subset of 23 domains without breaking conformant napplets | LOW | Just don't inject domains you don't implement — napplets are required to check presence. |

### Differentiators (Milestone B)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| `napplet.neighbours` / `window.myco.neighbours` mesh pub/sub API | **Not part of NIP-5D or any NAP domain** — confirmed across the full 23-domain reference, none address mesh/neighbour discovery. This is Myco's own addition layered on top of the standard surface, exactly as PROJECT.md frames it ("Myco's only addition is mesh behaviour"). No other napplet runtime (Kehto, uzel) has anything like it. | MEDIUM–HIGH | See Feature Dependencies below — this needs the mesh-relay-URL work to land first. Must be *explicit* (opt-in call), never an implicit side effect of `outbox.publish` — see Anti-Features. |
| Settings for embedded vs. external relay/Blossom backend (Citrine, standard Blossom server) | No other napplet runtime targets "point me at an unmodified third-party relay and keep working offline-first." Directly serves PROJECT.md's Milestone B goal of an unmodified Citrine/Blossom backend working via standard libraries (applesauce, nostr-tools) with no shim. | MEDIUM | `relay_backend: "embedded" \| "local-forward"` already sketched in `SettingsPatch` (`ffi-surface.md`) — extend, don't invent. |
| Rust-native napplet runtime on **Android** | uzel explicitly excludes Android and desktop-native from its own POC scope ("Do not add ... Android ... to this POC"). Shipping a conformant Rust runtime on mobile is itself the frontier — nobody else has done it. | HIGH | This is the actual hard part of Milestone B; treat everything else in this table as scaffolding around it. |
| napplets + nsites unified in one Library with a type badge | Reduces cognitive load of "two kinds of apps, two mental models" — a differentiator in UX terms even though the underlying plumbing is shared | LOW–MED | PROJECT.md already commits to this; low technical risk since it reuses existing sync/Discover/Circle-gating machinery. |

### Anti-Features (Milestone B)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|------------------|-------------|
| Implementing all 23 NAP domains for v1 | "Be a complete/compliant runtime" | Scope explosion under a hard timeline; even uzel — a purpose-built native-runtime POC — only proved 4–5 domains functionally. Matching uzel's bar *is* the reasonable v1 target, not the full union. | Ship `identity`, `storage`, `outbox`, `resource`, `inc` (emit/on only), `config` for v1; add the rest on demand as napplets need them |
| Inventing a bespoke, "more efficient" postMessage envelope | Perceived perf/ergonomics win | Breaks conformance with every napplet built against `@napplet/shim`/`@napplet/sdk`, and against Kehto; `@napplet/conformance`/`conformance-cli` exist specifically to catch this kind of drift | Match the `{ type: "domain.action", ...payload }` envelope exactly; run `@napplet/conformance-cli` against the Myco runtime if feasible |
| Implicit/ambient mesh publish (e.g. auto-relaying every `outbox.publish` onto the mesh) | "Just make everything mesh-aware automatically" | PROJECT.md names this explicitly as the thing to avoid: "Mesh publish/subscribe is explicit in nsite code — never accidental." An app author must never be surprised that a normal Nostr publish call silently left the device. | `napplet.neighbours`/`window.myco.neighbours` is a separate, deliberately-named API surface — using `outbox` never touches the mesh layer |
| Granting `allow-same-origin` (or its Android-WebView equivalent) "just for this one napplet" | Debugging convenience, or a napplet that "needs" same-origin storage | Explicit spec **MUST NOT** — collapses the entire trust boundary (service workers, cross-origin reach, shared storage) | Fix the capability gap via a proper NAP (e.g. `storage`, `resource`), not by relaxing the sandbox |
| Injecting `window.nostr` (NIP-07) into the napplet sandbox | "Napplets could reuse existing NIP-07-aware code" | Explicit spec **MUST NOT** — signing must be brokered through the shell, never exposed raw to untrusted iframe code | Route all signing through the shell's own key management (same identity key Myco already holds) |

---

## Feature Dependencies

```
[Milestone A]
Per-peer connection-state list ──requires──> existing FFI state (PairedPeer, BlePeer, BleStatus) [already shipped]
Per-peer "why not connected" reason code ──requires──> new failure-reason enum in FFI contract (not yet modeled)
Reason-code UI ──is only meaningful after──> churn-survival / reconnect logic lands (a flaky reconnect makes "reasons" noise)
Wi-Fi Aware default-on visibility ──requires──> Wi-Fi Aware status struct (new, mirrors existing BleStatus shape)

[Milestone B]
napplet.neighbours API ──requires──> mesh relay URL + TTL-in-protocol work (MESH_EVENT) landing first
napplet manifest ingestion (kind 35129) ──enhances──> existing nsite manifest+Blossom pipeline [reused, not rebuilt]
storage NAP scoping ──requires──> manifest aggregate-hash computation (dTag:aggregateHash key)
outbox NAP ──enhances──> existing peer relay pool + gossip fan-out [reused, not rebuilt]
resource NAP blossom: scheme ──requires──> existing embedded Blossom store [already shipped]
Settings: external relay/Blossom backend ──enhances──> existing SettingsPatch.relay_backend field [already sketched]
Full NAP domain rollout (media/ble/webrtc/etc.) ──conflicts with──> 3-day-adjacent solo-developer timeline; sequence after v1 ships
```

### Dependency Notes

- **Reason-code diagnostics require churn-survival to land first (or alongside):** a reason code that flickers between `handshake_pending` and `handshake_failed` every few seconds because the underlying reconnect logic is itself unstable is worse than no diagnostics at all — it teaches users to distrust the UI.
- **`napplet.neighbours` requires the mesh-relay-URL work:** PROJECT.md sequences TTL-in-protocol and the dedicated mesh relay URL as prerequisites; the neighbours API is explicitly "layered over that URL," so it cannot land first.
- **Napplet distribution reuses nsite plumbing:** because napplets ship on "the existing manifest + Blossom pipeline under their own event kind" (PROJECT.md), most of the sync/Discover/Circle-gating risk is already retired — the new risk is entirely in the runtime (sandboxing, NAP dispatch), not distribution.

---

## MVP Definition

### Launch With (Milestone A — v0.5, 2026-08-05)

- [ ] Per-peer status list with reachability-tier icon (connected / relayed-reachable / offline) — reuse the bitchat glyph pattern, built on already-shipped FFI fields
- [ ] Pending-pairing indicator (`PairedPeer.pairing`) surfaced in UI
- [ ] Wi-Fi Aware on-by-default + radio/scanning status visible
- [ ] Last-seen timestamp per peer
- [ ] One human-readable "why not connected" reason per peer, even if the reason enum starts coarse (`no_shared_transport` / `handshake_pending` / `handshake_failed` / `out_of_range`)

### Add After Validation (v1.x)

- [ ] RSSI/signal-strength bars — trigger: field feedback that raw connect/disconnect state isn't enough
- [ ] Mesh topology sheet (bitchat-style circular graph) — trigger: demo/support requests for "show me the whole mesh"
- [ ] Share-logs-to-developer via NIP-17 DM — trigger: field testing volume outpaces manual debugging

### Future Consideration (v2+)

- [ ] Multi-hop traceroute-style path diagnostics (Meshtastic-style) — defer until the mesh routinely has >1-hop topology worth tracing
- [ ] Graphical network-health dashboard — defer until peer count / churn data justifies it

---

### Launch With (Milestone B)

- [ ] Sandboxed napplet host (Android-WebView equivalent of the NIP-5D iframe boundary) with the JSON envelope wire format
- [ ] `identity`, `storage`, `outbox`, `resource`, `inc` (emit/on) NAP domains — matches uzel's own functional floor
- [ ] Kind-35129 manifest ingestion via the existing nsite manifest+Blossom pipeline
- [ ] `napplet.neighbours` / `window.myco.neighbours` explicit mesh pub/sub
- [ ] Settings: embedded vs. external (Citrine) relay backend; mesh relay URL exposed for standard Nostr libraries

### Add After Validation (v1.x)

- [ ] `config` NAP (declarative per-napplet settings) — trigger: first napplet that needs user-configurable options
- [ ] `intent` NAP — trigger: first cross-napplet "open this" use case (uzel itself punted on this)
- [ ] `@napplet/conformance-cli` wired into CI — trigger: once the runtime is stable enough to be worth regression-testing against drift

### Future Consideration (v2+)

- [ ] `ble`, `webrtc`, `serial` NAP domains bridging into Myco's own transport stack — high complexity, no demonstrated near-term napplet demand
- [ ] `media`, `notify`, `theme`, `keys`, `lists`, `count`, `common`, `dm`, `cvm`, `upload`, `fs`, `link` — add per actual napplet demand, not speculatively

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Per-peer connection-state list | HIGH | LOW | P1 |
| Pending-pairing indicator | HIGH | LOW | P1 |
| Wi-Fi Aware default-on + status | HIGH | LOW–MED | P1 |
| Per-peer "why not connected" reason code | HIGH | MEDIUM | P1 |
| Last-seen timestamp | MEDIUM | LOW | P1 |
| RSSI/signal bars | MEDIUM | LOW | P2 |
| Mesh topology sheet | MEDIUM | MEDIUM–HIGH | P2 |
| Share logs via NIP-17 DM | MEDIUM | MEDIUM | P2 |
| NIP-5D sandbox host on Android | HIGH | HIGH | P1 (Milestone B) |
| Core NAP set (identity/storage/outbox/resource/inc) | HIGH | MEDIUM | P1 (Milestone B) |
| Manifest ingestion (kind 35129) | HIGH | LOW–MED | P1 (Milestone B) |
| `napplet.neighbours` mesh API | HIGH | MEDIUM–HIGH | P1 (Milestone B) |
| External relay/Blossom settings | MEDIUM | MEDIUM | P1 (Milestone B) |
| Remaining 18 NAP domains | LOW (speculative) | HIGH (aggregate) | P3 |

**Priority key:**
- P1: Must have for launch
- P2: Should have, add when possible
- P3: Nice to have, future consideration

---

## Competitor / Reference Feature Analysis

### Peer diagnostics (Milestone A)

| Feature | Briar | bitchat | Meshtastic | Berty | Manyverse (SSB) | Myco's approach |
|---|---|---|---|---|---|---|
| Per-peer connection state | Not found in manual/UI | Yes — glyph-per-reachability-tier on every row | Yes — online dot + hop badge | Architecture supports multi-transport; no dedicated status UI found | Yes — colored dot (yellow=connecting, green=connected) | Per-peer icon row, reusing existing FFI fields |
| Signal strength | No | No (BLE, not RF-metric-driven in UI) | Yes — RSSI/SNR bars, well-documented | No | No | RSSI bars as P2 (data already in FFI surface) |
| "Why not connected" explainer | No | No (topology shows *what*, not *why*) | No (traceroute shows path, not handshake failure) | No | No | **Differentiator** — new reason-code UI, no direct precedent |
| Topology/graph view | No | Yes — `MeshTopologyView`, circular self-centered graph | Implicit via node list + traceroute | No | No | Candidate for v1.x, not v0.5 |
| Pending/connecting state | Only "add contact failed after 48h" for *new* contacts | Encoded in glyph state | N/A (no formal pairing step) | No | Yes — distinct connecting vs. connected dot colors | P1 — surface existing `pairing` field |

### Napplet runtime surface (Milestone B)

| Dimension | Kehto (reference shell) | uzel (Rust/Linux POC) | Myco's approach |
|---|---|---|---|
| Platform | Web (browser) | Linux desktop, Tauri | **Android**, Rust core over JNI — unclaimed by either reference |
| NAP domains implemented | Presumably the reference set (not audited here) | `identity`, `storage` (minimal), `outbox` (query/subscribe strong, publish light), `resource`, `inc` (emit/on only) | Match uzel's floor for v1: same five domains |
| Mesh/offline behavior | None documented | None — explicitly excludes offline/mesh from POC scope | **Differentiator** — `napplet.neighbours`, explicit and opt-in |
| External relay/Blossom backend | Standard Nostr relay config | Fixed to `purplepag.es`/`nos.lol` per POC | **Differentiator** — Citrine/Blossom backend toggle |

---

## Sources

**Milestone B — primary/authoritative (HIGH confidence, fetched and inspected directly):**
- https://napplet.run/ and https://napplet.run/docs/ (VitePress site, fetched raw HTML + rendered content)
- https://napplet.run/docs/spec.html — "NIP-5D spec status," names canonical spec + NAPs track as sole sources of truth, confirms alpha/drift status
- https://napplet.run/docs/naps/ — full NAP domain reference (23 domains, purposes, example calls)
- https://napplet.run/docs/guide/nip-5d.html — NIP-5D explained (wire format, sandbox, identity, manifest, security model)
- https://napplet.run/docs/guide/concepts.html — core concepts (envelope, NAPs, shell, sandbox, ACL, storage scoping, domain presence)
- https://napplet.run/docs/packages/core.html, /packages/shim.html, /packages/sdk.html, /packages/nap.html, /packages/vite-plugin.html — package-level API surfaces
- https://github.com/nostr-protocol/nips/pull/2303 — canonical NIP-5D text (living document)
- https://github.com/napplet/naps — NAPs track (per-domain capability contracts)
- https://github.com/jodobear/uzel — Rust/Linux napplet runtime POC; `README.md`, `uzel-poc-validated-pack/README.md`, `uzel-poc-validated-pack/STATUS.md` inspected directly for implemented-vs-deferred NAP domain scope

**Milestone A — comparable-app peer diagnostics (MEDIUM confidence; bitchat verified via direct source read, others via search-summarized docs):**
- https://github.com/permissionlesstech/bitchat — `bitchat/Views/MeshPeerList.swift` and `MeshTopologyView.swift` read directly (glyph-per-reachability-tier pattern, circular topology sheet)
- https://briarproject.org/manual/ — no per-contact transport/troubleshooting UI documented
- https://meshtastic.org/docs/software/android/user/discovery/ and DeepWiki node-metrics page — node list fields (battery, signal bars, hop badge, last-heard, MQTT-vs-LoRa badge), Traceroute/Neighbor Info tools
- https://berty.tech/docs/protocol, https://berty.tech/blog/bluetooth-low-energy/ — multi-transport architecture (Android Nearby / Multipeer Connectivity / BLE); no dedicated diagnostic UI found
- Manyverse/SSB FAQ and IPFS mobile design guide survey — Connections screen colored-dot connecting/connected states

**Project context:**
- `/Users/gump/Documents/development/fips/fips-pop/.planning/PROJECT.md`
- `/Users/gump/Documents/development/fips/fips-pop/reference/FIX-TODOS.md`
- `/Users/gump/Documents/development/fips/fips-pop-portbump/docs/reference/ffi-surface.md` (existing/proposed FFI state shape — `PairedPeer`, `BlePeer`, `BleStatus`, `SettingsPatch`)
- `/Users/gump/Documents/development/fips/fips-pop-portbump/docs/reference/nostr-kinds.md` (nsite kinds 15128/35128 for contrast with napplet kind 35129)

---
*Feature research for: peer-to-peer mesh app runtime (Android, BLE/Wi-Fi Aware, Nostr nsites + napplets)*
*Researched: 2026-08-04*
