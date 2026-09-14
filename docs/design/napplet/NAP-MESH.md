NAP-MESH
========

Hop-Limited Mesh Publish and Subscribe
--------------------------------------

`draft`

**NAP ID:** NAP-MESH
**Domain:** `mesh`
**Depends:**
- `relay` — wire · required — imports `EventTemplate` for publish input and `RelayEventResult` for every returned event.
**Web binding (NIP-5D):** `window.napplet.mesh` · `shell.supports("mesh")`

> Written in the napplet/naps template so it can be proposed there. Until it
> is merged upstream, Myco is the only runtime that offers it and this file is
> the authority. Implementation: `myco-napplet-runtime/src/nap/mesh.rs`.

## Description

NAP-MESH provides napplets with publish and subscribe over a local mesh — the devices physically around the user, reached over BLE, Wi-Fi Aware or the LAN — where reach is measured in **hops** rather than relays. A relay publish (NAP-RELAY, NAP-OUTBOX) answers "put this on my relays"; a mesh publish answers "flood this to the people near me, this far". The shell owns the mesh: which devices are peers, how an event is carried between them, how a copy arriving by a second path is deduplicated, and how far any flood may travel. The napplet chooses only the hop budget, within a cap the user sets.

This interface is for napplets whose data is local by nature — a doorbell, a room chat, a game between phones in the same building — and which must work in a room with no internet. It is not a substitute for relay access: an event published here reaches only as far as its hops carry it, and a napplet that also wants the open web publishes there separately.

## API Surface

| Operation | Parameters | Result | Wire |
|-----------|------------|--------|------|
| `info` | none | `MeshInfo` | `mesh.info` / `mesh.info.result` |
| `publish` | `template` (`EventTemplate`), optional `options` (`MeshPublishOptions`) | `MeshPublishResult` | `mesh.publish` / `mesh.publish.result` |
| `subscribe` | `filters` (`NostrFilter` or list), optional `options` (`MeshSubscribeOptions`) | `MeshSubscription` handle | `mesh.subscribe` plus push messages |

### Schemas

Primitive references:

| Name | Meaning |
|------|---------|
| `NostrFilter` | NIP-01 filter object. |
| `NostrEvent` | NIP-01 signed event object. |
| `EventTemplate` | External type owned by NAP-RELAY. Unsigned event template for shell signing. |
| `RelayEventResult` | External type owned by NAP-RELAY. |

`MeshInfo` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `online` | yes | boolean | The mesh is running on this device. |
| `peers` | yes | integer | Peers reachable right now. Reported, not promised. |
| `limits` | yes | `MeshLimits` | The user's caps. |

`MeshLimits` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `publishTtl` | yes | integer | Most hops a publish may ask for. |
| `subscribeTtl` | yes | integer | Most hops a subscribe backlog pull may ask for. |

`MeshPublishOptions` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `ttl` | no | integer | Hops the event may travel beyond this device. `0` stores locally only. Omitted means the user's cap. |

`MeshPublishResult` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `ok` | yes | boolean | True when the event was signed, stored and handed to the mesh. |
| `event` | no | `NostrEvent` | The signed event. |
| `eventId` | no | text | Its id. |
| `ttl` | no | integer | The hop budget actually used, after the shell's clamp. |
| `error` | no | text | Reason when `ok` is false. Every other field is then undefined. |

`MeshSubscribeOptions` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `ttl` | no | integer | Hops the backlog request may travel beyond this device. `0` asks nobody. Omitted means the user's cap. |

`MeshSubscription` members:

| Member | Type | Required | Notes |
|--------|------|----------|-------|
| `on("event", cb)` | function | yes | Registers a callback for `mesh.event` deliveries. The callback receives `RelayEventResult`. |
| `on("eose", cb)` | function | yes | Registers a callback for `mesh.eose`. The callback receives the effective `ttl`. |
| `on("closed", cb)` | function | yes | Registers a callback for `mesh.closed`. The callback receives optional `reason`. |
| `close()` | function | yes | Closes the subscription by sending `mesh.close` for the handle's subscription id. |

