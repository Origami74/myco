# Proximity: the `nearby` neighborhood

NAP-NEIGHBOR's optional proximity extension. Myco has the radios, so it
implements it: a neighborhood of the people around you right now, with no
invitation.

The spec leaves discovery to the runtime. The nearby handshake here (the
Bluetooth secret, `POST /nh/nearby`, the proof label) is Myco's proposal
for the proximity extension, not yet in the spec.

Code: `myco-core/src/neighbor/nearby.rs` (who is in reach, setup, the
loop), `host.rs` (the neighborhood and the setting), `link.rs` (ephemeral
links), `service.rs` (`POST /nh/nearby`).

## The neighborhood

- **Named `nearby`, auto-accepted.** Its id is derived from the name like any
  auto-accepted neighborhood's (Myco's derivation, see
  [README.md](README.md#what-a-neighborhood-is)), so Myco runtimes meet in
  the same one.
- **Kept by the runtime.** Present while Nearby is on, gone when it is off.
  It is a normal record in `neighborhoods.json` and follows the normal
  storage rules.
- **On by default**, existing installs included. The setting is stored as
  `nearbyOff` in `neighborhoods.json`, so a file without it reads as on.
- **Its own reach:** publish hops 1, pass on off. Everyone in reach is a
  stranger, so a post goes to those in reach and no further. Extensions
  are still shared, so AppStore's Around you shows what phones in reach
  hold.
- **Turning it off** forgets the neighborhood and what it held, and ends
  every ephemeral link, telling each peer. Turning it back on brings back
  the same id.

## Napplets

- Every napplet with `neighbor` sees `nearby` like any neighborhood.
- `neighbor.info` lists `nearby` among the features while it is on.
- `create`, `invite` and `setOffer` on it fail with `unsupported`.
- `leave` on it asks the user, then turns Nearby off.
- A peer on an ephemeral link reports `link: "proximity"` over a radio or
  cable, `link: "local"` over the local network.

## Who counts as nearby

A FIPS peer with a **direct** link, read from the node's peer snapshot
(`show_peers`, polled every second):

| Path | Counts | Link class |
| --- | --- | --- |
| BLE | yes | `proximity` |
| Wi-Fi Aware | yes, with a heard BLE secret | `proximity` |
| USB | yes, with a heard BLE secret | `proximity` |
| Plain UDP to a non-public address (same Wi-Fi, a hotspot) | yes, with a heard BLE secret | `local` |
| UDP to a public address (public nodes) | no | — |
| TCP (the internet lane) | no | — |

- fips lists only direct link peers, so anyone reached multi-hop is never
  in the snapshot.
- A live BLE, Aware or USB path counts even while another path is active.
- Wi-Fi Aware and the LAN/hotspot lane both ride UDP; the radios' lane
  record tells them apart.
- Every path also needs this phone to have heard the peer's Bluetooth
  secret (below): no setup works without one.
- A private address reached through a VPN would count as local. Rare, and
  accepted.

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
- **Forgotten, all at once,** as soon as nobody is directly in reach,
  when Nearby is turned off, and whenever the node stops or starts. Every
  ephemeral link ends, with everything nearby knew about it, and what the
  `nearby` neighborhood held is deleted. The peers are not told. The next
  meeting is between strangers, on fresh link keys.
- **Never written to disk.** Ephemeral links live in memory only.
- **Caps.** At most 32 ephemeral links, and 12 accepted setups a minute.
  Past either, `POST /nh/nearby` answers `503 {"error":"busy"}` without
  using the secret.
- **Presence** works as on any link: the 30 s offer check makes "Here now".

## The nearby secret

Each phone advertises a 4-byte secret over BLE, in the primary advert right
after its L2CAP PSM. Both share one service-data entry under `0x9C90`: 2
bytes PSM, then 4 bytes secret. The advert is exactly 31 bytes. Hearing
the secret is the proof of being near. It is not derived from any key, so
it does not depend on the FIPS npub.

- **Used once.** The first setup that proves it wins, and a new secret is
  advertised. Racing setups with the same secret get `409 used`, drop it
  and try again with the next secret they hear. A `409`, or a `403` after
  the first round, is retried up to 8 rounds within 25 s; the next tick
  starts over.
- **Replaced** after use, every 5 minutes, and when nearby starts over
  having forgotten something. Starting over with nothing to forget keeps
  it, so calling it again costs the advert nothing.
- **None while Nearby is off.** Nothing is advertised.
- **Heard secrets** are kept in a table of up to 32, each with when it was
  first and last heard and how often. One expires 60 s after it was last
  heard, or 6 minutes after it was first heard. At most 8 never-heard
  secrets are taken in per 10 s, and secrets heard at least twice are
  preferred, so a beacon cycling made-up secrets can't push real phones
  out. A request offers at most 16. The radio reports a secret when it
  changes and every 20 s while it is still heard. A phone never offers its
  own.
- **Not over the LAN.** mDNS does not carry the secret. A phone reached
  only over the local network does not link through Nearby: it is never
  dialed, so it never gets proofs it could brute-force offline, and its
  setups are refused.
- **Guessing wears a secret out.** After 1024 wrong proofs a secret is
  replaced, so a guess succeeds with at most 2^-22 chance per secret.

## Setup: `POST /nh/nearby`

The caller proves it heard the server's current secret.

```text
POST http://<server npub>.fips:4873/nh/nearby
Authorization: Nostr <NIP-98 over this URL, signed by the caller's new link key>

{ "pk":    "<caller's new link key, hex>",
  "offer": <caller's offer, kind 37516, signed by that key, no name>,
  "n":     ["<id of nearby>", "<other auto-accepted ids it carries>"],
  "proofs": ["<hex HMAC-SHA256(secret, \"nap-neighbor:nearby:\" ‖ pk)>", …] }

-> 200
{ "pk":    "<server's new link key, hex>",
  "offer": <server's offer, signed by that key, no name>,
  "n":     ["<ids both carry>"] }

-> 409   proof i was for a secret already used or replaced: retry with the next
{ "error": "used", "proof": i }
```

The server refuses (`403 refused`, nothing more) unless:

1. The request's source address maps to a peer directly in reach by radio
   (BLE, Wi-Fi Aware, USB). On the mesh a source is a FIPS address (`fd…`),
   derived from the device key, so it names exactly one npub. Only
   connections on the mesh stack count: on a kernel socket beside it, a
   host on the LAN could claim any `fd…` source.
2. Nearby is on here.
3. The NIP-98 header is signed by the `pk` in the body, for this URL.
4. The offer is valid and signed by that key.
5. `nearby` is among the ids both carry.
6. One proof is for the current secret. Checked last and atomically, so a
   refused request uses nothing up and of racing requests exactly one
   wins.

A request from a device that already has an ephemeral link replaces it: the
caller lost its side. The auth plane's rate limits apply
([ports.md](../../reference/ports.md#1a-link-service--4873)).

## Files

Myco's own, interim file share, until a napplet takes it over (roadmap
N13). Share a file from any app and pick Myco: it goes to the people in
Nearby.

- **How it travels.** The bytes are kept in `nearby`'s scope on the
  sending phone. An announcement is published into `nearby` like any post,
  so the phones in reach see it. A phone that saves it fetches the bytes
  over its nearby link, checked against the hash.
- **The announcement** is filedrop's: a NIP-94 kind 1063 event with `x`,
  `m`, `size`, `file` (name, type, size), `t=filedrop`, `alt` and a NIP-40
  `expiration` an hour out. The napplet that replaces this reads it as is.
  Encrypted announcements are filedrop's own and not listed here.
- **Anonymous.** Each announcement is signed by a key made for it and
  thrown away. Nothing says who shared a file, and two files can't be tied
  to one phone by their key.
- **Not private.** Anyone in reach can see and save it, unencrypted; the
  share sheet says so. Like everything in Nearby it is forgotten when
  nobody is in reach.
- **Limits.** 64 MB per file, the neighborhood's quota in all. One hop: a
  phone saves from the phone that shared, so the sender must still be in
  reach.
- **On the phone.** Myco is in Android's share menu; a sheet confirms
  before sending. The Nearby card's **Files** opens the list, with **Save**
  to a place the user picks. A notification says when someone in reach
  shares a file (checked every 10 s).

Code: `myco-core/src/neighbor/files.rs`, `NearbyFiles.kt`,
`NearbyFileSheets.kt`.

## Names

None. A nearby offer never carries a name, and nothing nearby sends does.
Napplets see nearby peers without one. Names travel only in invitations,
to people the user chose to invite.

## Privacy cost

Nearby shares update manifests with strangers in reach: what apps this
phone has installed, and which updates it shares. That is what lets
AppStore's Around you work. Turning **Share** off on `nearby`, or Nearby
off, stops it.

## Known gap

Release builds made without `MYCO_FIPS_REPO_PATH` are single-path. With
no node prefix in the advert, they can't tell that a peer is already
reached over Wi-Fi Aware, and may dial it over BLE too.

## Not built

Keeping a nearby link as a lasting one, and a per-napplet cap on posting
into `nearby`.
