# The Circle

The **Circle** is layer 2 of Myco ([concepts.md](../core/concepts.md)): the
list of people this phone has paired with, and everything that list decides.
It is the thing most easily confused with the mesh, so this page starts with
the difference.

---

## 1. A virtual mesh on top of the physical one

FIPS (layer 4) is the **physical mesh**. It links this phone to whatever
compatible node is in range — over BLE, Wi-Fi Aware, the LAN, TCP — and routes
datagrams through those links. It is promiscuous by design: a stranger's phone
across the café is a *peer*, gets a Noise-authenticated link, and may be the
hop your packet takes to reach a friend. FIPS decides **reachability**. It
knows nothing about trust, and it forgets a peer the moment the link drops.

The Circle is a **virtual mesh** laid over it. Its nodes are people, not
radios; its edges are pairings, made on purpose by two people holding their
phones together; it persists on both phones whether or not there is a link
between them. Everything Myco does *between* phones — pull an app, replicate
an event, ring a doorbell, send a file — happens along Circle edges and only
along Circle edges. FIPS carries the bytes; the Circle says whether they should
be sent at all.

```
   Circle (virtual)      alice ──── bob ──── carol            trust: who
                           │                  ╱
                           └──── dave ───────╯

   FIPS (physical)      alice ═ bob ═ mallory ═ carol         reach: how
                          ║             ║
                         dave ══════════╝
```

Alice can reach Carol only through Mallory's phone; Mallory forwards the
encrypted packets and learns nothing but that two node addresses are talking.
Mallory is a FIPS peer of everyone and in nobody's Circle: no relay of hers is
read, none of her events are taken, nothing is served to her. Dave is in
Alice's Circle and out of range: his row stays in her Circle, greyed, and
catches up when he is back.

So: **a peer is a fact about radio; a Circle member is a decision about
trust.** The docs never use one word for the other.

## 2. Built intentionally — a web of trust

There is no membership authority. Nobody approves a Circle, nobody signs a
roster, there is no join event. A Circle is exactly the set of people whose
phone you held against yours (or whose code you scanned), each of whom did the
same for you. It grows one deliberate act at a time, and each act is
**mutual** and **signed**: a one-time secret in the presented code proves the
scan happened, a signed request carries it to the presenter's auth service
over a Noise-encrypted link, and a signed accept comes back. Forgetting someone
is signed too, and reaches their phone, so the graph stays symmetric.

That makes the Circle a **web of trust** in the plain sense: each edge is a
first-hand attestation by two people, and the graph's shape *is* the social
one. Reach beyond your own edges — pulling from a friend of a friend, gossip
that hops three phones — is bounded on purpose (§4) and, where it is extended
later, will be extended along Circle edges with the members' consent, never by
enumerating the physical mesh. Design of the handshake:
[../core/identity-pairing.md](../core/identity-pairing.md).

## 3. What the Circle decides

| Concern | Rule |
| --- | --- |
| **Admission** | The relay and Blossom servers check every mesh connection against the Circle (the *gate*). A stranger reaches one thing: the auth service, to ask to pair. |
| **Sources** | An app is pulled from the holder who shared it, then any reachable Circle member, then the internet. Discovery ("around me") queries Circle members' relays. |
| **Gossip** | An accepted event is fanned out to Circle members with a hop budget; backlog is pulled from them; a member who reappears gets your open subscriptions replayed. All edges are Circle edges. |
| **Napplets** | NAP-MESH's "everyone nearby" is the Circle, `ttl` hops out. NAP-OUTBOX's mesh lane reaches a member's relay directly; a stranger's `.fips` URL in a relay list is dropped. A `blossom:` miss is asked of reachable members. |
| **Files** | Native encrypted file sharing is offered to, and accepted from, Circle members only. |
| **Per-member permissions** | Each contact carries `relay_read`, `relay_read_multihop`, `relay_write`, `blossom_write` — what *they* may do to *this* phone. Stored per member, defaults for everyone, no UI yet ([../nsite/nsite-permissions.md](../nsite/nsite-permissions.md) §2). |

Everything in the table is checked by npub or by the mesh address derived from
it. A phone's FIPS peer table is never consulted for any of them.

## 4. Reach

A Circle edge is one hop of trust; a hop budget says how many edges a thing may
travel. Today: published events flood 3 edges out (`EVENT_TTL`), backlog pulls
ask 2 edges out (`MAX_REQ_TTL`), and a napplet may ask for less but never more
([../core/event-gossip.md](../core/event-gossip.md), [NAP-MESH](../napplet/NAP-MESH.md)).
Blobs never flood; they are pulled from whoever has them, verified by hash.
The seen-set, not the budget, is what makes a flood terminate — the budget only
bounds cost and reach.

Because edges are pairings, "3 hops" means *friends of friends of friends*,
whatever the radios are doing. Two members in one room with no link between
them exchange nothing until a link exists; two members ten hops apart on the
physical mesh, one Circle edge apart, exchange everything.

## 5. What the Circle is not

- **Not a group.** There is no shared name, no shared membership view. Alice's
  Circle and Bob's Circle are two lists that happen to contain each other.
- **Not a channel.** Nothing is addressed to "the Circle"; things are addressed
  to members, and a hop budget lets them travel further.
- **Not the peer table.** The Dev tab's peer list is FIPS's; the Circle tab is
  Myco's. A row can be in one and not the other.
- **Not a roster.** No signer, no admin, no revocation authority beyond each
  person's own phone.

## 6. Where this is going

The Circle layer will own the **channel** between members. Today a member is
reached by dialling this phone's relay and Blossom at `<npub>.fips:4870` /
`:24243` — the mesh ports, which are **deprecated** and will be removed for both
servers. In their place, sync, gossip and file transfer between Circle members
run over a Circle-owned session, so the relay and the store are never sockets a
peer can dial, and admission is decided once, at the edge, rather than on every
listener. Transitive reach (a member's members, with consent) and set
reconciliation (NIP-77) then sit naturally on that channel. See the
[roadmap](../../roadmap.md).

---

## See also

- [../core/identity-pairing.md](../core/identity-pairing.md) — the handshake,
  NFC and QR, the auth service, unpairing.
- [../core/security.md](../core/security.md) — the gate, what a member can
  learn, what a stranger cannot.
- [../core/event-gossip.md](../core/event-gossip.md) — the push and pull planes
  along Circle edges.
- [../nsite/nsite-permissions.md](../nsite/nsite-permissions.md) — per-member
  permissions.
