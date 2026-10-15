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

NAP-MESH provides napplets with publish and subscribe over the user's **trusted mesh**: the devices the user has deliberately paired with, and whatever those devices pass things on to. Reach is measured in **hops** along those pairings rather than in relays. A relay publish (NAP-RELAY, NAP-OUTBOX) answers "put this on my relays"; a mesh publish answers "flood this to my people, this far". The shell owns the mesh: who is in it, how an event is carried between members, how a copy arriving by a second path is deduplicated, and how far any flood may travel. The napplet chooses only the hop budget, within a cap the user sets.

This interface is for napplets whose data belongs to the people around the user — a doorbell, a household chat, a game between friends' phones — and which must work with no internet. It is not a substitute for relay access: an event published here reaches only as far as its hops carry it, and a napplet that also wants the open web publishes there separately.

### The trusted mesh is not the radio mesh

Two meshes are involved, and this interface exposes only one of them.

- The **transport mesh** links a device to whatever compatible node is in range or on the network, over any transport, and routes packets through those links. It decides *reachability*. It is open by design: a stranger's device may be a link, and may carry packets between two members without being able to read them.
- The **trusted mesh** is a virtual mesh laid over it. Its nodes are people; its edges are pairings, made on purpose by both sides, mutual, and kept whether or not a link exists right now. It decides *who things are sent to at all*.

Every `mesh` operation travels along trusted-mesh edges only. "Peers" in this document are the user's paired members that are reachable right now, wherever the transport mesh happens to put them: a member several radio hops away is one trusted hop away, and a stranger in the same room is no hop at all. A hop budget counts trusted edges — `ttl: 2` means friends, and friends of friends — never radio links.

In Myco the trusted mesh is the **Circle** ([`../circle/circle.md`](../circle/circle.md)) and the transport mesh is FIPS.

## API Surface

| Operation | Parameters | Result | Wire |
|-----------|------------|--------|------|
| `info` | none | `MeshInfo` | `mesh.info` / `mesh.info.result` |
| `publish` | `template` (`EventTemplate`), optional `options` (`MeshPublishOptions`) | `MeshPublishResult` | `mesh.publish` / `mesh.publish.result` |
| `subscribe` | `filters` (`NostrFilter` or list), optional `options` (`MeshSubscribeOptions`) | `MeshSubscription` handle | `mesh.subscribe` plus push messages |
| `blobs` | `hashes` (list of sha256 hex, at most 64) | `MeshBlobsResult` | `mesh.blobs` / `mesh.blobs.result` |

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
| `peers` | yes | integer | Paired members reachable right now. Reported, not promised. |
| `limits` | yes | `MeshLimits` | The user's caps. |

`MeshLimits` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `publishTtl` | yes | integer | Most hops a publish may ask for. |
| `subscribeTtl` | yes | integer | Most hops a subscribe backlog pull may ask for. |

`MeshPublishOptions` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `ttl` | no | integer | Hops the event may travel beyond this device. `0` publishes on this device only. Omitted means the user's cap. |

`MeshPublishResult` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `ok` | yes | boolean | True when the event was signed, kept and handed to the mesh. |
| `event` | no | `NostrEvent` | The signed event. |
| `eventId` | no | text | Its id. |
| `ttl` | no | integer | The hop budget actually used, after the shell's clamp. |
| `error` | no | text | Reason when `ok` is false. Every field but `ok` is then undefined. |

`MeshSubscribeOptions` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `ttl` | no | integer | Hops the backlog request may travel beyond this device. `0` asks nobody. Omitted means the user's cap. |

`MeshBlobsResult` fields:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `peers` | yes | integer | Members that were asked: every paired member reachable right now. |
| `blobs` | yes | object | Each asked hash (lowercase) → how many of those members hold it. `0` when none do. |

`MeshSubscription` members:

| Member | Type | Required | Notes |
|--------|------|----------|-------|
| `on("event", cb)` | function | yes | Registers a callback for `mesh.event` deliveries. The callback receives `RelayEventResult`. |
| `on("eose", cb)` | function | yes | Registers a callback for `mesh.eose`. The callback receives the effective `ttl`. |
| `on("closed", cb)` | function | yes | Registers a callback for `mesh.closed`, or for a refused `mesh.subscribe`. The callback receives optional `reason`. |
| `close()` | function | yes | Closes the subscription by sending `mesh.close` for the handle's subscription id. |

**`info()`** — Reports whether the mesh is up, how many paired members are reachable, and the user's hop caps. A napplet reads the caps to tell the user what "everyone" means on this device. It never needs them to make a call: a budget above the cap is clamped, not refused.