**`info()`** — Reports whether the mesh is up, how many peers are reachable, and the user's hop caps. A napplet reads the caps to tell the user what "everyone nearby" means on this device. It never needs them to make a call: a budget above the cap is clamped, not refused.

**`publish(template, options?)`** — Signs `template` as the user under NAP-RELAY's rules (the shell owns `pubkey`, `created_at`, `id` and `sig`; the napplet owns `kind`, `content` and `tags`), stores the event on the device's own relay, and floods it to mesh peers with `min(options.ttl, limits.publishTtl)` hops of budget. Each peer that receives it stores it, delivers it to its own live subscriptions, and forwards it with one hop less until the budget is spent. The result carries the budget that was actually used.

**`subscribe(filters, options?)`** — Opens a live subscription. The shell registers the filters, answers with matching events already stored on this device, asks peers up to `min(options.ttl, limits.subscribeTtl)` hops out for theirs, and sends `mesh.eose`. `mesh.eose` marks the end of **this device's** backlog: peers' backlog streams in afterwards as `mesh.event`, as it arrives. Every later matching event — published here or carried in from a peer — is delivered the same way until the napplet closes the subscription or the shell does.

## Wire Protocol

`mesh.*` messages use the NIP-5D wire format (`{ "type": "domain.action", ...payload }`).

| Type | Direction | Payload fields |
|------|-----------|----------------|
| `mesh.info` | napplet -> shell | `id` |
| `mesh.info.result` | shell -> napplet | `id`, `online`, `peers`, `limits` |
| `mesh.publish` | napplet -> shell | `id`, `event` (`EventTemplate`), `ttl?` |
| `mesh.publish.result` | shell -> napplet | `id`, `ok`, `event?`, `eventId?`, `ttl?`, `error?` |
| `mesh.subscribe` | napplet -> shell | `id`, `subId`, `filters`, `ttl?` |
| `mesh.event` | shell -> napplet | `subId`, `result` (`RelayEventResult`) |
| `mesh.eose` | shell -> napplet | `subId`, `ttl` |
| `mesh.close` | napplet -> shell | `subId` |
| `mesh.closed` | shell -> napplet | `subId`, `reason?` |

Key design notes:
- Request/result pairs use `id` for correlation.
- `mesh.close` is fire-and-forget and has no `id`.
- Subscription deliveries are routed by `subId`. Subscription ids are scoped to this domain: a `mesh` subscription and a `relay` subscription may share an id without interference.
- `ttl` is a count of hops **beyond the requesting device**. A hop budget never rides inside the event: it is carried by the shell between peers, so the signed event is byte-identical everywhere it lands.
- The shell answers the local backlog before asking peers, and does not wait for peers before `mesh.eose`. The mesh is asynchronous; the wire says so rather than hiding it behind a long wait.

### Examples

**Publish two hops out:**
```
-> { "type": "mesh.publish", "id": "p1", "event": { "kind": 20666, "content": "ding", "tags": [["t", "doorbell"]] }, "ttl": 2 }
<- { "type": "mesh.publish.result", "id": "p1", "ok": true, "event": { "id": "ev1…", "pubkey": "ab12…", "kind": 20666, "content": "ding", "tags": [["t", "doorbell"]], "created_at": 1234567890, "sig": "…" }, "eventId": "ev1…", "ttl": 2 }
```

**Publish above the cap (cap is 3):**
```
-> { "type": "mesh.publish", "id": "p2", "event": { "kind": 1, "content": "hello" }, "ttl": 10 }
<- { "type": "mesh.publish.result", "id": "p2", "ok": true, "event": { … }, "eventId": "ev2…", "ttl": 3 }
```

