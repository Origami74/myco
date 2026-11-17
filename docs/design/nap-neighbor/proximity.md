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

## The nearby secret

Each phone advertises a 4-byte secret over BLE, in the primary advert right
after its L2CAP PSM (the service data under `0x9C90`: 2 bytes PSM, then 4
bytes secret, which fills the 31-byte advert). Hearing it is the proof of
being near. It is not derived from any key, so it does not depend on the
FIPS npub.

- **Used once.** The first setup that proves it wins, and a new secret is
  advertised. Racing setups with the same secret get `409 used`, drop it
  and try again with the next secret they hear, up to 8 rounds per dial;
  the next tick starts over.
- **Replaced** every 5 minutes if nobody used it, and whenever nearby
  starts over (nobody in reach, node stopped or started, Nearby off).
- **Heard secrets** are offered for a minute after they were last heard,
  at most 16. The radio reports one when it changes and every 20 s while it
  is still heard. A phone never offers its own.
- **mDNS** can carry the same secret later, for the local network. Until
  then a phone reached only over the LAN does not link through Nearby.

## Setup: `POST /nh/nearby`

The caller proves it heard the server's current secret.

```text
POST http://<server npub>.fips:4873/nh/nearby
Authorization: Nostr <NIP-98 over this URL, signed by the caller's new link key>

{ "pk":    "<caller's new link key, hex>",
  "offer": <caller's offer, kind 37516, signed by that key, no name>,
  "n":     ["<id of nearby>", "<other auto-accepted ids it carries>"],
  "proofs": ["<hex HMAC-SHA256(secret, \"myco-nearby:\" ‖ pk)>", …] }

-> 200
{ "pk":    "<server's new link key, hex>",
  "offer": <server's offer, signed by that key, no name>,
  "n":     ["<ids both carry>"] }

-> 409   proof i was for a secret already used or replaced: retry with the next
{ "error": "used", "proof": i }
```

The server refuses (`403 refused`, nothing more) unless:

1. The request's source address maps to a peer directly in reach. On the
   mesh a source is a FIPS address (`fd…`), derived from the device key, so
   it names exactly one npub.
2. Nearby is on here.
3. The NIP-98 header is signed by the `pk` in the body, for this URL.
4. The offer is valid and signed by that key.
5. `nearby` is among the ids both carry.
6. One proof is for the current secret. Checked last and atomically, so a
   refused request uses nothing up and of racing requests exactly one
   wins.

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