**`publish(template, options?)`** — Signs `template` as the user under NAP-RELAY's rules (the shell owns `pubkey`, `created_at`, `id` and `sig`; the napplet owns `kind`, `content` and `tags`), keeps the event on this device, delivers it to this device's live subscriptions, and floods it to reachable members with `min(options.ttl, limits.publishTtl)` hops of budget. Each member that receives it keeps it, delivers it to its own live subscriptions, and passes it on to its own members with one hop less until the budget is spent — or not at all, if that member does not let this device's events travel through it (see Shell Behavior). An ephemeral event (NIP-01 kinds 20000–29999) is delivered and flooded the same way but kept nowhere. The result carries the budget that was actually used.

**`subscribe(filters, options?)`** — Opens a live subscription. The shell registers the filters, answers with matching events already kept on this device, asks members up to `min(options.ttl, limits.subscribeTtl)` hops out for theirs, and sends `mesh.eose` once this device's backlog is done. Members' backlog streams in as `mesh.event` as it arrives, usually after `mesh.eose` but not necessarily. Every later matching event — published here or carried in from a member — is delivered the same way until the napplet closes the subscription or the shell does. Ephemeral events are never backlog: a subscription sees only those that arrive while it is open.

**`blobs(hashes)`** — Asks the reachable members whether they hold each blob, and answers with a count per blob. It is for a napplet that wants to know what the people around it can serve — an app store showing only apps whose files a member's phone has. It asks only: the shell sends each member's blob store a `HEAD` for each hash and downloads nothing. It asks only the trusted mesh: never the internet, never a public server, never a stranger. A member that does not answer in time counts as not holding the blob.

## Wire Protocol

`mesh.*` messages use the NIP-5D wire format (`{ "type": "domain.action", ...payload }`).

| Type | Direction | Payload fields |
|------|-----------|----------------|
| `mesh.info` | napplet -> shell | `id` |
| `mesh.info.result` | shell -> napplet | `id`, `online`, `peers`, `limits`, `error?` |
| `mesh.publish` | napplet -> shell | `id`, `event` (`EventTemplate`), `ttl?` |
| `mesh.publish.result` | shell -> napplet | `id`, `ok`, `event?`, `eventId?`, `ttl?`, `error?` |
| `mesh.subscribe` | napplet -> shell | `id`, `subId`, `filters`, `ttl?` |
| `mesh.subscribe.result` | shell -> napplet | `id`, `error` — only when the subscription was refused |
| `mesh.event` | shell -> napplet | `subId`, `result` (`RelayEventResult`) |
| `mesh.eose` | shell -> napplet | `subId`, `ttl` |
| `mesh.close` | napplet -> shell | `subId` |
| `mesh.closed` | shell -> napplet | `subId`, `reason?` |
| `mesh.blobs` | napplet -> shell | `id`, `hashes` |
| `mesh.blobs.result` | shell -> napplet | `id`, `peers`, `blobs`, `error?` |

Key design notes:
- Request/result pairs use `id` for correlation. `mesh.subscribe` carries an `id` too, answered only by a refusal; a subscription that opens answers with its stream.
- `mesh.close` is fire-and-forget and has no `id`.
- Subscription deliveries are routed by `subId`. Subscription ids are scoped to this domain: a `mesh` subscription and a `relay` subscription may share an id without interference.
- `ttl` is a count of trusted hops **beyond the requesting device**. A hop budget never rides inside the event: it is carried by the shell between members, so the signed event is byte-identical everywhere it lands.
- The shell does not wait for members before `mesh.eose`. The mesh is asynchronous; the wire says so rather than hiding it behind a long wait.

### Examples

**Ring the doorbell two hops out** (ephemeral: delivered, kept nowhere):
```
-> { "type": "mesh.publish", "id": "p1", "event": { "kind": 20666, "content": "ding", "tags": [["t", "doorbell"]] }, "ttl": 2 }
<- { "type": "mesh.publish.result", "id": "p1", "ok": true, "event": { "id": "ev1…", "pubkey": "ab12…", "kind": 20666, "content": "ding", "tags": [["t", "doorbell"]], "created_at": 1234567890, "sig": "…" }, "eventId": "ev1…", "ttl": 2 }
```

**Publish above the cap (cap is 3):**
```
-> { "type": "mesh.publish", "id": "p2", "event": { "kind": 1, "content": "hello" }, "ttl": 10 }
<- { "type": "mesh.publish.result", "id": "p2", "ok": true, "event": { … }, "eventId": "ev2…", "ttl": 3 }
```

