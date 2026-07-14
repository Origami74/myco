# Identity & pairing

This doc covers how Myco establishes a device identity and how two devices
become peers. The central simplification over the upstream
[nostr-vpn](../../reference/nostr-vpn) design is that Myco has **no network,
no roster, and no admin**. A "pairing" is not a membership grant — it is just
the act of learning another peer's npub and treating their relay + Blossom as a
data source. Everything downstream (mesh address, where to fetch, who to poll)
is derived deterministically from that one npub.

See [diagram 09 — the two identities (device vs nsite author)](diagrams/09-identity-model.svg)
and [diagram 02 — Pairing & transitive peer discovery](diagrams/02-pairing-transitive-discovery.svg).

Related: [propagation.md](./propagation.md) (how a learned peer becomes a
content source and how that reach goes transitive), [security.md](./security.md)
(key storage threat model, self-authenticating data, what a mutual pairing does and
does not authorize).

## 1. One device identity = three derived forms

Myco reuses the FIPS unification: a single Nostr keypair is the **device
identity**. There is exactly one secret to guard, and three public forms derive
from it. None of the public forms is stored as ground truth — all are recomputed
from the key.

This device key is **never used to author nsites.** It identifies *this device*
on the mesh, authenticates BLE links, and addresses this device's own
relay + Blossom. nsite *author* identities are entirely separate external keys
(see §1.1); the device holds and serves authors' signed events but never holds
an author's secret key and never signs on an author's behalf.

| Form | Derivation | Used as |
| --- | --- | --- |
| **npub** (`npub1…`) | bech32 of the 32-byte x-only pubkey | device identity in QR/UI; host of *this device's* relay + Blossom (`<npub>.fips`) |
| **node_addr** (16 bytes) | `SHA256(npub)[0:16]` | mesh routing address |
| **ULA IPv6** (`fd00::/8`) | `fd` ‖ `node_addr[0:15]` | what `<npub>.fips` resolves to (AAAA) |

This chain is established in
[fips-architecture.md](../../reference/fips/docs/design/fips-architecture.md) and
[fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md).
The load-bearing consequence for pairing: **once you have a peer's device npub,
you can compute where they live on the mesh with no further exchange.** No
directory, no lookup, no signed announcement. Scanning a QR that carries a device
npub is sufficient to address that peer's services at `<npub>.fips:4870` (relay)
and `<npub>.fips:24243` (Blossom), via the local DNS interceptor
([fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)).

### 1.1 Device identity vs. nsite author identity (do not conflate)

These are two different kinds of key and they never overlap:

- **Device identity** — the device's one Nostr keypair (this section). It is the
  FIPS mesh address, the BLE link credential, and the host of *this device's*
  relay + Blossom (`<npub_device>.fips`). It is **never** used to author nsites.
- **nsite author identity** — *external* keys belonging to whoever authored a
  site, created by external nsite tooling elsewhere. An author key appears here
  **only** as the URL host of a requested site (`<npub_author>.nsite`) and as the
  `authors` filter in relay queries. The app holds and serves an author's signed
  events but never holds their secret key and never signs on their behalf.

The practical consequence: **the site you want and the peer you fetch it from are
different keys.** A site is identified by its *author* npub; the peer holding a
copy is a *holder*, reached at `<npub_holder>.fips`. To pull a held site you query
that holder's relay with `{kinds:[15128 or 35128], authors:[<author_pubkey>]}` —
the holder's device key addresses the relay, the author key selects the events.

## 2. Identity storage

Proposed: the secret key (`nsec`) is generated on first launch and persisted in
the app's private `filesDir` (`Context.getFilesDir()`), owned by the Rust core
(`myco-core`) and never surfaced to the WebView or to JS.

- **Generation:** on first run the core generates a fresh keypair if no key file
  exists, mirroring how the nostr-vpn core seeds its data dir on first launch
  (the Android side hands the core a data-dir path —
  [MainActivity.kt:50–55](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/MainActivity.kt) —
  and the core is responsible for what lives there).
- **Scope:** `filesDir` is app-private storage on Android, not world-readable.
  Whether to additionally wrap the key with the Android Keystore / a
  user-supplied passphrase is an open question deferred to
  [security.md](./security.md).
