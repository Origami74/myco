# Live event gossip: push + pull for app data over the mesh

> Status: BUILT. The push plane, the pull plane, the `MESH` envelope, the
> proxy-owned seen-set and query ids are all in the tree. Remaining open items
> are marked **TBD / open**.

[Propagation](./propagation.md) specifies how author-signed **nsite manifests**
(kinds 15128/35128) + content-addressed blobs spread and survive partitions. This
document covers the sibling problem: how an **in-app Nostr client** (e.g.
[`myco-bitchat`](../../myco-bitchat/README.md)) gets *arbitrary* app events — a
chat message, a reaction — to the people physically around it, when the only
relay it can reach is the device's own embedded relay (`ws://localhost:4870`).

The enabling idea is the same one the whole project rests on: **the embedded
relay is fed by the FIPS mesh, so physical proximity *is* the transport.** A
plain Nostr client pointed at localhost becomes a nearby-chat client, because the
mesh gossips its events between peers.

> **Scope.** Core/native work in `myco-core`. The WebView never reaches `.fips`
> peers; only the native engine does. The nsite is an ordinary Nostr client and
> is unaware any of this exists — it just sees nearby peers' events appear in its
> localhost relay.

---

## 0. Two boundaries that stay plain NIP-01

Everything below sits between two links that must keep speaking stock NIP-01, and
one link that is ours to shape.

| Link | Protocol | Why |
| --- | --- | --- |
| nsite ↔ `ws://localhost:4870` | Plain NIP-01, no Myco verb or key | The in-app client is an ordinary Nostr client |
| proxy ↔ backing relay | Plain NIP-01 | Lets any relay sit behind Myco |
| proxy ↔ proxy over `.fips` | Plain NIP-01 **or** a `MESH` envelope (§2) | Nobody else is listening on it |

The Myco-specific code lives in one place: the **mesh relay proxy**
([`myco-core/src/mesh_relay.rs`](../../myco-core/src/mesh_relay.rs)), which
serves both the loopback socket and the mesh socket and holds the store behind
it. The store ([`myco-relay`](../../myco-relay/src/lib.rs)) has no mesh, ttl, or
circle concepts in it at all.

A `MESH` frame arriving on the **loopback** socket is refused with a `NOTICE`.
That is boundary 1 enforced rather than assumed, and it is the only route by
which an nsite could otherwise have asked for extra hops.

If a future change wants to put something Myco-shaped on the loopback socket or
on the backend link, that is not a tweak to this design — it is a reversal of it.
Background: [`reference/thinning-custom-relay.md`](../../reference/thinning-custom-relay.md).

---

## 1. Two planes, never conflated

Event gossip is split into two independent planes. Keeping them separate is what
makes the whole thing terminate and stay cheap.

### Plane A — Push (live fan-out)

When a device **originates** an event, or **receives** one in a push frame with
budget left, it forwards that event to its circle peers. This is a **live wave**:
it ripples outward a bounded number of hops while the event is fresh, then stops.
It is bounded by the envelope's `ttl` (§2–3) and triggered **only** by the push
path — origination and relayed `EVENT`s — never by a write landing in the store
(§4, the load-bearing invariant).

### Plane B — Pull (backlog reconcile on contact)

When a peer comes into range, the arriving device **pulls recent events** from
that one neighbour — the last *N* / last few minutes for the app's kind(s). This
is a direct **1-hop** request between two now-adjacent relays, so it carries **no
mesh metadata**. It is store-only: pulled events are stored for serving and
display, and **do not** re-enter the push wave.

A **multi-hop** pull also exists — a hop-bounded `REQ` flood used for *discovery*
rather than chat backlog (§7). It is driven by the core, not by an nsite.

### Why both

Push gives fast live delivery to the nearby cluster. Pull is what reaches
everyone push didn't: a device five hops away never receives the live wave, but
the moment it moves near *anyone* who stored the event, it pulls it directly.
Physical movement + neighbour catch-up does the long-range work — the way bitchat
actually spreads — which is exactly why a **short hop budget is fine** (§3).

---

## 2. The `MESH` envelope: hop state beside the message, not inside it

The push hop count is a property of the **live wave, not the event**. So it
travels *beside* the NIP-01 message, in an envelope that wraps it.

### 2.1 The frame

```jsonc
["MESH", {"ttl": 2}, ["EVENT", <event>]]
["MESH", {"ttl": 1, "qid": "…", "budgetMs": 5000}, ["REQ", <sub_id>, <filter>, …]]
```

The inner element is **exactly** what would go on the wire to any relay: the
event object and every filter object are canonical NIP-01, byte for byte. The
proxy reads the metadata, decides, and passes the inner element through
unchanged. Nothing is re-encoded anywhere in the path, so there is nothing to
strip and nothing to smuggle.

