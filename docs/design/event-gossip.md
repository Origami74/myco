# Live event gossip: push + pull for app data over the mesh

> Status: DESIGN / proposal for a not-yet-built feature. Voice is "the app
> will…". Open questions are marked **TBD / open**.

[Propagation](./propagation.md) specifies how author-signed **nsite manifests**
(kinds 15128/35128) + content-addressed blobs spread and survive partitions. This
document covers the sibling problem: how an **in-app Nostr client** (e.g.
[`myco-bitchat`](../../myco-bitchat/README.md)) gets *arbitrary* app events — a
chat message, a reaction — to the people physically around it, when the only
relay it can reach is the device's own embedded relay (`ws://localhost:4870`).

The enabling idea is the same one the whole project rests on: **the embedded
relay is fed by the FIPS mesh, so physical proximity *is* the transport.** A
plain Nostr client pointed at localhost becomes a nearby-chat client — *if* the
mesh gossips its events between peers. Today it does not: the general `FanoutSink`
is a P3 no-op stub and the peer pull is hardcoded to manifest kinds
([nsite-layer.md §2.1](./nsite-layer.md), [propagation.md](./propagation.md)).
This is the design for filling that gap.

> **Scope.** This is core/native work in `myco-core` + `myco-relay`. The WebView
> never reaches `.fips` peers; only the native engine does. The nsite is an
> ordinary Nostr client and is unaware any of this exists — it just sees nearby
> peers' events appear in its localhost relay.

---

## 1. Two planes, never conflated

Event gossip is split into two independent planes. Keeping them separate is what
makes the whole thing terminate and stay cheap.

### Plane A — Push (live fan-out)

When a device **originates** an event, or **receives** one via a push frame with
budget left, it forwards that event to its connected circle peers. This is a
**live wave**: it ripples outward a bounded number of hops while the event is
fresh, then stops. It is **`event-ttl`-bounded** (§2–3) and triggered **only** by
the push path — origination and relayed EVENTs — never by a write landing in the
relay store (§4, the load-bearing invariant).

### Plane B — Pull (backlog reconcile on contact)

When two relays become adjacent (a peer comes into range), the arriving device
**pulls recent events** directly from that one neighbour — the last *N* / last
few minutes for the app's kind(s). This is a direct **1-hop** request between two
now-adjacent relays, so it carries **no `event-ttl`**. It is store-only: pulled
events are stored for serving and display, and **do not** re-enter the push wave.
(A multi-hop pull — a TTL-bounded `REQ` flood, **`req-ttl`** — is the read-side
counterpart, used for *discovery* rather than chat backlog; §7.)

### Why both

Push gives fast live delivery to the nearby cluster. Pull is what reaches
everyone push didn't: a device five hops away never receives the live wave, but
the moment it moves near *anyone* who stored the event, it pulls it directly.
Physical movement + neighbour catch-up does the long-range work — the way bitchat
actually spreads — which is exactly why a **short `event-ttl` is fine** (§3).

---

## 2. `event-ttl`: a transient field on the event

The push hop count is named **`event-ttl`** (Rust `event_ttl`; the read-side
counterpart is **`req-ttl`**, §7). It is a property of the **live wave, not the
event**, but it does **not** need a custom frame to carry it. It rides as a
**non-signed, non-persisted top-level field** on the event, over the standard
NIP-01 `["EVENT", …]` message:

```
["EVENT", {
  id, pubkey, created_at, kind, tags, content, sig,
  "event-ttl": 2     // ← NOT in the signed preimage, NOT a tag; read by mesh relays, stripped on store
}]
```

**Signature and id stay intact.** The signed preimage is
`[0, pubkey, created_at, kind, tags, content]`. `event-ttl` is neither in it nor
a tag, so `id` (`sha256` of the preimage) and `sig` are unaffected — any client
verifies the event and simply ignores `event-ttl`. (Putting the hop count in
`tags` *would* break the signature; a top-level sibling field does not.)

**The relay reads it on receipt, then stores canonical.** A conforming mesh relay
grabs `event-ttl` for the forward decision (§3), then persists exactly
`{id, pubkey, created_at, kind, tags, content, sig}` — **never the `event-ttl`**.
When it forwards to a peer it re-attaches the decremented value on the outgoing
`["EVENT", …]`. Handling `event-ttl` this way — read on receipt, strip on store,
re-attach on forward — is **part of the mesh protocol every relay in the mesh
must implement** (this doc happens to describe the `myco-relay` implementation,
but any other implementation has to adhere). Both ends of every hop handle the
field; it is invisible to everything else.

