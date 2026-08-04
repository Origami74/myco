# Stack Research

**Domain:** Android P2P mesh app runtime — napplet host, relay-to-relay protocol extension, browser-facing mesh API, pluggable relay/Blossom backends
**Researched:** 2026-08-04
**Confidence:** MEDIUM — protocol sources (napplet.run/docs, Kehto's `RUNTIME-SPEC.md`, Citrine source) are HIGH confidence and were fetched directly; the "standard Rust stack" for napplets does not yet exist as a mature published crate, so those recommendations are HIGH-confidence *architecture* conclusions but necessarily build-it-yourself rather than "install package X."

## The one finding that reframes all four questions

**Napplets are not a JS/WASM-embedding problem.** Per Kehto's `RUNTIME-SPEC.md` (the reference runtime's own protocol reference, cross-checked against the NIP-5D PR and the NAP domain registry): a napplet is verified bytes (`index.html` + assets) assembled by the **host** and injected into a **browser iframe** via `srcdoc`, sandboxed with `sandbox="allow-scripts"` (no `allow-same-origin`). The JS inside that iframe runs in the **browser's own JS engine** — there is no WASM runtime, no embedded scripting VM, and no bytecode sandbox anywhere in the reference design. Myco already ships a JS engine capable of this: the Android `WebView`. This means Myco needs *zero* new JS/WASM-embedding crates. What it needs is a Rust-side **manifest resolver + verifier** (fetch a signed Nostr event, fetch content-addressed blobs, recompute a hash, reject on mismatch — the exact shape `nsite-deck` already implements for nsites) and a **native-to-shell bridge** so the top-level shell page can ask Rust to do privileged things (relay queries, Blossom fetches, signing, ACL). The iframe-to-shell leg of the protocol (`postMessage`, `MessageEvent.source` identity) is pure web-platform behavior and needs no native involvement at all.

