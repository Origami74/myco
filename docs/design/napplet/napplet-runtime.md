# Napplets: a NIP-5D runtime inside Myco

This document is the **app runtime** layer of Myco: a conformant
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

Related docs: [./nsite-layer.md](../nsite/nsite-layer.md) (the content layer this builds on),
[./app-shell.md](../core/app-shell.md) (the per-app window model),
[./deep-links.md](../core/deep-links.md) (the existing `myco://app/…` link),
[./identity-pairing.md](../core/identity-pairing.md) (the device key this deliberately does not
reuse), [../circle/circle.md](../circle/circle.md) (what "everyone nearby" means),
[../reference/nostr-kinds.md](../../reference/nostr-kinds.md) (event kinds).

> Status: built through S2, S3's `NAP-OUTBOX`, `NAP-RESOURCE` (`blossom:`
> only), `NAP-LINK` and `NAP-THEME` (default themes, no picker), and S5. Each
> stage below says what shipped. Not built: S2b (intents, `NAP-INC`), S4
> (composition), the rest of S3 (`storage`, `notify`, `config`). Login with your
> own key is the roadmap's N1.

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
| D3 | Identity | A user key **separate** from the mesh device key, generated as a guest on first launch; the person can log in with their own `nsec`, reveal it, or log out. |
| D4 | Relay scope | Local relay, mesh peers, and internet relays when reachable. |
| D5 | Mesh | Standard NAPs behave exactly as specified. Mesh rides those contracts through `<npub>.fips` relay URLs (§7.4); a Myco mesh NAP covers only what has no standard equivalent. |
| D6 | First milestone | A full verified resolve — manifest, blobs, aggregate, `srcdoc`, handshake. No shortcuts that get thrown away. |
| D7 | Specification drift | Pin one `napplet/naps` revision and re-audit deliberately (§8). |
| D8 | Capability policy | An install-time review screen; grants stored per library entry as two sets — `granted` and `denied` — and switchable per capability on the app's sheet afterwards (live — an open window obeys on its next call, and relaunches). A launch may grant a declared domain this build newly implements, but never one the user switched off: "never decided" and "said no" are different slots — and only if the domain was on the list the review sheet showed; a later manifest declaring more goes back through the review sheet before it gets it. A granted `relay` covers publishing with no per-event prompt, except — interim, until the permission model adds prompts — kinds 0, 3, 5 and 10000–19999, which are refused per call. |
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
3. Recompute the NIP-5A aggregate over the `path` tags alone. Only `path` tags feed it —
   `config`, `requires` and `archetype` do not. The `["x", "<hex>", "aggregate"]` tag is
   *corroboration, not the source*: NIP-5D says the runtime recomputes the aggregate and
   that the tag, **if carried**, must match. A napplet without one still has an identity,
   and it is the same identity it would have had with the tag present. Requiring the tag
   would reject conformant napplets for nothing — the author's signature already covers
   the `path` tags, and every blob is hash-checked against them.
4. Assemble the verified `/index.html` and inject it as `iframe.srcdoc`, carrying the
   `connect-src` policy as a `<meta http-equiv="Content-Security-Policy">` so it survives
   into the iframe's opaque origin.

The napplet's identity is the `(dTag, aggregateHash)` tuple **computed** from those
verified bytes. The runtime assigns it; the napplet never asserts it. Any verification
failure rejects the load outright — no iframe is ever created from unverified bytes.

**Napplets are single-file.** Not a Myco restriction — NIP-5D's Manifest section says it
outright: *"A napplet is a single self-contained `/index.html`."* An opaque origin has
nowhere to resolve a relative subresource to, which is why the build tooling's single-file
mode inlines everything into one `index.html`. A manifest describing a multi-file bundle is
therefore not a napplet, and is rejected where the manifest is parsed — before a blob is
fetched, with an error naming the offending files. The runtime does not inline at load time
to compensate: that would mean assembling bytes the author never signed as a unit, with the
aggregate attesting to a file set nobody ever ran.

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

#### Secure context

`*.localhost` is potentially trustworthy, and the `srcdoc` frame inherits that from the
shell. So the napplet has a secure context: `navigator.clipboard.writeText` works on a
tap inside it, and `crypto.subtle` is available.

Camera, microphone, geolocation and clipboard *read* stay denied, because no
`WebChromeClient` grants them — `NappletActivity` sets none, and that is deliberate.
Keep it that way. Any secure-context API a later WebView adds is reviewed against this
note before the shell is touched.

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

#### The back gesture