The wrapper is **verb-agnostic**. `["MESH", meta, <anything NIP-01>]` carries
`COUNT`, a future `NEG-OPEN`, or anything else without the proxy learning what
they are, and it is one grep to find every mesh frame in a log.

The shape lives in
[`myco-core/src/mesh_wire.rs`](../../myco-core/src/mesh_wire.rs).

### 2.2 What the metadata carries

| Field | Type | Meaning |
| --- | --- | --- |
| `ttl` | `u8` (omitted when 0) | Remaining forward hops. `0` means store it, do not pass it on. |
| `qid` | string, optional | Query id for a pull, stamped by the originating proxy so each node serves a given query once (§7). |
| `budgetMs` | `u32`, optional | Relative time budget for a pull. **Carried but not yet enforced** — every hop currently uses a hardcoded timeout. |

Future fields (a path vector, an origin hint, a rate class) extend the metadata,
never the inner message.

### 2.3 A plain NIP-01 frame is still valid on the mesh link

`["EVENT", …]` and `["REQ", …]` with no envelope mean "no mesh metadata": store
it, do not forward it. That is the right default for poking a peer with `nak`
over `.fips`, and it means an old or foreign relay on the link degrades to
single-hop rather than misbehaving.

### 2.4 Why not a field on the event (the design this replaced)

This section used to argue the opposite case: that the hop count should ride as a
**non-signed top-level `event-ttl` key added to the event object**, with a
matching `req-ttl` key added to a **filter object** inside a `REQ`.

The argument's technical premise was correct and is not why it was dropped. The
signed preimage is `[0, pubkey, created_at, kind, tags, content]`, so an extra
top-level sibling key leaves `id` and `sig` verifying fine. Three other problems
sank it:

- **Every relay in the mesh had to implement Myco's protocol.** Read on receipt,
  strip on store, re-attach on forward was a mesh-wide requirement. That is
  exactly the requirement we wanted to drop, because it is what stopped Citrine,
  strfry, or nostr-rs-relay sitting behind Myco.
- **Correctness depended on backend behaviour we do not control.** A relay that
  round-tripped unknown top-level keys would store and re-serve `event-ttl`,
  restarting a wave that should have ended.
- **`req-ttl` travelled through the query language.** A backend that ignores
  unknown filter keys is harmless; one that reads them as a tag constraint
  silently returns nothing. Either way, routing state was riding inside a filter.

The envelope keeps the useful half — the hop count is still outside anything
signed or stored — and concentrates the fork on the one link where it costs
nothing.

### 2.5 Why a wrapper and not new verbs

An earlier draft used flat `MESH-EVENT` and `MESH-REQ` verbs. With those, the
proxy has to take apart and rebuild every message type it knows, so each new verb
means new proxy code and another chance to mangle a filter. The wrapper forwards
anything NIP-01 without knowing what it is.

Two other shapes were rejected:

- **Trailing metadata on a stock verb**, e.g. `["EVENT", ev, {"ttl":2}]` — the
  `REQ` version is ambiguous with a filter, and metadata placed before the
  filters reads to any relay as a match-everything filter.
- **A per-connection ttl handshake** — stateful, and wrong as soon as one socket
  carries waves at different depths.

### 2.6 Originating hop budget

A local nsite sets nothing, and **cannot** set anything: it may not send a `MESH`
frame, so its publishes always originate at the gossiper's default (**3**,
`DEFAULT_EVENT_TTL` in [`gossip.rs`](../../myco-core/src/gossip.rs)). A single
message key must never change a message's cost by orders of magnitude, so
per-message reach is not a client-facing knob.

---

## 3. Forward rule

On receiving a push frame — an `EVENT` inside a `MESH` envelope, from a mesh peer:

1. **Seen?** If `event.id` is already in the proxy's seen-set → store it anyway
   (storing is idempotent) but **forward nothing**.
2. **New?** Store it, fan it to this device's live subscriptions, then let
   `fwd = min(meta.ttl, MAX_EVENT_TTL, peer_clamp)`. If `fwd > 0`, send
   `["MESH", {"ttl": fwd - 1}, ["EVENT", event]]` to **every circle peer except
   the sender** (split-horizon).

A `REQ` response is not a push frame and never triggers step 2. Neither does a
plain `["EVENT", …]` with no envelope, which arrives with no budget.

### The knobs

| Knob | Value | Meaning |
| --- | --- | --- |
| Originating ttl | **3** | How far *my own* events travel. The originator stamps it. |
| `MAX_EVENT_TTL` | **3** | Clamp on forwarding, so a peer sending `ttl: 255` cannot turn this device into an amplifier. Set to the originate default so own-origin waves aren't clamped by neighbours. |
| `relay_write_multihop` | per peer, default **on** | A per-peer clamp. Off means an inbound event's budget is treated as 0: store it, show it, never pass it on. See [nsite-permissions.md](./nsite-permissions.md). |