**Subscribe to a chat, one hop of backlog:**
```
-> { "type": "mesh.subscribe", "id": "s1", "subId": "room", "filters": [{ "kinds": [9], "#h": ["kitchen"] }], "ttl": 1 }
<- { "type": "mesh.event", "subId": "room", "result": { "event": { … } } }   // kept on this device
<- { "type": "mesh.eose", "subId": "room", "ttl": 1 }
<- { "type": "mesh.event", "subId": "room", "result": { "event": { … } } }   // a member's backlog, arriving later
<- { "type": "mesh.event", "subId": "room", "result": { "event": { … } } }   // a live event
-> { "type": "mesh.close", "subId": "room" }
```

**Subscribe without the grant:**
```
-> { "type": "mesh.subscribe", "id": "s2", "subId": "bell", "filters": [{ "kinds": [20666] }] }
<- { "type": "mesh.subscribe.result", "id": "s2", "error": "capability mesh was not granted to this napplet" }
```

**Info:**
```
-> { "type": "mesh.info", "id": "i1" }
<- { "type": "mesh.info.result", "id": "i1", "online": true, "peers": 4, "limits": { "publishTtl": 3, "subscribeTtl": 2 } }
```

**Who can serve two files:**
```
-> { "type": "mesh.blobs", "id": "b1", "hashes": ["b167…f553", "0000…0001"] }
<- { "type": "mesh.blobs.result", "id": "b1", "peers": 3, "blobs": { "b167…f553": 2, "0000…0001": 0 } }
```

### Error Handling

`mesh.publish.result` carries `ok: false` and `error` when the publish was refused (not granted, malformed `ttl`) or the template could not be signed or the event kept. Per the registry's error model, every field but `ok` is then undefined; `ok` stays, as NAP-RELAY requires of every publish result.

`mesh.subscribe` fails in one of two ways, and the binding surfaces both through the same `closed` callback:
- **Refused before it opens** — not granted, no `subId`, unreadable filters, malformed `ttl`: `mesh.subscribe.result` with `id` and `error`. No `mesh.closed` follows; there was no subscription to close.
- **Ended after it opened** — the local backlog could not be read, or the shell terminated it: `mesh.closed` with `subId` and `reason`.

A `ttl` that is not a whole number from 0 to 255 is an error, not a zero: a napplet that asked for reach and silently got none would have no way to tell. A `ttl` above the cap is **not** an error; it is clamped and the effective value reported.

`mesh.info.result` carries `error` only when the mesh cannot be queried at all; an offline mesh is `online: false`, not an error.

`mesh.blobs.result` carries `error` when `hashes` is missing, holds something that is not a sha256 in hex, or holds more than 64. Too many is refused whole, not cut short: a napplet that got answers for the first 64 of 100 would read the rest as "nobody has it". An offline mesh is `peers: 0` with every count `0`, not an error. A shell that cannot ask the mesh about blobs at all says so with `error`, rather than answering zeros.

## Shell Behavior