A napplet has no history for back to walk. Its frame has an opaque origin, where
`history.pushState` throws, and the shell never navigates. No NAP defines "back"
either. So Myco delivers the system back gesture as what it already means on the web:
an **Escape keydown**, sent into the napplet's document.

The contract for napplet authors:

- **You handled it** (closed a page, a dialog, a drawer): call `preventDefault()` on the
  `keydown`. The napplet stays open.
- **You did not**: leave it unhandled at your top level, and Myco closes the napplet.

This is plain, portable DOM behaviour (Escape = dismiss), so a napplet that already
closes its dialogs on Escape gets back for free. It needs no new NAP.

Blink's own default actions for Escape consume it too, so they count as handled: closing
a modal `<dialog>` (its `cancel`), closing an open popover, a `CloseWatcher`, and
clearing a non-empty `<input type=search>`. A napplet with one of those open goes back
by closing it, without any script of its own.

**Back cannot be trapped.** A napplet that consumes every Escape would otherwise keep
the user in it. Myco offers at most three backs in a row to a napplet that consumes
them; with no touch on the napplet since, the fourth back closes the window without
asking. Three is enough to walk back up a few nested pages; a touch starts the count
over.

How the window tells the two apart: `NappletActivity` dispatches a real
`KEYCODE_ESCAPE` down and up into the WebView. A key the page does not consume comes
back through `WebViewClient.onUnhandledKeyEvent` as the same `KeyEvent` object; an
unhandled key-down from its own back closes the window, and a consumed one is never
reported. The shell keeps keyboard focus in the napplet's frame so the napplet is the
one that hears it. If focus somehow sits in the shell, the shell does not consume
Escape, so back closes the napplet as it always did. Back also closes the window
directly while the napplet is still opening, after a renderer crash, and when the
renderer stays unresponsive after a back. The review sheet and the link dialog take
back before the napplet does; an open keyboard closes first.

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
Adding a napplet by `naddr` fetches its **manifest only** — signature, author and
address checked — and the install-time review screen shows `requires` from it. Nothing
is downloaded before the user says yes: "Add" fetches the blob, verifies it against the
manifest that was reviewed (kept in memory, so a newer version published meanwhile
cannot slip in), stores both, and only then records grants on the library entry. The
Apps panel grows a type discriminant and its 🦆 / ＠ annotations.

*Done when* a real napplet, fetched by `naddr`, renders on a phone and completes the
handshake. Every implemented API is injected whatever was granted, and
`shell.supports()` answers from *implemented* — "does this runtime do relay?" is a fact
about Myco; the permission lives behind the call, where a refusal is one failed action
the napplet can react to rather than a namespace it reads as permanent absence.

### S2 — Publish and subscribe

The user key is generated as a guest on first launch and persisted beside the device key
(§7.1). The same step publishes a kind 0 for it: a guest profile named
`Myco Guest <5 digits>`, with a link to Myco in the bio, so a new user is
never a bare pubkey and every event they publish carries an invitation.

`NAP-RESOURCE`, `NAP-RELAY` (`subscribe`, `publish`, `query`) and a read-only
`NAP-IDENTITY` come up. `NAP-RESOURCE` is `blossom:` only for now (`nap/resource.rs`):
every ask reads this device's Blossom store first; a miss goes through the `BlobFetcher`
seam — the Circle's stores over the mesh, then the public servers unless offline-only —
is verified by hash, **stored**, and only then delivered, so the second ask from any
napplet is local and the room can serve it over the mesh. `mime` is sniffed from the
bytes, never a header; raw SVG is refused (`blocked-by-policy`) for want of a sandboxed
rasterizer — checked over the whole body once the first kilobyte reads as text, not the
first kilobyte alone. Bytes cross the JSON channel as base64 and the shell builds the `Blob` the
vendored shim expects. `https:`, `htree:` and `nostr:` report `unsupported-scheme`. Signing is mediated: the napplet asks, Rust signs, no napplet ever
sees a key. A `relay` grant accepted at install covers publishing, with no per-event
prompt (D8) — which means a granted napplet can publish as you at will, so the review
screen has to say so in words a person understands, and revoking a grant has to be
reachable. Interim, until the permission model has per-event prompts, the one parser
behind `relay.publish`, `outbox.publish` and `mesh.publish` (`sign_template`) refuses
kinds 0, 3, 5 and 10000–19999: a napplet may post as you, not rewrite your profile,
contacts or relay list, or delete your events. The refusal is `ok: false` with the
reason on the `.result` frame. The runtime's own first-use kind 0 and 10002 do not go
through it.

