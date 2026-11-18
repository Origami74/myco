# Napplet API: NAP-NEIGHBOR

What napplets see of neighborhoods. Spec:
[napplet/naps#146](https://github.com/napplet/naps/pull/146).

Code:

- `myco-napplet-runtime/src/nap/neighbor/` — the wire (`mod.rs`), schema
  types (`model.rs`), the `NeighborHost` seam (`seam.rs`), addresses
  (`address.rs`), and routing addresses through other NAPs (`route.rs`).
- `myco-core/src/neighbor/host.rs` — `Neighbors`, Myco's `NeighborHost`.

## The `neighbor` domain

It carries no data. It tells a napplet:

- every neighborhood the user carries (`neighbor.list`),
- which of the user's peers carry each, whether they are reachable, and
  their names as the spec's `Peer.name` defines them (`neighbor.peers`),
- a `neighborhood://` address for each.

```text
-> { "type": "neighbor.list", "id": "n1" }
<- { "type": "neighbor.list.result", "id": "n1", "neighborhoods": [
     { "id": "h_2c", "name": "Household", "autoAccept": false,
       "peers": 3, "reachable": 2, "address": "neighborhood://h_2c" } ] }
```

## Addresses

```text
neighborhood://<n>        the whole neighborhood
neighborhood://<n>/<p>    one peer in it
```

- This format is internal to Myco. Napplets must not parse or build
  addresses: NAP-NEIGHBOR makes them opaque.
- Tokens are minted per napplet. An address means nothing in another
  napplet and is refused there.
- The scheme never reaches DNS or a socket.
- A subscription on an address never tells the napplet which peer an event
  came from.

## How other NAPs carry addresses

| NAP | Call | With an address |
| --- | --- | --- |
| NAP-RELAY | `relay.subscribe` | Reads the neighborhood ([data-plane.md](data-plane.md)) |
| NAP-OUTBOX | `outbox.publish` | Publishes into it |
| NAP-OUTBOX | `outbox.*` reads | Reads it, as `relay.subscribe` on the address does |
| NAP-RESOURCE | `resource.bytes` | Honors `xs=<address>`. The blob stays in that neighborhood: not in the device-wide store |
| NAP-RESOURCE | `resource.check` | Myco's extension, not upstream ([data-plane.md](data-plane.md#resourcecheck-mycos-nap-resource-extension)) |
| NAP-UPLOAD | `upload.upload` | `destination = <address>` on the `blossom-neighborhood` rail |

## What Myco supports

| Feature | State |
| --- | --- |
| `info`, `list`, `peers` | yes |
| `create` (asks the user) | yes |
| `leave` | yes (on `nearby`: turns Nearby off) |
| `setOffer` | yes |
| `mute` | yes |
| `invite` | no (invitations are made in the Neighbors tab) |
| `nearby` (proximity extension) | yes, while Nearby is on ([proximity.md](proximity.md)) |
| `napplet-updates` (extension) | yes ([extensions.md](extensions.md)) |

## Grants

Neighborhoods are not tied to napplets. Any napplet with the `neighbor`
domain sees and uses every neighborhood on the device — the ones the user
starts or joins in the Neighbors tab and the ones any napplet created.

Each napplet still gets its own ids and `neighborhood://` addresses for
them, so an id one napplet holds means nothing to another.

Confirmation follows the spec: `create`, `leave` and a `setOffer` that
widens access ask the user. `mute` and a narrowing `setOffer` do not, and
they apply for every napplet ([security.md](../core/security.md#52-the-napplet-sandbox--a-program-behind-a-permission-model)).
