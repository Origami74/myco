# Links

A link is a pairwise relationship between two devices. It is set up once
from an invitation and then used over any network.

Code: `myco-core/src/neighbor/link.rs` (the protocol, no sockets) and
`myco-core/src/neighbor/service.rs` (the link service over HTTP).

NAP-NEIGHBOR leaves the peering protocol out of scope. Everything on this
page is Myco's: invitations, the setup proof, link keys, offers (kind
37516), NIP-98 use and the `/nh/…` routes.

## Link keys

- Each side generates a **link key** per link. It is neither the device's
  FIPS key nor the user's social key.
- Two of a person's peers cannot tie their links together by key.
- Link keys live in `links.json`. They never leave Rust. Ephemeral
  (nearby) links are never written there.
- Over FIPS the transport still shows the device key.
- A `links.json` that can't be read is renamed `links.json.bad` and Myco
  starts with no links, rather than overwrite it.

## Invitations

1. A taps **Invite** and picks one or more neighborhoods (or creates one).
2. Myco makes a link key and a one-time 32-byte secret, valid 15 minutes.
3. The invitation is shown as a QR code, a shareable link, and an NFC tag
   while the sheet is open.

```text
https://napplet.link/neighborhood/join#invite=<base64url(JSON)>

{ "v": 1,
  "link": ["http://<npub>.fips:4873/nh/<A's link pk>"],
  "pk":   "<A's link pk>",
  "s":    "<secret, hex>",
  "n":    [{ "id": "<hex>", "name": "Household" }],
  "exp":  <unix seconds>,
  "from": "Alice" }
```

- **In the fragment.** The payload rides after `#`, which a browser never
  sends, so napplet.link's web host never sees the secret.
- **Shell-reserved.** `neighborhood` is a shell-reserved archetype: Myco's
  shell handles `neighborhood/join` itself and never resolves it to a
  napplet ([../napplet/napplet-runtime.md](../napplet/napplet-runtime.md#nappletlink)).
  The link opens Myco where Myco is installed, and napplet.link's web page
  elsewhere.
- **Older spellings** are read too: `?invite=` in the query,
  `napplet:neighborhood/join?invite=…` or `#invite=…`, and
  `myco://link/<base64url(JSON)>`.
- **Expired** invitations can't be joined. The join sheet says so.

## Setup

1. B scans or taps (**Join**), sees who is inviting and to what, and picks
   which neighborhoods to accept. The sheet also shows where the
   invitation came from (link, QR code or phone tap) and a short code of
   A's link key, which A's invite sheet shows too, so they can compare.
2. B makes its own link key and calls `POST <link>/setup` with its key, the
   accepted ids, its signed offer, and a proof:
   `HMAC-SHA256(s, A.pk ‖ B.pk ‖ url)`.
3. A checks the proof, and that the secret is unused and unexpired. It
   spends the secret and answers with its own offer.
4. Both store the link. It carries the ids A offered and B accepted.

The proof binds both keys and the URL, so a relayed answer is useless at
another endpoint or with another key. Every refusal says only `refused`.

## Names

Names travel only in invitations and setup, to the one person they are
for: the inviter's `from` in the invitation, and each side's name in the
offer it signs during setup. Kept names are trimmed to 64 characters.

- **Ephemeral (nearby) links carry no name.** Neither side's offer has
  one.
- **Later offers carry no name.** A name changed after setup does not reach
  existing peers.
- **Nothing advertises a name over Bluetooth.** Names older builds still
  advertise are read, never sent.

## Changing and ending a link

- **Adding a neighborhood** to a link (`POST /neighborhoods`) needs the
  other user's yes. Until then it is pending on their side, at most 8 per
  link (the oldest dropped first). On an ephemeral
  link, auto-accepted ones both carry are added at once and others ignored.
- **Removing** one takes effect at once.
- **Ending a link** ("Disconnect") stops every neighborhood with that
  person and tells them (`POST /end`). What was received stays on the phone.

## Offers

An offer is a replaceable event, kind **37516** (Myco's, a placeholder
until the protocol has a number), signed by the link key, `d` tag `offer`,
optional `name` tag (setup only). Its content lists services:

| Type | Myco offers | Flags |
| --- | --- | --- |
| `link` | `http://<npub>.fips:4873/nh/<pk>` | — |
| `relay` | `ws://<npub>.fips:4873/nh/<pk>/relay` | read, write |

- Both live under the link service on `:4873`. Neighborhood blobs are
  read through the link route (`/nh/<pk>/blob/…`); there is no separate
  Blossom service.
- `read` / `write` cover the neighborhood's content. `own` would open what a
  device keeps outside any neighborhood. Myco does not offer it.
- An offer is enforced on every request: with relay reads off, a peer's
  `REQ` is refused; with relay writes off, its `EVENT`; with blob reads off,
  its blob requests ([policy.md](policy.md#offer-switches)).
- A peer may list several URLs per service (FIPS, Tor, public). Myco prefers
  FIPS, since it needs no internet. Today Myco only offers FIPS URLs.

## The link service (`:4873`)

The auth plane on `:4873` carries the link service. `{pk}` is this side's
link key for the link, so each link has its own URL.

```text
POST /nh/nearby                 NIP-98, direct peers only -> offer, ids on the link
POST /nh/{pk}/setup             one-time secret proof     -> offer, ids on the link
GET  /nh/{pk}/offer             NIP-98                    -> current offer
POST /nh/{pk}/neighborhoods     NIP-98  {add, remove}     -> ids, pending
POST /nh/{pk}/end               NIP-98                    -> 204
GET  /nh/{pk}/relay?n=&ttl=     NIP-98, WebSocket         -> NIP-01 scoped to n
GET  /nh/{pk}/blob/{sha256}?n=  NIP-98                    -> a blob in n's scope
HEAD /nh/{pk}/blob/{sha256}?n=  NIP-98                    -> whether GET would serve it
```

- **Auth.** Everything after setup carries a NIP-98 `Authorization` header
  signed by the caller's link key (kind 27235, ±60 s). The signer must be a
  link this device holds, on the `{pk}` named.
- **Replays.** Every header carries a random `nonce` tag, and a `POST`
  carries a `payload` tag with the body's sha256. An accepted header is
  remembered for 120 s and refused if sent again.
- **`/nh/nearby`** sets up an ephemeral link from any device directly in
  reach that proves it heard this phone's Bluetooth secret. See
  [proximity.md](proximity.md).
- **Limits.** See [ports.md](../../reference/ports.md#1a-link-service--4873).
- The relay route is described in [data-plane.md](data-plane.md).

**Dialling a peer.** URLs from a peer's offer or invitation are dialled
only if they name a `.fips` host, a mesh address or a public internet
host. Loopback, LAN, link-local and other private (ULA) addresses are
refused, and a name is checked again after DNS. Answers are read up to
64 KiB.

## Presence

Every 30 s, Myco asks each peer for its offer. An answer marks the link
reachable for 120 s ("Here now") and refreshes the peer's endpoints. It
carries no name.
