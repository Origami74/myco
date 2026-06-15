# Nostr Event Kinds Reference

The Nostr event kinds Myco reads, stores, serves, and replicates. Myco
**never authors, signs, or publishes** nsite events — it holds and re-emits
events authored *elsewhere* (by external nsite tooling) and the
content-addressed blobs they reference. Two families:

1. **nsite content kinds** — the author-signed site manifests the
   gateway/relay/Blossom layer serves and propagates (kinds `15128`, `35128`).
   These are *established facts*, verified from the nsite protocol and the
   reference implementation.
2. **FIPS discovery kinds** — used by `fips-core` for *future* public-node
   peering over the internet (kinds `37195`, `21059`, `10050`). Not needed for
   the offline BLE demo; documented here so the surface is complete.

Design context: [../design/nsite-layer.md](../design/nsite-layer.md),
[../design/propagation.md](../design/propagation.md). Protocol sources are
cited inline.

---

## Summary table

| Kind | Name | Class | Layer | Status in Myco |
| ---- | ---- | ----- | ----- | ------ |
| `15128` | nsite root-site manifest | Replaceable | nsite content | **Used** |
| `35128` | nsite named-site manifest | Param-replaceable (`d`) | nsite content | **Used** |
| `34128` | legacy per-file nsite event | Param-replaceable (`d`) | nsite content | **Not used** (legacy) |
| `10002` | NIP-65 relay list | Replaceable | discovery hint | Online fallback only |
| `10063` | BUD-03 user Blossom servers | Replaceable | discovery hint | Online fallback only |
| `37195` | FIPS overlay advert | Param-replaceable (`d`) | FIPS discovery | Future public peering |
| `21059` | FIPS traversal signaling | Ephemeral | FIPS discovery | Future public peering |
| `10050` | NIP-17 inbox relay list | Replaceable | FIPS discovery | Future public peering |

---

## nsite content kinds

Source of truth:
[../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)
(NIP-5A, "Pubkey Static Websites"), and the reference implementation in
[../../reference/site-deck/internal/sync/service.go](../../reference/site-deck/internal/sync/service.go)
and [../../reference/site-deck/internal/gateway/handlers.go](../../reference/site-deck/internal/gateway/handlers.go).

### Kind 15128 — root-site manifest

- **Class:** replaceable. Exactly one per pubkey — the pubkey's root site.
- **`d` tag:** MUST NOT be present.
- **Content:** empty.
- **URL host:** `npub1…` (NIP-19 bech32 of the author pubkey).

### Kind 35128 — named-site manifest

- **Class:** parameterized-replaceable. One per `(pubkey, d-tag)` — a named
  sub-site under the pubkey ("like a sub-domain").
- **`d` tag:** REQUIRED — the site identifier. For a canonical URL the d-tag
  MUST match `^[a-z0-9-]{1,13}$` and MUST NOT end with `-` (so 50-char
  `pubkeyB36` + ≤13-char d-tag fits one 63-char DNS label).
- **Content:** empty.
- **URL host:** `<pubkeyB36><dTag>` — the 50-char lowercase-base36 pubkey
  directly followed by the d-tag, no separator. Encoder/decoder + regex:
  [../../reference/site-deck/internal/gateway/base36.go](../../reference/site-deck/internal/gateway/base36.go).

### Tag layout (both kinds)

| Tag | Required | Meaning |
| --- | --- | --- |
| `["d", "<identifier>"]` | 35128 only | Site identifier (the named-site d-tag). Absent on 15128. |
| `["path", "/abs/path.ext", "<sha256>"]` | yes (≥1) | Maps one absolute file path to the sha256 of its bytes (a Blossom blob). |
| `["server", "<blossom-url>"]` | no | Hint: a Blossom server that may hold the blobs. Online fallback only — ignored on the offline `.fips` path. |
| `["title", "<text>"]` | no | Human-readable site title (shown in Library / loading page). |
| `["description", "<text>"]` | no | Short site description. |
| `["source", "<http-url>"]` | no | Link to the site's source repo/archive. |

