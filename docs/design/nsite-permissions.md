# Permissions: per-peer grants and per-application capabilities

> Status: MIXED. **Per-peer permissions (§2) are built** — the record ships on
> every circle contact and is enforced on both content ports, though no UI
> exposes it yet. **Per-application capabilities (§3 onward) are still a
> proposal**: the `Origin` → siteKey mapping and the rate gate are not
> implemented. Open questions are marked **TBD / open**.

An nsite is a static web app served from the local gateway
([nsite-layer.md](./nsite-layer.md)). Most just render; some want to do more —
publish into the mesh ([event-gossip.md](./event-gossip.md)), read location, hold
blobs.

There are two different permission questions here, and conflating them is the
main thing this doc is trying to prevent.

| Layer | Question it answers | Subject | Status |
| --- | --- | --- | --- |
| **Per-peer** (§2) | What may a paired *device* do to us? | A circle contact, identified by its mesh address | Built |
| **Per-application** (§3–§6) | What may an *app we run* do on our behalf? | An nsite, identified by its `siteKey` | Proposed |

They are independent. A peer with full grants still cannot make our device gossip
a kind an app is not allowed to publish, and a fully-trusted app still cannot get
data out of a peer that has revoked our reads.

> **Scope.** Enforcement is native, in the **mesh relay proxy**
> ([`myco-core/src/mesh_relay.rs`](../../myco-core/src/mesh_relay.rs)) and the
> Blossom server. The WebView/nsite JS is untrusted, so a capability can never
> live in JS — only be *requested* from there.

---

## 1. Where enforcement lives

Both layers are enforced in the same place: the **mesh relay proxy**, the one
piece of Myco-specific code on the content path. It terminates both sockets — the
loopback socket the WebView talks to and the `.fips` socket peers talk to — so it
is the only component that can see an `Origin` header *and* a peer's mesh address.
The store behind it is a plain NIP-01 relay with no Myco concepts in it, and is
meant to be swappable, so policy cannot live there.

The blob plane is the same shape: `myco-blossom` is a generic BUD-01 store and
takes an access function from `myco-core`, so it stays free of circle knowledge.

Pairing itself is enforced nowhere on the content path, because it does not travel
there any more — it has its own service on `:4871`
([identity-pairing.md](./identity-pairing.md)).

---

## 2. Per-peer permissions

Paired used to be all-or-nothing: a peer in the circle could read everything,
publish anything, and upload blobs. It is now six flags on the peer's circle
record, persisted in `circle.json`.

| Plane | Flag | Default | Meaning |
| --- | --- | --- | --- |
| Relay | `relayRead` | on | May open a `REQ` and receive our stored events |
| Relay | `relayReadMultihop` | on | Their `REQ` may be forwarded to our other peers |
| Relay | `relayWrite` | on | May publish events to us |
| Relay | `relayWriteMultihop` | on | Events from them may be forwarded onward by us |
| Blossom | `blossomRead` | on | May `GET` / `HEAD` blobs from us |
| Blossom | `blossomWrite` | **off** | May `PUT /upload` to us |

Read every flag as a grant **we** make to **them**. "Multihop" means specifically
whether *their* traffic travels further through *us* — not anything we send them.

### 2.1 Two checks, not one

- **At accept.** A peer that is not in the circle is refused **before the
  WebSocket upgrade**, with a plain `403`. There are no exceptions on either
  content port. A stranger costs a TCP accept and one small response rather than
  an upgrade and a round of frames, which matters on a BLE link.
- **Per message / per route.** An admitted peer's flags then apply: read against
  `REQ`, write against `EVENT`, and on Blossom the read/write split falls out of
  the HTTP method.

Membership and flags are consulted **live** on every request, so adding a peer,
removing one, or changing a grant takes effect immediately. Revocation also drops
connections that are already open, rather than only blocking the next one.

### 2.2 The multihop flags are hop-budget clamps

Neither multihop flag is a separate check. They are per-peer values for the hop
budgets the push and pull planes already carry
([event-gossip.md §3, §7](./event-gossip.md)):

- `relayWriteMultihop` off → an inbound event's budget is treated as **0**: store
  it, show it, never pass it on.
- `relayReadMultihop` off → an inbound `REQ`'s budget is clamped to **0**: answer
  from our own store, forward nothing.

So `MAX_EVENT_TTL` and `MAX_REQ_TTL` stop being global constants and become
per-peer ceilings, and the pull plane's amplification bound gets a per-peer dial.

### 2.3 Why Blossom write defaults off

Uploads cost us disk, and nothing in normal operation needs them: propagation is
pull-based, so peers fetch blobs from the holder rather than being handed them.
Serde defaults are written so a missing field cannot silently grant it — an older
`circle.json` loads with uploads off.

The one casualty is the dev-menu peer speedtest, which works by `PUT`ting a
payload to the target peer's Blossom. It is not exempted: it works only against a
peer who has granted uploads, and the dev menu should say so plainly rather than
report a generic failure.

### 2.4 Not in the UI yet

Every peer gets the defaults. The record exists now so that turning a knob later
is a UI change rather than a storage migration.

---

## 3. The unit of the application layer is the `Origin`

An nsite **is** an application, identified by its `siteKey` (the `npub` for a root
site, `npub:dTag` for a named one — [nsite-layer.md §3.2](./nsite-layer.md)).
Per-application permissions are a record stored **per-siteKey** on the device.

