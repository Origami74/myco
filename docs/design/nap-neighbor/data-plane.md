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
- `REQ` answers from that neighborhood's view.
- `EVENT` is kept once and passed on to the other links carrying `n` while
  `ttl > 0` and policy allows.
- One connection per exchange (send one event, or pull until `EOSE`), with a
  20 s cap.
- The client dials the peer's relay URLs from its offer, FIPS first.

## Publish and subscribe

| Request | Neighborhood address | Peer address |
| --- | --- | --- |
| Publish | Kept in `n`; sent to every link carrying `n` with `ttl = publish hops − 1` | Kept in `n`; sent to that peer only, `ttl = 0` |
| Subscribe | Local view, then a backlog pull from links carrying `n` (if backlog hops > 0), then live | Not supported yet |
| Blob read | Local scoped store only | Not supported |
| Upload | Kept in the local scoped store | Not supported |

- Sends are spawned: a slow peer holds up nobody.
- Each event id is passed on once per neighborhood.
- Events by muted authors are hidden and not passed on.
- Pulled backlog is kept and shown to live subscribers, not passed on.

## Blobs

Blobs are not fetched from or pushed to peers yet. **Blob push hops**
([policy.md](policy.md)) is stored but pushes nothing until links forward
blobs.