This corrects an assumption implicit in the research question ("what crates exist for embedding a JS/WASM app runtime") — the honest answer is "none are needed," and reaching for one (wasmtime, deno_core, boa, quickjs-rs) would be a net-negative: it duplicates the WebView's JS engine and steps outside the browser-native security model (`srcdoc` + iframe sandbox + `MessageEvent.source`) the spec is built on.

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `nostr` | **0.44.7** (bump from pinned 0.44.3) | Event parsing, Schnorr verification, NIP-19, tag handling for napplet manifests (kinds `5129`/`15129`/`35129`) | Latest *stable* release on the 0.44 line already in the workspace (`Cargo.lock` shows `0.44.3`); a same-minor patch bump, zero API churn. `0.45.0` only exists as `-alpha.1`…`-alpha.8` (newest `alpha.8`, 2026-07-31) — do not move to it, see "What NOT to Use." HIGH confidence (crates.io API, verified directly). |
| `negentropy` | **0.5.0** (already present) | Set-reconciliation for manifest sync (nsites today; napplet manifests should reuse the same path) | Already resolved in `Cargo.lock` at exactly this version, paired with `nostr` 0.44.x — no compatibility work needed. Citrine (the target external relay) implements the matching wire protocol server-side (`NegentropyHandler.kt`, confirmed via source), so the existing negentropy sync code keeps working unmodified against an external backend. HIGH confidence. |
| Android `WebView` (existing) + `iframe`/`srcdoc` | n/a (platform) | Executes napplet JS; provides the sandbox | This *is* the napplet runtime's execution engine. Reuse the existing per-nsite WebView infrastructure — a napplet's shell page is just another gateway-served resource, hosting a nested sandboxed iframe. No new engine dependency. HIGH confidence (Kehto `RUNTIME-SPEC.md`, fetched directly). |
| `androidx.webkit:webkit` | **1.16.0** (latest stable; 1.17.0 is RC only) | `WebViewCompat.addWebMessageListener` — the shell↔native bridge for `window.myco` and any privileged napplet NAP domains (`relay`, `storage`, `identity`, …) | Origin-scoped, reflection-free JS↔native messaging, replacing the legacy `addJavascriptInterface` reflection surface. This is the current Android-documented best practice specifically *because* it matches the spec's own trust model: allow-list the shell's own origin (`allowedOriginRules`), never the sandboxed napplet iframe (which has an opaque `srcdoc` origin and won't match an origin rule regardless — defense in depth for free). MEDIUM-HIGH confidence (Android docs + Google source snippets via WebSearch, not Context7-verified). |
| Hand-rolled `axum` WebSocket relay (existing `myco-relay`) | axum 0.8 (existing) | Host for the new `MESH_EVENT` relay-to-relay verb | Keep it. Do **not** migrate to `nostr-relay-builder` to get this — see "What NOT to Use." Full control over the WebSocket frame parsing is exactly what's needed to add a non-NIP-01 verb between Myco peers while keeping the *client-facing* (backend) side pure NIP-01. HIGH confidence (architectural, verified against `nostr-relay-builder`'s public API surface). |
| `tokio-tungstenite` (existing, 0.24) | 0.24 | WebSocket client for talking to an **external** relay (Citrine, or any NIP-01 relay) with zero mesh-specific framing | The backend-facing side of the relay abstraction must speak *only* standard NIP-01 (`EVENT`/`REQ`/`CLOSE`/`EOSE`/`OK`/`NOTICE`) — no custom verbs — precisely so Citrine (or anything else) is a drop-in. This crate is already a dependency; no new one needed. HIGH confidence. |
| `reqwest` (existing, 0.12) | 0.12, `rustls-tls` | HTTP client for an external BUD-01 Blossom backend | `BlobStore` is already a seam in `nsite-deck`; an external-Blossom implementation is a thin `reqwest` GET/PUT/HEAD client against BUD-01 semantics. No new dependency. HIGH confidence. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `sha2` (existing) | 0.10 | NIP-5A aggregate-hash recomputation over napplet manifest `path` tags | Reuse the exact routine `nsite-deck` already uses for nsite manifests (kind `15128`/`35128`) — napplet manifests (kind `15129`/`35129`/`5129`) use the *same* NIP-5A aggregate scheme, one kind number higher. Don't reimplement. |
| `serde_json` (existing) | 1.x | Parse/emit the NIP-5D JSON envelope (`{ "type": "<domain>.<action>", ...payload }`) shuttled across the shell↔native bridge | The envelope is intentionally "just JSON" (napplet.run's own framing) — no schema/codegen library needed. |
| `url` (existing indirectly via reqwest/fips) | 2.x | Parse `resource` NAP scheme URLs (`https://`, `blossom://`, `nostr://`, `data:`) for the `resource` domain if implemented | Only needed if Myco implements the `resource` NAP domain (sandboxed byte fetching) — optional for a v1 domain subset. |
| `thiserror` / `anyhow` (existing) | 2 / 1 | Error types for manifest verification failures (bad signature, hash mismatch, missing blob) | Standard project pattern already in use; extend, don't introduce a second error-handling convention. |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| Kehto (`github.com/kehto/web`) run locally | Conformance/reference target — watch real NIP-5D envelopes flow between a shell and a napplet | Not a dependency. Clone and run it during Rust-runtime development to compare wire behavior against Myco's implementation; its `RUNTIME-SPEC.md` is the single best "what does a conformant host actually do" document available (better than the NIP-5D PR text alone, which is intentionally minimal). |
| `@napplet/conformance-cli` (`0.16.2` per Kehto's pin) | Headless Playwright-driven protocol conformance runner | JS/npm tool, dev-only. Useful for validating that Myco's shell-side JS bootstrap (the piece that assembles `window.napplet.*`) behaves per-spec, independent of the Rust backend. Optional but cheap; catches shell-bootstrap regressions the Rust unit tests can't see. |
| `napplet/naps` registry (`github.com/napplet/naps`) pinned at a specific commit | Authoritative NAP domain wire contracts | Pin a commit SHA when implementing each NAP domain (`relay`, `storage`, `inc`, `identity`, …) — the spec explicitly self-describes as "alpha… experimental and a moving target." Kehto itself pins `5ac0490461ca6fec2f0d2e45b4835cf9bc08de24`; do the same rather than tracking `master`. |

## Installation

```toml
# workspace Cargo.toml — bump, don't add
[workspace.dependencies]
nostr = "0.44.7"          # was 0.44 (resolved 0.44.3) — same minor line, patch bump only
negentropy = "0.5"         # already resolved at 0.5.0, no change needed — listed for clarity

# no new Rust crates are required for the napplet runtime itself —
# manifest verification reuses nsite-deck's existing sha2/nostr/serde_json path,
# transport reuses the existing axum (mesh side) / tokio-tungstenite (backend side) split
```

```gradle
// android/app/build.gradle.kts — the one genuinely new Android dependency
dependencies {
    implementation("androidx.webkit:webkit:1.16.0")
}
```

```bash
# dev-only, not shipped — clone the reference runtime to diff behavior against
git clone https://github.com/kehto/web
cd web && pnpm install
```

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|--------------------------|
| Hand-roll the NIP-5D manifest resolver/verifier in Rust, reusing `nsite-deck`'s existing sha2/nostr plumbing | `jodobear/uzel` + `jodobear/nampplets` (a fork of `pablof7z/nampplets`) as Cargo path/git dependencies | Only if that project reaches a tagged, ratified release with an Android target. Today it's Linux/Tauri-only, its own `compatibility.lock` lists `supported_domains = []` for every platform (i.e., it self-reports zero conformant NAP domains as of this research), and it layers its own "NMP" runtime-core vocabulary (`provider-identity`, `provider-inc`, `provider-lists`, …) that is **not** the same as the ratified NAP domain list published at `napplet.run/docs/naps/` (`relay`, `storage`, `inc`, `identity`, `theme`, `resource`, `outbox`, …) — confirmed by direct comparison of both sources. Treat it strictly as an architecture-shape reference (principal/grant/session separation, content-addressed identity), never as a dependency. |
| Hand-rolled `axum` WebSocket relay for the mesh side, kept exactly as-is, extended with a `MESH_EVENT` verb | `nostr-relay-builder` (rust-nostr, 0.44.1 stable) | If Myco ever wants a fully NIP-01-compliant relay with negentropy/NIP-42/NIP-11 handled for free *and no custom verbs* — i.e., for the **backend** relay role (replacing hand-rolled `myco-relay` internals), not the peer-to-peer mesh wire. Its public surface (`builder`/`local`/`mock` modules) has no documented hook for adding a non-NIP-01 message type, so it actively fights the `MESH_EVENT` requirement. |
| `androidx.webkit` `WebViewCompat.addWebMessageListener` for the shell↔native bridge | `addJavascriptInterface` | Only as a last resort if a target device's WebView build predates message-listener support — feature-detect with `WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)` and fall back, per Android's own migration guidance. Given Myco already requires a modern Chromium WebView for existing nsite features, this should rarely trigger. |
| Delegate all napplet JS execution to the existing per-nsite `WebView` (iframe + `srcdoc`) | An embedded JS/WASM engine crate (`wasmtime`, `deno_core`, `boa`, `quickjs-rs`) | Never, for this spec. Napplets are explicitly a browser-iframe sandboxing model (`MessageEvent.source` identity, `srcdoc` CSP) — an embedded engine would have to reimplement that trust model from scratch and would run *outside* it, which is a strictly worse security posture, not a better one. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|--------------|
| `nostr` `0.45.0-alpha.*` | Newest is `0.45.0-alpha.8` (2026-07-31) — pre-release, breaking-change surface, not what any downstream (fips, nsite-deck) is built against. Verified live against crates.io. | Stay on `0.44.7`, the latest stable patch on the pinned line. |
| `jodobear/uzel` / `jodobear/nampplets` (`nmp-native-runtime-core`, `nmp-native-runtime-ffi`, etc.) as Cargo dependencies | Pre-alpha proof-of-concept: Linux-only, Tauri desktop shell, private `AF_UNIX` daemon architecture, `redb` store, unratified `compatibility.lock` (`supported_domains = []` everywhere), and — per direct comparison — its "provider" domain vocabulary diverges from the ratified NAP domain names at `napplet.run/docs/naps/`. None of this maps cleanly onto an Android/JNI/WebView host. | Implement the NIP-5D + NAP-domain surface directly against `napplet.run/docs`, `RUNTIME-SPEC.md` (Kehto), and the `napplet/naps` registry, using Myco's own `nsite-deck` patterns (already handles the sibling nsite protocol, NIP-5A hashing, manifest resolution). |
| An embedded JS/WASM engine (`wasmtime`, `deno_core`, `boa`, `quickjs-rs`, etc.) | Solves a problem the spec doesn't have. Napplets run as plain JS in a sandboxed `iframe`/`srcdoc`, using the host's own JS engine — Android's WebView already is that engine. Embedding a second one adds attack surface and steps outside the `MessageEvent.source`-based trust model the protocol depends on. | Android `WebView` (existing), `iframe sandbox="allow-scripts"` + `srcdoc` injection, exactly as Kehto's reference host does it. |
| `android.webkit.WebView.addJavascriptInterface` for the shell bridge | Exposes annotated methods reflectively to **every frame** in the WebView, including nested iframes, with no origin scoping — a known Android WebView RCE-class footgun, and specifically wrong here because the napplet iframe (untrusted, opaque-origin) lives inside the same WebView as the trusted shell. | `androidx.webkit:webkit` 1.16.0, `WebViewCompat.addWebMessageListener` with `allowedOriginRules` scoped to the shell's own origin only. |
| `nostr-relay-builder` as a wholesale replacement for `myco-relay`'s mesh-facing WebSocket layer | No documented extension point for a non-NIP-01 verb; adopting it to "get NIP-01 for free" on the mesh side would require forking it the moment `MESH_EVENT` needs to ride the same socket. | Keep hand-rolled `axum` + `tokio-tungstenite` on the mesh-facing side; consider `nostr-relay-builder` only for a *pure* NIP-01 backend role (out of scope here, since Citrine already fills it externally). |

## Stack Patterns by Variant

**If implementing the napplet runtime (addition 1):**
- Rust side: manifest resolver (fetch kind `15129`/`35129`/`5129` event, verify signature, verify each `path` blob's sha256, recompute NIP-5A aggregate, compare to the `["x","<hex>","aggregate"]` tag) — a near-direct port of the nsite manifest path already in `nsite-deck`, one kind number over.
- Serving side: reuse the existing gateway (`nsite-deck::gateway`) pattern — a napplet's shell HTML is gateway-served like an nsite, with the sandboxed napplet iframe's `srcdoc` assembled server-side (in Rust) from the verified blobs, never client-fetched unverified.
- Bridge side: `androidx.webkit` `WebViewCompat.addWebMessageListener`, scoped to the shell page's own origin, forwarding NAP-domain requests (`relay.*`, `storage.*`, `identity.*`, …) into the existing JNI `dispatch(actionJson)` reducer as new `NativeAppAction` variants — no new FFI mechanism, extend the one that exists.
- Start with a **small NAP domain subset**: `shell` (mandatory handshake) + `storage` + `identity` (read-only) + the mesh-specific `napplet.neighbours` API cover the milestone's stated surface; defer `cvm`, `ble`, `fs`, `media` — they're all independently optional per the NAP registry ("shells may support any subset of NAPs").

**If implementing the `MESH_EVENT` relay-to-relay extension (addition 2):**
- The extension lives *only* on the peer-to-peer leg (`PeerRelayPool` / `MeshGossiper` ↔ another Myco device's mesh-facing WebSocket, `ws://<npub>.fips:4870`) — never on the backend leg.
- The backend (embedded `myco-relay` internals, or an external Citrine) only ever sees clean, standard `EVENT`/`REQ`/`CLOSE` — no TTL, no hop metadata. The mesh gossip layer is the only component that speaks both dialects, translating `MESH_EVENT{event, ttl}` on the wire to a plain `EVENT` write into whichever backend is configured.
- This is *why* an unmodified relay like Citrine can be a drop-in backend at all: TTL never touches the event body or the relay's NIP-01 surface, matching the "TTL lives in the relay-to-relay protocol, not in event bodies" requirement directly.

**If implementing pluggable external relay + Blossom backends (addition 4):**
- The seams already exist (`RelayBackend`, `BlobStore` in `nsite-deck::seams`) — add a second implementation of each backed by `tokio-tungstenite` (relay) / `reqwest` (Blossom) speaking pure standard protocol, selected in settings alongside the existing embedded implementations. No new trait design needed, just new implementations.
- Citrine defaults to `127.0.0.1:4869` — one port *below* Myco's own embedded relay default (`4870`) and identical to the un-offset `nsite-deck` reference default. Citrine's own source comments show it was already built with nsite/NIP-5A interop in mind (`DEFAULT_NSITE_RELAYS`, a built-in nsite installer). Do not hardcode `4870` for the external case — the port must be user-configurable in settings.
- Citrine rejects several kinds as standalone events by default (`13`, `9734`, `22242`, `24242`, `27235` — signed artifacts that should never be published as top-level events); this has no effect on nsite/napplet manifest kinds (`15128`/`35128`/`15129`/`35129`/`5129`) but is worth knowing if the mesh ever carries those other kinds through the same backend.
- No mature, widely-adopted **Android** Blossom-server app equivalent to Citrine was found (closest is `nostrnative/bloom`, a TypeScript hybrid relay+Blossom server, not Android-native). Recommend the external-Blossom setting accept any BUD-01-conformant HTTP endpoint (local or remote) rather than assuming a companion Android app exists — the `reqwest`-based client doesn't care where the server runs.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|------------------|-------|
| `nostr = "0.44.7"` | `negentropy = "0.5.0"` | Already resolved together in the current `Cargo.lock`; confirmed no conflict on bump from `0.44.3` → `0.44.7` (same minor line). |
| `androidx.webkit:webkit:1.16.0` | `WebViewFeature.WEB_MESSAGE_LISTENER` | Feature-detect at runtime with `WebViewFeature.isFeatureSupported(...)` before calling `addWebMessageListener` — the underlying system WebView package, not just the androidx artifact, must support it. Fall back to `addJavascriptInterface` only if unsupported (should be rare on any device that already runs Myco's existing WebView-based nsite feature set). |
| Kehto's pinned `@napplet/*` line (`core 0.31.1`, `nap 0.31.2`, `sdk 0.27.2`, `shim 0.29.2`) | `napplet/naps` registry commit `5ac0490461ca6fec2f0d2e45b4835cf9bc08de24` | JS-side reference versions only (not Rust dependencies) — cite them if/when validating Myco's Rust implementation against Kehto's conformance suite, and re-check them since the spec is explicitly a moving target. |
| Citrine (external relay) | NIP-01, NIP-42 (optional, owner-key-gated), NIP-77 (negentropy, confirmed via `NegentropyHandler.kt`), NIP-86 (relay management API) | No NIP-11-derived `supported_nips` list was directly retrievable during this research; NIP-77 support was confirmed by locating the handler source file directly, not by NIP-11 introspection. Treat as MEDIUM confidence on the exact NIP list, HIGH confidence on NIP-77 presence specifically. |

## Sources

- `https://napplet.run/docs` and `https://napplet.run/docs/naps/` — fetched directly (WebFetch). NAP domain table, sandbox model, envelope format. HIGH confidence.
- `github.com/kehto/web`, file `RUNTIME-SPEC.md` — fetched directly via `gh api` (raw content). The single most detailed conformant-host reference found; cites NIP-5D PR #2303 and `napplet/naps` commit `5ac0490461ca6fec2f0d2e45b4835cf9bc08de24` as its own authorities. HIGH confidence.
- `github.com/nostr-protocol/nips/pull/2303` (NIP-5D) — fetched via WebFetch. Envelope format, identity-via-`MessageEvent.source`, sandbox attributes, manifest kinds. HIGH confidence.
- `github.com/jodobear/uzel` and `github.com/jodobear/nampplets` (fork of `github.com/pablof7z/nampplets`) — inspected directly via `gh api` (`Cargo.toml`, `README.md`, crate listing). Confirmed pre-alpha status, Linux/Tauri scope, divergent domain vocabulary from the ratified NAP registry. HIGH confidence on what these repos *are*; explicitly not used as an implementation dependency per user correction during this research.
- `github.com/greenart7c3/Citrine` — inspected directly via `gh api` (`Settings.kt`, `server/` directory listing, README, latest release `v3.0.1`, 2026-06-19). Confirmed NIP-77 (`NegentropyHandler.kt`), NIP-42 (`AuthGate.kt`), NIP-86 (`Nip86Handler.kt`), default port `4869`, built-in nsite support. HIGH confidence.
- `crates.io` API (`crates.io/api/v1/crates/...`) — queried directly for `nostr`, `nostr-relay-builder`, `negentropy` version data. HIGH confidence.
- Project `Cargo.lock` (this repo) — confirmed currently-resolved versions of `nostr` (`0.44.3`), `negentropy` (`0.5.0`), `jni` (`0.21.1`). HIGH confidence (primary source).
- Android developer documentation on `WebViewCompat.addWebMessageListener` vs `addJavascriptInterface` — via WebSearch, not Context7-verified directly; cross-checked against `dl.google.com` Maven metadata for the current `androidx.webkit` version (`1.16.0` stable, `1.17.0-rc01` pre-release). MEDIUM-HIGH confidence.
- `docs.rs/nostr-relay-builder` — fetched via WebFetch for architecture/extensibility assessment. MEDIUM confidence (module-level docs only, not full source audit).

---
*Stack research for: napplet runtime, mesh relay protocol extension, browser mesh API, pluggable relay/Blossom backends*
*Researched: 2026-08-04*
