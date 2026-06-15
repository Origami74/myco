# The nsite Layer: Embedded Relay, Blossom, and Gateway

This document proposes the **content layer** of Myco: the embedded Nostr
relay, the embedded Blossom blob server, and the localhost HTTP gateway that
turns an author's `npub` into a browsable "app" in the WebView. Myco never
authors, signs, or publishes nsites itself; it **stores, serves, and replicates**
sites that were authored *elsewhere* by external nsite tooling. It is the layer
that makes `http://<npub_author>.nsite` resolve to a static site, fetching that
site's already-signed manifest and content-addressed blobs from a reachable peer
over FIPS the first time, then serving it from a local cache forever after —
including fully offline.

It mirrors the nsite-deck model (see
[../../reference/site-deck/docs/sync-architecture.md](../../reference/site-deck/docs/sync-architecture.md)
and [../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)),
re-implemented in Rust and re-pointed at FIPS peers instead of public relays.

See the component view ([diagrams/05-nsite-layer-architecture.svg](diagrams/05-nsite-layer-architecture.svg))
and the browse-request lifecycle ([diagrams/04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg)).

Related docs: [./propagation.md](./propagation.md) (how a cached site becomes a
new source and hops device-to-device), [../reference/nostr-kinds.md](../reference/nostr-kinds.md)
(event kinds), [../reference/ports.md](../reference/ports.md) (localhost ports
and how each is exposed over FIPS).

---

## 1. Where this sits in the stack

```
WebView (no URL bar)
   │  http://<npub_author>.nsite   /   http://<pubkeyB36><dTag>.nsite
   ▼
Local gateway (HTTP)  ──────────────────────────┐
   │  resolve host → siteKey                     │
   │  manifest event → <path> → sha256           │
   │  serve blob direct from local Blossom        │
   │  blobs missing? ↓ (still syncing)           │
   ▼                                             │
Embedded relay (ws://localhost:4869)             │  all in-process,
   │  manifest event: kind 15128 / 35128         │  across the
   ▼                                             │  myco-* crates
Embedded Blossom (http://localhost:24242)        │  (one .so)
   │  blobs by sha256                            │
   ▼                                             │
Sync engine ── over FIPS ──► reachable holder ───┘
   <npub_holder>.fips:4869 (relay)  ·  <npub_holder>.fips:24242 (blossom)
```

### The crate workspace

The native library is built from **four crates**, split along **reuse
boundaries** so the genuinely reusable piece stays free of any particular relay,
blob store, or radio:

- **`nsite-deck`** *(reusable core)* — the app-agnostic **nsite host**: the
  gateway (§3) and the sync engine (§2.4). This is the genuinely novel,
  reusable know-how — manifest resolution (kind 15128/35128 → `path → sha256`),
  serving an nsite under `<host>.nsite`, and orchestrating sync. It must know
  **nothing** about FIPS, BLE, Android, *or* any concrete relay/Blossom: it is
  "an embedded nsite host" and no more.
- **`myco-relay`** *(reusable primitive)* — a generic embedded Nostr relay: the
  event store + the `ws://localhost:4869` server. Useful to any Nostr app.
- **`myco-blossom`** *(reusable primitive)* — a generic embedded Blossom blob
  store: the content-addressed store + the `http://localhost:24242` server.
  Useful to anything that needs Blossom (notably: there is **no good Android
  Blossom app**, which is exactly why this is worth having standalone).
- **`myco-core`** *(app crate)* — the thin Myco-specific glue: the FIPS
  endpoint, `AndroidBleIo`, the JNI-JSON FFI reducer, identity/pairing, and the
  wiring that picks which backends to plug in.

`nsite-deck` reaches everything outside itself through **four trait seams**,
mirroring how `fips-core` abstracts its radio behind `BleIo`:

- **storage seams** (provided by `myco-relay` / `myco-blossom`):
  - **`RelayBackend`** — store/query manifest events. The default is the embedded
    `myco-relay`; **Citrine-forward is just an alternate `RelayBackend`** (§2.1).
  - **`BlobStore`** — `get` / `has` / `put` blobs by sha256, backed by
    `myco-blossom` (§2.2).
