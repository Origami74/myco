# Permissions: what other phones and apps may do

> Status: **§2 (per-peer permissions) is removed** — no other phone reaches
> the relay or Blossom, so there is nothing to grant per peer. **§3–§6
> (per-application capabilities for nsites) are superseded**, not
> built: the per-app permission model shipped for **napplets** instead —
> install review, a grant per capability domain, checked on every call,
> switchable on the app's sheet — in
> [../napplet/napplet-runtime.md](../napplet/napplet-runtime.md). An nsite
> stays pure-static and gets nothing; the one thing an nsite can do today that
> looks like a capability (publish to `ws://localhost:4870`) no longer reaches
> anyone else: the relay is loopback only. §3–§6 are kept as the record of
> why an `Origin`-keyed model was considered and where it would have bitten.

An nsite is a static web app served from the local gateway
([nsite-layer.md](./nsite-layer.md)). Most just render; some want to do more —
publish to people nearby, read location, hold blobs.

There are two different permission questions here, and conflating them is the
main thing this doc is trying to prevent.

| Layer | Question it answers | Subject | Status |
| --- | --- | --- | --- |
| **Per-peer** (§2) | What may another *device* do to us? | A linked peer, by its link key | Only the link service; see §2 |
| **Per-application** (§3–§6) | What may an *app we run* do on our behalf? | An nsite, identified by its `siteKey` | Proposed |

They are independent.

> **Scope.** Enforcement is native. The WebView/nsite JS is untrusted, so a
> capability can never live in JS — only be *requested* from there.

---

## 1. Where enforcement lives

- **Other phones** reach only the link service on `:4873`
  ([nap-neighbor/links.md](../nap-neighbor/links.md#the-link-service-4873)).
  Every request after setup is NIP-98-signed by a link key, and answers come
  only from the neighborhoods that link carries.
- **Apps** reach the relay on loopback (`ws://localhost:4870`) and, for
  napplets, the same hub in-process. Per-app checks belong in the **relay
  proxy** ([`myco-core/src/mesh_relay.rs`](../../../myco-core/src/mesh_relay.rs)),
  the one piece of Myco-specific code in front of the store. The store behind
  it is a plain NIP-01 relay, meant to be swappable, so policy cannot live
  there.

---

## 2. Per-peer permissions

**Removed.** Earlier builds opened the relay and Blossom to linked devices
and kept six per-peer flags (relay read/write and their multihop clamps,
Blossom read/write). Neither store is reachable from another phone now, so
the flags went with them.

What a linked peer may do is set per neighborhood instead:

- **Posts reach** and **pass on** ([nap-neighbor/policy.md](../nap-neighbor/policy.md)).
- **Get** and **Share** for each update extension
  ([nap-neighbor/extensions.md](../nap-neighbor/extensions.md)).
- Ending a link, or dropping a neighborhood from it.

---

## 3. The unit of the application layer is the `Origin`

An nsite **is** an application, identified by its `siteKey` (the `npub` for a root
site, `npub:dTag` for a named one — [nsite-layer.md §3.2](./nsite-layer.md)).
Per-application permissions are a record stored **per-siteKey** on the device.

The enforcement hook is the **WebSocket / HTTP `Origin`**. The nsite loads at
`http://<host>.localhost` and talks to the localhost relay (`ws://localhost:4870`); every request carries
`Origin: http://<host>.localhost`. The proxy maps **`Origin → siteKey → permission
record`** and applies it.

**Not built.** The proxy does no `Origin` check today; a loopback connection is
simply trusted. This is where it is added.

---

## 4. The capability record (proposed)

> **Partly moot.** Local events are no longer fanned out to the mesh, so
> `gossip-kinds` and `event-hops` have nothing to gate today. How far a post
> travels is set per neighborhood
> ([nap-neighbor/policy.md](../nap-neighbor/policy.md)).

| Capability | v1 default | Meaning | Enforced at |
| --- | --- | --- | --- |
| `gossip-kinds` | **all kinds** | which event kinds may be fanned out to the mesh | proxy fan-out path, keyed by `Origin` |
| `event-hops` | **3** | max reach for this app's pushed events — a per-app version of the hop budget | proxy fan-out path |
| `rate` | lenient (§5) | publish / subscribe rate caps | proxy ingress, per `Origin` |
| `location` | *(future)* | geolocation, granted at a **chosen accuracy** (coarse → fine), e.g. for geohash rooms | WebView geolocation bridge (Kotlin) |
| `blob-quota` | *(future)* | Blossom storage budget for app-authored blobs | Blossom `PUT` path |

The hop default matched the event-gossip default (gossip is since removed); here it is the **per-app clamp**, so a
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

The link service on `:4873` follows the same lenient spirit with its own limits,
since it is the only port another phone can reach
([nap-neighbor/links.md](../nap-neighbor/links.md#the-link-service-4873)).

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

Per-peer choices (§2) already live on the neighborhood page, apart from any
per-app list.

---

## 7. Open questions

- **Prompt model** — per-use vs. install-time for sensitive caps; how revocation
  surfaces. **TBD / open.**
- **Default-deny for unknown kinds?** Whether `gossip-kinds` should ever narrow
  from "all" to an allow-list for apps the user hasn't explicitly trusted.
  **TBD / open.**
- **Trust tiers** — should an app shared by a neighbor get a more generous record than a
  freshly-installed one? Ties into [security.md](../core/security.md). **TBD / open.**

---

## See also

- [./nsite-updates.md](./nsite-updates.md) — how updates travel through
  neighborhoods.
- [./nsite-layer.md](./nsite-layer.md) — the gateway, `siteKey` resolution, and
  the JS sandbox / capability open question (§7) this answers.
- [nap-neighbor/](../nap-neighbor/README.md) — links, neighborhoods and their
  per-neighborhood settings.
- [./security.md](../core/security.md) — the trust model both layers sit inside.
- [../../reference/thinning-custom-relay.md](../../../reference/thinning-custom-relay.md) —
  D10, where the (since removed) per-peer flags were decided.