**Why a field, not a custom envelope.** The relay↔relay link must keep speaking
standard NIP-01 — the message stays `["EVENT", {...}]`, the verb every relay
already implements and every stock Nostr tool can read for debugging. A bespoke
gossip frame would replace that with a non-NIP-01 websocket message: a strictly
bigger break, for no gain. An unknown top-level field is ignored by anything that
doesn't care; only mesh relays implementing this protocol act on it.

**Storage dropping `event-ttl` is the mechanism, not a loss.** Because the field
is stripped on store, a **pushed** EVENT arrives carrying `event-ttl`
(forward-eligible), while a **stored** event re-served via `REQ` (backlog/pull,
§1 Plane B) comes back with **none** — so it is never re-flooded. The push/pull
split thus enforces itself: this is exactly the §4 invariant, for free. Storing
an event is the terminal state of a hop ("this fact now lives here"); a wave's
remaining hop-count is meaningless at rest.

> **Originating `event-ttl`.** A local nsite need not set anything — the
> originating relay stamps the default (**3**) when a WebView client publishes
> without one. A client *may* suggest a per-message reach by including the field,
> subject to the `max-event-ttl` clamp (§3).

---

## 3. Forward rule

On receiving a publish-form `["EVENT", event]` that carries an `event-ttl` (a
live push; a `REQ`-response EVENT has none and is pull/backlog, never forwarded):

1. **Seen?** If `event.id` is already in my relay store / seen-set → **drop**
   (store nothing new, forward nothing).
2. **New?** Store the canonical event (without `event-ttl`), then let
   `fwd = min(event-ttl, max-event-ttl)`; if `fwd > 0`, send
   `["EVENT", { …event, "event-ttl": fwd - 1 }]` to **every circle peer except
   the sender** (split-horizon).

Two knobs, distinct:

- **Originating `event-ttl`** — how far *my own* events travel; the originator
  stamps it. Proposed default **3** (experimental).
- **`max-event-ttl`** — a clamp on forwarding, so a misbehaving peer can't send
  `event-ttl: 255` and turn this device into a flood amplifier. "The max I will
  honour." Set to the originate default (**3**) so own-origin waves aren't clamped
  by neighbours. (This is also the per-nsite permission knob — §7.)

### Loop safety is the seen-set, not the `event-ttl`

A Nostr `id` is content-derived and **stable**: re-broadcasting a signed event is
idempotent — every device, any number of hops away, computes the same id. So each
device forwards each id **at most once**, and the flood **always terminates**,
even with `event-ttl` set arbitrarily high. It bounds only **reach and cost**; it
is not a loop guard. (This is the seen-set + hop-budget pattern already specified for
manifest fan-out in [nsite-layer.md §2.1](./nsite-layer.md), generalized here to
app events.)

### Worked example — originating `event-ttl` = 3 (= forwards still allowed)

```
A (origin) ─ttl3─▶ B ─ttl2─▶ C ─ttl1─▶ D ─ttl0─▶ E
                   (1 hop)   (2 hops)  (3 hops)  E stores, does NOT forward
```

The wave reaches ~4 hops out, then dies cleanly. Anyone further gets it via
Plane B when they next come into range.

---

## 4. The load-bearing invariant

> **A stored event never re-enters the push flood. Push is triggered by a
> publish-form EVENT carrying `event-ttl > 0` — origination, or a forwarded hop —
> never by a write landing in the relay store.**

This is what the §2 strip-on-store mechanism buys: forwarding keys off the
`event-ttl` that rode in on the live `["EVENT", …]`, and a stored event has none.

The naive propagator ("subscribe to the relay, fan out on every new event")
breaks here. With it, Plane B bites you: a device that comes into range and pulls
the last 21 messages would store them — and a store-triggered push would kick off
a **fresh wave for old messages, with a fresh `event-ttl`, every time someone new
arrives**. The seen-set stops infinite loops, but you still get pointless
re-flood churn — worse once a peer has GC'd an id at expiry (§5) and sees it as
"new" again.

Keeping `event-ttl` *only* on the live push EVENT, and making store-insertion
**not** a push trigger, is what cleanly separates the two planes. Note this diverges from
the manifest propagator's store-triggered fanout described in
[nsite-layer.md §2.1](./nsite-layer.md): manifests are replaceable and rare, so
re-emitting on store is cheap and desirable; high-rate ephemeral app events are
not, so their push must be `event-ttl`-driven. **A gossiped event kind is therefore a
distinct path from manifest fanout, not the same code.**

