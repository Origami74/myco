# Roadmap

Where **Myco** is and where it goes next. The first plan (P0–P6, a two-phone
offline BLE demo) is done; this page is the second one. Each item has a
one-line goal and an **exit criterion** — the observable condition that says
it is done. Detail lives in the linked design docs; the day-to-day record is
[CHANGELOG.md](../CHANGELOG.md).

For orientation see [getting-started.md](./getting-started.md); for the doc
map, the [index](./README.md).

---

## Status — 2026-09-15

**Shipped** (v0.6.0, plus the unreleased `feat/napplet-runtime` branch):

- **The mesh.** BLE L2CAP with per-peer PSM discovery, Wi-Fi Aware (several
  phones per lane), the LAN lane (mDNS), TCP when online; multi-path per peer
  with standby links; an app-owned TUN scoped to Myco's uid; `.fips` DNS. The
  Dev tab shows every peer, lane, RTT and connect attempt.
- **Pairing and the Circle.** Mutual, signed pairing by NFC bump or QR over
  the auth service; single-use invite secrets; unpairing that reaches the other
  phone; the Circle gate on the relay and Blossom; per-peer permissions stored
  (no UI yet). Native encrypted file sharing between Circle members.
- **nsites.** Paste a link or scan a share; holder-first pull over the mesh,
  then any Circle member, then the internet; staged updates; discovery ("around
  me"); home-screen pins; deep links (`myco://app/<host>/<path>`); a custom relay
  or Blossom instead of the embedded ones.
- **Gossip.** Hop-limited push (3) and pull (2) between Circle members, the
  `MESH` envelope, seen-set loop safety, backlog replay on reconnect.
- **Napplets** (unreleased). NIP-5D manifests fetched by `naddr` or shared by
  bump; verified resolve into a sandboxed iframe; a user key with a guest
  profile and relay list; install review and per-app permission switches;
  NAPs: `shell`, `identity`, `relay` (pool reads, relay-pool publish), `outbox`
  (NIP-65 plans over local/mesh/internet lanes), `mesh` (hop-limited
  publish/subscribe, user-capped — Myco's own, [NAP-MESH](./design/napplet/NAP-MESH.md)),
  `resource` (`blossom:` only, local store first, fetched blobs kept).

**Not built**, from the first plan: NIP-77 negentropy reconcile; LRU eviction
with a size cap (Storage shows counts and offers "delete cache"; nothing
evicts on its own); transitive peer-list polling (reach is the Circle, plus
gossip hops); Linux interop (P6) as a tested pair; external-browser access
(NAT46). All still on the list below.

---

## Next

Ordered by what unblocks what. Each is its own PR or short series.

### N1 — Login

**Goal.** Let a person bring their own Nostr identity instead of the generated
guest user key, so what a napplet publishes is *them*. Two ways, one seam
behind the runtime's `Signer`:

- **Paste an `nsec`** (or scan it) into Settings › Identity. Stored like the
  device key; replaces the guest user key; the guest profile is not re-published.
- **Amber** (NIP-55, `nostrsigner:` intents): the key never enters Myco.
  `Signer::sign` and `public_key` round-trip through the Amber app;
  `publishEncrypted` becomes possible the same way. Falls back to the guest key
  when Amber is absent or declines.

**Exit criterion.** A napplet's `identity.getPublicKey()` returns the chosen
key; `relay.publish` produces an event signed by it; switching back to guest
works; with Amber, no key material is ever on disk or in memory in Myco.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) §7.1
(two identities) · [identity-pairing.md](./design/core/identity-pairing.md) §2 (storage).

### N2 — Drop mesh from nsites

**Goal.** An nsite talks to `ws://localhost:4870` like any relay; today an
event it publishes there is also flooded to the Circle at the default hop
budget, and its `REQ`s are recreated against Circle members. That made sense
before napplets; now the mesh is a *granted* capability (NAP-MESH) with a user
cap, and an nsite has no grant and no review screen. Make the loopback relay
socket **local-only**: nsite publishes are stored and shown here, forwarded
nowhere; nsite subscriptions are not replayed to peers. Reaching the room is
what a napplet is for.

**Exit criterion.** An nsite's publish is not seen on a paired phone; a
napplet's `mesh.publish` still is; the chat nsite in the demo set is either
ported to a napplet or documented as local-only.

**Design docs.** [event-gossip.md](./design/core/event-gossip.md) §0, §2.6 ·
[nsite-permissions.md](./design/nsite/nsite-permissions.md) §3 (the `Origin`
question this closes).

### N3 — Notifications

**Goal.** NAP-NOTIFY for napplets — `notify.show` from a napplet becomes an
Android notification in Myco's channel, tapping it deep-links back into the
napplet — with the grant on the review screen and the permissions sheet. The
Kotlin half exists for file offers (`FileOfferNotifier`); this generalises it.
A closed napplet cannot notify (no background execution); a doorbell that
should ring while the app is closed needs a Myco-side subscription, which is a
later item.

**Exit criterion.** A napplet with `notify` granted posts a notification while
its window is open; without the grant the call is refused; the notification
opens the napplet.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) S3 ·
[NAP-NOTIFY](https://github.com/napplet/naps/pull/11) (registry draft).

### N4 — Amber login

Folded into N1 as its second path; listed here because it is the one that
matters to people who already have an identity. Ships after the paste path,
on the same `Signer` seam.

### N5 — An app store napplet in place of the Discover tab

**Goal.** Retire the built-in Discover tab and ship "around me" as a
**napplet** — the first-party app store. It lists what your Circle holds
(nsites and napplets), shows who has each one, lets you install with one tap,
and surfaces new arrivals — all through the NAPs everyone else gets: `mesh`
for the room, `outbox` for reach beyond it, `resource` for icons, `intent`
(N6+) to hand an install to Myco. Dogfoods the runtime on the one feature that
needs every mesh capability, and lets the store evolve like any other app —
shared, updated and forked over the mesh — instead of being frozen into a
release.

**Exit criterion.** The Discover tab is gone; the store napplet ships
preinstalled, lists the same holders and apps the tab did, installs from the
list, and works with no internet. Needs an install intent (a napplet asking
Myco to fetch and review an app by pointer) that cannot skip the review
screen.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) S2b
(intents) · [NAP-MESH](./design/napplet/NAP-MESH.md) · [circle.md](./design/circle/circle.md).

