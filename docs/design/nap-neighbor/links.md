# Links

A link is a pairwise relationship between two devices. It is set up once
from an invitation and then used over any network.

Code: `myco-core/src/neighbor/link.rs` (the protocol, no sockets) and
`myco-core/src/neighbor/service.rs` (the link service over HTTP).

## Link keys

- Each side generates a **link key** per link. It is neither the device's
  FIPS key nor the user's social key.
- Two of a person's peers cannot tie their links together by key.
- Link keys live in `links.json`. They never leave Rust.
- Over FIPS the transport still shows the device key. Moving that onto the
  link key needs multi-identity support in fips.

## Invitations

1. A taps **Invite** and picks one or more neighborhoods (or creates one).
2. Myco makes a link key and a one-time 32-byte secret, valid 15 minutes.
3. The invitation is shown as a QR code, a shareable link, and an NFC tag
   while the sheet is open.

```text
https://napplet.link/neighborhood/join?invite=<base64url(JSON)>

{ "v": 1,
  "link": ["http://<npub>.fips:4873/nh/<A's link pk>"],
  "pk":   "<A's link pk>",
  "s":    "<secret, hex>",
  "n":    [{ "id": "<hex>", "name": "Household" }],
  "exp":  <unix seconds>,
  "from": "Alice" }
```

The link is the `neighborhood/join` convention (NAP-INTENT's napplet
link). Myco's shell handles it itself and never hands it to a napplet: it
opens Myco where Myco is installed, and napplet.link's web page elsewhere.
The `napplet:neighborhood/join?invite=…` spelling and the older
`myco://link/<base64url(JSON)>` are read too.

## Setup

1. B scans or taps (**Join**), sees who is inviting and to what, and picks
   which neighborhoods to accept.
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
for: the inviter's `from` in the invitation, the joiner's name in its setup
request. Nothing advertises a name over Bluetooth any more. Names older
builds still advertise are read, never sent.

## Changing and ending a link

- **Adding a neighborhood** to a link (`POST /neighborhoods`) needs the
  other user's yes. Until then it is pending on their side.
- **Removing** one takes effect at once.
- **Ending a link** ("Disconnect") stops every neighborhood with that
  person and tells them (`POST /end`). What was received stays on the phone.

## Offers

An offer is a replaceable event, kind **37516** (a placeholder until the
protocol has a number), signed by the link key, `d` tag `offer`, optional
`name` tag. Its content lists services:

| Type | Myco offers | Flags |
| --- | --- | --- |
| `link` | `http://<npub>.fips:4873/nh/<pk>` | — |
| `relay` | `ws://<npub>.fips:4873/nh/<pk>/relay` | read, write |

- Both live under the link service on `:4873`. Neighborhood blobs are
  read through the link route (`/nh/<pk>/blob/…`); there is no separate
  Blossom service.
- `read` / `write` cover the neighborhood's content. `own` would open what a
  device keeps outside any neighborhood. Myco does not offer it.
- An offer is a statement, not access control. Every request is still
  checked.
- A peer may list several URLs per service (FIPS, Tor, public). Myco prefers
  FIPS, since it needs no internet. Today Myco only offers FIPS URLs.

## The link service (`:4873`)

The auth plane on `:4873` carries the link service. `{pk}` is this side's
link key for the link, so each link has its own URL.

```text
POST /nh/{pk}/setup          one-time secret proof     -> offer, ids on the link
GET  /nh/{pk}/offer          NIP-98                    -> current offer
POST /nh/{pk}/neighborhoods  NIP-98  {add, remove}     -> ids, pending
POST /nh/{pk}/end            NIP-98                    -> 204
GET  /nh/{pk}/relay?n=&ttl=  NIP-98, WebSocket         -> NIP-01 scoped to n
GET  /nh/{pk}/blob/{sha256}?n=  NIP-98               -> a blob in n's scope
```

- **Auth.** Everything after setup carries a NIP-98 `Authorization` header
  signed by the caller's link key (kind 27235, ±60 s). The signer must be a
  link this device holds, on the `{pk}` named.
- **Body limit.** 8 KiB.
- The relay route is described in [data-plane.md](data-plane.md).

## Presence

Every 30 s, Myco asks each peer for its offer. An answer marks the link
reachable for 120 s ("Here now") and refreshes the peer's name and
endpoints.
