# Data plane

How neighborhood events and blobs are stored and carried over links.

Code: `myco-core/src/neighbor/data.rs` (store and routing) and
`myco-core/src/neighbor/wire.rs` (the scoped relay, both sides).

NAP-NEIGHBOR leaves the peering protocol out of scope. The scoped relay,
the `/nh/…` routes and the blob `HEAD` below are Myco's protocol.

## Scoped store

Each neighborhood keeps its events in its own store
(`neighborhood-relay/<neighborhood id>/`). Blobs share one directory,
`neighborhood-blobs/`, named by their hash. A scope table
(`neighborhood-scopes.json`) maps each event id or `blob:<sha256>` to the
neighborhoods it belongs to.

- **Replaceable and deleted stay inside one neighborhood.** A newer
  replaceable event, or an author's deletion, kept in one neighborhood leaves
  every other neighborhood's copy alone.
- **An event kept in two neighborhoods is stored twice.** A blob is stored
  once.
- **Leaving a neighborhood deletes its store**, and the blobs no other
  neighborhood keeps.
- Nothing published into a neighborhood lands in the user's own relay or in
  another neighborhood.
- A blob fetched through a neighborhood stays in that neighborhood's scope.
  It is not put in the device-wide store, where any napplet could read it
  by hash (NAP-NEIGHBOR's Segregation rule).
- Per event and neighborhood, the table also records whether it was
  published here and how many hops it has left. That decides what peers
  are given ([policy.md](policy.md#what-a-peer-is-given)).
- Each neighborhood keeps up to 10 000 events and 256 MiB of blobs; past
  that its oldest leave. Leaving a neighborhood deletes what only it held.

## Scoped relay

```text
ws://<npub>.fips:4873/nh/{pk}/relay?n=<neighborhood id>&ttl=<hops>
  Authorization: NIP-98 by the peer's link key, `u` = the URL in http form
```

- Admits only a link that carries `n`.
- `REQ` answers with what may be passed on
  ([policy.md](policy.md#what-a-peer-is-given)), then `EOSE`, and stays
  open: new events that may be passed on follow until `CLOSE` or the socket
  ends. Up to 16 open per connection. With relay reads off in this side's
  offer, `REQ` gets `CLOSED` `"restricted: …"`.
- `EVENT` is kept once and passed on to the other links carrying `n` while
  `ttl > 0` and policy allows. With relay writes off, it gets `OK false`
  `"restricted: …"`.
- A send or a one-off pull uses one connection per exchange, with a 20 s cap.
  A pull reads at most 2000 events, and only those matching the filter are
  kept. A followed subscription keeps its connection.
- The client dials the peer's relay URLs from its offer, FIPS first.

Socket limits: 4 per link (a 5th gets `429`); on each, messages and frames
up to 64 KiB and 20 frames/s with bursts of 100. A socket closes when its link
ends or stops carrying `n`, and after 5 minutes idle with no subscription
open.

## Publish and subscribe

| Request | Neighborhood address | Peer address |
| --- | --- | --- |
| Publish | Kept in `n`; sent to every link carrying `n` with `ttl = publish hops − 1` | Kept in `n`; sent to that peer only, `ttl = 0` |
| Subscribe | Local view, then followed on every link carrying `n` (if backlog hops > 0), then live | Not supported (`unsupported`) |
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

A subscription never tells the napplet which peer an event came from. It
sees events:

- **New ones** are kept and delivered.
- **Ones already kept** are delivered again when a peer in reach sends
  them. That is the napplet's sign that someone can serve them now, for
  example to ask `resource.check` again.
- **Nobody in reach** means the subscription has only the backlog, then
  EOSE at once.

## Blobs

A blob read that misses here is asked of the links carrying `n`, one at a
time:

```text
GET /nh/{pk}/blob/{sha256}?n=<id>   NIP-98 by the link key
  200 bytes | 404 not held | 403 link doesn't carry n
```

- The serving side answers only if its offer for `n` has blob reads on, and
  only from `n`'s scope, plus the `index.html` and icon of napplets
  installed here where it shares `napplet-updates`
  ([extensions.md](extensions.md#napplet-updates)). Nothing else on the
  phone is reachable this way.
- The fetching side checks the bytes hash to `sha256` (64 MiB cap), then
  keeps them in `n`'s scope only, so the next read is local and it can
  serve them in turn.
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

## `resource.check` (Myco's NAP-RESOURCE extension)

Myco's own addition, not in NAP-RESOURCE. `window.napplet.resource.check(urls)` resolves one
item per URL, `{ url, available: "yes" | "no" | "unknown", size?, error? }`,
without transferring anything:

- held on this phone: `yes` with its size (or the error `bytes` would give,
  e.g. raw SVG);
- `blossom:` with a `neighborhood:` `xs`: asked of that neighborhood as above.
  Nothing is transferred, so a `yes` can't promise what `bytes` checks on
  the bytes themselves (the size cap, raw SVG). Without the `neighbor`
  grant: `no`, with the error `bytes` gives (`not-found`);
- anything else: `unknown`.

Blobs are not pushed: **Blob push hops** ([policy.md](policy.md)) is stored,
not built.