**Subscribe, one hop of backlog:**
```
-> { "type": "mesh.subscribe", "id": "s1", "subId": "bell", "filters": [{ "kinds": [20666] }], "ttl": 1 }
<- { "type": "mesh.event", "subId": "bell", "result": { "event": { … } } }
<- { "type": "mesh.eose", "subId": "bell", "ttl": 1 }
<- { "type": "mesh.event", "subId": "bell", "result": { "event": { … } } }   // a peer's backlog, arriving later
<- { "type": "mesh.event", "subId": "bell", "result": { "event": { … } } }   // a live event
-> { "type": "mesh.close", "subId": "bell" }
```

**Info:**
```
-> { "type": "mesh.info", "id": "i1" }
<- { "type": "mesh.info.result", "id": "i1", "online": true, "peers": 4, "limits": { "publishTtl": 3, "subscribeTtl": 2 } }
```

### Error Handling

`mesh.publish.result` carries `ok: false` and `error` when the template could not be signed or the event could not be stored. Per the registry's error model, every other result field is then undefined.

`mesh.subscribe` failures arrive as `mesh.closed` carrying `reason`, so a napplet handles them on the path it already has for a subscription ending.

A `ttl` that is not a whole number from 0 to 255 is an error, not a zero: a napplet that asked for reach and silently got none would have no way to tell. A `ttl` above the cap is **not** an error; it is clamped and the effective value reported.

`mesh.info.result` carries `error` only when the mesh cannot be queried at all; an offline mesh is `online: false`, not an error.

## Shell Behavior

- The shell MUST sign publish templates under NAP-RELAY's rules: the napplet never sets `pubkey`, `created_at`, `id` or `sig`.
- The shell MUST store a published event locally before or regardless of forwarding it, so a `ttl` of `0` is a working local publish.
- The shell MUST clamp a requested `ttl` to the user's cap and MUST report the effective budget in `mesh.publish.result.ttl` and `mesh.eose.ttl`.
- The shell MUST NOT let a napplet raise the shell's own forwarding clamp for events arriving from peers, whatever budget those events carry.
- The shell MUST respond to every `mesh.info` and `mesh.publish` request with a result message carrying the same `id`.
- The shell MUST deliver matching events as `mesh.event` until the napplet sends `mesh.close` or the shell terminates the stream with `mesh.closed`.
- The shell MUST send `mesh.eose` once per subscription, after the local backlog and without waiting for peers.
- The shell SHOULD deduplicate by event id so a copy arriving by a second path is delivered at most once per subscription; a napplet MUST still tolerate a duplicate.
- The shell MAY keep separate caps for publish and subscribe. A flooded read costs more than a flooded write — every hop answers as well as forwards — so a lower subscribe cap is the expected default.
- The shell MAY enforce ACL checks on `mesh` capabilities, and MAY revoke the grant at any time; revocation takes effect on the next call and the next delivery.

## Security Considerations

- **Amplification.** The hop budget is the only per-message cost control a napplet holds, and it is bounded by a cap the napplet cannot change. The shell's forwarding clamp for peers' events is a separate, shell-owned number; nothing in this interface reaches it.
- **Acting as the user.** A `mesh` grant lets a napplet publish as the user to everyone nearby, repeatedly, with no per-event prompt. Runtimes SHOULD say so in words at grant time, as for `relay`.
- **What the mesh reveals.** `mesh.info.peers` is a count. This interface exposes no peer identities, addresses, transports or signal data; a napplet learns that people are nearby, not who or where. A separate NAP would be needed for presence and is deliberately not this one.
- **Untrusted filters and templates.** Filters and templates arrive from untrusted code. The shell parses them fully and refuses anything unreadable rather than guessing; a napplet that asked for something specific and got everything, or nothing, would have no way to tell.
- **Loop safety** is the shell's, by event id, not the budget's: re-broadcasting a signed event is idempotent, so a shell forwards each id at most once and a flood terminates whatever budget it carried.

## Implementations

- Myco (Android) — `myco-napplet-runtime/src/nap/mesh.rs` over the FIPS mesh; hop carriage per `docs/design/core/event-gossip.md`.