Relay access sits behind one resolver with three lanes: the local relay, mesh relays
addressed as `ws://<npub>.fips:4870`, and internet relays when reachable (§7.4).
`relay.query` reads the whole pool — the local relay and the configured relays unless
offline-only — bounded and deduplicated by id; `relay.subscribe` answers the local backlog,
sends `EOSE`, and pulls the pool into the local relay behind it, so what arrives is
delivered live. `options.relay` targets one relay instead, validated like any
napplet-named URL. Relay selection *by author* is NAP-OUTBOX's (S3).

**Napplet-named relays are shell policy.** NAP-RELAY prescribes `options.relay` (NIP-29
groups are its example) and says "the shell controls which relays the napplet can access";
NAP-OUTBOX says `options.relays` "never bypasses shell ACLs" and napplets "MUST NOT be able
to force connections to private network relays or disallowed hosts". Myco's policy, in one
place (`validate_relay_url` and `OutboxService::allowed`): a `.fips` host is a mesh lane
only when its npub is a Circle member and not our own; loopback, link-local, private and
`.local`/`.localhost` hosts are refused; any other `wss://`/`ws://` host is allowed while
the internet is; offline-only refuses them all. Note what that permits: a napplet with the
`relay` grant can make this phone open a connection to a public relay of its choosing and
put napplet-chosen filter values on the wire — an exfiltration channel the CSP in the
iframe does not close. It is the conformant reading of the specs and a `relay` grant
already lets the napplet publish as the user; tightening it (a relay allowlist, or a
separate grant for naming relays) is a policy knob to revisit with the permission model
(roadmap).

How the URL check works: `validate_relay_url` parses with the `url` crate (no hand-rolled
prefix matching), refuses any userinfo, unwraps a v4-mapped v6 address before judging it,
and counts CGNAT (`100.64/10`) and multicast as private beside loopback, link-local and
RFC 1918; the parser normalises `127.1`, decimal and hex shorthand, so those need no
special case. A `.fips` host is accepted only as `ws://<npub>.fips`, optionally `:4870` or
a bare `/`, and is always dialled as the rebuilt `ws://<npub>.fips:4870` (§7.4). At most
ten relays may be named per call. The outbox resolves an internet lane before dialling
and refuses it when any resolved address is private (the connect re-resolves; a
pre-connected stream is the follow-up).

**Accepted policy**, listed so it is explicit rather than discovered:

- Napplet-named internet relays (`options.relay`, `options.relays`) are an exfiltration
  channel by design — data rides in the URL even when the relay refuses the connection.
- `relay.subscribe` is unscoped: a napplet reads every other napplet's events and
  everything the user has published.
- `relay.publish` signs any kind, except the interim set above (0, 3, 5, 10000–19999).
- All three are roadmap items under the unified permission model.

*Done when* a profile napplet renders a kind 0 and can publish an edit.

### S2b — Intents and deep links

**Not built** beyond `myco://napplet/<naddr>` opening install review and a
share by bump or QR. The archetype registry, `NAP-INTENT` and `NAP-INC` are
still to come.

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

`NAP-STORAGE` (scoped per identity tuple), `NAP-THEME`, `NAP-NOTIFY`, `NAP-LINK` and
`NAP-CONFIG`.

