# Proximity: the `nearby` neighborhood

NAP-NEIGHBOR's optional proximity extension. Myco has the radios, so it
implements it: a neighborhood of the people around you right now, with no
invitation.

Code: `myco-core/src/neighbor/nearby.rs` (who is in reach, setup, the
loop), `host.rs` (the neighborhood and the setting), `link.rs` (ephemeral
links), `service.rs` (`POST /nh/nearby`).

## The neighborhood

- **Named `nearby`, auto-accepted.** Its id is derived from the name like any
  auto-accepted neighborhood's, so every Myco meets in the same one.
- **Kept by the runtime.** Present while Nearby is on, gone when it is off.
  It is a normal record in `neighborhoods.json` and follows the normal
  storage rules.
- **On by default**, existing installs included. The setting is stored as
  `nearbyOff` in `neighborhoods.json`, so a file without it reads as on.
- **Turning it off** forgets the neighborhood and ends every ephemeral link,
  telling each peer. Turning it back on brings back the same id.

## Napplets

- Every napplet with `neighbor` sees `nearby` like any neighborhood.
- `neighbor.info` lists `nearby` among the features while it is on.
- `create`, `invite`, `setOffer` and `leave` on it fail with `unsupported`.
  Only the user turns it off, in the shell.
- A peer on an ephemeral link reports `link: "proximity"` over a radio or
  cable, `link: "local"` over the local network.

## Who counts as nearby

A FIPS peer with a **direct** link, read from the node's peer snapshot
(`show_peers`, polled every second):

| Path | Counts | Link class |
| --- | --- | --- |
| BLE | yes | `proximity` |
| Wi-Fi Aware | yes | `proximity` |
| USB | yes | `proximity` |
| Plain UDP to a non-public address (same Wi-Fi, a hotspot) | yes | `local` |
| UDP to a public address (public nodes) | no | — |
| TCP (the internet lane) | no | — |

- fips lists only direct link peers, so anyone reached multi-hop is never
  in the snapshot.
- A live BLE, Aware or USB path counts even while another path is active.
- Wi-Fi Aware and the LAN/hotspot lane both ride UDP; the radios' lane
  record tells them apart.
- A private address reached through a VPN would count as local. Rare, and
  accepted for now.

The check sits behind the `DirectPeers` trait, so host tests use a fake.

## Ephemeral links

A link set up without a secret, for one encounter.

- **Who dials.** Every 10 s the runtime looks at the direct peers. For each
  one without an ephemeral link, the side with the lower device npub dials.
  One link per pair, not two.
- **Where.** The peer's link service at `http://<npub>.fips:4873`.
- **What it carries.** Only auto-accepted neighborhoods both sides carry:
  `nearby`, plus any other they share. One joined later on either side is
  added automatically.
- **When it ends.** About 5 minutes after the peer stops being direct.
  Also when the peer is in reach but stops answering for 2 minutes (it
  restarted and forgot the link); it is then set up afresh.
- **Forgotten, all at once,** as soon as nobody is directly in reach, and
  whenever the node stops or starts. Every ephemeral link ends, with
  everything nearby knew about it; the peers are not told. The next
  meeting is between strangers, on fresh link keys.
- **Not kept.** Ephemeral links are dropped when Myco starts.
- **Presence** works as on any link: the 30 s offer check makes "Here now".

## Setup: `POST /nh/nearby`

No secret. The FIPS session already authenticated the caller's device key,
and the server checks the request came from it.

```text
POST http://<server npub>.fips:4873/nh/nearby
Authorization: Nostr <NIP-98 over this URL, signed by the caller's new link key>

{ "pk":    "<caller's new link key, hex>",
  "offer": <caller's offer, kind 37516, signed by that key, no name>,
  "n":     ["<id of nearby>", "<other auto-accepted ids it carries>"] }

-> 200
{ "pk":    "<server's new link key, hex>",
  "offer": <server's offer, signed by that key, no name>,
  "n":     ["<ids both carry>"] }
```

The server refuses (`403 refused`, nothing more) unless:

1. The request's source address maps to a peer directly in reach. On the
   mesh a source is a FIPS address (`fd…`), derived from the device key, so
   it names exactly one npub.
2. Nearby is on here.
3. The NIP-98 header is signed by the `pk` in the body, for this URL.
4. The offer is valid and signed by that key.
5. `nearby` is among the ids both carry.

A request from a device that already has an ephemeral link replaces it: the
caller lost its side. The auth plane's per-address rate limit applies.

## Names

None. A nearby offer never carries a name, and nothing nearby sends does.
Napplets see nearby peers without one. Names travel only in invitations,
to people the user chose to invite.

## Not yet

- Keep from Nearby: turning an ephemeral link into a lasting one with a
  shared emoji code.
- Caps for crowds.
- A per-napplet rate cap on posting into `nearby`.