- The shell MUST send every `mesh` operation along trusted-mesh edges only — to members the user paired with — and MUST NOT treat a device as a peer because the transport mesh can reach it.
- The shell MUST sign publish templates under NAP-RELAY's rules: the napplet never sets `pubkey`, `created_at`, `id` or `sig`.
- *(Myco, NAP-LOCAL.)* A signed event in place of a template is flooded **as it is** — kept, then forwarded with the capped budget even if this device has seen it — provided it verifies and was delivered to this napplet. NAP-LOCAL bounds this: only kinds the mesh floods at all (not app manifests, not gift wraps), not an event a member sent without permission to travel further, and the same event at most once per 30 s. See [`NAP-LOCAL.md`](NAP-LOCAL.md).
- The shell MUST keep a published non-ephemeral event on this device before or regardless of forwarding it, and MUST deliver any published event to this device's live subscriptions, so a `ttl` of `0` is a working local publish. Ephemeral kinds are delivered and flooded but not kept, as NIP-01 says.
- The shell MUST clamp a requested `ttl` to the user's cap and MUST report the effective budget in `mesh.publish.result.ttl` and `mesh.eose.ttl`.
- The shell MUST NOT let a napplet raise the shell's own forwarding clamp for events arriving from members, whatever budget those events carry.
- The shell MAY decline to pass on a member's events at all, per member. Myco does unless the user gave that member the multihop write permission (`relay_write_multihop`): the member's events are kept and shown here, and go no further.
- The shell MUST respond to every `mesh.info`, `mesh.publish` and `mesh.blobs` request with a result message carrying the same `id`.
- The shell MUST deliver matching events as `mesh.event` until the napplet sends `mesh.close` or the shell terminates the stream with `mesh.closed`.
- The shell MUST send `mesh.eose` once per subscription, after this device's backlog and without waiting for members. Members' backlog MAY arrive on either side of it.
- The shell SHOULD deliver a member's backlog to the subscription that asked even when this device has seen the event before; an event seen once and since let go of is otherwise lost to it.
- The shell MUST bound a subscribe's backlog pull in time. Myco: the pull carries a 10 s budget that farther hops share, and this device listens to each member it asked for at most 8 s; later answers are dropped and only live events follow.
- The shell SHOULD deduplicate by event id so a copy arriving by a second path is delivered at most once per subscription; a napplet MUST still tolerate a duplicate. Myco remembers the last 4096 ids per subscription.
- The shell MAY keep separate caps for publish and subscribe. A flooded read costs more than a flooded write — every hop answers as well as forwards — so a lower subscribe cap is the expected default. Myco: publish 3, subscribe 2, each settable lower and never higher.
- The shell MUST answer `mesh.blobs` from the trusted mesh alone, with requests that fetch no body (Myco: BUD-01 `HEAD /<sha256>` to each member's Blossom). It MUST NOT ask the internet or follow a member's redirect off the mesh.
- The shell MUST bound `mesh.blobs` itself, whatever the napplet's call rate. Myco: six requests at a time, 4 s each, 12 s per call; a member that fails is skipped for the rest of the call; each (member, blob) answer is remembered — a hit for 60 s, a miss for 20 s — so a repeat call inside that asks nobody.
- The shell MAY enforce ACL checks on `mesh` capabilities, and MAY revoke the grant at any time; revocation takes effect on the next call and the next live delivery.

## Web Binding

`window.napplet.mesh` is frozen and offers `info`, `publish`, `subscribe` and `blobs`.

- `info()` and `blobs()` return promises that **reject** with the shell's `error`.
- `publish()` never rejects on a refusal: it resolves to `MeshPublishResult`, so a napplet branches on `ok`. It waits as long as a signer may take (5 min backstop) because the template is signed first.
- `subscribe()` returns at once with `{ id, on, close }`. `id` is the `subId`. `on(name, cb)` returns `{ close }`, which removes that one callback. A refused subscribe calls the `closed` callbacks with the reason.

## Security Considerations

- **Trust, not proximity.** Nothing here reaches a device the user has not paired with, however close it is. A stranger on the transport mesh may carry encrypted packets between members; it never receives a `mesh` publish, is never asked for backlog, and is never asked about a blob.
- **Amplification.** The hop budget is the only per-message cost control a napplet holds, and it is bounded by a cap the napplet cannot change. The shell's forwarding clamp for members' events is a separate, shell-owned number; nothing in this interface reaches it.
- **Acting as the user.** A `mesh` grant lets a napplet publish as the user to all their members, repeatedly, with no per-event prompt. Runtimes SHOULD say so in words at grant time, as for `relay`. *(Myco currently grants `mesh` to every napplet by default; this is temporary, and it will become opt-in.)*
- **What the mesh reveals.** `mesh.info.peers` is a count, and so is each `mesh.blobs` answer. This interface exposes no member identities, addresses, transports or signal data; a napplet learns that some of the user's people are reachable, not who or where. A separate NAP would be needed for presence and is deliberately not this one.
- **Untrusted filters, templates and answers.** Filters and templates arrive from untrusted code; the shell parses them fully and refuses anything unreadable rather than guessing. Members' backlog answers are checked against the subscription's own filters before delivery.
- **Asking is not free.** Every hash in `mesh.blobs` costs one request per member, often over a slow link. The cap of 64, the shell's own bounds and its memory of recent answers keep a napplet polling on a timer from turning the members' radios into its cache check.
- **Loop safety** is the shell's, by event id, not the budget's: every device forwards an event id it receives from a member at most once, so a flood terminates whatever budget it carried. The one deliberate repeat is the originating device passing an event on again under NAP-LOCAL, bounded per event in time.

## Implementations

- Myco (Android) — `myco-napplet-runtime/src/nap/mesh.rs` over the Circle (`docs/design/circle/circle.md`), carried by FIPS; hop carriage per `docs/design/core/event-gossip.md`. Core seam: `NappletMeshSink` in `myco-core/src/napplet.rs`. `mesh.blobs`: `myco-core/src/mesh_blobs.rs`. Web binding: `myco-napplet-runtime/assets/myco-prelude.js`.