**`NAP-OUTBOX` shipped** (registry draft PR #32, pinned copy in
`reference/naps/drafts/NAP-OUTBOX.md`): `getEvent`, `query`, `subscribe`/`close`, `publish`,
`resolveRelays`. Two seams — `OutboxResolver` (a NIP-65 plan per direction, with `source`
and `missing_authors`) and `LaneTransport` (`query` / `publish` / `pull_into_local` over the
three `RelayLane`s) — implemented by `OutboxService` in `myco-core/src/outbox.rs`. Reads
wait for their lanes, bounded, and say `incomplete` when one never answered. A publish
answers at a quorum (`LaneTransport::publish_quorum`): once the event is stored here and two
other lanes — or the only one — have taken it; the rest finish in the background and are
absent from the result's `relays` map, which lists what answered in time. A publish naming
`toInboxes` waits for every lane, since the spec makes an inbox a required target whose
failure must be reported. A
subscribe answers the local backlog and pulls the remote lanes *into* the local relay, which
is what delivers them live (the spec has no `outbox.eose`, and this is why). Napplet-supplied
relay URLs are validated: `ws`/`wss` only, never loopback or a private network. An author
whose kind 10002 the local store lacks is looked up in the pool once (the configured relays
and every Circle member's mesh relay, bounded), stored — the local relay is the cache — and
a miss remembered for ten minutes; a list older than a day is served as `source: cache` and
refreshed behind the answer. NIP-66 relay intelligence is not used: it is a MAY, and the
offline case has no monitors to ask.

**`NAP-LINK` shipped** (`nap/link.rs`, the standard wire): `link.open {url, options?:
{label?}}` → `link.open.result {status: "opened" | "denied", error?}`. The runtime
crate only *classifies*; the host decides whether to admit, because only the host can
see what is on screen:

- `https:` / `http:` (an authority is required; userinfo, whitespace and control
  characters are `invalid-url`) → `ToShell::OpenExternal`. `NappletActivity` hands it to
  the system browser — directly if the user touched the napplet in the last five
  seconds, otherwise after a one-tap "Open this link?" naming the real host. A
  `postMessage` carries no user gesture, so the recent touch stands in for one.
- `nostr:` or `napplet:` + an `naddr` of kind 35129 / 15129 / 5129 →
  `ToShell::ReviewNapplet`. The window dispatches the same `FetchNapplet` a scanned code
  does and draws the existing install-review sheet (`NappletReviewSheet`, a Compose
  overlay on the napplet's window) **over the running napplet**, so the user keeps their
  place. Nothing on this path installs: "Add" on the sheet is the user's answer, which is
  §7.7's rule — a napplet can never install directly. The review slot is the app-wide
  one, so a sheet dismissed in the napplet is dismissed everywhere.
- Any other scheme, another NIP-19 entity (`npub`, `note`…), or an `naddr` of a
  non-napplet kind → `denied` / `unsupported-scheme`. Malformed → `invalid-url`.

`opened` means "handed to a surface the user answers", not "the user accepted"; a
napplet learns nothing about what happened next. Admission (`LinkGate` in
`myco-core/src/napplet.rs`, device-wide) refuses with `blocked-by-policy` while a review
is already in the slot, within 5 s of the last admitted review link, and within 2 s of
the last admitted web link — so a napplet cannot stack or spam sheets or browser tabs.
`options.label` is untrusted display text and is never shown or used to decide
anything.

**`NAP-THEME` shipped**, default themes only (`nap/theme.rs`): `theme.get` →
`theme.get.result {theme: {colors: {background, text, primary}, title}}`. Two themes
mirror the app palette (`ui/theme/Theme.kt`): *Myco Light* (`#ffffff` / `#0f172a` /
`#059669`) and *Myco AMOLED* (`#000000` / `#ffffff` / `#34d399`). Which one is the
app's light/dark mode, reported per window: `NappletActivity` calls
`nappletSetAppearance` right after open (before the handshake, so the first `theme.get`
is already right) and again from `onConfigurationChanged` — the activity handles
`uiMode` in place rather than restarting the napplet — and a change is pushed as
`theme.changed` to an established session granted `theme`. A session nobody told
answers light. A theme picker is later work.

**Both are default grants.** `link` is user-mediated — every outcome ends at the
browser (behind a touch or a tap) or at the install review, and admission is
rate-limited — so granting it costs the user nothing they do not answer themselves.
`theme` is read-only and tells a napplet no more than its own `prefers-color-scheme`.
Existing installs pick both up at their next open, like any default a build newly
implements; a switch turned off on the app's sheet stays off. The vendored
`@napplet/shim` already installs `window.napplet.link` and `window.napplet.theme`, so
the Myco prelude supplement needed nothing for either.

**`mesh` is a default grant too — for now.** The upstream napplet tooling (the Vite
plugin and `napplet deploy`) keeps only upstream NAP domains in a manifest's
`requires` and drops Myco-only ones, so a napplet published with it cannot ask for
`mesh`, and a grant that can only come from `requires` could never be given.
Until the tooling keeps extension domains, `mesh` is in `DEFAULT_GRANTS`. It is
still listed on the install sheet (NAP-MESH's "acting as the user" warning holds),
capped by the user's App reach setting, and switchable off per app. Existing
installs pick it up at their next open unless the user switched it off — silently,
with no sheet, deliberately: a default is never reviewed, and asking about one would
be a sheet for every installed app at once. Remove it from the defaults once the
tooling is fixed — and note that removal alone does not revoke it: the widened grant
is persisted in each entry's `granted`, so taking `mesh` out of the defaults needs a
migration that drops it where it was never reviewed.

With `mesh` on for every app, the roadmap's
[mesh rate limits](../../roadmap.md#later) item (a per-session token bucket on
`mesh.publish` / `mesh.subscribe`) is more urgent: any installed napplet can now flood
the Circle up to the user's App reach cap.

### S4 — Composition

**Not built.**

Several napplets in one window: layout strategy, and INC channels held open between live
napplets. NAP-INTENT and NAP-INC already exist by then; this is about napplets sharing a
surface rather than replacing each other's windows.

### S5 — Mesh as a NAP

**Shipped as NAP-MESH** ([`NAP-MESH.md`](NAP-MESH.md)), in the registry's template form so
it can be proposed upstream. Narrower than first sketched, and differently shaped: not a
peer directory but **hop-limited publish and subscribe**. A relay publish is "put this on
my relays"; a mesh publish is "flood this to the people near me, this far". The hop budget
is the whole difference, and it is the one thing a napplet may choose here that it may
choose nowhere else — within a cap the user sets (Settings › App reach; publish default 3,
backlog pull default 2, the mesh's own `EVENT_TTL` / `MAX_REQ_TTL`). `mesh.info` reports
only a peer *count*; presence, transports and circle membership stay behind the seam and
would be a separate NAP.

Runtime: `MeshSink` seam (`limits` / `reach` / `publish` / `pull`), `nap/mesh.rs`,
domain-scoped session subscriptions. Core: `NappletMeshSink` over the relay hub and the
Circle pool; `RelayHub::accept_local_with_ttl` and `accept_pulled`. Web projection: the
vendored `@napplet/shim` filters unknown domains, so `assets/myco-prelude.js` installs
`window.napplet.mesh` — and `window.napplet.shell`, which the vendored build also lacks —
after it.

With `mesh` in place, `relay.publish` means what NAP-RELAY says: the shell's relay pool —
this device's own relay (so its live subscriptions hear it) plus the internet relays when
reachable, best-effort and off the napplet's result — and never the Circle flood. The two
domains now say what they mean, and a `relay` grant is no longer a back door to the mesh.
`RelayPoolSink` in `myco-core/src/napplet.rs`. Outbox-model relay selection (NIP-65) is
NAP-OUTBOX's and still to come; until then the pool is the default relay set.

---

## 7. Open questions and hazards

### 7.1 Two identities on one device

D3 keeps two keys apart: the device key signs mesh traffic, pairing and gossip, and the
user key signs only napplet-originated events. Settings keeps them apart too: the account
header at the top is the user key, and "Device name" below it is the device.

**The account** (`account.rs`, roadmap N1). A guest is generated on first launch — and on
the next launch of an install from before this, which had one only after its first
napplet. From the Account page the person can:

- **reveal the nsec**, after a warning never to share it — the one path by which the user
  key leaves Rust (`accountRevealNsec`, outside the reducer so the secret is in no state
  snapshot; the dialog sets `FLAG_SECURE`, and the copy is marked sensitive);
- **log out** — the key leaves the disk and the signer's slot, and every identity call is
  refused until the next login. A marker file keeps the next launch from answering "no
  key" with a new guest. The dialog offers to reveal the key first;
- **log in** again: a new guest, a pasted `nsec` (or hex), or a signer app (NIP-55,
  Amber).

**A signer login** (`external_signer.rs`, roadmap N2) keeps no secret: `user-signer.json`
holds the pubkey and the signer's package. A signature is a request queued in Rust and
carried by Kotlin, which long-polls the queue (`signerNextRequest`) like the napplet frame
pump. Kotlin asks the signer's content resolver first — it answers in the background once
the user chose "remember" — and otherwise shows the signer's approval screen through an
invisible `SignerActivity`, since a request can come from any Myco window. The answer
(`signerRespond`) is not trusted: the event must be the one asked for — same id, same
pubkey — with a valid signature. rust-nostr's `nostr-android-signer` was considered and
not used: it brings gRPC over a socket, a second UniFFI library and a proxy bound to one
activity, where this is one queue on the existing JNI surface.

The napplet signer reads the account's slot on every call, so a login or logout reaches
an open napplet on its next call.

The guest profile's number is five random digits, drawn once at key generation and
persisted with the key rather than derived from the pubkey. Collisions across the mesh are
expected and harmless: the pubkey is the identity, the number is a label.

The guest picture is the Myco logo, bundled as one JPEG and re-tinted with a two-hue
gradient drawn from the pubkey (`guest_avatar.rs`), so guests look different at a glance.
The output is deterministic, so its hash never needs storing. It goes to the local
Blossom at once and to a few public Blossom servers (BUD-02) when online; `picture`
names the first server that took it, by hash.

The kind 0 and relay list go to the local relay at once — the configured custom relay, when
there is one, where the first-use profile used to stay unpublished — then to the public
relays. Offline,
the account retries with backoff and the sidecar (`user-guest.json`) remembers the profile is
pending across launches. None of it blocks anything. Guests from before this are not
re-published. A user who edits their profile through a napplet overwrites it, bio link
included — the link is a default, not a watermark.

### 7.2 Update semantics versus content addressing

A napplet's identity *is* its aggregate hash, so every build is a different identity, while
the library row tracks an addressable `(pubkey, dTag)` pointer.

Settled the way nsites settle it (`nsite-updates.md` §1): the version **served** is the
one whose index blob is here, pinned through the content layer's active-version map
(`ManifestStore` in `napplet.rs`, `Content::set_active` behind it). A newer manifest
landing in the relay with no blob behind it — pulled by a subscription, flooded by a peer
— does not displace the one that opens. The pin moves when a fetch brings the new bytes:
"Check for updates" refreshes every installed napplet from its pointer's relays beside
the nsite check (`NappletHost::refresh`, bytes first, manifest, then pin), and a version
it brings in is passed on to the Circle, as an nsite update found by the check is (§7.3).
A newer version pushed by a Circle peer moves the pin the same way (§7.3). The tile
reports `ready` or `missing` from the same pin. The same check also runs on its own, on
foreground and every 6 h, through the throttle in `nsite-updates.md` §3.1; under
offline-only the napplet half is skipped (counted as checked, none updated). An open
window keeps its session — it
pinned the aggregate at open — and sees the new version at its next launch. The task
outlives a trip to the background, so re-opening the napplet from Apps or Recents
brings back that same window, still on the old version. So when the window returns to
the foreground, `NappletActivity` asks the core whether the served version has moved
past the one its session opened (`NappletHost::newer_version`: a later-or-equal
`created_at` with a different aggregate; the pinned version, so its bytes are here).
If it has, a dialog offers "Restart" — the same relaunch a grant change uses, a new
session on the new version — or "I'll restart later". It is offered once per napplet
and version: shown, the version is recorded in the app's preferences
(`napplet_restart_prompts`, keyed by shell host) and not offered again, even after the
process dies; a later update asks again. Never on a fresh open, and never over the
install-review sheet or a link confirmation. "Later" lasts only until the next
configuration change the manifest does not absorb (a rotation, but not a `uiMode`
switch): that recreates the window, which opens the version now served. Storage
across versions is still open: napplets have no storage capability yet.

### 7.3 Mesh replication of the new kinds

**Updates travel the Circle the way nsite updates do.** The gossiper routes the napplet
manifest kinds `15129` / `35129` to their own handler (`Content::on_napplet_manifest_event`)
instead of plain-flooding them. It is the nsite policy
([nsite-updates.md §4](../nsite/nsite-updates.md)) — the same hop budget, split-horizon
and multihop clamp — with the napplet's checks in front:

- **Checked first, on every phone.** The manifest must be signed and parse as NIP-5D
  (one `index.html`, an aggregate over its paths). One that does not, or that is older
  than the version the relay already keeps for its slot, stops here.
- **Not installed here** (no Library entry for that author and `d` tag): passed on at
  once and nothing is fetched — a pure relay, as for an nsite we don't run. Holding no
  bytes costs the next phone nothing: it asks the sender, gets a 404, and falls back to
  the public servers (nsite-updates.md §4.2).
- **Installed, not newer than the pinned version:** a downgrade or a replay. Not
  fetched, not pinned, not passed on.
- **Installed and newer:** the index blob is fetched from the sender's mesh Blossom
  first, then the public Blossom servers unless offline-only; verified against the
  manifest; stored; pinned — and only then passed on, so the next phone can fetch the
  bytes from this one. A download that fails still passes the manifest on, so the wave
  never stalls on one phone; the old version keeps serving.

A napplet's pin never moves back — every pin (open, install, "Download again", the update
check, a push) refuses a version older than the pinned one and accepts an equal one — so
two versions downloading at once cannot leave the older one served, and a window opening
on the old version cannot re-pin it. Equal is allowed because the real re-pins are of the
version already served. The blob fetch is capped at NAP-RESOURCE's per-blob limit, so a
sender cannot stream an unbounded body before the hash check. The version activates as
§7.2 says: an open window keeps its session, the next launch opens the new one.

**An update never widens grants.** Author and `d` tag are the address, and the
signature ties a manifest to its author — a napplet by someone else under the same name
is a different napplet, relayed like any other this phone lacks. Nothing on this path
writes grants: a new version declaring a domain the user never reviewed opens without it,
and the domain comes back as unreviewed for the review sheet (`open_with`).

**Snapshots (`5129`) stay plain events.** A snapshot is an immutable build: nothing in the
Library installs by it, so there is nothing to update, and the not-installed branch
would pass it on unchanged anyway.

Still not done: first delivery to a phone that has never had the napplet. It arrives by
the share handoff (bump, QR) and the public relays; a Circle Discover listing for
napplets is a roadmap item.

### 7.4 The outbox model over the mesh

"Offline" is the wrong frame in a mesh. NIP-65 does not require the internet; it requires
*reachable relay URLs* — and Myco already gives every device one: `<npub>.fips`, resolved
over FIPS, serving a relay on `:4870`.

So a kind 10002 relay list can name `ws://npub1….fips:4870` alongside `wss://` internet
relays, and the outbox model works unmodified. Same NIP-65 logic, same NAP-OUTBOX
contract, same napplet code — the URLs simply happen to resolve over the mesh. A napplet
written for the open web works in a room with no internet, and neither the napplet nor the
specification needs to know why.

The work this implied is done with NAP-OUTBOX (S3): a `.fips` URL in a relay list becomes a
`RelayLane::Mesh` and is reached through `PeerRelayPool` — and only ever as
`ws://<npub>.fips:4870` rebuilt from its npub, never as the string given, so a list or a
napplet cannot smuggle userinfo, a path or another port into the pool's dial; the user's own kind 10002 — the
configured relays — is published beside the guest profile on first launch, so the
user's own outbox plan resolves as NIP-65; per-lane reachability is reported
(`incomplete`, the per-relay map on publish — for the lanes that answered by the quorum)
rather than failing hard. Policy lives in one
place (`outbox.rs`): a mesh relay is a lane only if its npub is a Circle member, our own is
never one, and internet lanes go when offline-only.

The user's own list names **no** `.fips` relay. A `ws://<npub>.fips` URL is the device key,
and the list is signed by the user key: one inside the other is the link D3 keeps apart,
published in an event anyone may keep. Circle members reach this device's relay by policy
— they are paired, they know the device — and need no tag to say so. A peer's list *may*
name its `.fips` relay if that peer chooses to; Myco reads such lists, and does not write one.

The open question is answered by not hiding it: a mesh relay's URL is a `ws://<npub>.fips`
URL, and `relayHints` and `resolveRelays` show it as such. A napplet cannot do anything with
the address that it could not already do by asking — it still never opens a socket — and
hiding it would make the plan a lie about where an event was seen.

What NAP-OUTBOX does **not** do is flood. A mesh lane is one directed connection to one
peer's relay, exactly as a `wss://` lane is; reaching everyone nearby with a hop budget is
NAP-MESH's (S5), behind its own grant.

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
silent install. The same holds for a napplet's own `link.open` (S3's NAP-LINK): it can
put the review in front of the user, never past them. Payloads reach the napplet as data after the handshake, through the same
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

### 7.11 Pulling blobs over the mesh leaks what you are looking at

**Open.** NAP-RESOURCE (S2) fetches a `blossom:` blob the local store lacks from every
reachable Circle member at once, then from the public servers. That is the right order
for availability — the room is what this app is for — and the wrong one for privacy, in
three ways.

**The ask is a disclosure.** "Do you have `<sha>`?" tells every phone in the room which
picture or file you are opening, tied to your device npub, at that moment. A public server
learns the same, but a public server is a stranger; a Circle member is someone who knows
you, and the hash is trivially matched to the event that referenced it — they hold the
same event. Interest in a specific message's attachment is a specific fact about you.

**The keep is a broadcast.** "Anything queried is saved" means a blob you merely viewed is
now served from your phone to anyone in your Circle who asks (the mesh Blossom gates on
Circle membership, not on why you hold it). You become a host of content you did not
publish and may not endorse, and a peer probing your store learns what you have looked at.

**The serve is a probe.** The inverse of the first point: answering "yes" reveals what you
hold, to anyone paired with you, for the cost of a hash. Hashes of well-known files are
well known.

None of this is new to the mesh — nsite sync pulls blobs from peers too — but an nsite's
blobs are an app someone chose to install, and a napplet's are the attachments of whatever
its feed happens to contain. The scale and the specificity are different.

Options, roughly in order of how much they cost:

1. **Ask the peer who told you.** A blob referenced by an event that arrived from a peer
   can be asked of *that* peer without telling them anything they do not know — they sent
   the reference. This is the `holder` pattern nsite sync already uses. Anything else goes
   to the internet, or nowhere.
2. **Internet first when online, mesh only when not.** Cheap, and it inverts the current
   order; costs mesh availability for the online case, and still discloses offline.
3. **Do not keep viewer-only fetches, or keep them unserved.** A "fetched for a napplet"
   mark that the mesh Blossom refuses to serve until the user shares or the blob is
   referenced by something they published. Breaks "anything queried is saved" as a mesh
   promise, keeps it as a local cache.
4. **A per-napplet or per-Circle switch.** Honest, and one more thing nobody sets.

Recommendation: (1) as the default — it is the one that leaks nothing new — with (2) as the
fallback path, and (3) worth doing regardless, since it closes the serve-side leak for
content the user never chose to host. Until decided, `BlossomFetcher` stays as shipped and
this section is the warning label. See also NAP-RESOURCE's own note that sidecar prefetch
"can leak user interest to resource hosts": the mesh makes the hosts your friends.

---

## 8. Specification pinning

Everything except NAP-SHELL is Draft, and NIP-5D is an open pull request. Expect churn in
NAP-RELAY, NAP-IDENTITY and NAP-OUTBOX in particular — the three S2 depends on.

Per D7, freeze against a specific `napplet/naps` revision and record it here on change.

- `napplet/naps` — master at `a040914` (2026-09-15), checked out at
  `reference/naps`; the drafts this runtime implements are open pull requests,
  pinned as copies in `reference/naps/drafts/`: NAP-RELAY (#2), NAP-OUTBOX
  (#32), NAP-RESOURCE (#13). NAP-MESH is Myco's own, in this tree.
- `@napplet/shim` — `0.29.2`, vendored verbatim (`assets/vendor/README.md`), with
  Myco's supplement (`assets/myco-prelude.js`) for `shell` and `mesh`, and one
  override: `outbox.publish` (see below).
- NIP-5D — `nostr-protocol/nips` PR #2303 (living), read 2026-08-19 at blob `2e8fcc4657`.
- NIP-5A — `nostr-protocol/nips` master.
- Kehto, the reference web runtime (`reference/kheto-web`), pins
  `5ac0490461ca6fec2f0d2e45b4835cf9bc08de24` of the registry — read as a
  reference implementation, not as authority.

One correction worth carrying upstream: the NAP registry README describes a napplet as
"a NIP-5A manifest (a Nostr event, kind 35128)". That names the parent specification and
the parent's kind; napplet manifests are `5129` / `15129` / `35129` under NIP-5D, as every
implementation and the build tooling agree.

### The publish-timeout override

The vendored `outbox.publish` rejects with "outbox.publish timed out" after 30 s. A
publish is signed before it is answered, and with a signer app (NIP-55, Amber) signing is
a person reading an approval screen — often longer than 30 s. The napplet then saw a
failure while the user was still approving, and the event could go out anyway.

The supplement replaces `window.napplet.outbox.publish` with a copy that posts the same
`outbox.publish` envelope and resolves the same object (`ok`, `event`, `eventId`,
`relays`, `error`), but waits **5 minutes** (`SIGNING_TIMEOUT_MS`, mirrored as
`myco_napplet_runtime::SIGNING_TIMEOUT`). `mesh.publish` gets the same wait. The vendored
router ignores our results: it resolves only ids in its own pending map. `relay.publish`
and `relay.publishEncrypted` need nothing — the vendored ones set no timer.

The 5 minutes is a backstop, not the bound. The host bounds signing itself: the signer
app gets 120 s (`external_signer::ANSWER_TIMEOUT`), then the relay write a few seconds, and
the napplet gets an answer either way — the event, or `ok: false` and why. The backstop
sits well clear of that so it never fires first; a test in `external_signer.rs` holds the
order. The vendored file stays untouched, so re-vendoring keeps the override working as
long as the wire does. The `prelude.rs` tests check the source text, not behaviour (no JS
engine in the test tree): they fail if the vendored `outbox.publish` no longer uses the
30 s constant, which is the cue to re-check the envelope and whether the override is
still needed. Like the vendored send, the override retries a message the structured clone
refuses (a template held in a reactive Proxy) as a JSON snapshot.

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