---

## 5. Ephemerality (the chat case)

The driving consumer, `myco-bitchat`, makes events **expire** (NIP-40
`["expiration", <ts>]`, +10 min) and shows a new arrival only the last ~21. For
that to hold, `myco-relay` must **GC events past their `expiration` tag**, so the
store stays small, the Plane-B backlog stays bounded, and the room is genuinely
ephemeral. Expiry also bounds the seen-set: ids age out with their events, so
memory is naturally capped. (An event re-pulled after its own expiry is simply
gone — there is nothing to re-serve, which is the intended behaviour.)

---

## 6. Phasing

- **P1 — circle fan-out, 1 hop.** Generalize the stubbed `FanoutSink` to gossip
  published app events — v1 default **all kinds** except the manifest kinds
  15128/35128, which keep their own store-triggered path (§4) — to direct circle
  peers on origination/receipt. Ignore `event-ttl` (treat as 1). Turns the client
  from a local toy into real two-device nearby chat.
- **P2 — multi-hop flood.** Carry `event-ttl` as a transient event field (§2),
  add the §3 forward rule (seen-set + split-horizon + clamp), decrement per hop.
- **P3 — Plane B pull.** Backlog reconcile on peer contact (`{kinds, since}` or
  piggyback the negentropy reconcile already planned for manifests,
  [nsite-layer.md §2.4](./nsite-layer.md)), store-only.

---

## 7. Decisions and open questions

**Decided — gossip eligibility is per-application.** Which kinds an nsite may
fan out is a **per-application permission**, enforced by mapping the WebSocket
`Origin` → siteKey → permission record. **v1 default: all kinds**, with the
default `event-ttl` (3) / `req-ttl` (2) as per-app clamps and lenient rate
limits — i.e. default-allow, no prompts. The Android-style request/grant flow
comes later. Full model: [nsite-permissions.md](./nsite-permissions.md).

**Decided — rate limits start lenient.** Sane per-`Origin` caps that stop a
runaway app from saturating a BLE link without throttling normal chat; slow-down
over hard-fail. Starting numbers in [nsite-permissions.md §4](./nsite-permissions.md).

Still open:

- **Best-`event-ttl` re-forward.** If the same id arrives first with a low
  `event-ttl` (not forwarded) then later via a shorter path with a higher one,
  the seen-set says "already handled" and the device under-propagates slightly.
  "Remember the best seen and re-forward the improvement" is a refinement; for v1
  we accept the minor under-reach (Plane B covers it). **TBD / open.**
- **`req-ttl` — multi-hop pull / discovery.** The read-side counterpart of
  `event-ttl`: a `REQ` carrying a `req-ttl` floods the *query* outward, each relay
  answering from its store and forwarding the decremented `REQ` to peers, results
  merged back — the natural mechanism for **transitive discovery** ("nsites around
  me," rooms/people nearby, [nsite-layer.md §6](./nsite-layer.md)'s `SearchNsites`
  "transitive reach"). **Reconciliation is negentropy ([NIP-77](https://github.com/nostr-protocol/nips/blob/master/77.md))**,
  not blind re-pulls — the same set-reconciliation the manifest sync uses
  ([nsite-layer.md §2.4](./nsite-layer.md)), so a hop settles "what do you have
  that I don't" in ~log bandwidth. It is **heavier than the push flood** (responses
  routed back, a separate query-id seen-set, result merge/dedup — the Gnutella
  cost), so it is a discovery feature, P3+, not v1 chat backlog. Default `req-ttl`
  **2** (experimental), below `event-ttl` (3) because flooded reads cost more.
  **TBD / open.**
- **Closest / fastest source selection.** When several reachable peers hold the
  wanted events, prefer the nearer / faster one (and possibly pull different
  slices from different holders in parallel — the data is content-addressed/
  self-authenticating, so any holder will do). Overlaps the multi-source open
  question in [nsite-layer.md §5.2](./nsite-layer.md). Likely future. **TBD / open.**

---

## See also

- [./propagation.md](./propagation.md) — manifest + blob propagation, the
  store-and-forward layer, source discovery.
- [./nsite-layer.md](./nsite-layer.md) — embedded relay/Blossom/gateway, the
  manifest fanout + negentropy this generalizes (§2.1, §2.4).
- [./nsite-permissions.md](./nsite-permissions.md) — the per-application
  capability model that gates fan-out (gossip-kinds, TTL clamps, rate).
- [../../myco-bitchat/README.md](../../myco-bitchat/README.md) — the in-app Nostr
  chat client that consumes this.
