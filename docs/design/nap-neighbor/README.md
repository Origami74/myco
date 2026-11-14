# Neighborhoods (NAP-NEIGHBOR)

Neighborhoods are how Myco devices share with each other. They replace the
Circle and pairing.

Napplets reach them through **NAP-NEIGHBOR**, proposed in
[napplet/naps#146](https://github.com/napplet/naps/pull/146). The wire
protocol underneath (links, offers, scoped traffic) is Myco's draft of a
transport-independent Nostr protocol. It is not a NIP yet.

## Pages

| Page | Covers | Code |
| --- | --- | --- |
| [links.md](links.md) | Invitations, setup proof, link keys, offers, NIP-98, the link service on `:4873` | `myco-core/src/neighbor/{link,service}.rs` |
| [data-plane.md](data-plane.md) | Scoped store and blobs, the scoped relay, publish and subscribe, hops, dedup | `myco-core/src/neighbor/{data,wire}.rs` |
| [policy.md](policy.md) | Per-neighborhood settings: reach presets, blob push hops, Apps | `myco-core/src/neighbor/store.rs` |
| [app-peers.md](app-peers.md) | Myco's own basics over neighborhoods, the `:4870` / `:24243` gate | `myco-core/src/neighbor/apps.rs` |
| [napplet-api.md](napplet-api.md) | What NAP-NEIGHBOR exposes, `neighborhood://` addresses, how other NAPs carry them | `myco-napplet-runtime/src/nap/neighbor/`, `myco-core/src/neighbor/host.rs` |
| [ui.md](ui.md) | Neighbors tab, neighborhood page, invite and join | `myco-core/src/neighbor/view.rs`, `android/…/app/myco/neighbor/` |

## Words

| Word | Meaning |
| --- | --- |
| **Neighborhood** | A propagation channel ("Household", "Sports club"). No members, no roster, no admin. |
| **Link** | A pairwise relationship between two devices, set up once from an invitation, used over any network. |
| **Link key** | A key generated for one link. Not the FIPS device key, not the user's social key. |
| **Invitation** | A one-time `myco://link/…` payload: QR, share link or NFC tap. |
| **Offer** | A signed event (kind 37516) listing the endpoints one side serves on a link. |
| **App peer** | A linked device in a neighborhood with **Apps** on. Myco fetches apps and updates from app peers. |

## What a neighborhood is

- **An id, not a group.** 32 bytes (`neighbor/id.rs`). Random for a
  neighborhood someone starts, so two "Book club"s never merge. Derived from
  a name (`sha256("nostr-neighborhood:" || lowercase name)`) for
  auto-accepted ones, so everyone typing the same name meets.
- **Carried on links.** Each link carries the neighborhoods both sides agreed
  to. An event arriving on one link is passed on to the device's other links
  in the same neighborhood, within local limits.
- **No member control.** Nobody decides who is "in". Reach is decided link
  by link. Anyone who has an id may pass it on, but it grants nothing: a
  device only takes a neighborhood's traffic on links where it agreed to
  carry it.
- **Not private.** People can pass posts on, even outside the neighborhood.
  The join sheet says so.
- **Names are local.** A display name travels with the id when shared. Each
  device can rename it locally; the id stays.

## Keys

Three keys, never mixed:

| Key | Is | Used for |
| --- | --- | --- |
| Device key | The FIPS node identity (`identity.nsec`) | Mesh transport, `<npub>.fips` addresses |
| User key | The social Nostr identity (guest, `nsec`, or signer app) | Events napplets publish |
| Link key | One per link, generated at setup | Link setup, NIP-98 auth, signing offers |

See [../core/identity.md](../core/identity.md) for the device key.

## Files on the device

| File | Holds |
| --- | --- |
| `neighborhoods.json` | Each neighborhood: id, local name, policy, offer, mutes, napplet grants and tokens |
| `links.json` | Links and open invitations, with link secret keys |
| `neighborhood-relay/` | Neighborhood events |
| `neighborhood-blobs/` | Neighborhood blobs |
| `neighborhood-scopes.json` | Event id or `blob:<sha256>` → the neighborhoods it belongs to |

## Proximity (optional extension)

NAP-NEIGHBOR makes proximity optional. The planned extension adds an
auto-accepted `nearby` neighborhood on short-lived links to devices in
radio range. It is not built.

## Removed with the Circle

- The Circle, pairing, `myco://pair/`, the `/pair` endpoint and kinds
  9101–9103. `circle.json` and `outbound_pairs.json` are deleted on first
  launch. Circles are not migrated: people re-invite each other.
- Event gossip of local events, REQ pull forwarding, backlog resync and
  outbox mesh lanes. Neighborhoods carry that traffic now.
- NAP-MESH (the `mesh` domain) and the **App reach** setting. NAP-NEIGHBOR
  replaces them.

## See also

- [../core/concepts.md](../core/concepts.md) — glossary.
- [../core/security.md](../core/security.md) — what a link authorizes.
- [../../reference/ports.md](../../reference/ports.md) — `:4873`, `:4870`, `:24243`.
- [../napplet/napplet-runtime.md](../napplet/napplet-runtime.md) — the napplet host.