The site icon is conventionally the blob mapped at `/favicon.ico`. A custom
not-found page is the blob mapped at `/404.html`.

### Worked example — root site (kind 15128)

```jsonc
{
  "kind": 15128,
  "pubkey": "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5",
  "created_at": 1727373475,
  "content": "",
  "tags": [
    ["path", "/index.html",  "186ea5fd14e88fd1ac49351759e7ab906fa94892002b60bf7f5a428f28ca1c99"],
    ["path", "/about.html",  "a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef123456"],
    ["path", "/favicon.ico", "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"],
    ["title", "My Nostr Site"],
    ["description", "A static website hosted on Nostr"]
  ],
  "id": "…",
  "sig": "…"
}
```

### Worked example — named site (kind 35128)

```jsonc
{
  "kind": 35128,
  "pubkey": "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5",
  "content": "",
  "tags": [
    ["d", "blog"],
    ["path", "/index.html", "186ea5fd14e88fd1ac49351759e7ab906fa94892002b60bf7f5a428f28ca1c99"],
    ["path", "/post.html",  "a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef123456"],
    ["title", "My Blog"]
  ],
  "id": "…",
  "sig": "…"
}
```

### Query filters Myco uses

```jsonc
// root manifest
{ "kinds": [15128], "authors": ["<pubkey>"] }

// named manifest
{ "kinds": [35128], "authors": ["<pubkey>"], "#d": ["<identifier>"] }
```

These run against the **local** relay first (fast path) and, on a miss, against
the source peer's relay over `.fips` (`ws://<npub>.fips:4869`). See
[../design/nsite-layer.md §5](../design/nsite-layer.md).

> **Set reconciliation.** Between two connected relays, Myco reconciles the
> manifest **event** set with **negentropy ([NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md))**
> run over these same filters (`NEG-OPEN` → `NEG-MSG` rounds → the missing ids),
> then pulls only the diff. Blobs are never reconciled — they stay content-addressed
> pull-by-sha256. See [../design/propagation.md §5](../design/propagation.md) and
> [../design/nsite-layer.md §2.4](../design/nsite-layer.md).

### Kind 34128 — legacy, NOT used

The original nsite design used kind `34128`: one event *per file*, with the path
in a `d` tag and the sha256 in an `x` tag. Myco does **not** read or write
it — Myco is manifest-based (one `15128`/`35128` event maps all paths).
Documented only so old `34128` events seen on a relay are recognized and
ignored. (Source: NIP-5A "Legacy Support".)

