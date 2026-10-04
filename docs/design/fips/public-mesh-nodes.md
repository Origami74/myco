# Public Mesh Nodes (internet lane)

> Status: built (roadmap N10). Core: `myco-core/src/public_nodes.rs`.
> Android: `app.myco.core.PublicNodesLane`, Settings › Mesh › Internet, and
> the Dev tab's "Public nodes" card. Not yet verified on two phones on
> different networks — see [Testing](#testing).

When a phone is online it can also peer with public FIPS nodes over the
internet. Circle members who are not in the same room then reach each other
through the mesh — gossip, pulls, file sharing, app updates — as if they
were nearby.

## The trade-off

Off by default. The Settings page says, next to the switch:

- A public node sees this phone's **IP address** and which **mesh
  addresses** it talks to.
- It does not see **contents**. Every Circle session is end-to-end Noise
  above it, and a public node is not a Circle member: the Circle gate on
  the relay, Blossom and pairing services is unchanged.

Mesh-only (Settings › Developer) overrides the switch: nothing is fetched
and nothing is dialled while it is on, and turning it on drops existing
public links.

## Where the node list comes from

### Adverts on Nostr

Public fips nodes announce themselves as **kind 37195** events
(fips's `ADVERT_KIND`), published to `wss://relay.damus.io`,
`wss://nos.lol` and `wss://offchain.pub`:

```json
{
  "kind": 37195,
  "tags": [["d", "fips-overlay-v1"], ["protocol", "fips-overlay-v1"],
           ["version", "1"], ["expiration", "1791155167"]],
  "content": "{\"identifier\":\"fips-overlay-v1\",\"version\":1,
               \"endpoints\":[{\"transport\":\"udp\",\"addr\":\"217.77.8.91:2121\"},
                              {\"transport\":\"tcp\",\"addr\":\"217.77.8.91:443\"}]}"
}
```

`d` = `fips-overlay-v1` is the stable (FMP v0, fips `master`) mesh. Nodes on
fips `next` advertise `fips-overlay-v1-next`; Myco ignores them, since it
cannot speak their protocol.

Myco reads the three relays with two filters: recent adverts from anyone
(`since` two hours ago, `limit` 500), and the recommended nodes by author,
so hundreds of browser nodes cannot crowd them out of the first.

### Validation

Relay content is untrusted. `parse_advert` keeps an advert only if:

- the kind, `d` tag, `protocol` tag, identifier and version match;
- the content is at most 4 KiB with at most 16 endpoints;
- the Schnorr signature verifies (checked here, not only by the relay pool);
- it is unexpired (NIP-40), at most two hours old, and not dated more than
  ten minutes ahead;
- at least one endpoint is **UDP at a literal, public socket address**.

No hostnames, so an advert's author does not choose what the phone
resolves. No `nat` endpoints, which need fips's Nostr hole punching, which
Myco does not run. Nothing loopback, private, link-local, CGNAT, unique-local
(the mesh's own `fd00::/8` included), documentation, benchmarking, multicast
or reserved, so an advert cannot point a phone at its own LAN.

On a live read in October 2026, about 500 adverts gave 28 dialable nodes.
The rest were NAT-only browser nodes.

### The recommended set

[join.fips.network](https://join.fips.network) stars the project's own test
nodes (`test-us01`, `test-de01`, …). Myco recommends the same ones: listed
first and starred.

`next` nodes are excluded everywhere. Their adverts carry
`d=fips-overlay-v1-next` and are refused, and recommended entries named
`*-next` (`test-us03-next`) are dropped from the shipped list, from a
refresh and from a list saved by an older build. They are never shown,
selected or dialled.

The site publishes no machine-readable list. It is a single-page app with
the npubs compiled into its bundle as `{name:"…",npub:"…"}` literals. So:

- **Shipped**: Myco carries a copy (`SHIPPED_RECOMMENDED`).
- **Refreshed**: while the feature is on and the internet is up, Myco reads
  the site's `index.html`, follows its one `assets/index-*.js`, and pulls
  the literals out — at most daily, hourly after a failure, with size caps.
- **Kept on failure**: a refresh that finds nothing keeps the list in hand.
  A redesign of the site costs freshness, never the feature.

The refreshed list is stored in `settings.json`. To update the shipped copy,
read the same literals out of the current bundle.

## Choosing and dialling

### Only what advertises

The Settings list and the dial plan contain only nodes with a live kind
37195 advert. A recommended node that stops advertising drops out of the
list, and comes back when it advertises again.

### Preselection

Myco ticks **at most three** recommended nodes for the user, **at random**,
so phones spread over the test nodes instead of all piling onto the first
ones in the list.

- **When**: on the first advert read that finds recommended nodes
  advertising. Only those are eligible.
- **Stable**: the pick is saved in `settings.json` (npub → last seen
  advertising) and kept across launches and refreshes.
- **Replaced when gone**: a picked node not seen advertising for a day is
  dropped, and another advertising recommended node is drawn in its place.
- **No top-up with strangers**: with fewer than three recommended nodes
  advertising, the pick is smaller. A node from outside the recommended list
  is run by someone nobody vouched for, so dialling it is the user's call.

The randomness is a xorshift seeded from the OS once per draw; tests inject
the draws.

### The user's choices

Stored as deltas against the preselection: `added` (ticked by the user) and
`removed` (a preselected node the user unticked). The user wins. An unticked
node stays off and is never drawn again, and another is drawn to keep three.

### How many

Myco holds **two** public links at once. One is a single point of failure.
More is battery spent on redundancy, since the public nodes peer with each
other. Of the selected nodes, recommended ones are tried first, in the site's
order, then the newest other one.

### How

Each dial is the control socket's `connect` — the same path platform peers
use — with transport `udp/internet`. That is a dedicated fips UDP instance:

- **Outbound-only**: an ephemeral port, no inbound handshakes. A phone behind
  NAT is never dialled from the internet anyway.
- **Bound at node start**, like every other instance, so the switch never
  restarts the node. With the feature off it carries nothing.
- **Pinned by Kotlin** to the best validated non-VPN network
  (`requestNetwork` with `INTERNET` + `NOT_VPN`). Myco is subject to its own
  VPN, and with the SOCKS exit on that VPN claims all public IPv4. An
  unpinned socket would send the mesh's own traffic into the tunnel it
  carries.
- **Backup role** on a multi-path core: an internet path to a peer carries
  traffic only while no radio or LAN path is eligible.
- **IPv4 only**, because fips's outbound-only socket binds `0.0.0.0:0`.
  Public IPv6 endpoints are listed but not dialled.

fips's `connect` makes an ephemeral peer with no auto-reconnect, so Myco
redials: 30 s after a dial that did not come up, doubling to 10 minutes. A
dial younger than 30 s counts toward the two links.

### When it stops

- **Switch off, or mesh-only on**: every connected public node that is not a
  Circle member is disconnected (`disconnect` over the control socket). A
  Circle member who happens to run a public node keeps their link.
- **A node deselected**: that link is dropped.
- **Internet breaker tripped** (`Content::internet_looks_down`): no reads, no
  dials; existing links are left to fips's own liveness.

## Battery

- **On screen**: adverts re-read every 10 minutes; redials as above.
- **Off screen** (`ProcessLifecycleOwner` stop): adverts every hour, and no
  node redialled more often than every 5 minutes. Existing links stay up —
  dropping them would cost a handshake each to come back, and staying
  reachable in a pocket is the point.
- A public node never replaces a radio path. It is a different peer from any
  Circle member, so a member reachable over BLE, Aware or the LAN keeps that
  direct link.

## Diagnostics

The Dev tab's "Public nodes (internet)" card shows whether the lane is on or
held (mesh-only, no internet), how many dialable nodes the last read found
and from how many relays, and each connected, connecting or selected node
with its endpoint, round trip and last dial error. Public nodes also appear
in the peer list like any other direct peer.

## Not done

- **TCP endpoints** are not dialled. They would need a fips TCP transport
  that Kotlin can pin, and fips exposes only UDP sockets to the embedder
  today.
- **NAT-only nodes** (`udp:nat`) need fips's Nostr rendezvous, which Myco
  does not run.
- **Tor**, the private alternative the roadmap mentions, is not wired.
- **A route view per Circle member** ("reached via test-de01") is not shown.
  fips's `show_peers` lists only direct peers.

## Testing

Host: `cargo test -p myco-core public_node` covers parsing and validation,
the join.fips.network extraction, selection deltas, the dial plan (target,
backoff, background, mesh-only, breaker, switch-off, deselection, Circle
exemption) and persistence.

On device (the N10 exit criterion), with two paired phones on different
networks and no radio path between them (one on Wi-Fi, one on cellular,
Bluetooth off on both):

1. On both: Settings › Mesh › Internet → on. Within a minute the Dev tab's
   card shows two connected nodes.
2. The Circle shows the other phone as reachable.
3. Send a message from a napplet that uses `mesh.publish`; it arrives.
4. Switch the option off on one phone. Its public links drop from the Dev
   tab, and the other phone goes unreachable.
5. Turn on mesh-only with the option on: links drop and are not redialled.
