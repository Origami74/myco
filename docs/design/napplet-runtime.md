# Napplets: a NIP-5D runtime inside Myco

This document proposes the **app runtime** layer of Myco: a conformant
[NIP-5D](https://github.com/nostr-protocol/nips/pull/2303) napplet runtime, written in
Rust, hosted in an Android WebView. Where the nsite layer makes an author's static site
browsable, this layer makes a small Nostr app *runnable* — with the relay, the blob
store, the mesh and the signing key all sitting behind a capability seam the app asks
through, rather than reaches around.

Napplets and nsites live side by side. They arrive the same way, appear in the same Apps
panel (a napplet marked 🦆, an nsite ＠), and open the same way: their own task, their own
window, no chrome. What differs is the trust model. An nsite is a document Myco serves. A
napplet is a program Myco *hosts*, and hosting means mediating — every capability it uses
is one Myco decided to grant, implemented by Myco, on the napplet's behalf.

Related docs: [./nsite-layer.md](./nsite-layer.md) (the content layer this builds on),
[./app-shell.md](./app-shell.md) (the per-app window model),
[./deep-links.md](./deep-links.md) (the existing `myco://app/…` link),
[./identity-pairing.md](./identity-pairing.md) (the device key this deliberately does not
reuse), [../reference/nostr-kinds.md](../reference/nostr-kinds.md) (event kinds).

---

## 1. What a napplet is

A napplet is a Nostr applet: a small app that does one thing well. A chat widget, a feed
viewer, a profile editor, and a relay manager are four napplets, not one app with four
tabs — the runtime composes them, they do not compose themselves.

A napplet is distributed exactly like an nsite: a signed manifest event whose `path` tags
map file paths to sha256 hashes, with the bytes themselves in Blossom. That is not a
coincidence. **NIP-5A** ("Pubkey Static Websites", kinds `15128` / `35128`) is the nsite
spec Myco already implements. **NIP-5D** is that same manifest shape at kinds `5129` /
`15129` / `35129`, plus three tags:

| Tag | Meaning |
|-----|---------|
| `["requires", "<domain>"]` | A capability the napplet needs from its runtime |
| `["archetype", "<slug>", "<convention>"]` | A role it can be invoked as by other napplets |
| `["config", "<json-schema>"]` | Declarative per-napplet configuration |

and one promotion: the NIP-5A aggregate hash — `["x", "<hex>", "aggregate"]`, computed
over the `path` tags alone — becomes the napplet's **identity**, not merely an integrity
check.

The capability seam itself is specified separately, in the
[NAP registry](https://github.com/napplet/naps). A NAP is one capability contract:
`NAP-RELAY` says a runtime can proxy relay reads and writes and here is exactly how a
napplet asks; `NAP-INTENT` says a runtime can open another napplet by role. NAPs are
transport-neutral; the *web projection* binds them to iframes, `postMessage`, and
`window.napplet.*`. Myco implements the web projection, because Myco's host is a WebView.

---

## 2. Decisions

| # | Decision | Choice |
|---|----------|--------|
| D1 | Runtime core | Written here, in Rust. No dependency on the `nmp-native-runtime-*` crates. |
| D2 | Rendering | One shell WebView per napplet; the napplet itself in a `sandbox="allow-scripts"` `srcdoc` iframe. |
| D3 | Identity | A user key **separate** from the mesh device key, generated on first napplet use and seeded with a guest profile. |
| D4 | Relay scope | Local relay, mesh peers, and internet relays when reachable. |
| D5 | Mesh | Standard NAPs behave exactly as specified. Mesh rides those contracts through `<npub>.fips` relay URLs (§7.4); a Myco mesh NAP covers only what has no standard equivalent. |
| D6 | First milestone | A full verified resolve — manifest, blobs, aggregate, `srcdoc`, handshake. No shortcuts that get thrown away. |
| D7 | Specification drift | Pin one `napplet/naps` revision and re-audit deliberately (§8). |
| D8 | Capability policy | An install-time review screen; grants stored per library entry. A granted `relay` covers publishing — no per-event prompt. |
| D9 | Acquisition | Fetch online when added by `naddr`; local and mesh-replicable from then on. |
| D10 | Crate | A new `myco-napplet-runtime`, over shared NIP-5A primitives in `nsite-deck`. |
| D11 | Intents | Android Intents and NAP-INTENT resolve through one shared resolver, bridged both ways, landed early. Claiming the `nostr:` URI scheme is deferred. |
| D12 | Shell origin | One loopback origin per napplet, `<pubkeyB36><dTag>.localhost`, mirroring nsite hosts. |

On D1: [uzel](https://github.com/) is a Linux napplet runtime built on the
`nmp-native-runtime-*` crates behind a Tauri daemon. It is worth reading for shape. It is
not worth depending on — the crates are an upstream Myco does not steer, designed around a
desktop daemon boundary, with no Android story.

---

## 3. What this reuses

Most of the runtime is wiring. The parts that carry over unchanged:

- **`myco-relay`** — the local Nostr event store and WebSocket server. Holds manifests.
- **`myco-blossom`** — the sha256-addressed blob store. Holds napplet files.
- **`nsite-deck`** — the shape to follow, and now a shared dependency: trait seams
  (`RelayBackend`, `BlobStore`, `PeerSource`, `FanoutSink`), manifest parsing, verified
  sync, and the base36 host-label encoder. Transport-agnostic and testable off-device.
- **`MeshGossiper` / `PeerRelayPool`** — event fanout and per-peer subscriptions. Once a
  napplet manifest reaches the local relay, mesh replication is nearly free.
- **`remote_backend.rs` / `remote_blobs.rs`** — the online relay and Blossom fetch paths
  that D9 needs.
- **`NsiteActivity`** — not the class, but the technique: a chrome-less WebView task that
  serves everything through `shouldInterceptRequest` at a `<host>.localhost` origin.
  Chromium treats `*.localhost` as loopback and a secure context, which is what makes the
  whole approach work.
- **`LibraryItem` / `AppsScreen`** — the Apps panel, which grows a type discriminant.

---

## 4. Resolution and identity

A napplet is rendered from bytes the runtime verified itself. Nothing else is trusted —
not a gateway, not a host, and least of all the napplet.

1. Resolve the signed manifest event (`35129` named, `15129` root, `5129` snapshot) and
   verify its signature.
2. Fetch each `path` tag's blob from Blossom by sha256, and verify that
   `sha256(blob)` equals the tag's hash.
3. Recompute the NIP-5A aggregate over the `path` tags alone, and assert it equals the
   `["x", "<hex>", "aggregate"]` tag. Only `path` tags feed the aggregate — `config`,
   `requires` and `archetype` do not.
4. Assemble the verified `/index.html` and inject it as `iframe.srcdoc`, carrying the
   `connect-src` policy as a `<meta http-equiv="Content-Security-Policy">` so it survives
   into the iframe's opaque origin.

The napplet's identity is the `(dTag, aggregateHash)` tuple **computed** from those
verified bytes. The runtime assigns it; the napplet never asserts it. Any verification
failure rejects the load outright — no iframe is ever created from unverified bytes.

**Napplets are single-file.** An opaque origin has nowhere to resolve a relative
subresource to, which is why the napplet build tooling inlines everything into one
`index.html`. A manifest describing a multi-file bundle is rejected at load with a clear
error, rather than rendered partially. The runtime does not inline at load time: that
would mean assembling bytes the author never signed as a unit.

### 4.1 Aggregate verification for nsites too

Myco does not verify the aggregate today — the nsite manifest parser reads `path`,
`server`, `title` and `description`, and no `x` tag. Every file is individually
hash-checked, so nothing served is corrupt, but there is no single check that a served
site is *the whole site its author signed*; a manifest could be re-signed with files
removed. Since NIP-5A is where the aggregate is defined and `nsite-deck` is where NIP-5A
lives, adding it there hardens nsites and gives napplets their identity primitive in one
change.

---

## 5. Architecture

### 5.1 Crate layout

```
myco-napplet-runtime/          transport-agnostic, no Android
  manifest.rs    parse and validate the NIP-5D kinds and their tags
  resolve.rs     manifest → blobs → verify → assembled artifact
  artifact.rs    index.html assembly and CSP meta injection
  session.rs     per-napplet session: identity tuple, grants, state
  nap/           one module per capability domain: shell, resource, relay, identity, …
  dispatch.rs    envelope routing: `domain.action`, id correlation, error model
  seams.rs       RelayBackend / BlobStore / Signer / OutboxResolver / NapTransport

nsite-deck/                    shared NIP-5A primitives
  aggregate.rs   the aggregate hash, used by both manifest families

myco-core/
  napplet.rs     wires the runtime to myco-relay, myco-blossom, PeerRelayPool, keys
```

The split follows the trust boundary, not the file format. The manifest layer is shared
because NIP-5D *is* NIP-5A plus tags. Everything above it — sessions, capability
dispatch, grants, artifact assembly — is napplet-only, and mixing it into the crate that
serves untrusted static documents would make both harder to reason about.

### 5.2 The shell

The shell is an HTML page shipped inside the APK. It is trusted code, never content, and
deliberately not updatable over the mesh.

It is served at a **per-napplet** loopback origin, `<pubkeyB36><dTag>.localhost` (D12),
by `NappletActivity`'s own `shouldInterceptRequest` — a separate client from the nsite
one, serving only shell assets. Per-napplet origins mean shell-side storage, caches and
cookies partition per napplet automatically, inherited from the browser rather than
enforced by us. `nsite-deck`'s base36 encoder already produces these labels.

The shell's whole job is:

- create the sandboxed iframe and set `srcdoc` to the assembled, verified bytes,
- carry `postMessage` in both directions,
- verify `MessageEvent.source` on every inbound message and bind it to the session,
- forward capability calls to Rust, and push results and subscription events back.

It stays thin on purpose. Every policy decision, every capability implementation, and all
verification live in Rust. The shell holds no key and opens no connection of its own.

### 5.3 The shell ↔ Rust channel

The transport is a seam — one trait, `NapTransport { recv() -> Envelope, send(Envelope) }`,
with several implementations. Dispatch, policy and capabilities never learn which is in
use, which is what keeps the following a choice rather than an architecture.

**`WebViewCompat.addWebMessageListener` — the device path.** `androidx.webkit` injects a
named JavaScript object into **only the frames matching `allowedOriginRules`**. Set that
to this window's shell origin — exact, never a wildcard — and both the sandboxed napplet
iframe (opaque origin) and every other napplet's shell origin fail to match. Messages go
up via `postMessage` and come back through `JavaScriptReplyProxy` on our own Handler.
In-process throughout: no port, no listening socket, no TCP hop. Kotlin sits in the path
as a byte pipe with no logic in it. Requires
`WebViewFeature.isFeatureSupported(WEB_MESSAGE_LISTENER)` — WebView 88 or newer.

**`WebMessagePort` — the fallback.** Available since API 23, so on every device Myco
supports. Create the channel and hand one port to the shell frame with an explicit
`targetOrigin`. Port ownership *is* the capability: a frame never given the port cannot
reach the runtime. Marginally more lifecycle to manage, identical cost profile.

**A loopback WebSocket — the desktop harness only.** Its value is not on the phone: it is
what lets the shell be driven from a desktop browser against a host build of the runtime,
which is the only cheap way to exercise any of this without a device. On Android it would
cost an open port every app on the phone can reach, plus a bearer-token scheme to
compensate. It stays as a `NapTransport` implementation behind a development flag.

Two approaches were considered and rejected:

- **`addJavascriptInterface`** — the injected object lands in *every* frame with
  JavaScript enabled. There is no origin scoping and no per-frame control, so the
  napplet's own iframe would hold the bridge and could call the runtime directly,
  bypassing the capability seam entirely. `addWebMessageListener` is the origin-scoped
  successor to precisely this API.
- **`shouldInterceptRequest` as a transport** — tempting, since the machinery exists and
  runs off the main thread, but `WebResourceRequest` exposes no request body. Calls would
  have to be smuggled through the URL and push would need a never-ending `InputStream`
  imitating SSE.

Because origin rules and port ownership *are* the capability, no bearer token is needed on
the device path at all. `androidx.webkit` becomes a new dependency.

### 5.4 The intent bridge

Two planes with the same shape, one layer apart:

| | Android Intent | NAP-INTENT |
|---|---|---|
| Carrier | `android.content.Intent`, intent-filters | `intent.invoke` over `postMessage` |
| Address | URI + action | archetype + convention URI |
| Resolution | OS package manager | runtime, over installed napplets' `archetype` tags |
| Chooser | system dialog | shell UI |

Both mean *some handler, open this payload*. So both use **one resolver**, in Rust, with
Android as an additional entry point rather than a parallel implementation: a link opened
from outside the app and an in-napplet `intent.invoke` must reach identical code.

**Inbound.** A URI arrives by VIEW, NDEF, share sheet, or home-screen shortcut. Myco
normalizes it to `(archetype, convention, payload)`, runs the resolver, and opens the
handler's window with the payload delivered after the handshake completes. The entry point
is `myco://napplet/<naddr>[?params]`, the napplet sibling of the existing
`myco://app/<host>/<path>`. Claiming the `nostr:` scheme, which would let any Nostr URI on
the phone open in a napplet, is deferred (§7.9).

**Outbound.** When no installed napplet handles an archetype, the URI goes to the Android
chooser and another Nostr app can take it; `NAP-LINK` does the same for ordinary external
links. Myco stops being a silo without ever handing a napplet raw intent access — the
runtime issues the Intent, the napplet only asks.

**Convention URIs** are normalized in the shell, because the shell *is* the web binding.
Per the projection: strip the query from the stable identity, percent-decode each unique
`name=value` pair as text, and place those pairs in the payload. No type coercion; `+` is
a literal plus sign. Reject fragments, malformed percent-encoding, repeated names, and a
query combined with an explicit payload — before any message is sent. Routing is exact
equality over the queryless identity: no prefixes, no wildcards, no normalization.

One sequencing consequence: NAP-INTENT delivers its selected convention over
runtime-attested NAP-INC, so the intent bridge brings NAP-INC in with it. They land
together.

### 5.5 Window model

**`NappletActivity` is a new Activity.** `NsiteActivity` does not grow napplet
responsibilities. The two hosts share a look, not a codebase — their intent contracts,
request interception, navigation policy, lifecycle and trust boundaries all differ, and
merging them would put capability plumbing inside the class that renders untrusted nsite
content.

What is genuinely shared is *chrome-less WebView task* plumbing, not nsite behaviour, and
it becomes a helper both call — a helper rather than a base class, so nsite semantics
cannot leak in by inheritance:

- edge-to-edge layout, top inset and IME padding,
- status and navigation bar contrast sniffing, and the black splash,
- the Recents task title and favicon.

What each owns alone:

| | `NsiteActivity` | `NappletActivity` |
|---|---|---|
| Intent contract | `EXTRA_HOST` + deep path | `naddr` pointer + convention payload |
| Task key | host data URI | `(pubkey, dTag)` — see §7.8 |
| Loads | `http://<host>.localhost/` | the shell at `<pubkeyB36><dTag>.localhost` |
| Interception | the nsite gateway, by host | shell assets only |
| Content | the served page and its subresources | verified bytes in a sandboxed `srcdoc` iframe |
| Lifecycle | WebView history | a shell session: open, handshake, capability traffic, teardown |

Origin separation here is a security boundary, not tidiness. The capability channel is
scoped to a shell origin (§5.3), so the nsite WebView client must refuse to serve any
shell origin, and the napplet client must refuse nsite hosts. Otherwise an nsite could
navigate itself into a shell origin and inherit the channel.

Sessions are keyed by `(dTag, aggregateHash, windowId)`. Composing several napplets into
one window is deferred, but hosting the napplet inside a shell page keeps it reachable.

---

## 6. Delivery

Each stage ends in something demonstrable.

### S0 — Foundations

The NIP-5A aggregate hash lands in `nsite-deck`, with nsite manifests verifying it (§4.1).
The new crate gets its skeleton, seams, and a manifest parser for the NIP-5D kinds and
their extra tags. A fixture napplet is built with the napplet Vite plugin in its
single-file mode. Tests reject a bad signature, a blob hash mismatch, an aggregate
mismatch, a multi-file bundle, and a missing `/index.html`.

*Done when* the fixture's aggregate recomputes and matches off-device, and nsites verify
their aggregate for the first time.

### S1 — Render

Resolution runs end to end against the local relay and Blossom. The shell page, iframe
injection, CSP meta and the `shell.ready` / `shell.init` handshake come up.
`NappletActivity` arrives with the shared chrome helper extracted, the per-napplet shell
origin routed, `NapTransport` over `addWebMessageListener` with the `WebMessagePort`
fallback, `androidx.webkit` added, and cross-origin refusal on both WebView clients.
Adding a napplet by `naddr` fetches its manifest and blobs online once, verifies, and
stores. The Apps panel grows a type discriminant and its 🦆 / ＠ annotations, and the
install-time review screen shows `requires` and records grants on the library entry.

*Done when* a real napplet, fetched by `naddr`, renders on a phone and completes the
handshake, with `shell.supports()` answering truthfully from granted ∩ implemented.

### S2 — Publish and subscribe

The user key is generated on first napplet use and persisted beside the device key, never
leaving Rust. The same step publishes a kind 0 for it: a guest profile named
`Myco Guest <5 digits>`, with a link to Myco on Zapstore in the bio, so a new user is
never a bare pubkey and every event they publish carries an invitation.

`NAP-RESOURCE`, `NAP-RELAY` (`subscribe`, `publish`, `query`) and a read-only
`NAP-IDENTITY` come up. Signing is mediated: the napplet asks, Rust signs, no napplet ever
sees a key. A `relay` grant accepted at install covers publishing, with no per-event
prompt (D8) — which means a granted napplet can publish as you at will, so the review
screen has to say so in words a person understands, and revoking a grant has to be
reachable.

Relay access sits behind one resolver with three lanes: the local relay, mesh relays
addressed as `ws://<npub>.fips:4870`, and internet relays when reachable (§7.4).

*Done when* a profile napplet renders a kind 0 and can publish an edit.

### S2b — Intents and deep links

Early rather than late: this is how a napplet gets *reached*, and what makes the Apps
panel feel like a system rather than a list.

The archetype registry indexes installed napplets by role from their manifest tags and
backs `intent.available()`. The resolver opens a sole handler directly, offers a chooser
for several, and falls through to Android for none. NAP-INC lands as the runtime-attested
delivery channel NAP-INTENT needs, then NAP-INTENT itself. On the Android side:
`myco://napplet/<naddr>`, a share-sheet target, home-screen shortcuts, and outbound
hand-off to the OS chooser. The URI-to-archetype table is built here even though `nostr:`
is not yet claimed — the resolver needs the same normalization regardless.

*Done when* a `myco://napplet/<naddr>` link from outside the app and a napplet invoking
`napplet:profile/open?pubkey=…` reach the same window through the same resolver.

### S3 — Fill out the seam

`NAP-STORAGE` (scoped per identity tuple), `NAP-THEME`, `NAP-NOTIFY`, `NAP-LINK`,
`NAP-OUTBOX` and `NAP-CONFIG`.

### S4 — Composition

Several napplets in one window: layout strategy, and INC channels held open between live
napplets. NAP-INTENT and NAP-INC already exist by then; this is about napplets sharing a
surface rather than replacing each other's windows.

### S5 — Mesh as a NAP

Smaller than it first appears, because §7.4 puts publish/subscribe on the standard
contracts already. What remains has no standard equivalent: peer presence, transport and
reachability state, circle membership. It should be specified as a NAP-WORD candidate and
proposed upstream rather than bolted onto an existing domain.

---

## 7. Open questions and hazards

### 7.1 Two identities on one device

Myco has exactly one keypair today, and `own_npub` is both the mesh device identity and
the social one. D3 splits them: the device key keeps signing mesh traffic, pairing and
gossip, and the user key signs only napplet-originated events. The Identity screen must
not conflate them.

Generating the user key lazily means no migration for existing installs — but a user who
already has a Nostr identity has no import path until one is added. Accepted for now.

The guest profile's number is five random digits, drawn once at key generation and
persisted with the key rather than derived from the pubkey. Collisions across the mesh are
expected and harmless: the pubkey is the identity, the number is a label. The kind 0
always goes to the local relay; whether it also goes to the mesh or to internet relays is
deferred, and it must never block a napplet launch. A user who edits their profile through
a napplet overwrites it, bio link included — the link is a default, not a watermark.

### 7.2 Update semantics versus content addressing

A napplet's identity *is* its aggregate hash, so every build is a different identity, while
the library row tracks an addressable `(pubkey, dTag)` pointer. Open: whether storage
carries across versions, what happens to an open window when an update lands, and when a
pinned session re-resolves. Deferred — but §7.8's task-keying choice quietly picks a side,
so it wants revisiting before deep links ship.

### 7.3 Mesh replication of the new kinds

Napplets replicate for free only if the peer-sync filters and gossip paths know about
`5129` / `15129` / `35129` and pull their blobs. A small change, easy to forget, worth an
explicit test.

### 7.4 The outbox model over the mesh

"Offline" is the wrong frame in a mesh. NIP-65 does not require the internet; it requires
*reachable relay URLs* — and Myco already gives every device one: `<npub>.fips`, resolved
over FIPS, serving a relay on `:4870`.

So a kind 10002 relay list can name `ws://npub1….fips:4870` alongside `wss://` internet
relays, and the outbox model works unmodified. Same NIP-65 logic, same NAP-OUTBOX
contract, same napplet code — the URLs simply happen to resolve over the mesh. A napplet
written for the open web works in a room with no internet, and neither the napplet nor the
specification needs to know why.

The work this implies is small: resolve `<npub>.fips` on the Rust side of relay
connections, since napplets never open sockets and `PeerRelayPool` reaches peers by mesh
IPv6 today; publish our own kind 10002 naming our mesh relay so peers can route back;
report per-lane reachability in results rather than failing hard. One question stays open —
whether a napplet may see that a relay is mesh-local, or whether that stays opaque.

### 7.5 The WebView floor

`srcdoc`, `sandbox` and CSP `<meta>` are old features, but Myco targets phones with old
WebViews. `minSdk` is 29; what actually matters is the WebView package version, since
`WEB_MESSAGE_LISTENER` needs WebView 88 or newer and older devices fall back to
`WebMessagePort` (§5.3). Both paths, and `srcdoc` CSP behaviour, need verifying on the
oldest supported device early rather than late.

### 7.6 Shell host labels

`<pubkeyB36><dTag>.localhost` inherits NIP-5A's constraint: a 50-character base36 pubkey
plus a `d` tag of at most 13 characters, so the whole thing fits one 63-character DNS
label. Napplet `d` tags are governed by NIP-5D and need not obey it. A fallback is needed
for a `d` tag that does not fit — most likely a short hash of it — and the mapping must
stay injective, or two napplets share an origin and the partitioning D12 buys is gone.

### 7.7 Inbound intents are untrusted

Any app on the phone can send Myco an Intent. An inbound intent may **open** a napplet and
**carry a payload**; it may never grant a capability, bypass the install review screen, or
cause a publish. An `naddr` naming a napplet manifest routes to install review, never to a
silent install. Payloads reach the napplet as data after the handshake, through the same
path an in-runtime convention takes — no privileged side channel.

One question is unsettled: resolver results and `intent.available()` reveal which napplets
are installed. That is fine inside the runtime and questionable to expose to an arbitrary
calling app, so what an outside caller learns from a failed resolve needs deciding.

### 7.8 Task keying

`NsiteActivity` keys its task by host data URI so re-opening re-surfaces the same Recents
card. A napplet's identity changes on every build. The proposal is to key the **task** on
the addressable `(pubkey, dTag)` pointer so updates reuse the card, while the **session**
pins the aggregate hash. It needs deciding before deep links ship, because a deep link
names the pointer, not the hash.

### 7.9 Claiming the `nostr:` scheme

Deferred. The URI-to-archetype table is still built in S2b, since the resolver needs the
same normalization, but no `nostr:` intent-filter ships yet.

When it returns, the questions are: claim by default or behind a toggle, and what to do
with a URI we have no handler for. Handing it back to the OS chooser can loop straight back
to us without an explicit "not us" marker. Narrow filters enabled per installed archetype
are the best behaviour and the most work, since intent-filters are static in the manifest
and would need component enable/disable at runtime.

### 7.10 Conformance

The napplet conformance suite is browser-shaped. The WebSocket transport in §5.3 exists
partly so the shell can be exercised in a desktop browser against a host build, which is
the only cheap way to run any of it. Passing the suite is not a goal this cycle (D7), but
nothing here should make it impossible later.

---

## 8. Specification pinning

Everything except NAP-SHELL is Draft, and NIP-5D is an open pull request. Expect churn in
NAP-RELAY, NAP-IDENTITY and NAP-OUTBOX in particular — the three S2 depends on.

Per D7, freeze against a specific `napplet/naps` revision and record it here on change.
Kehto, the reference web runtime, pins `5ac0490461ca6fec2f0d2e45b4835cf9bc08de24`; adopting
the same revision keeps the two implementations comparable.

Also recorded, and re-audited on change:

- NIP-5D — `nostr-protocol/nips` PR #2303 (living),
- NIP-5A — `nostr-protocol/nips` master,
- Kehto's runtime specification — read as a reference implementation, not as authority.

One correction worth carrying upstream: the NAP registry README describes a napplet as
"a NIP-5A manifest (a Nostr event, kind 35128)". That names the parent specification and
the parent's kind; napplet manifests are `5129` / `15129` / `35129` under NIP-5D, as every
implementation and the build tooling agree.

---

## 9. References

- [NAP registry](https://github.com/napplet/naps) — capability contracts, archetypes, and
  the web projection
- [NIP-5D](https://github.com/nostr-protocol/nips/pull/2303) — the napplet manifest and
  web binding
- [NIP-5A](https://github.com/nostr-protocol/nips/blob/master/5A.md) — pubkey static
  websites, the aggregate hash, and the manifest shape both specifications share
- Kehto — the reference web runtime, and the closest thing to prior art for the resolution
  pipeline in §4
- uzel — a Linux native runtime; read for shape, not adopted (D1)