The enforcement hook is the **WebSocket / HTTP `Origin`**. The nsite loads at
`http://<host>.nsite` and talks to the localhost relay (`ws://localhost:4870`) and
Blossom (`http://localhost:24243`); every request carries
`Origin: http://<host>.nsite`. The proxy maps **`Origin → siteKey → permission
record`** and applies it.

**Not built.** The proxy does no `Origin` check today; a loopback connection is
simply trusted. This is where it is added.

---

## 4. The capability record (proposed)

| Capability | v1 default | Meaning | Enforced at |
| --- | --- | --- | --- |
| `gossip-kinds` | **all kinds** | which event kinds may be fanned out to the mesh | proxy fan-out path, keyed by `Origin` |
| `event-hops` | **3** | max reach for this app's pushed events — a per-app version of `MAX_EVENT_TTL` ([event-gossip.md §3](./event-gossip.md)) | proxy fan-out path |
| `rate` | lenient (§5) | publish / subscribe rate caps | proxy ingress, per `Origin` |
| `location` | *(future)* | geolocation, granted at a **chosen accuracy** (coarse → fine), e.g. for geohash rooms | WebView geolocation bridge (Kotlin) |
| `blob-quota` | *(future)* | Blossom storage budget for app-authored blobs | Blossom `PUT` path |

The hop default matches the protocol default in
[event-gossip.md](./event-gossip.md); here it is the **per-app clamp**, so a
single app cannot exceed it even if its client asks for more.

**There is no per-app pull-hop capability.** An nsite's `REQ` never fans out to
peers at all — it is answered from the local store and returns `EOSE` at
local-store speed. Multi-hop pull is a core-driven operation (discovery, update
checks), so there is nothing for an app-level clamp to bite on. If an nsite ever
needs transitive reach, it gets an explicit API, and that API is where the
capability would attach.

`location` is **graded, not boolean.** The grant carries a precision — naturally
expressed as a **geohash length** (fewer chars = coarser; e.g. ~5 chars ≈
neighbourhood, ~7 ≈ block) — and the native bridge **truncates** the fix to that
precision before the nsite ever sees it, so the device never hands out more
accuracy than was granted. The chosen precision *is* the room granularity for a
geohash room (coarser grant → larger shared room), which keeps the privacy dial
and the product behaviour the same control.

---

## 5. v1 policy: default-allow, lenient limits

Every app is granted everything by default — **all kinds gossip-eligible**, the
default hop budget, lenient rate limits — with **no prompts**. nsites stay "pure
static content that just works." The point of writing the record down is that the
*enforcement points* (Origin attribution, the hop clamp, the rate gate) can ship
before any policy does, so flipping a default to deny later is configuration, not
new plumbing.

Rate limits aim to stop a runaway or malicious app from saturating a BLE link,
**not** to throttle a person chatting. Proposed starting caps, **per `Origin`**
(all *experimental*):

- **Publish (`EVENT`):** ~20 events / 10 s burst, ~100 / min sustained.
- **Subscriptions (`REQ`):** up to ~32 concurrent open subscriptions.

Over-limit is **slow-down, not hard-fail** where possible (queue/delay rather
than reject), so a chatty moment degrades gracefully instead of dropping messages.

The auth service on `:4871` follows the same lenient spirit with its own limits,
since it is the only port an unpaired device can reach
([identity-pairing.md](./identity-pairing.md)).

---

## 6. Future: Android-style request / grant

Additive, layered on §5 — turns default-allow into default-deny-for-sensitive +
prompt:

1. **Declare.** The nsite manifest declares the capabilities it wants, so the user
   can see "this app wants: nearby chat, location" at install.
2. **Grant.** Declaration is only a *request* — the manifest is author-signed and
   could claim anything. The **user grants**, per-siteKey; sensitive caps
   (`location`) prompt at first use. Grants are revocable in Settings.
3. **Enforce.** Same native points as §4 — only the *default* changes from allow
   to deny-until-granted for the sensitive subset.

The same UI problem exists one layer down: per-peer flags (§2) also need a place
to live, and the two lists are different enough ("this device" vs "this app") that
they probably want separate screens.

---

## 7. Open questions

- **Prompt model** — per-use vs. install-time for sensitive caps; how revocation
  surfaces. **TBD / open.**
- **Default-deny for unknown kinds?** Whether `gossip-kinds` should ever narrow
  from "all" to an allow-list for apps the user hasn't explicitly trusted.
  **TBD / open.**
- **Trust tiers** — should a paired-circle app get a more generous record than a
  freshly-installed one? Ties into [security.md](./security.md). **TBD / open.**
- **Surfacing per-peer flags** — which of the six are worth showing a user at all,
  and what a sensible preset ("read-only peer", "no relaying") looks like.
  **TBD / open.**

---

## See also

- [./event-gossip.md](./event-gossip.md) — the push and pull planes these clamp,
  and the `MESH` envelope the hop budgets travel in.
- [./nsite-layer.md](./nsite-layer.md) — the gateway, `siteKey` resolution, and
  the JS sandbox / capability open question (§7) this answers.
- [./identity-pairing.md](./identity-pairing.md) — the auth service that creates
  the circle these per-peer grants attach to.
- [./security.md](./security.md) — the trust model both layers sit inside.
- [../../reference/thinning-custom-relay.md](../../reference/thinning-custom-relay.md) —
  D10, where the per-peer flags were decided.