### Loop safety is the seen-set, not the hop budget

A Nostr `id` is content-derived and **stable**: re-broadcasting a signed event is
idempotent — every device, any number of hops away, computes the same id. So each
device forwards each id **at most once**, and the flood **always terminates**,
even with `ttl` set arbitrarily high. The budget bounds only **reach and cost**;
it is not a loop guard.

### Worked example — originating ttl = 3

```
A (origin) ─ttl3─▶ B ─ttl2─▶ C ─ttl1─▶ D ─ttl0─▶ E
                   (1 hop)   (2 hops)  (3 hops)  E stores, does NOT forward
```

The wave reaches ~4 hops out, then dies cleanly. Anyone further gets it via
Plane B when they next come into range.

---

## 4. The load-bearing invariant

> **A stored event never re-enters the push flood. Push is triggered by a
> publish-form EVENT carrying `ttl > 0` — origination, or a forwarded hop —
> never by a write landing in the relay store.**

### What enforces it

The proxy keeps its **own seen-set** of event ids and consults it *before* the
store is touched. Storing happens either way; only a **first sighting** fans out.
So the trigger for push is the proxy's own novelty judgement about an inbound
push frame, not anything the store reports.

This replaces the earlier mechanism, which leaned on strip-on-store: a pushed
`EVENT` carried `event-ttl` and a stored one came back without it, so the split
enforced itself. That worked, but it made a correctness property depend on the
backend faithfully dropping an unknown key.

### The naive propagator still breaks

"Subscribe to the relay, fan out on every new event" fails here. Plane B bites
you: a device that comes into range and pulls the last 21 messages would store
them, and a store-triggered push would kick off a **fresh wave for old messages,
with a fresh budget, every time someone new arrives**.

### The seen-set also fixes the churn this section used to warn about

The old text noted that the problem gets worse once a peer has GC'd an id at
expiry (§5) and sees it as "new" again. That was a real hole in the store-based
version: storage answers "do I hold this?", which is a different question from "have
I handled this?". An id GC'd at NIP-40 expiry looks new to the store on the next
pull.

The proxy's seen-set has its own retention, independent of what the store keeps:

| Case | Remembered until |
| --- | --- |
| Event with a NIP-40 `expiration` | Its own expiry |
| Event with no expiry (e.g. a manifest) | 30 minutes |
| Either, minimum | 60 seconds — so an already-stale arrival still cannot be re-forwarded at every hop |

Bounded at 4096 ids, oldest evicted first. Novelty is a property of this node's
history, so this node keeps it.

### Manifests are the same plane, a different policy

Manifest kinds (15128/35128) travel this same push plane, but with an
interest-aware download-then-forward policy and an active-version gate rather
than the plain forward rule above. See
[nsite-updates.md §4](./nsite-updates.md).

---

## 5. Ephemerality (the chat case)

The driving consumer, `myco-bitchat`, makes events **expire** (NIP-40
`["expiration", <ts>]`, +10 min) and shows a new arrival only the last ~21. For
that to hold, the store **GCs events past their `expiration` tag**, so the store
stays small, the Plane-B backlog stays bounded, and the room is genuinely
ephemeral. An event re-pulled after its own expiry is simply gone — there is
nothing to re-serve, which is the intended behaviour.

Expiry also bounds the seen-set: an expiring event's id is remembered exactly
until it expires (§4), so memory is naturally capped. Note that this is now a
*coincidence of retention policy*, not a dependency — the seen-set caps itself,
and NIP-40 GC is not something an arbitrary backend guarantees.

---

## 6. What shipped

- **Circle fan-out.** Published app events are gossiped to circle peers — v1
  default **all kinds** except the manifest kinds 15128/35128, which take the
  interest-aware path (§4).
- **Multi-hop flood.** The `MESH` envelope (§2), the §3 forward rule (seen-set +
  split-horizon + clamp), decrementing per hop.
- **Multi-hop pull.** Hop budget, mandatory query id, and a carried time budget
  on the `REQ` side (§7), driven by the core rather than by a client.
- **Per-peer clamps.** `relay_write_multihop` and `relay_read_multihop` turn
  `MAX_EVENT_TTL` and `MAX_REQ_TTL` into per-peer values.

Still to come: **Plane B backlog reconcile on peer contact** as a first-class
step (`{kinds, since}`, or piggybacking the negentropy reconcile planned for
manifests, [nsite-layer.md §2.4](./nsite-layer.md)). Today a reappearing peer is
caught up by replaying open local subscriptions against it, which covers the
common case but is not the same as a reconcile.

---

## 7. Decisions and open questions

### Decided — the pull plane is core-driven

