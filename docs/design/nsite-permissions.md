# nsite permissions: per-application capabilities

> Status: DESIGN / proposal for a not-yet-built feature. Voice is "the app
> will…". Open questions are marked **TBD / open**.

An nsite is a static web app served from the local gateway
([nsite-layer.md](./nsite-layer.md)). Most just render; some want to do more —
publish into the mesh ([event-gossip.md](./event-gossip.md)), read location, hold
blobs. This doc defines a **per-application capability model** and where the
native layer enforces it.

The v1 stance is deliberately **permissive (default-allow)**: the chat MVP must
just work, and there is no grant UI yet. What goes in now are the **enforcement
hooks** (so tightening later is config, not replumbing) and sane, lenient limits
that stop runaway abuse without throttling normal use. A later Android-style
request/grant flow (§5) layers on top.

> **Scope.** Enforcement is native (`myco-core` / `myco-relay` / the Android
> shell). The WebView/nsite JS is untrusted, so a capability can never live in
> JS — only be *requested* from there.

---

## 1. The unit is the application, identified by `Origin`

An nsite **is** an application, identified by its `siteKey` (the `npub` for a root
site, `npub:dTag` for a named one — [nsite-layer.md §3.2](./nsite-layer.md)).
Permissions are a record stored **per-siteKey** on the device.

The enforcement hook is the **WebSocket / HTTP `Origin`**. The nsite loads at
`http://<host>.nsite` and talks to the localhost relay (`ws://localhost:4869`) and
Blossom (`http://localhost:24242`); every request carries
`Origin: http://<host>.nsite`. The native side maps **`Origin → siteKey →
permission record`** and applies it. (The relay does no `Origin` check today —
this is where it is added.)

---

## 2. The capability record

| Capability | v1 default | Meaning | Enforced at |
| --- | --- | --- | --- |
| `gossip-kinds` | **all kinds** | which event kinds may be fanned out to the mesh | relay fan-out path (`FanoutSink`), keyed by `Origin` |
| `event-ttl` | **3** | max reach for this app's pushed events (the `max-event-ttl` clamp of [event-gossip.md §3](./event-gossip.md), per-app) | relay fan-out path |
| `req-ttl` | **2** | max reach for this app's discovery `REQ` floods ([event-gossip.md §7](./event-gossip.md)) | relay query path |
| `rate` | lenient (§4) | publish / subscribe rate caps | relay ingress, per `Origin` |
| `location` | *(future)* | geolocation, granted at a **chosen accuracy** (coarse → fine), e.g. for geohash rooms | WebView geolocation bridge (Kotlin) |
| `blob-quota` | *(future)* | Blossom storage budget for app-authored blobs | Blossom `PUT` path |

The `event-ttl` / `req-ttl` defaults match the protocol defaults in
[event-gossip.md](./event-gossip.md); here they are the **per-app clamp**, so a
single app cannot exceed them even if its client asks for more.

`location` is **graded, not boolean.** The grant carries a precision — naturally
expressed as a **geohash length** (fewer chars = coarser; e.g. ~5 chars ≈
neighbourhood, ~7 ≈ block) — and the native bridge **truncates** the fix to that
precision before the nsite ever sees it, so the device never hands out more
accuracy than was granted. The chosen precision *is* the room granularity for a
geohash room (coarser grant → larger shared room), which keeps the privacy dial
and the product behaviour the same control.

---

## 3. v1 policy: default-allow

Every app is granted everything by default — **all kinds gossip-eligible**, the
default TTLs, lenient rate limits — with **no prompts**. nsites stay "pure static
content that just works." The point of writing the record down now is purely that
the *enforcement points* (Origin attribution, the TTL clamps, the rate gate) ship
with P1/P2 of the fan-out, so flipping any default to deny later is configuration,
not new plumbing.

---

## 4. Rate limits — sane, not strict

The goal is to stop a runaway or malicious app from saturating a BLE link, **not**
to throttle a person chatting. Start lenient and tighten by observation.

Proposed starting caps, **per `Origin`** (all *experimental*):

- **Publish (`EVENT`):** ~20 events / 10 s burst, ~100 / min sustained.
- **Subscriptions (`REQ`):** up to ~32 concurrent open subscriptions.
- **Discovery floods (`req-ttl` > 0):** a much tighter cap (e.g. a few / min) —
  a flooded read is far costlier than a local one ([event-gossip.md §7](./event-gossip.md)).

Over-limit is **slow-down, not hard-fail** where possible (queue/delay rather
than reject), so a chatty moment degrades gracefully instead of dropping messages.

---

## 5. Future: Android-style request / grant

Additive, layered on §3 — turns default-allow into default-deny-for-sensitive +
prompt:

1. **Declare.** The nsite manifest declares the capabilities it wants, so the user
   can see "this app wants: nearby chat, location" at install.
2. **Grant.** Declaration is only a *request* — the manifest is author-signed and
   could claim anything. The **user grants**, per-siteKey; sensitive caps
   (`location`) prompt at first use. Grants are revocable in Settings.
3. **Enforce.** Same native points as §2 — only the *default* changes from allow
   to deny-until-granted for the sensitive subset.

---

## 6. Open questions

- **Prompt model** — per-use vs. install-time for sensitive caps; how revocation
  surfaces. **TBD / open.**
- **Default-deny for unknown kinds?** Whether `gossip-kinds` should ever narrow
  from "all" to an allow-list for apps the user hasn't explicitly trusted, and how
  that interacts with the mutual-pairing handshake
  ([identity-pairing.md](./identity-pairing.md)). **TBD / open.**
- **Trust tiers** — should a paired-circle app get a more generous record than a
  freshly-installed one? Ties into [security.md](./security.md). **TBD / open.**

---

## See also

- [./event-gossip.md](./event-gossip.md) — `event-ttl` / `req-ttl`, the fan-out
  path these caps gate.
- [./nsite-layer.md](./nsite-layer.md) — the gateway, `siteKey` resolution, and
  the JS sandbox / capability open question (§7) this answers.
- [./security.md](./security.md), [./identity-pairing.md](./identity-pairing.md) —
  trust model and pairing the grant flow builds on.
