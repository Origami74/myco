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
   │  cache hit? serve from filesystem (<1ms)    │
   │  miss/stale? ↓                              │
   ▼                                             │
Embedded relay (ws://localhost:4869)             │  all in-process,
   │  manifest event: kind 15128 / 35128         │  inside the
   ▼                                             │  myco-core crate
Embedded Blossom (http://localhost:24242)        │  (one .so)
   │  blobs by sha256                            │
   ▼                                             │
Sync engine ── over FIPS ──► reachable holder ───┘
   <npub_holder>.fips:4869 (relay)  ·  <npub_holder>.fips:24242 (blossom)
```

### A reusable crate: `nsite-deck`

These four components — gateway, relay, Blossom, sync — are an **app-agnostic
content layer**, and the design keeps them in a **separate, reusable Rust crate,
`nsite-deck`**, so the same code can host nsites in other apps (a desktop
viewer, a different mobile shell, a headless server) — not just Myco. The crate
must know **nothing** about FIPS, BLE, or Android; it is "an embedded nsite host
+ cache" and no more.

It stays reusable by depending on **two transport seams** (traits the host app
implements), mirroring how `fips-core` abstracts its radio behind `BleIo`:

- **`PeerSource`** — *pull*: `fetch_manifest(author, dTag)` and
  `fetch_blob(sha256)` from some reachable source. The sync engine (§2.4) calls
  this; it does not care *how* the bytes arrive.
- **`FanoutSink`** — *push*: `broadcast(event)` to connected peers. The relay
  (§2.1) calls this when it accepts an event; it does not know who the peers are
  or what carries the bytes.

`myco-core` is then a thin **app crate** that: embeds the FIPS endpoint, owns
identity / pairing / the JNI-JSON FFI reducer, implements `AndroidBleIo`, and
provides the FIPS-backed `PeerSource` (fetch over `<npub>.fips`) and `FanoutSink`
(`EVENT` to each connected `<peer>.fips:4869`). It depends on `nsite-deck`;
together they cross-compile into the single `libmyco_core.so`. Kotlin owns the
radio and the TUN; Rust owns the content layer and the mesh. See
[diagrams/01-system-layering.svg](diagrams/01-system-layering.svg) and
[diagrams/05-nsite-layer-architecture.svg](diagrams/05-nsite-layer-architecture.svg).

> **Why Rust, not Go.** The reference nsite-deck is Go (Khatru relay +
> Khatru/Blossom). The locked decision is to re-implement the relay + Blossom in
> Rust (in the `nsite-deck` crate) so there is **one** cross-compiled `.so` and
> **one** FFI surface, rather than embedding a second Go runtime. The Go
> reference remains the behavioural spec; the Rust port must match its wire
> behaviour (kinds, manifest tags, URL scheme, atomic swap).

> **Naming note.** The reusable crate shares the name *nsite-deck* with the Go
> reference project it descends from; they are different codebases (Go reference
> vs. our Rust crate). **TBD / open** whether to publish it under a less
> overloaded crate name.

---

## 2. Proposed Rust implementation

> Everything in this section is **proposed**, not chosen. Crate names are
> candidates; the load-bearing requirement is the *behaviour*, not the
> dependency.

### 2.1 Embedded relay

A NIP-01 relay with a local event store, listening on `ws://localhost:4869`.
The Go reference uses [Khatru](https://github.com/fiatjaf/khatru) over a BoltDB
event store and advertises NIPs 1, 9, 11, 12, 15, 16, 20, 33
([embedded.go](../../reference/site-deck/internal/relay/embedded.go)). The Rust
port needs the same minimal surface: accept `EVENT`, answer `REQ` with a stored
event set, honour `CLOSE`, and serve a NIP-11 document.

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

**Relay-mesh fanout (multiplex to connected peers).** Beyond serving the local
gateway, the embedded relay is a node in a *relay mesh*: when it accepts a Nostr
event — whether pushed in by a peer or pulled in by the sync engine — it
**re-broadcasts that event to every other connected peer** (an `["EVENT", …]` to
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
  the relay never re-signs. Forwarding an already-signed event is relay
  behaviour, not authoring.

This is wanted **early** (roadmap P2 — see [../reference/config.md](../reference/config.md)
`[propagation] fanout`), so two connected relays gossip events both ways as soon
as a FIPS link exists, with the heavier transitive-discovery and scale-dedup
machinery layered on later (P4). See [diagrams/07-relay-mesh-fanout.svg](diagrams/07-relay-mesh-fanout.svg).

### 2.2 Embedded Blossom

A content-addressed blob server on `http://localhost:24242`, implementing the
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

Pulls a manifest + its blobs from a source, verifies every blob's sha256,
writes a new version directory, and atomically swaps it in. The Go reference is
[service.go](../../reference/site-deck/internal/sync/service.go); the only
Myco change is **the source**: a reachable FIPS peer instead of public
relays/Blossom (§5).

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
(`.localhost` is an alternative under consideration — see the open question
below.) Either way the browser reaches the localhost gateway over IPv4 — which is why it works in
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

> **Open question — host suffix.** `.localhost` vs `.nsite` for the WebView
> host. `.localhost` needs no DNS interception at all (RFC 6761 — resolvers MUST
> map it to loopback), but custom subdomains of `.localhost` are not universally
> honoured. `.nsite` needs the `*.nsite → 127.0.0.1` interceptor but is what the
> reference already implements. **TBD / open.**

---

## 4. The gateway flow: resolve → cache → serve

The browse-request lifecycle, per
[diagrams/04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg).

### 4.1 Resolve and normalize

1. WebView requests `http://<host>.nsite/<path>`.
2. Gateway resolves `host → (npub, identifier) → siteKey`.
3. Normalize the path: `/` and any directory path fall back to
   `…/index.html`; an extensionless path is treated as a directory. (Matches the
   spec's index fallback.)

### 4.2 Cache hit — the fast path

If `siteKey` is cached, serve the file straight off the filesystem
(`current/<path>`), nginx-style — a single file read, sub-millisecond, no relay
query and no blob fetch. Update the last-accessed timestamp for LRU. If the
path is missing from a cached site, fall back to the site's `/404.html`, then a
gateway 404. This is `serveFromCache`
([handlers.go](../../reference/site-deck/internal/gateway/handlers.go)).

### 4.3 Cache miss or stale — fetch, then serve

On a miss the gateway runs a single-flight sync (`startOrJoinSync`: at most one
sync per `siteKey`, late requests join the in-flight one), then:

- **Sync completes within ~1s** → serve from the freshly built cache.
- **Sync takes longer than ~1s** → return a **loading page** (HTTP 503 with a
  spinner) that subscribes to the local relay for the manifest (to show
  title/description as soon as it arrives) and polls a status endpoint; when the
  status flips to `ready`, the page redirects to the real content. This is the
  `writeLoadingPage` / `handleLoadingStatus` pair in the reference.
- **Sync fails** → a friendly sync-error page (no source reachable, missing
  blobs, etc.).

The 1-second threshold and the poll-then-redirect loading page are taken
directly from the reference and are a **proposed default** (tunable).

### 4.4 Atomic version swap

A site is never served half-updated. Sync builds an isolated new version, and
only an all-or-nothing swap makes it live:

```
cache/<siteKey>/
├─ current  ────────────►  v<timestamp>/   (atomic pointer swap)
├─ v<timestamp>/           ← new version, fully built before swap
│  ├─ index.html
│  └─ …
└─ v<older>/               ← kept (default: last 2 versions), then GC'd
```

The reference does this with a symlink renamed atomically over `current`
([sync-architecture.md](../../reference/site-deck/docs/sync-architecture.md),
`activateVersion` in [service.go](../../reference/site-deck/internal/sync/service.go)).

> **Open question — symlinks on Android.** App-private storage on Android may
> not honour POSIX symlink-rename atomicity the way the Linux reference assumes.
> Proposed fallback: a `current.json` pointer file written atomically
> (write-temp-then-rename), with the gateway reading the active version dir name
> from it. **TBD / open.**

---

## 5. Sync over FIPS: pulling a manifest + blobs from a peer

This is the one place Myco diverges substantially from nsite-deck. The Go
sync engine fetches from **public** relays/Blossom over the internet; Myco
fetches from a **reachable FIPS peer** over the mesh — including offline over
BLE.

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
> exact `.fips` vs `.nsite`/`.localhost` split.

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
   Any mismatch or missing blob **aborts the whole sync** (the half-built
   version dir is deleted). This is what makes any source trustworthy: the data
   is self-authenticating, so it does not matter *who* served it.
5. **Write + mirror.** Write each verified file into `v<timestamp>/` under its
   real path, and mirror the blob into the local Blossom (so this device can
   re-serve it to the next peer — the propagation hop).
6. **Activate.** Atomic version swap (§4.4), then store the author's signed
   manifest event (unmodified) into the **local** relay so subsequent loads hit
   the fast path and so the local relay can answer the loading page's
   subscription. Re-emitting an already-signed event relay-to-relay like this is
   normal relay behaviour, not authoring — the author's signature stays valid.

Steps 1–6 mirror `Sync` in
[service.go](../../reference/site-deck/internal/sync/service.go), with public
relays/Blossom replaced by the single peer's `.fips` endpoints. The manifest's
own `["server", …]` hints (public Blossom URLs) are **not** used on the offline
path — there is no internet — though they remain a valid fallback when online.

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

---

## 6. Offline serving from cache

Once a site has been synced, it is served entirely from the local filesystem
cache and never needs the network again:

- **Cached site, offline** → served instantly from `current/`, identical to the
  online fast path. Reload and back-to-Library work with no radio.
- **Uncached site, offline** → the sync has no reachable source, so the gateway
  shows the sync-error / "not reachable" page (HTTP 503). It is not a crash;
  the moment a holding peer comes into BLE range, a retry succeeds.

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

- **LRU cache, default cap 2 GB** (proposed default). Oldest-accessed sites are
  evicted first when over cap, using a small SQLite index of
  `(siteKey, size, last_accessed)`
  ([cache/database.go](../../reference/site-deck/internal/cache/database.go),
  [cache/manager.go](../../reference/site-deck/internal/cache/manager.go)).
- **Pinned sites are exempt.** A site the user adds to their Library is pinned and
  never evicted.
- **Version retention.** Keep the last 2 versions per site by default (current +
  previous), GC older ones after a swap.
- **Blob dedup.** Because the local Blossom is content-addressed, a blob shared
  across sites (e.g. a common JS lib) is stored once and re-served to peers
  regardless of which site first pulled it.

---

## 7. Open questions

- **JS sandbox / capability API.** v1 nsites are **pure static content**. What
  an nsite's JavaScript may call back into (e.g. query reachable peers, trigger
  a sync) is a later milestone and an explicit open design question (called out
  on [diagram 04](diagrams/04-nsite-browse-flow.svg)). **TBD / open.**
- **Host suffix** — `.localhost` vs `.nsite` (§3.2). **TBD / open.**
- **Atomic swap mechanism** on Android storage — symlink vs pointer file
  (§4.4). **TBD / open.**
- **Multi-source / parallel pull** from several reachable holders (§5.2).
  **TBD / open.**
- **Relay store choice** — full `nostr-rs-relay`-style store vs a hand-rolled
  `rusqlite` store sized to the tiny query surface Myco actually uses (§2.1).
  **TBD / open.**

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