- **No export in v1:** there is no UI to display or copy the `nsec`. Backup /
  key portability is **TBD / open**.

> Contrast with nostr-vpn: nostr-vpn also stores a Nostr identity, but it pairs
> that identity with *network* state (which network you joined, the admin set,
> the roster). Myco stores **only the key**; there is no network record to
> persist because there is no network.

## 3. One identity per device (default)

**Proposed default:** one identity per device in v1. The app does not present an
account picker; the single on-disk key *is* the user.

- Simplifies the FIPS endpoint (one node_addr, one ULA, one advertised service
  UUID), the QR (one npub to show), and the BLE handshake (one pubkey to send in
  the `[0x00][pubkey:32]` pre-handshake exchange —
  [ble/io.rs](../../reference/fips/src/transport/ble/io.rs)).
- **Multi-persona is a later milestone.** When added, each persona is an
  independent keypair → independent node_addr/ULA → independent Library and source
  set. Personas would not share collected peers by default. Open question:
  whether the FIPS endpoint can host multiple node identities concurrently on
  one BLE radio, or whether persona switching tears down and rebuilds the
  endpoint. **TBD / open.**

## 4. QR pairing

### 4.1 Reuse: the scanner and the deep-link path

Myco reuses nostr-vpn's pairing UX wholesale; only the *payload semantics*
change.

- **Camera scanner.** The CameraX + ML Kit `BarcodeScanning` dialog is reused
  as-is:
  [QrScannerDialog.kt](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt).
  It is a self-contained Compose component — back-camera preview, QR-only
  barcode options ([:137–142](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt)),
  single-emit guard, and an `onScanned: (String) -> String?` callback that
  returns an error string to keep scanning or `null` to accept
  ([:204–208](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt)).
  Myco changes only what that callback validates (see below).
- **Deep-link intent.** nostr-vpn already wires a deep-link path: the activity
  reads `intent.dataString` and, if it matches the scheme, dispatches an import
  action ([MainActivity.kt:274–279](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/MainActivity.kt),
  with the same handling in `onNewIntent`,
  [:389–397](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/MainActivity.kt)).
  Myco reuses this structure with its own scheme, so a `myco://` link
  opened from anywhere (a chat, an NFC tag, another app) reaches the same
  add-peer code path as a scanned QR.

### 4.2 Our payload: `myco://pair/<base64>`

**Proposed default payload:**

```
myco://pair/<base64(JSON)>
```

where the JSON carries:

```json
{ "npub": "npub1…", "name": "green sammy", "pairSecret": "…" }
```

The **memorable name** is a human handle for this device — a colour + name like
`green sammy` or `blue james`, suggested automatically when you generate your code
(and editable). It's carried in the QR so peers recognise you by it in their
**Library** and **circle**, and because it's something you can say out loud, it
doubles as a quick check that you paired with the right person. It grants no
authority.

The **`pairSecret`** is a **long, high-entropy random string** (≈256 bits) — a
**single-use** bearer token (see §6.1). It authenticates the mandatory pairing
handshake — proving the peer actually scanned *this* invite — but is itself **not**
a membership or authorization token. Its unguessable length is the whole defence:
there is no key-exchange ceremony, just **echo-and-match** over the already-encrypted
mesh channel (§6.1). The `base64`
is URL-safe, unpadded — matching the encoding nostr-vpn uses for its own payloads
([invite.rs:10](../../reference/nostr-vpn/crates/nostr-vpn-core/src/invite.rs)).

Crucially, the payload carries **no MAC and no PSM**. Those are radio-layer
details that are (a) volatile (Android MAC randomization) and (b) not needed for
addressing — FIPS identifies a peer by the pubkey it sends during the BLE
pre-handshake, not by MAC, and the listener PSM is learned from BLE adverts at
connect time (see BLE doc / [propagation.md](./propagation.md)). The QR is a
*stable, transport-independent* identity; the radio details come over the air.

Validation in the `onScanned` callback is a prefix check, exactly paralleling
nostr-vpn's `if (!invite.startsWith("nvpn://invite/", …))` guard
([MainActivity.kt:375–382](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/MainActivity.kt)) —
ours checks `myco://pair/` and rejects anything else with "Not a Myco
peer code."