- **transport seams** (provided by `myco-core` over FIPS):
  - **`PeerSource`** — *pull / reconcile*: `fetch_manifest(author, dTag)`,
    `fetch_blob(sha256)`, and `reconcile(filter)` (negentropy / NIP-77 set
    reconciliation — see §2.4) against some reachable source. The sync engine
    (§2.4) calls these; it does not care *how* the bytes arrive.
  - **`FanoutSink`** — *push*: `broadcast(event)` to connected peers. The
    **nsite-deck propagator** (§2.1) drives this — it subscribes to the relevant
    relays and re-publishes accepted manifests to peer relays; the seam does not
    know who the peers are or what carries the bytes. The relay itself never
    fans out (it is a plain NIP-01 store + socket).

So `myco-core` is the only crate that names FIPS *or* a concrete relay/Blossom: it
instantiates `myco-relay` + `myco-blossom`, hands them to `nsite-deck` as the
`RelayBackend` / `BlobStore`, implements `PeerSource` / `FanoutSink` over FIPS, and
binds the relay/Blossom ports into the mesh (`<npub>.fips:4869` / `:24242`). All
four crates cross-compile into the single `libmyco_core.so`. Kotlin owns the radio
and the TUN; Rust owns the content layer and the mesh. See
[diagrams/01-system-layering.svg](diagrams/01-system-layering.svg) and
[diagrams/05-nsite-layer-architecture.svg](diagrams/05-nsite-layer-architecture.svg).

> **Why relay + Blossom are their own crates.** A Nostr relay and a Blossom
> server are generic primitives — nothing about them is nsite-specific — so
> keeping them *out* of `nsite-deck` is what lets `nsite-deck` stay a clean,
> reusable nsite host, and lets the relay/Blossom be reused on their own. The
> cost is two more crates in the workspace; each boundary is one the pluggable
> backend already needed, so none of it is speculative.

> **Why Rust, not Go.** The reference nsite-deck is Go (Khatru relay +
> Khatru/Blossom). The locked decision is to re-implement relay + Blossom in
> Rust (as `myco-relay` / `myco-blossom`) so there is **one** cross-compiled
> `.so` and **one** FFI surface, rather than embedding a second Go runtime. The
> Go reference remains the behavioural spec; the Rust port must match its wire
> behaviour (kinds, manifest tags, URL scheme).

> **Naming note.** The `nsite-deck` crate shares the name *nsite-deck* with the
> Go reference project it descends from; they are different codebases (Go
> reference vs. our Rust crate). **TBD / open** whether to publish it under a
> less overloaded crate name.

---

## 2. Proposed Rust implementation

> Everything in this section is **proposed**, not chosen. Crate names are
> candidates; the load-bearing requirement is the *behaviour*, not the
> dependency.

### 2.1 Embedded relay

