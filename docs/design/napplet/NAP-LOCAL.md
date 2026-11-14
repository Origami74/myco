NAP-LOCAL
=========

Keep on This Device, and Pass On What You Were Shown
----------------------------------------------------

`draft` · **provisional**

**NAP ID:** NAP-LOCAL
**Domain:** `local` (plus `resource.keep` in NAP-RESOURCE, and signed input to every publish)
**Depends:**
- `relay` — wire · required — imports `EventTemplate` and the NIP-01 event shape.
**Web binding (NIP-5D):** `window.napplet.local` · `shell.supports("local")`; `window.napplet.resource.keep`

> A Myco addition, written in the napplet/naps template. The napplet.run spec
> may settle keeping differently — folded into `relay.publish`, say — and this
> interface would move with it. Implementation:
> `myco-napplet-runtime/src/nap/local.rs`, `src/delivered.rs`,
> `nap/relay.rs` (`signed_or_template`), `nap/resource.rs` (`keep`).

## Description

Myco's shell caches everything a napplet reads, and the cache forgets (see
`docs/design/core/architecture.md`, "Kept and cached"). NAP-LOCAL lets a napplet
say what should **stay**, and lets it **pass on** an event it was shown, without
ever signing someone else's words as the user.

Two things, one rule:

- **Keep.** `local.publish` stores an event in this device's own relay, which
  nothing evicts, and publishes it nowhere else. `resource.keep` does the same
  for a blob, in this device's own Blossom. Kept is not private: app peers
  ([nap-neighbor/app-peers.md](../nap-neighbor/app-peers.md)) and the device's own nsites read that relay and Blossom, and with a custom
  relay or Blossom configured, kept means written to that server.
- **Pass on.** Every publish (`local`, `relay`, `outbox`) accepts a
  **signed event** in place of a template and sends it **as it is**: kept here,
  then to relays again.

The rule: a signed event or a blob is accepted only if the runtime **delivered
it to this napplet** — in a query answer, a subscription, an outbox answer or a
resource fetch. A napplet passes on what it was shown, never an event it got
from nowhere.

## API Surface

| Operation | Parameters | Result | Wire |
|-----------|------------|--------|------|
| `local.publish` | `event` — an `EventTemplate`, or a signed NIP-01 event | `{ ok, event, eventId, error? }` | `local.publish` / `local.publish.result` |
| `resource.keep` | `url` (`blossom:sha256:<hex>`) or `sha256` | `true` | `resource.keep` / `resource.keep.result` · `resource.keep.error` |
| `relay.publish`, `outbox.publish` | as their NAPs, `event` may be signed | as their NAPs | unchanged |

An `event` is **signed** when it carries a `sig`. A template has none.

## Shell Behavior

- A template given to `local.publish` MUST be signed under NAP-RELAY's rules and
  stored locally only: no relay, no neighborhood. Signing as the user is the `relay`
  grant's power, so a template also needs `relay`; it is refused before
  anything is signed.
- `resource.keep` MUST need the `local` grant, whatever `resource` allows.
- A signed event published through `relay` or `outbox` is kept under
  that domain's grant, not `local`'s: passing an event on means holding it.
- A signed event MUST verify, and MUST have been delivered to this napplet;
  otherwise the publish fails with `ok: false`. This holds for the user's own
  events too.
- The shell MUST NOT re-sign a signed event as a template.
- Kinds a publish would refuse as a template stay refused when signed.
- A NIP-70 protected event (`["-"]`) MAY be kept but MUST NOT be sent on — not
  to relays, not into a neighborhood — by anyone but its author.
- A `sig` that is empty or `null` leaves a template a template.
- Every successful publish of a signed event MUST keep it in the local relay.
  Then: `local` sends it nowhere; `relay` pushes it to the relay pool; `outbox`
  fans it out as planned — **even if this device has seen it before**.
- *(Myco.)* The same event is passed on to relays at most once per 30 s.
- *(Myco.)* Publishing into a `neighborhood:` address goes to NAP-NEIGHBOR's
  data plane, not here; see [nap-neighbor/](../nap-neighbor/README.md).
- `resource.keep` MUST refuse a blob not delivered to this napplet
  (`blocked-by-policy`) or no longer held here (`not-found`). Keeping sends
  nothing; nothing pushes blobs to peers.
- Keeps are permanent: there is no unkeep.

### Delivered, remembered

The shell records every event id and blob hash it hands a napplet in a per-napplet
**scalable bloom filter** (`src/delivered.rs`): a chain of filters, each twice
the capacity of the last and with a tighter false-positive rate, keyed randomly
per filter. Overall false-positive rate stays under 0.1%. A napplet's ledger (its
event chain and its blob chain) is capped at 8 MB: once a doubled filter would
take more than half a chain's share, new filters repeat the last one's size and
the chain becomes a sliding window, dropping its oldest filter. It lives in
memory, is shared by all of a napplet's open windows, and is dropped when the
last one closes.

A false positive lets a napplet keep or pass on an event it was not shown — one
that must still be validly signed, and that it already holds. No false
negatives.

## Security Considerations

- **Not a signing oracle.** The shell signs only templates; signed events pass
  through untouched. A napplet cannot get the user's signature on words it did
  not write in a template.
- **Amplification.** A rebroadcast needs no signature, so a signer app's
  approvals do not pace it. The shell passes the same event on to relays
  at most once per 30 s.
- **Leaking neighborhood posts.** A napplet granted `relay` or `outbox` can
  send an event it was shown in a neighborhood — a neighbor's note, chat with
  a NIP-40 expiry — to public relays, where the author may not have meant it
  to go. Protected (NIP-70) events are refused; everything else is the napplet's
  call under the grant the user gave it.
- **Disk.** Keeps are unquota'd and permanent. A napplet granted `local` can
  fill the phone with what it was shown, blobs included; switching `local` off
  per app stops `local.publish` and `resource.keep`. Passing an event on keeps
  it too, under the publishing grant.

## Implementations

- Myco (Android) — runtime: `nap/local.rs`, `delivered.rs`, `EventSink::keep` /
  `rebroadcast`, `NapContext::kept_blobs`. Core: `OutboxService` (keep,
  rebroadcast, `PASS_ON_COOLDOWN`), per-napplet ledgers in `NappletHost`.