### N6 — Release the napplet runtime

**Goal.** Cut v0.7.0 from `feat/napplet-runtime` after the two-phone checks:
share a napplet by bump with no internet; doorbell rings across phones; a
picture loads by `blossom:` from the other phone's store; permissions switch
live. README and the intro diagrams updated to say "apps", not "sites".

**Exit criterion.** Tagged, on GitHub Releases and Zapstore
([publish.md](./how-to/publish.md)); the demo runbook passes on two phones.

---

## Later

Each its own milestone with its own design pass. Roughly in order of pull.

- **Eviction.** An LRU cap on the Blossom store (default 2 GB) with pinned apps
  exempt; today the cache only shrinks when the user asks —
  [nsite-layer.md](./design/nsite/nsite-layer.md) §6.
- **Set reconciliation (NIP-77 negentropy)** between Circle members, so backlog
  catch-up is a sync rather than a replay of every open subscription —
  [propagation.md](./design/nsite/propagation.md) §5.
- **Transitive reach.** Poll a Circle member's Circle (with their consent) so
  discovery and pulls go past direct pairings —
  [identity-pairing.md](./design/core/identity-pairing.md) §6.
- **Peer permissions UI.** The per-peer record exists (`relay_write`,
  `relay_read_multihop`, …) with defaults for everyone; a switch per Circle
  member — [nsite-permissions.md](./design/nsite/nsite-permissions.md) §2.
- **Blob privacy over the mesh.** Whether a napplet's `blossom:` miss should
  ask every Circle member, or only the peer whose event referenced it —
  [napplet-runtime.md](./design/napplet/napplet-runtime.md) §7.11.
- **More NAPs.** `storage` (per-napplet key-value), `intent` + `inc` (open
  another napplet by role; napplet-to-napplet channels), `theme`, `link`,
  `config`; `resource` beyond `blossom:` (`https:`, `nostr:`, SVG
  rasterization) — [napplet-runtime.md](./design/napplet/napplet-runtime.md) S2b–S4.
- **Background subscriptions.** A Myco-side subscription that survives the
  napplet's window closing, so a doorbell can ring with the app closed. Needs
  the notification path (N3) and a battery story.
- **Relay read-auth.** The relay is open-read to Circle members; NIP-42 `AUTH`
  and per-peer read scoping would let a member hold private apps —
  [security.md](./design/core/security.md) §3.
- **External browsers (NAT46 / `.nsite`).** Let system Chrome reach a site;
  the in-process gateway serves only Myco's own WebViews —
  [ports.md](./reference/ports.md) §3.
- **Linux interop.** An Android phone and a Linux `BluerIo` node as a tested
  BLE pair; the per-peer PSM patch already makes it possible —
  [ble-interop.md](./design/fips/ble-interop.md).
- **USB transport** for seeding large sites —
  [usb-transport.md](./design/fips/usb-transport.md) (not started).
- **Multi-persona.** More than one device key per phone —
  [identity-pairing.md](./design/core/identity-pairing.md) §3.
- **Public-node peering** over the internet via FIPS discovery kinds —
  [nostr-kinds.md](./reference/nostr-kinds.md).

---

## The first plan, for the record

| Phase | Was | Landed |
| --- | --- | --- |
| P0 | scaffold, FIPS up, identity persisted | v0.1 |
| P1 | BLE peering with per-peer PSM discovery, developer UI | v0.1 |
| P2 | relay + Blossom + gateway, an nsite as a full-screen task | v0.2 |
| P3 | pairing + sync over the mesh; Circle; discovery | v0.3 |
| P3.5 | the consumer UI (bottom-nav shell, sheets, intro, names) | v0.4 |
| P4 | the two-phone airplane-mode demo | v0.4 |
| P5 | propagation at scale | partly: gossip planes, backlog replay; not eviction or negentropy |
| P6 | Linux interop | not as a tested pair |

Beyond it: Wi-Fi Aware and the LAN lane (v0.5–0.6), multi-path (v0.6), file
sharing, NFC, deep links, custom stores, and the napplet runtime (unreleased).
