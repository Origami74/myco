# Data plane

How neighborhood events and blobs are stored and carried over links.

Code: `myco-core/src/neighbor/data.rs` (store and routing) and
`myco-core/src/neighbor/wire.rs` (the scoped relay, both sides).

## Scoped store

Neighborhood events live in their own store (`neighborhood-relay/`, blobs in
`neighborhood-blobs/`), next to a scope table
(`neighborhood-scopes.json`): event id or `blob:<sha256>` → the
neighborhoods it belongs to.

- One copy per event, even if it arrived in two neighborhoods.
- Nothing published into a neighborhood lands in the user's own relay or in
  another neighborhood.

## Scoped relay

```text
ws://<npub>.fips:4873/nh/{pk}/relay?n=<neighborhood id>&ttl=<hops>
  Authorization: NIP-98 by the peer's link key, `u` = the URL in http form
```

- Admits only a link that carries `n`.
- `REQ` answers from that neighborhood's view, then `EOSE`, and stays open:
  new events kept in `n` that match follow until `CLOSE` or the socket
  ends, while the link still carries `n`. Up to 16 open per connection.
- `EVENT` is kept once and passed on to the other links carrying `n` while
  `ttl > 0` and policy allows.
- A send or a one-off pull uses one connection per exchange, with a 20 s cap.
  A followed subscription keeps its connection.
- The client dials the peer's relay URLs from its offer, FIPS first.

## Publish and subscribe

| Request | Neighborhood address | Peer address |
| --- | --- | --- |
| Publish | Kept in `n`; sent to every link carrying `n` with `ttl = publish hops − 1` | Kept in `n`; sent to that peer only, `ttl = 0` |
| Subscribe | Local view, then followed on every link carrying `n` (if backlog hops > 0), then live | Not supported yet |
| Blob read | Local scoped store, then each link carrying `n` in turn | That peer only |
| Upload | Kept in the local scoped store | Not supported |

- Sends are spawned: a slow peer holds up nobody.
- Each event id is passed on once per neighborhood.
- Events by muted authors are hidden and not passed on.
- Pulled backlog is kept and shown to live subscribers, not passed on.

### Following peers

A subscription is replicated to the peers for as long as the napplet
listens:

- **On subscribe**, it opens on every reachable link carrying `n`.
- **A peer who joins or comes back in reach** gets it too. A link starting,
  ending, changing what it carries, or answering again after
  2 min of silence wakes the subscription to check.
- **A peer who leaves** has it closed: the link ended or stopped carrying
  `n`.
- **A dropped connection** is retried after 15 s while the link is in reach.
- **The napplet closing it**, or its window closing, ends it on every peer.

## Blobs

A blob read that misses here is asked of the links carrying `n`, one at a
time:

```text
GET /nh/{pk}/blob/{sha256}?n=<id>   NIP-98 by the link key
  200 bytes | 404 not held | 403 link doesn't carry n
```

- The serving side answers only from `n`'s scope, and only if its offer for
  `n` has blob reads on. Nothing else on the phone is reachable this way.
- The fetching side checks the bytes hash to `sha256` (64 MiB cap), then
  keeps them in `n`'s scope, so the next read is local and it can serve
  them in turn.
- One hop: a peer that doesn't hold the blob does not ask its own links.
- Pulls stream: each event a peer sends is kept and reaches subscribers as
  it arrives, not when the peer is done.

**Without fetching.** `HEAD` on the same route answers 200 or 404 with no
body: whether the `GET` would serve it. A napplet asks through
`resource.check` (below); Myco asks every link carrying `n` at once, each
bounded at 8 s, and answers `yes` as soon as one has it.

```text
HEAD /nh/{pk}/blob/{sha256}?n=<id>  NIP-98 (method HEAD)  -> 200 | 404 | 403
```

## `resource.check` (local NAP-RESOURCE extension)

Not in NAP-RESOURCE yet. `window.napplet.resource.check(urls)` resolves one
item per URL, `{ url, available: "yes" | "no" | "unknown", size?, error? }`,
without transferring anything:

- held on this phone: `yes` with its size (or the error `bytes` would give,
  e.g. raw SVG);
- `blossom:` with a `neighborhood:` `xs`: asked of that neighborhood as above;
- anything else: `unknown`.

Blobs are not pushed. **Blob push hops** ([policy.md](policy.md)) is stored
but pushes nothing yet.
