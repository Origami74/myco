# Device identity

This doc covers the **device key**: the one Nostr keypair that is this
phone's FIPS node identity and mesh address.

How two phones come to share things is not here. That is
[nap-neighbor/](../nap-neighbor/README.md#keys): links set up from a one-time
invitation, each with its own **link key**, which is neither the device key
nor the user key.

> **Three keys on this phone.** Everything below is about the **device key**.
> A napplet publishes as a separate **user key** (`user.nsec`): a guest
> generated on first launch, replaceable by logging in with an `nsec` or a
> signer app (NIP-55, Amber — then no user key is stored here at all); see
> [../napplet/napplet-runtime.md §7.1](../napplet/napplet-runtime.md).
> Each link has its own **link key** ([nap-neighbor/](../nap-neighbor/README.md#keys)).

See [diagram 09 — the two identities (device vs nsite author)](../diagrams/09-identity-model.svg).

Related: [security.md](./security.md) (key storage threat model,
self-authenticating data).

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
[fips-architecture.md](../../../reference/fips/docs/design/fips-architecture.md) and
[fips-ipv6-adapter.md](../../../reference/fips/docs/design/fips-ipv6-adapter.md).
The load-bearing consequence: **once you have a peer's device npub,
you can compute where they live on the mesh with no further exchange.** No
directory, no lookup, no signed announcement. A device npub (from a link offer's `.fips` URL, say)
is sufficient to address that peer's services at `<npub>.fips:4870` (relay)
and `<npub>.fips:24243` (Blossom), via the local DNS interceptor
([fips-ipv6-adapter.md](../../../reference/fips/docs/design/fips-ipv6-adapter.md)).

### 1.1 Device identity vs. nsite author identity (do not conflate)

These are two different kinds of key and they never overlap:

- **Device identity** — the device's one Nostr keypair (this section). It is the
  FIPS mesh address, the BLE link credential, and the host of *this device's*
  relay + Blossom (`<npub_device>.fips`). It is **never** used to author nsites.
- **nsite author identity** — *external* keys belonging to whoever authored a
  site, created by external nsite tooling elsewhere. An author key appears here
  **only** as the URL host of a requested site (`<npub_author>.localhost`) and as the
  `authors` filter in relay queries. The app holds and serves an author's signed
  events but never holds their secret key and never signs on their behalf.

The practical consequence: **the site you want and the peer you fetch it from are
different keys.** A site is identified by its *author* npub; the peer holding a
copy is a *holder*, reached at `<npub_holder>.fips`. To pull a held site you query
that holder's relay with `{kinds:[15128 or 35128], authors:[<author_pubkey>]}` —
the holder's device key addresses the relay, the author key selects the events.

## 2. Identity storage

The secret key (`identity.nsec`) is generated on first launch and persisted in
the app's private `filesDir`, owned by the Rust core (`identity_store.rs`) and
never surfaced to a WebView.

- **Generation:** on first run the core generates a fresh keypair if no key file
  exists; the Android side hands the core a data-dir path and the core owns
  what lives there ([settings.md](../../reference/settings.md)).
- **Scope:** `filesDir` is app-private storage on Android, not world-readable.
  Whether to additionally wrap the key with the Android Keystore / a
  user-supplied passphrase is an open question deferred to
  [security.md](./security.md).
- **No export in v1:** there is no UI to display or copy the `nsec`. Backup /
  key portability is **TBD / open**.

## 3. One identity per device (default)

**Proposed default:** one identity per device in v1. The app does not present an
account picker; the single on-disk key *is* the user.

- Simplifies the FIPS endpoint (one node_addr, one ULA, one advertised service
  UUID) and the BLE handshake (one pubkey to send in
  the `[0x00][pubkey:32]` pre-handshake exchange —
  [ble/io.rs](../../../reference/fips/src/transport/ble/io.rs)).
- **Multi-persona is a later milestone.** When added, each persona is an
  independent keypair → independent node_addr/ULA → independent Library and source
  set. Personas would not share collected peers by default. Open question:
  whether the FIPS endpoint can host multiple node identities concurrently on
  one BLE radio, or whether persona switching tears down and rebuilds the
  endpoint. **TBD / open.**