A `REQ` from a **loopback** client returns its stored backlog and `EOSE` at
local-store speed and **never** fans out. That removes the multi-second hang a
client used to see when one peer was slow.

Multi-hop pull is a **core** operation instead — discovery ("nsites around me")
and update checks call the peer pool directly and write results into polled
state. The proxy keeps only the forwarding half, for a `REQ` that arrives from a
mesh peer with hops left. If an nsite ever needs transitive reach, it gets an
explicit API rather than a magic filter key.

`MAX_REQ_TTL` is **2**, below the push default of 3, because flooded reads cost
more. A peer's `relay_read_multihop` clamps it to 0 for that peer.

### Decided — a query id is mandatory on a pull

Split-horizon on the immediate sender is not a loop guard on a graph. A circle is
not a tree, so the same query arrives by several paths and would be re-fanned
every time. Event-id dedup at merge hides that in the *results* while the cost has
already been paid: at ttl 2 across a circle of 10, roughly 110 `REQ`s over BLE
for one query.

The originating proxy stamps a random `qid`, forwarded copies carry the same one,
and each node **serves a given query once** — it still answers from its own
store, but does not fan it out again. This is also the amplification bound:
without it, one peer's `REQ` makes us issue N. Bounded at 512 remembered ids,
FIFO.

### Decided — carry a relative budget, do not compose deadlines

`budgetMs` is **relative**, never a wall-clock deadline, because mesh clocks are
not synced. Each hop is meant to spend about 60% of what it received on the hop
below and keep the rest to receive and relay, so a depth-2 peer is not squeezed
inside its parent's timeout.

**Not yet enforced.** The field is stamped, decayed per hop, and carried, but
every hop still applies a hardcoded timeout. Enforcement is a separate change.

### Decided — the recursion stays synchronous for now

No per-query routing table. With `EOSE` unblocked, a slow deep hop costs a
truncated result set rather than a hung client, which is an acceptable trade at
`MAX_REQ_TTL = 2`. Full asynchronous response routing — responses flowing back
tagged by query id, each hop relaying toward the requester — stays deferred until
multi-hop pull has a user that justifies the routing state and its expiry rules.

### Decided — gossip eligibility is per-application

Which kinds an nsite may fan out is a **per-application permission**, to be
enforced by mapping the WebSocket `Origin` → siteKey → permission record. **v1
default: all kinds**, with the protocol hop budgets as per-app clamps and lenient
rate limits — default-allow, no prompts. The Android-style request/grant flow
comes later. Full model: [nsite-permissions.md](./nsite-permissions.md), which is
still a proposal: the per-app record and the `Origin` mapping are not built yet.

### Decided — rate limits start lenient

Sane per-`Origin` caps that stop a runaway app from saturating a BLE link without
throttling normal chat; slow-down over hard-fail. Starting numbers in
[nsite-permissions.md §4](./nsite-permissions.md).

### Still open

- **Best-ttl re-forward.** If the same id arrives first with a low `ttl` (not
  forwarded) then later via a shorter path with a higher one, the seen-set says
  "already handled" and the device under-propagates slightly. "Remember the best
  seen and re-forward the improvement" is a refinement; for v1 we accept the
  minor under-reach (Plane B covers it). **TBD / open.**
- **Negentropy reconcile.** [NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md)
  would settle "what do you have that I don't" in ~log bandwidth, replacing blind
  re-pulls on the pull plane and in manifest sync
  ([nsite-layer.md §2.4](./nsite-layer.md)). Not implemented, and an external
  relay backend may or may not support it — capability detection is a future
  step. **TBD / open.**
- **Closest / fastest source selection.** When several reachable peers hold the
  wanted events, prefer the nearer / faster one (and possibly pull different
  slices from different holders in parallel — the data is content-addressed /
  self-authenticating, so any holder will do). Overlaps the multi-source open
  question in [nsite-layer.md §5.2](./nsite-layer.md). **TBD / open.**

---

## See also

- [./propagation.md](./propagation.md) — manifest + blob propagation, the
  store-and-forward layer, source discovery.
- [./nsite-layer.md](./nsite-layer.md) — the relay/blob backends, gateway, and
  the negentropy reconcile this would use (§2.1, §2.2, §2.4).
- [./nsite-permissions.md](./nsite-permissions.md) — the per-peer permissions
  that clamp these planes, and the proposed per-application capability model.
- [./identity-pairing.md](./identity-pairing.md) — pairing, which no longer
  travels on this plane at all.
- [../../reference/thinning-custom-relay.md](../../reference/thinning-custom-relay.md) —
  why the envelope, the seen-set, and the auth plane are shaped this way.
- [../../myco-bitchat/README.md](../../myco-bitchat/README.md) — the in-app Nostr
  chat client that consumes this.