> **Note on online-fallback kinds.** When *online*, the sync engine may consult
> the author's `10002` (NIP-65 relay list) and `10063`
> ([BUD-03](https://github.com/hzrd149/blossom/blob/master/buds/03.md) user
> Blossom servers) to find public sources, exactly as the Go reference does
> ([service.go](../../reference/site-deck/internal/sync/service.go)). On the
> **offline** BLE path these are irrelevant — the source is a single reachable
> peer's `.fips` services.

---

## Propagation: the manifest event *is* the propagated unit

There is **no** net-new announcement kind. The hybrid propagation default —
**announce widely, pull content on demand** — is built entirely on the
author-signed manifests above:

- **The flooded unit is the author-signed manifest itself** (kind `15128` /
  `35128`). It is small, self-authenticating (the author's signature travels
  with it), and re-emitted **unmodified** by relays — replicating an existing
  author-signed event relay-to-relay is ordinary relay behaviour, **not**
  authoring. Because the bytes are unchanged, the author's signature stays
  valid. A holder re-emitting an author's manifest needs **no new kind and no
  holder signature**.
- **"Announce widely"** = flood/replicate those manifest events across reachable
  relays, with a hop budget of **TTL = 5** (a project default).
- **"Pull on demand"** = fetch the large content-addressed **blobs** (by sha256
  from Blossom) only when a site is actually opened — the manifest tells you
  *what* a site is; the blobs are deferred until needed.
- **Discovery / "nsites around me"** = simply the set of manifests you have
  *received* (via flood) or *queried* from reachable relays. No separate "I have
  it" event exists; possession of the manifest is the advertisement.

Loop-suppression and dedup during the flood are done over the **manifest events
themselves** — 16-byte SHA-256 IDs in a per-node seen-set — while steady-state
catch-up between connected relays uses **negentropy (NIP-77)** reconciliation so a
peer offers only manifests you lack (events only; blobs stay pull-by-sha256). TTL=5,
that dedup story, transitive peer discovery, and the privacy question of *which
manifests you choose to replicate* are all detailed in
[../design/propagation.md](../design/propagation.md).

---

## FIPS discovery kinds (future public-node peering)

These are `fips-core`'s own Nostr event kinds, used to find and rendezvous with
peers **over the public internet** (NAT traversal, overlay adverts). They are
**not** used in the v1 offline BLE demo — BLE peering is QR + adverts, not Nostr.
They are listed so the full Nostr surface Myco *could* touch is documented,
and so their numbers are not accidentally reused for nsite kinds.

Verified from
[../../reference/fips/docs/reference/nostr-events.md](../../reference/fips/docs/reference/nostr-events.md).
All three are signed with the node's FIPS identity key (the same secp256k1
keypair Nostr uses — there is no separate Nostr key).

| Kind | Name | Class | Encryption | Purpose |
| ---- | ---- | ----- | ---------- | ------- |
| `37195` | Overlay advert | Param-replaceable (`d`=`fips-overlay-v1`) | Signed only | Publish a node's reachable transport endpoints (UDP/TCP/Tor, or `nat`). |
| `21059` | Traversal signaling | Ephemeral (20000–29999) | NIP-44 inside NIP-59 gift wrap | Carry `TraversalOffer` / `TraversalAnswer` during a UDP NAT hole-punch. |
| `10050` | NIP-17 inbox relay list | Replaceable | Signed only | Tell dialers which relays to send gift-wrapped traversal offers to. |

Notes:

- **`37195`** sits in the parameterized-replaceable range; its digits visually
  spell **FIPS** (7=F, 1=I, 9=P, 5=S). Its `d` tag is the fixed literal
  `fips-overlay-v1`; content is an `OverlayAdvert` JSON document
  (`endpoints`, optional `signalRelays`, `stunServers`). NIP-40 `expiration`
  (default 3600s) ages adverts out.
- **`21059`** is the NIP-59 gift-wrap outer kind; the inner seal is a kind-13
  event and the rumor is the actual offer/answer. Ephemeral → conforming relays
  do not store it.
- **`10050`** is standard NIP-17 (DM inbox relays), distinct from `10002`
  (NIP-65 general read/write relays). FIPS publishes its own `10050` on startup
  so dialers know where to deliver `21059` offers.

Full schemas, tag semantics, and the gift-wrap envelope:
[../../reference/fips/docs/reference/nostr-events.md](../../reference/fips/docs/reference/nostr-events.md)
and `../../reference/fips/docs/design/fips-nostr-discovery.md`.

---

## See also

- [../design/nsite-layer.md](../design/nsite-layer.md) — how the manifest kinds
  are fetched, verified, and served.
- [../design/propagation.md](../design/propagation.md) — flooding the
  author-signed manifests and device-to-device hopping.
- [./ports.md](./ports.md) — the localhost ports the relay/Blossom listen on.
- [../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)
  — NIP-5A, the authoritative nsite manifest spec.
- [../../reference/fips/docs/reference/nostr-events.md](../../reference/fips/docs/reference/nostr-events.md)
  — the authoritative FIPS discovery-kind spec.