### 4.3 Contrast: what we drop from `nvpn://invite/`

nostr-vpn's QR is a **network invite**, not a peer card. Its decoded shape
([NetworkInvite in invite.rs:19–40](../../reference/nostr-vpn/crates/nostr-vpn-core/src/invite.rs))
carries a versioned, multi-field membership document:

| nvpn://invite/ field | Why Myco drops it |
| --- | --- |
| `networkId`, `networkName` | no network construct exists |
| `inviteSecret` | **kept, but reframed:** Myco carries a one-time `pairSecret` — a long random string echoed back over the Noise-encrypted channel — that proves the peer saw your invite, *not* a join-as-membership / authorization token |
| `admins[]` | no admin role; data is self-authenticating, not authority-gated |
| `participants[]` | no roster; you collect peers one npub at a time |
| `inviterEndpoints[]`, `relays[]` | endpoints derive from npub via `<npub>.fips`; no relay hints needed |

The upstream parser even *requires* an admin to exist
([invite.rs:89–110](../../reference/nostr-vpn/crates/nostr-vpn-core/src/invite.rs)) —
an invite with no admin is an error. Myco has no such concept. We keep the
prefix-and-base64 *envelope* shape and the URL-safe-no-pad encoding, and throw
away the entire membership document inside it. Our envelope decodes to three
fields (`npub`, `name`, `pairSecret`), where `pairSecret` authenticates the
pairing handshake but grants **no membership or admin authority**.

## 5. Peer as data source

When you scan a peer's `myco://pair/<base64>` (or open the deep link), the app:

1. Decodes and validates the npub, memorable name, and `pairSecret`.
2. Derives `node_addr` and the `fd00::` ULA from the npub (§1) — no network call.
3. **Initiates the mandatory handshake** against the inviter's on-device pairing
   endpoint at `<npub>.fips` (§6.1): echoes `pairSecret` back over that
   Noise-encrypted channel, the inviter matches it and confirms.
4. On a completed handshake, adds two sources to your source set, addressed
   deterministically:
   - relay at `<npub>.fips:4870`
   - Blossom at `<npub>.fips:24243`
5. Stores the npub (and memorable name) in your **circle** (the local peer list).

Pairing is **handshake-mandatory and always mutual**: scanning is not a purely
local act — it initiates the secret-echo handshake against the inviter's on-device
`<npub>.fips` endpoint, and completion makes you a **mutual** source (each side
holds the other; see §6). **Both devices must be reachable at pairing time** —
they're physically together when the QR is scanned, so the BLE link is up.
Whether anything is actually reachable later is a liveness question resolved over
FIPS (live-path only; see [propagation.md](./propagation.md)).

Why this is safe without authorization: all content the peer serves is
**self-authenticating** — Nostr events are signed by their (external) author and
Blossom blobs are content-addressed by SHA-256
([nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)). A
malicious or impersonating source cannot forge an nsite whose *author* key it
doesn't hold, and cannot substitute blob content without changing the hash.
Re-serving an author's already-signed events is normal relay behaviour and does
not make the serving device the author. So "anyone can be a source" carries no
trust cost; verification happens at fetch/serve time, covered in
[security.md](./security.md).

> This is the inversion of the nostr-vpn model: there, being added to a network
> *granted reachability* (and was gated by an invite secret + admin). Here,
> learning an npub grants nothing on the peer's side — it only configures *your*
> client to point at *their* always-derivable address.

## 6. Mutual pairing and transitive authorization

Every pairing is **mutual** (§5): completing the handshake makes each side a
source for the other and additionally authorizes each side to **poll the other
for their collected peer list**, transitively widening reach (the right half of
[diagram 02](diagrams/02-pairing-transitive-discovery.svg)).

A mutual pairing is formed by **invite-pairing with a one-time secret** (§6.1) —
**not** by both parties scanning each other. A single scan plus a
secret-authenticated round-trip over the mesh yields the mutual relationship, and
the secret is what *proves* the peer actually saw your invite. Invite-pairing is
**the** pairing method; there is no separate one-way, no-handshake mode.

### 6.1 Invite-pairing via a one-time secret