A NIP-01 relay with a local event store, listening on `ws://localhost:4869` — the
**`myco-relay`** crate, which implements `nsite-deck`'s `RelayBackend` seam.
The Go reference uses [Khatru](https://github.com/fiatjaf/khatru) over a BoltDB
event store and advertises NIPs 1, 9, 11, 12, 15, 16, 20, 33
([embedded.go](../../reference/site-deck/internal/relay/embedded.go)). The Rust
port needs the same minimal surface: accept `EVENT`, answer `REQ` with a stored
event set, honour `CLOSE`, and serve a NIP-11 document — plus, net-new vs. the Go
reference, speak **negentropy ([NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md))**
(`NEG-OPEN` / `NEG-MSG` / `NEG-CLOSE`) for set reconciliation (§2.4) and advertise
NIP-77 in its NIP-11.

**Embedded from day one — relay *backend* is a pluggable seam.** The relay and
Blossom are both **bundled and in-process from day one**; they are simple enough
that there is no "forward to an external relay first, embed later" phase. The
relay *backend*, however, is a **pluggable seam** (the `RelayBackend` trait): the **default** is the
embedded store described here; an **optional** backend **forwards to a local
relay app on the device — e.g. [Citrine](https://github.com/greenart7c3/Citrine)** —
for developers who already run one. The embedded backend is the default and the
earliest path; forwarding is a configuration choice on top of it, not a
prerequisite. (Blossom has no such seam — see §2.2.)

**Proposed candidates / patterns:**

- An `nostr-rs-relay`-style store: a SQLite (or redb) backed event store with a
  filter index over `kind`, `authors`, and `#d`. The query surface the gateway
  and sync engine actually use is tiny — `{kinds, authors, #d, limit}` — so a
  hand-rolled store over `rusqlite` is viable and avoids pulling a full relay
  framework.
- The [`nostr` / `nostr-sdk`](https://github.com/rust-nostr/nostr) crates for
  event types, signature verification, NIP-19 (`npub`) encode/decode, and the
  relay-message wire format.
- An `axum` (or `tower`) WebSocket handler for the relay socket and the Blossom
  HTTP routes, sharing one Tokio runtime with the FIPS endpoint.

**Replaceable-event semantics matter.** Kind `15128` is replaceable (one per
pubkey); kind `35128` is parameterized-replaceable (one per `(pubkey, d-tag)`).
The store MUST keep only the newest event per slot, matching the dedup the Go
sync does by `(kind, d-tag)`
([service.go `deduplicateManifests`](../../reference/site-deck/internal/sync/service.go)).

**Propagator fanout (multiplex to connected peers).** The embedded relay is a
**plain NIP-01 store + socket** — it never fans out. Fanout is done by a separate
**propagator process inside `nsite-deck`**: it **subscribes** to the relevant
relays (the local relay plus every connected peer's relay) and, when a new Nostr
manifest arrives — whether pushed in by a peer or pulled in by the sync engine —
it **publishes that event to every other connected peer** (an `["EVENT", …]` to
each `<peer>.fips:4869`), excluding the peer it came from. This is what makes the
"announce widely" half of propagation push-based and automatic instead of a
flood the app has to orchestrate (see [./propagation.md §2](./propagation.md)).
Constraints:

- **Events fan out; blobs do not.** Only Nostr events (in practice the small,
  self-authenticating manifests) are multiplexed. Blossom blobs are *not* events
  and stay **pull-only** — fanning megabytes to N peers would saturate a BLE
  link.
- **Source-excluded + de-duplicated.** A seen-set keyed by the 16-byte
  SHA-256 event id (and a hop budget, default TTL 5) prevents echoing an event
  back to its sender or re-flooding one already forwarded. Same dedup the
  propagation layer uses ([./propagation.md §5](./propagation.md)).
- **Re-emitted unmodified.** Forwarded events keep the author's signature intact;
  the propagator never re-signs. Forwarding an already-signed event is normal
  publish behaviour, not authoring.

This is wanted **early** (roadmap P3 — see [../reference/config.md](../reference/config.md)
`[propagation] fanout`), so two connected nodes gossip events both ways as soon
as a FIPS link exists, with the heavier transitive-discovery and scale-dedup
machinery layered on later (P5). See [diagrams/07-relay-mesh-fanout.svg](diagrams/07-relay-mesh-fanout.svg).

### 2.2 Embedded Blossom

A content-addressed blob server on `http://localhost:24242` — the **`myco-blossom`**
crate, implementing the `BlobStore` seam — **always embedded**
— unlike the relay, Blossom has **no pluggable forward-to-an-app backend**,
because there is no good Android Blossom app to forward to, so embedding is the
only sensible path. It implements the
[BUD-01](https://github.com/hzrd149/blossom/blob/master/buds/01.md) surface the
gateway and sync need:

- `GET /<sha256>` → blob bytes (+ `Content-Type`, `Content-Length`).
- `PUT /upload` → store blob, key by `sha256(body)`.
- `HEAD /<sha256>` → existence check.

The Go reference stores blobs as files named by their hash under a `blobs/`
dir, with a BoltDB metadata index, and **disables auth** for the local server
([embedded.go](../../reference/site-deck/internal/blossom/embedded.go)). The
Rust port keeps the same shape: blobs on the filesystem keyed by sha256, a small
metadata index, no auth on localhost.

**Proposed candidates / patterns:** a thin `axum` handler over a
filesystem blob store; sha256 via `sha2`; reuse the same `rusqlite`/redb handle
as the relay for the metadata index. Blobs are immutable and self-authenticating
(the hash *is* the identity), so the store needs no per-blob signature check —
only a `sha256(bytes) == name` verify on read/write.

### 2.3 The gateway

An HTTP server on localhost that the WebView talks to. It does host resolution,
the cache hit/miss decision, the loading page, and (on miss) drives the sync
engine. This is the Go `gateway` package
([handlers.go](../../reference/site-deck/internal/gateway/handlers.go)),
re-implemented in Rust. The request lifecycle is §4.

### 2.4 The sync engine

Pulls a manifest + its blobs from a source, verifies every blob's sha256, and
mirrors them into the **local Blossom + relay** so the gateway can serve them
direct (§4). The Go reference is
[service.go](../../reference/site-deck/internal/sync/service.go); two Myco
changes: **the source** is a reachable FIPS peer instead of public
relays/Blossom (§5), and v0 **does not** build version directories or do an
atomic swap — it serves direct from the content-addressed store, deferring the
reference's htdocs/version-dir machinery to the roadmap (§4.4).

**Set reconciliation (negentropy / NIP-77).** Rather than blindly re-pulling a
peer's full manifest set, the sync engine reconciles **event** sets with a
connected peer using **negentropy ([NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md))**
— range-based reconciliation that settles "which manifests do you have that I
don't" in ~log bandwidth, which matters on a constrained BLE link. It runs over the
peer's relay (`NEG-OPEN` on a manifest filter → `NEG-MSG` rounds → the missing
ids), then the diff is fetched as usual; **blobs stay content-addressed
pull-by-sha256 and are not part of reconciliation**. The `negentropy` Rust crate is
already in the rust-nostr dependency graph (used by `nostr-relay-pool`), so this is
a proven dependency, not new R&D. This is the **pull / catch-up** counterpart to the
push-based propagator fanout (§2.1): the propagator gossips newly-accepted manifests
live; negentropy reconciles whatever was missed while disconnected or on first
contact. A basic reconcile lands in **roadmap P3** (alongside fanout); scale
hardening is **P5**.

---

## 3. The manifest model and URL scheme

### 3.1 Manifest

An nsite is described by a single **manifest event** — a signed Nostr event
whose tags map absolute paths to blob hashes. There is no separate per-file
event (that was the legacy kind `34128` model; Myco does not use it).

- **Root site** — kind `15128`, no `d` tag. One replaceable event per pubkey.
- **Named site** — kind `35128`, with a `d` tag = site identifier. One per
  `(pubkey, d-tag)`. Think of these as sub-apps under one identity.

Each path is a tag of the form:

```
["path", "/index.html", "<sha256-of-the-file>"]
```

Optional tags: `["server", "<blossom-url>"]` (hints — Myco largely ignores
these in favour of peer Blossom, see §5), `["title", …]`, `["description", …]`,
`["source", "<repo-url>"]`. The site icon is whatever the manifest maps at
`/favicon.ico`.

Full tag layout and a worked example: [../reference/nostr-kinds.md](../reference/nostr-kinds.md).
The site → manifest → blobs data model is drawn in [diagrams/06-nsite-data-model.svg](diagrams/06-nsite-data-model.svg).
Protocol source: [../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md).

### 3.2 URL scheme — host is the identity

The site is addressed entirely by its hostname; there is no URL bar and the path
component behaves like any static host.

| Site type | Host label |
| --- | --- |
| Root | `npub1…` (NIP-19 bech32 of the author pubkey) |
| Named | `<pubkeyB36><dTag>` — 50-char base36 pubkey directly followed by the d-tag |

`pubkeyB36` is the raw 32-byte pubkey in lowercase base36, always exactly 50
characters; `dTag` matches `^[a-z0-9-]{1,13}$` and must not end with `-`. The
50+13 fits inside a single 63-char DNS label, avoiding wildcard-cert and
multi-level-subdomain problems. The Go encoder/decoder and the matching regex
are in [base36.go](../../reference/site-deck/internal/gateway/base36.go); the
Rust port reproduces them exactly.

**Host suffix.** Like nsite-deck, Myco serves under `<host>.nsite` — an A
record to `127.0.0.1` provided by the DNS interceptor (`*.nsite → 127.0.0.1`).
The browser reaches the localhost gateway over IPv4 — which is why it works in
**any** WebView including Chromium, where IPv6-only `.fips` resolution is
suppressed. The browser **never** resolves `.fips`; only the native sync engine
does (§5, and [../reference/ports.md](../reference/ports.md)).

**Resolution into a `siteKey`.** The gateway strips the host suffix and resolves
the leading label(s) into `(npub, identifier)`, then forms a cache key —
`"npub"` for root sites, `"npub:identifier"` for named sites (the same
`npub:dTag` convention used for the manifest slot in
[../reference/nostr-kinds.md](../reference/nostr-kinds.md)). The Go
`resolveStem` walks alias chains and canonical named-site labels with a depth
bound ([handlers.go](../../reference/site-deck/internal/gateway/handlers.go));
the Rust port keeps the same logic. An unresolvable host bounces the browser to
the Library UI's "not in your Library" page rather than surfacing a raw error.

> **Locked — host suffix.** The WebView/browser loads nsites under `<host>.nsite`
> **only**; `.localhost` is **dropped**. Custom subdomains of `.localhost` are not
> universally honoured, so the `*.nsite → 127.0.0.1` interceptor is the path — it
> is also what the reference already implements. **Locked.**

---

## 4. The gateway flow: resolve → manifest → blob → serve

The browse-request lifecycle, per
[diagrams/04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg).

> **v0 serves DIRECT from the local relay + Blossom.** There are no version
> directories, no `current/` pointer, and no atomic swap in v0. Each request
> resolves the manifest from the local relay, maps `<path> → sha256`, fetches
> that blob from the local Blossom, and serves it with a content-type inferred
> from the path extension. The reference's path-named **htdocs** serving cache
> (write blobs out as files under a `current/` dir) is a **later speed
> optimization, deferred to the roadmap** — not the v0 serving model (§4.4).

### 4.1 Resolve and normalize

1. WebView requests `http://<host>.nsite/<path>`.
2. Gateway resolves `host → (npub, identifier) → siteKey`.
3. Normalize the path: `/` and any directory path fall back to
   `…/index.html`; an extensionless path is treated as a directory. (Matches the
   spec's index fallback.)

### 4.2 Serve direct from the content-addressed store — the v0 path

If the site's manifest is in the local relay, serve the requested path direct,
per request:

1. **Look up the manifest event** for `siteKey` in the local relay
   (kind `15128` / `35128`, newest per slot).
2. **Map `<path> → sha256`** from the manifest's `["path", …, <sha256>]` tags.
3. **Fetch that blob** from the local Blossom by hash (`GET /<sha256>`).
4. **Serve it** with a `Content-Type` **inferred from the path extension** (the
   manifest is path→hash only; the extension is the type signal), plus
   `Content-Length`. Update the last-accessed timestamp for LRU.

If the path is not in the manifest, fall back to the site's `/404.html`, then a
gateway 404.

**"All referenced blobs present?" check.** Before serving a site, the gateway
verifies that **every** blob the manifest references is present in the local
Blossom (a `HEAD /<sha256>` per tag, or an index lookup). If any are missing the
site is **still syncing** — the gateway shows the loading page (§4.3) rather than
serving a half-present site. Once all referenced blobs are present the site is
servable direct, instantly, with no further network — including fully offline.

### 4.3 Manifest missing or site incomplete — fetch, then serve

If the manifest is absent, or the "all blobs present?" check fails, the gateway
runs a single-flight sync (`startOrJoinSync`: at most one sync per `siteKey`,
late requests join the in-flight one), then:

- **Sync completes within ~1s** → serve direct (§4.2).
- **Sync takes longer than ~1s** → return a **loading page** (HTTP 503 with a
  spinner) that subscribes to the local relay for the manifest (to show
  title/description as soon as it arrives) and polls a status endpoint; the page
  renders the status copy below and, when the status flips to `ready` (manifest
  stored + all referenced blobs present), redirects to the real content. This is
  the `writeLoadingPage` / `handleLoadingStatus` pair in the reference.

The status endpoint returns the FFI `siteStatus` shape (`{ state, filesPulled,
filesTotal, message }`, see [../reference/ffi-surface.md](../reference/ffi-surface.md)).
The loading/error page renders one row per state, with the exact user-facing copy
and action:

| `state` | Page copy | Action |
| --- | --- | --- |
| `syncing` | `Getting <title>… <n>/<total> files` | none (progress) |
| `unreachable` | `Can't reach anyone who has this app yet. Bring the device you got it from nearby, then Retry.` | **Retry** |
| `incomplete` | `This app didn't download completely. Try again.` | **Retry**; abort the sync and do **not** cache (matches §5.2 step 4 verify) |
| unknown host / not in Library | — | bounce the browser to the Library "not in your Library" page (§3.2) |

The 1-second threshold and the poll-then-redirect loading page are taken
directly from the reference and are a **proposed default** (tunable).

### 4.4 Deferred: the htdocs serving cache

> **Roadmap, not v0.** v0 serves direct from Blossom (§4.2) and needs none of
> this. As a later **speed optimization**, the reference's path-named **htdocs**
> cache writes each blob out as a real file under its manifest path in a
> `current/` dir, so a hot path is a single nginx-style file read instead of a
> manifest lookup + blob fetch:
>
> ```
> htdocs/<siteKey>/
> ├─ current  ────────────►  v<timestamp>/   (atomic pointer swap)
> ├─ v<timestamp>/           ← new version, fully built before swap
> │  ├─ index.html
> │  └─ …
> └─ v<older>/               ← kept (default: last 2 versions), then GC'd
> ```
>
> The reference builds this with a version dir swapped atomically over
> `current` — a symlink rename
> ([sync-architecture.md](../../reference/site-deck/docs/sync-architecture.md),
> `activateVersion` in [service.go](../../reference/site-deck/internal/sync/service.go)).
> This htdocs cache is **derived** from the Blossom blob store (§6), never the
> source of truth, and is purely a serving-speed optimization. When/if it lands,
> the Android atomic-swap mechanism (symlink-rename vs an atomically-written
> `current.json` pointer file) is itself an open question. **Deferred — TBD /
> open.**

---

## 5. Sync over FIPS: pulling a manifest + blobs from a peer

This is the one place Myco diverges substantially from nsite-deck. The Go
sync engine fetches from **public** relays/Blossom over the internet; Myco
fetches from a **reachable FIPS peer** over the mesh — including offline over
BLE.

**Source resolution order.** For any missing event or blob, Myco tries sources in
this order, stopping at the first hit:

1. **Local relay + Blossom** — already held; serve direct (§4.2), no fetch.
2. **FIPS peers** — a reachable holder over `.fips` (your circle and, transitively,
   their peers; §5.1–5.2). The offline mesh path.
3. **Servers/relays the nsite event lists** — the manifest's `["server", …]` hints
   plus the author's NIP-65 `10002` relay list / BUD-03 `10063` Blossom servers.
   This is the **online fallback** (needs IP internet), so it is the **last resort**
   — and a Settings toggle (`[sync] offline_only`, see
   [config.md](../reference/config.md)) disables it entirely so Myco never reaches
   the IP internet (tiers 1–2 only).

### 5.1 Reaching the holder's services over `.fips`

**Two different keys.** The site you want is identified by its **author** key
(`npub_author`, the `.nsite` host) — an external key Myco never holds the
secret for and never signs with. The peer you *fetch it from* is a **holder**:
any device that has cached and re-serves the author's signed manifest + blobs.
Each holder is reachable at its own **device** key, `npub_holder`, which is that
device's one Nostr keypair (used for the FIPS mesh address, BLE link auth, and as
the address of its relay+Blossom — never to author nsites). These are not the
same key: the author identifies *what* you want, the holder identifies *who you
get it from*.

A holder is reachable at the mesh address
`fd00:: = fd + SHA256(npub_holder)[0:15]`, and the `.fips` DNS interceptor maps
`<npub_holder>.fips → fd00::` (AAAA). Because FIPS multiplexes mesh datagrams to
localhost ports (the IPv6 adapter runs on FSP port 256, and `curl
http://<npub>.fips:port/` already works in the FIPS Android reference), the
holder's own relay and Blossom are reachable at:

- `ws://<npub_holder>.fips:4869` — the holder's embedded relay
- `http://<npub_holder>.fips:24242` — the holder's embedded Blossom

You then query that holder's relay *for the author's events*:
`{ kinds:[15128 or 35128], authors:[<author_pubkey>] }`. The holder serves the
author's already-signed manifest unmodified; it is not the author of anything it
re-serves.

No separate gateway port is needed on a reachable path — the localhost services
*are* the mesh endpoints. Sources:
[../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md)
and [../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md).

> **Note.** The *sync engine* (native Rust) is what dials `<npub>.fips`. The
> WebView never does. See [../reference/ports.md](../reference/ports.md) for the
> exact `.fips` vs `.nsite` split.

### 5.2 The pull sequence

For a `siteKey` authored by `npub_author`, fetched from a reachable holder
`npub_holder` (chosen from the manifests you have received / the Library; selection
is detailed in [./propagation.md](./propagation.md)):

1. **Fetch the manifest.** Query the holder's relay
   `ws://<npub_holder>.fips:4869` for the author's manifest event:
   - root: `{ "kinds":[15128], "authors":[<author_pubkey>] }`
   - named: `{ "kinds":[35128], "authors":[<author_pubkey>], "#d":[<identifier>] }`
   Keep the newest per slot.
2. **Build the file list.** Extract every `["path", <path>, <sha256>]` tag →
   `path → hash` map.
3. **Pull blobs by hash.** For each hash, `GET <sha256>` from the holder's Blossom
   `http://<npub_holder>.fips:24242`. Fetch concurrently (the reference caps at 8).
   Because blobs are content-addressed, Myco can prefer the **local**
   Blossom first (a blob already cached for another site is the same blob) and
   only go to the peer on a miss.
4. **Verify.** Re-hash each blob; `sha256(bytes)` MUST equal the manifest hash.
   Any mismatch or missing blob **aborts the whole sync**. This is what makes any
   source trustworthy: the data is self-authenticating, so it does not matter
   *who* served it.
5. **Mirror into local Blossom.** Store each verified blob (keyed by sha256) in
   the **local Blossom**, so this device can both serve it direct (§4.2) and
   re-serve it to the next peer — the propagation hop. No version dir is written
   in v0; the content-addressed store *is* the cache.
6. **Activate.** Store the author's signed manifest event (unmodified) into the
   **local** relay. With all referenced blobs now present (§4.2's check), the
   site is immediately servable direct; the stored manifest also lets the local
   relay answer the loading page's subscription. Re-emitting an already-signed
   event relay-to-relay like this is normal relay behaviour, not authoring — the
   author's signature stays valid.

Steps 1–6 mirror `Sync` in
[service.go](../../reference/site-deck/internal/sync/service.go), with public
relays/Blossom replaced by the single peer's `.fips` endpoints. The manifest's own
`["server", …]` hints (public Blossom URLs) are **tier 3** of the source-resolution
order above — the online fallback, used only when reachable and only if
`[sync] offline_only` is not set; on the offline mesh path they are skipped.

> **Open question — source selection & multi-source pull.** v1 pulls from one
> chosen holder. Pulling different blobs from different reachable holders in
> parallel (the data is content-addressed, so any holder will do) is a natural
> extension but is **TBD / open**. Manifest-driven source discovery lives in
> [./propagation.md](./propagation.md).

### 5.3 Why this is propagation, not just caching

FIPS transport gives **live-path multi-hop only** — there is no store-and-forward
in the mesh itself. The store-and-forward behaviour ("cache Alice's site and
re-serve it to Carl tomorrow, offline") is **net-new at this layer**: step 5
mirrors every blob and the manifest into the local relay + Blossom, so the
device *becomes a new holder* for that site. Carl syncing from this device sees
the exact same self-authenticating manifest and blobs that Alice (the external
author) signed — this device re-serving them does not make it the author. This is
the split the project calls the Pillars of Propagation; the policy around it
(announce widely the author-signed manifests, pull the blobs on demand) is
[./propagation.md](./propagation.md).

### 5.4 Eager refresh of pinned sites

Library-pinned sites are kept current in the background so they stay
offline-ready *before* the next open. The **nsite-deck propagator** (§2.1) keeps
an internal subscription for kinds **15128 / 35128** across the relays it is
connected to; when a **newer** manifest arrives for a Library-pinned
`(author, dTag)`, it **auto-runs the §5.2 pull in the background** — fetch the
manifest's blobs by hash from a reachable holder's `.fips` Blossom, verify each
sha256, and mirror the verified blobs + the signed manifest into the local
Blossom + relay. The pinned app is then up to date for the next open with no
foreground sync. This reuses the §5.2 sequence verbatim (including the
abort-on-mismatch verify, step 4); only the *trigger* differs — a newer manifest
seen on subscription rather than a browse request.

---

## 6. Offline serving from cache

Once a site has been synced — manifest in the local relay, all referenced blobs
in the local Blossom — it is served entirely from the local content-addressed
store and never needs the network again:

- **Cached site, offline** → served direct from the local relay + Blossom (§4.2),
  identical to the online path. The nsite opens and runs fully with no radio
  (any in-app reload/navigation it implements works against the local stores).
- **Uncached site, offline** → the sync has no reachable source, so the gateway
  shows the `unreachable` page (HTTP 503): *"Can't reach anyone who has this app
  yet. Bring the device you got it from nearby, then Retry."* with a **Retry**
  action (§4.3). It is not a crash; the moment a holding peer comes into BLE
  range, the retry succeeds.

This is the offline guarantee in
[sync-architecture.md](../../reference/site-deck/docs/sync-architecture.md): all
cached nsites work without network; only first-time/uncached sites need a
reachable source.

### Discovery: "nsites around me"

Beyond loading a known `<host>.nsite`, the Library can surface sites that reachable
holders have. The set of discoverable sites is **the author-signed manifests this
device has** — those received via the flood (small, self-authenticating manifest
events re-emitted unmodified by relays) plus those **queried from every reachable
relay** (your own, plus each paired peer's relay at `<npub_holder>.fips:4869`,
plus their collected peers — see [./propagation.md](./propagation.md) for
transitive reach) for kinds **15128 / 35128**. Present them **newest-first,
de-duplicated by `(author, dTag)`**. Selecting one runs the normal §4 sync/serve
flow (fetching the large blobs on demand). This is exposed as a `SearchNsites`
FFI action ([../reference/ffi-surface.md](../reference/ffi-surface.md)); ranking
beyond recency is **TBD / open**.

### Storage and eviction

**Two stores, kept distinct.** The **Blossom blob store** is the
content-addressed store Myco *retains*: blobs keyed by sha256, embedded from day
one, it is both the store-and-forward source (what this device re-serves to the
next peer) and what serving reads from direct (§4.2). The deferred **htdocs**
cache (§4.4) is a *derived*, path-named serving cache layered on top — a speed
optimization, never the source of truth, and not present in v0. The 2 GB cap
below governs the **Blossom blob store**.

- **LRU cache, default cap 2 GB** (proposed default), over the Blossom blob
  store. Oldest-accessed sites are evicted first when over cap, using a small
  SQLite index of `(siteKey, size, last_accessed)`
  ([cache/database.go](../../reference/site-deck/internal/cache/database.go),
  [cache/manager.go](../../reference/site-deck/internal/cache/manager.go)).
- **Pinned sites are exempt.** A site the user adds to their Library is pinned and
  never evicted.
- **Blob dedup.** Because the local Blossom is content-addressed, a blob shared
  across sites (e.g. a common JS lib) is stored once and re-served to peers
  regardless of which site first pulled it.
- **Version retention (deferred).** Multi-version retention + GC belongs to the
  htdocs cache (§4.4); it is **not** part of the v0 serve-direct model.

---

## 7. Open questions

- **JS sandbox / capability API.** v1 nsites are **pure static content**. What
  an nsite's JavaScript may call back into (e.g. query reachable peers, trigger
  a sync) is a later milestone and an explicit open design question (called out
  on [diagram 04](diagrams/04-nsite-browse-flow.svg)). **TBD / open.**
- **htdocs serving cache** — the deferred path-named cache and its Android
  atomic-swap mechanism (symlink-rename vs `current.json` pointer file)
  (§4.4). **Deferred to the roadmap — TBD / open.**
- **Multi-source / parallel pull** from several reachable holders (§5.2).
  **TBD / open.**
- **Relay store choice** — full `nostr-rs-relay`-style store vs a hand-rolled
  `rusqlite` store sized to the tiny query surface Myco actually uses (§2.1).
  **TBD / open.**
- **nsite-scoped propagation (capability-gated)** — whether the propagator's
  fanout should be scoped per nsite / gated by a pairing capability rather than
  flooding every accepted manifest to every connected peer. Tracked in
  [./propagation.md](./propagation.md)'s "nsite-scoped propagation
  (capability-gated)" open question (ties into the mandatory mutual pairing
  handshake, identity-pairing §6.1). **TBD / open.**

---

## See also

- [./propagation.md](./propagation.md) — manifest flooding, source discovery,
  device-to-device hopping, the store-and-forward layer.
- [../reference/nostr-kinds.md](../reference/nostr-kinds.md) — kind 15128 /
  35128 tag layout, FIPS discovery kinds.
- [../reference/ports.md](../reference/ports.md) — localhost ports and how each
  is (or is not) exposed over FIPS.
- [diagrams/04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg),
  [diagrams/03-offline-propagation.svg](diagrams/03-offline-propagation.svg),
  [diagrams/01-system-layering.svg](diagrams/01-system-layering.svg).