Modeled on **NIP-46 (Nostr Connect)** — a connection token carrying a one-time
secret plus a rendezvous, where the other side connects, authenticates with the
secret, and an ack completes the handshake. FIPS supplies the rendezvous (the
mesh), so no public relay is required.

1. **Invite token** (shown as a QR / link): `{npub_A, name, pairSecret}`.
   `<npub_A>.fips` is derivable from `npub_A` (§1), so the token carries no address.
   The `pairSecret` is a **long random string** (≈256 bits), **single-use**, and
   optionally **TTL-bounded**.
2. **B scans once** and opens a connection to **A's pairing endpoint at
   `<npub_A>.fips`** — an **on-device** endpoint (an FSP port or a channel on the
   embedded relay; no third party). That channel is already **Noise-XK encrypted and
   authenticated to A's device key** (§1), so B is talking to the real A in
   confidence. B simply **echoes the `pairSecret` back** over it, along with
   `{npub_B, name}`. No key-exchange protocol is needed: because the secret is a long
   random string, echoing it *inside the encrypted channel* reveals nothing to anyone
   else and cannot be guessed.
3. **A matches the secret** against the one it generated (single-use, not yet
   redeemed) and shows a **confirm prompt** — B's npub + memorable name — that A's
   user taps **OK** to accept. A then **acks** (+ A's memorable name), marks the
   secret consumed, and stores B in its circle — a **mutual** pairing from a
   **single** scan. Both sides now hold each other, so each may poll the other's
   peer list.

This is **stronger** than a bare trust-on-first-use scan (see
[security.md § 4](./security.md)): two things already hold the line — FIPS **Noise
XK** authenticates and encrypts the channel to `<npub_A>.fips` (no relay-MITM,
nothing on the wire to capture), and the **OK prompt** keeps a human in the loop to
confirm the memorable name. The long random `pairSecret` supplies the missing piece —
proof the peer actually scanned *this* invite (and anti-spam, since your npub is
public). Because the endpoint is on-device, invite-pairing works peer-to-peer /
offline — both sides must be reachable when B completes (an optional public
rendezvous for fully-async pairing is a possible later extension).

Once mutually paired:

- Alice can ask Ben "who are your peers?" and learn Carl and Dana. She then
  derives Carl's and Dana's `<npub>.fips` addresses (§1) and can fetch their
  nsites too — multi-hop over FIPS, with Ben as an intermediate hop on the live
  path.
- **The completed invite-pairing is the authorization.** Because every pairing is
  mutual, Ben answers Alice's peer-list poll precisely because they completed
  invite-pairing — a lightweight, symmetric consent that exists in v1. It is *not*
  an admin grant and confers no write access: Alice can read Ben's list of npubs,
  that is all.
- The shared data is a **list of npubs** (the cheapest possible currency:
  re-deriving everything else from each npub is free). No endpoints or membership
  tokens cross this poll.
- Transitive reach is **not transitive trust.** Content fetched from Carl via Ben
  is still verified by signature/hash at the consumer (§5). Ben cannot launder
  unsigned or tampered content through the poll; he can only *name* peers.

How the poll is carried, how often, scope/TTL, and whether Carl's content is
pulled eagerly or on demand are propagation concerns — see
[propagation.md](./propagation.md). Author-signed manifest events (kinds
15128 / 35128) flood with a default TTL of 5 hops, while the large blobs stay
pull-only (fetched only when a site is opened).

### Open questions

- **Peer-list poll wire format & endpoint.** Because all pairings are now mutual,
  the **transitive peer-list-poll authorization exists in v1** (any completed
  pairing authorizes the poll) — but the poll *mechanism itself* can still land
  later. Is the poll a dedicated Nostr event kind on the peer's relay, a
  Blossom-style listing, or a new FSP request? **TBD / open.**
- **Invite-pairing wire detail — the v1 mechanism.** The long random `pairSecret`
  in invite-pairing (§6.1) *is* the proof Alice paired with Ben: echoed back inside
  the Noise-encrypted channel, an unguessable single-use string a captured/relayed
  invite cannot reproduce. Invite-pairing is **the** v1 pairing path, so these are v1
  must-build, with detail still open: the token format and secret length, the
  on-device endpoint (a dedicated FSP port vs. a NIP-46-style channel on the embedded
  relay), and single-use / TTL / rate-limit enforcement. **TBD / open.**
- **Revocation / unpairing.** Removing a peer locally stops *us* polling them,
  but there is no negative announcement. Whether a peer should stop answering an
  old mutual pairing after the counterpart removed them is **TBD / open**.
- **Key backup / portability.** No `nsec` export in v1 (§2); migration story is
  **TBD / open**.
- **Multi-persona on one radio.** Whether the FIPS endpoint can host concurrent
  identities or must rebuild on persona switch (§3). **TBD / open.**
- **Memorable-name trust.** The memorable name is sender-chosen, non-unique text;
  the Library UI must never treat it as authority, should visually distinguish it
  from the npub, and should flag collisions (two peers who chose `green sammy`).
  Whether to derive the colour deterministically from the npub (so it's harder to
  spoof) is **TBD / open**.

## 7. Implemented in v0.0.3 — NFC tap-to-pair, single-use ledger, unpairing

How the app actually ships the §5–6 model, including a path the design above did
not anticipate (NFC) and where the wire detail landed.

**NFC tap-to-pair.** Alongside the QR path (§4), a device can present its invite
over NFC. While the user is on the Circle tab the app emulates a standard NDEF
Type-4 tag (Android host card emulation, AID `D2760000850101`) whose single URI
record is the same `myco://pair/<base64>` payload as the QR. The counterpart's OS
reads it through normal NDEF tag dispatch — no "reader mode" on either side — and
delivers the `myco://` link to the app via an intent filter, reaching the same
add-peer code path as a scan. Both phones present-and-poll at once, so a single
bump can pair symmetrically.

**Sharing an app over NFC, and the reader-mode exception.** The same emulation also
serves the app-share payload: the share sheet presents a `myco://share/<base64>`
URI (`{nsite, npub, name, secret}`), so a tap opens the shared app *and* pairs,
exactly like scanning its QR. This needed one carve-out from the "no reader mode"
rule above. The share and *Add an app* surfaces are modal bottom sheets, each shown
in its **own focused window**, where the OS's passive `NDEF_DISCOVERED` dispatch
(which reaches the Activity fine from the full-screen Circle tab) no longer arrives.
So while the *Add an app* sheet is open the app claims foreground NFC **reader
mode** and reads the tapped peer's tag itself. It is scoped strictly to that sheet
(the Circle bump path is unchanged and still uses passive dispatch), and it forwards
**only `myco://`-scheme URIs** to the same handler as a scan — an unrelated URL tag
held to the phone is read but ignored, never opened.

**Where the secret lives.** §6.1 framed the `pairSecret` as echoed back inside the
Noise-encrypted channel to `<npub_A>.fips`. The implementation keeps that property:
the scanner/tapper sends a signed pair **request** (kind 9101) to
`<npub_A>.fips:4870` — already Noise-XK encrypted and authenticated to A — carrying
the secret. What differs from the doc is *who matches it*: A enforces single-use
locally, via a small persisted ledger of the secrets it has issued. Each presented
code mints a fresh secret; it is consumed on first accept and the presented payload
rotates, so a captured QR/tag can't pair twice.

**Auto-accept scope.** A request auto-accepts only while A is actively presenting
(on the Circle tab) and the secret matches a live issued one. A request that
arrives otherwise — A on another screen, or Myco launched by the tap — surfaces an
accept/ignore prompt instead, keeping the §6.1 human-in-the-loop confirmation.

**Unpairing (partially answers the open question above).** Forgetting a peer now
sends a signed **pair-remove** event (kind 9103) to their relay; on receipt they
drop the sender from their Circle, so the two sides stay symmetric. It is
best-effort and fire-once — an offline peer is not retried, so their Circle keeps a
stale entry until they forget us or we re-pair. A durable (queue + ack) handshake
is left open.

**NFC exposure.** While presenting, the emulated tag answers *any* NFC reader, not
just another Myco device, with `{npub, name, pairSecret}`. This is acceptable: the
npub is already public, the name is sender-chosen and low-sensitivity, and the
secret is single-use (a captured tag can't be replayed). It is deliberately limited
to the moments the user is on the Circle tab, not the whole time the app is open.
