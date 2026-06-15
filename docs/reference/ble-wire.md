# BLE Wire Reference

The exact on-wire facts for FIPS BLE: link type, PSM, service UUID, advert
format, the pre-handshake identity exchange, link authentication, and MTU
handling. This is the *what-on-the-wire* companion to the strategy doc,
[../design/ble-interop.md](../design/ble-interop.md) (the *why*).

Every constant below is confirmed against the local fips-core checkout; the
source line is cited where it is load-bearing. Myco's `AndroidBleIo` must
match these byte-for-byte to interoperate with the reference `BluerIo`.

## Link type

| Property | Value |
| --- | --- |
| Channel | L2CAP Connection-oriented Channel (CoC) |
| Mode | **SeqPacket** — message boundaries preserved |
| Framing | **None.** One `send` = one FIPS packet = one `recv`. |
| Reliability | Reliable, in-order (L2CAP guarantees) |
| Min Android API | 29 (Android 10) for L2CAP CoC |

SeqPacket preservation of message boundaries is *why* FIPS adds no framing
over BLE — unlike the TCP/Tor transports, which need the FMP length prefix to
delimit packets in a byte stream
([../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs#L5)).
A zero-length `recv` signals the peer closed the channel.

## PSM

```text
DEFAULT_PSM = 0x0085  (133 decimal)  — LEGACY default only
```

In the L2CAP **dynamic** PSM range (0x0080–0x00FF)
([../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs#L49)).
`0x0085` is a **legacy default / last resort** — the fallback when no
per-peer PSM has been learned yet. The old model, where the whole mesh agrees
on this one number and every node both **listens** on and **dials** it, is
**dropped**: only the reference Linux backend (BlueZ) can bind a fixed
listener PSM, and relying on it does not survive contact with Android or
macOS.

Dynamic listener PSM assignment is the **general** case, not a BlueZ quirk
turned inside out. Android's `listenUsingInsecureL2capChannel()` returns an
**OS-assigned** PSM (almost never 133), and Apple's
`CBPeripheralManager.publishL2CAPChannel` likewise yields an OS-assigned
`CBL2CAPPSM`. BlueZ is the **outlier** that *can* bind a fixed value. So
rather than special-casing Android, every node — Linux, Android, macOS —
**advertises its own OS-assigned listener PSM** and every dialer **reads the
peer's PSM before `connect()`** (see
[Per-peer PSM advertisement scheme (all platforms)](#per-peer-psm-advertisement-scheme-all-platforms)
and the [PSM problem](../design/ble-interop.md#the-psm-problem)).
`createL2capChannel(psm)` on Android and the equivalent dialers elsewhere can
dial any PSM, so the learned value is what gets used.

## FIPS service UUID

```text
9c90b790-2cc5-42c0-9f87-c9cc40648f4c
```

Defined as `bluer::Uuid::from_u128(0x9c90_b790_2cc5_42c0_9f87_c9cc_4064_8f4c)`
in
[../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs#L131).
Derivation noted in source: `SHA-256("FIPS: welcome to cryptoanarchy")` with
UUID v4 version/variant bits applied. This is the **single, fixed** UUID for
the whole FIPS mesh; both advertiser and scanner filter on it.

> **Confirmed from source.** UUID, derivation string, and `from_u128`
> literal all read directly from
> [io.rs](../../reference/fips/src/transport/ble/io.rs#L127-L132). It is
> distinct from bitchat's GATT UUID `F47B5E2D-4A9E-4C5A-9B3F-8E1D2C3A4B5C`,
> which is why the two meshes cannot interoperate
> (see [why not bitchat](../design/ble-interop.md#why-not-bitchat-as-a-transport)).

## Advertisement format

Adverts are **UUID-only** — they carry the FIPS service UUID and **no
identity material**. Identity is exchanged only after connect, over the
encrypted channel
([../../reference/fips/src/transport/ble/discovery.rs](../../reference/fips/src/transport/ble/discovery.rs#L1-L6)).

The reference advertiser
([io.rs](../../reference/fips/src/transport/ble/io.rs#L389-L401)) emits:

| Field | Value |
| --- | --- |
| Type | Peripheral |
| Service UUIDs | `{ 9c90b790-…-40648f4c }` |
| Local name | `"fips"` |
| Min interval | 400 ms |
| Max interval | 600 ms |

The scanner sets a discovery filter on the FIPS UUID and yields each matching
device's address as a `BleAddr`
([io.rs](../../reference/fips/src/transport/ble/io.rs#L246-L273)). On Android,
`AndroidBleIo::start_scanning` must filter to the same UUID and surface
`BleAddr` values for the scan/probe loop.

`BleAddr` wire/string form is `adapter/AA:BB:CC:DD:EE:FF` (6-byte BD_ADDR plus
an adapter label)
([../../reference/fips/src/transport/ble/addr.rs](../../reference/fips/src/transport/ble/addr.rs#L37-L49)).
On Android the address is the (possibly randomized) BLE device address; the
adapter label is a fixed tag. The address is a **transient dialing handle**,
not an identity — see
[MAC randomization](../design/ble-interop.md#why-mac-randomization-is-harmless).

## Pre-handshake pubkey exchange

Immediately after the L2CAP channel is established (inbound *and* outbound),
both sides exchange a fixed 33-byte identity message **before** any FMP/Noise
traffic
([../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs#L662-L714)):

```text
byte 0        : 0x00          (PUBKEY_EXCHANGE_PREFIX)
bytes 1..=32  : x-only pubkey (32 bytes)
total         : 33 bytes      (PUBKEY_EXCHANGE_SIZE)
```

Rules, exactly as the reference enforces them:

- **Both sides send and both sides receive** their own `[0x00][pubkey:32]`.
  The exchange is symmetric, not request/response.
- The prefix `0x00` distinguishes this message from FMP packets, whose
  version byte is ≥ `0x01`. (The exchange is **temporary** — it goes away
  when FMP migrates from the IK to the XX handshake, which carries identity
  inline.)
- **Receive timeout: 5 s** (`PUBKEY_EXCHANGE_TIMEOUT_SECS`). A peer that
  connects but never sends its pubkey is dropped rather than blocking the
  accept/probe loop forever.
- A received message of the wrong length, or a wrong prefix byte, is a
  protocol error; the connection is dropped.
- The received 32 bytes parse as a secp256k1 `XOnlyPublicKey`; failure ⇒
  drop.

The peer's pubkey returned here drives three things, all above the trait:
the `node_addr`-based **cross-probe tiebreaker** (smaller node_addr's outbound
wins), the **discovery buffer** entry that lets the node layer auto-connect,
and the identity the subsequent **Noise IK** handshake authenticates.
`AndroidBleIo` does **not** implement any of this — it only carries the 33
bytes like any other `send`/`recv`.

## Noise IK link authentication

After the pubkey exchange, FMP runs the **Noise IK** link handshake over the
same L2CAP `Stream` to authenticate the link hop-by-hop and derive the link
keys. This is FIPS's two-layer crypto: Noise IK hop-by-hop (FMP) under Noise
XK end-to-end (FSP). None of it is BLE-specific and none of it lives in
`AndroidBleIo` — from the transport's view it is opaque application bytes
moving through `send`/`recv`. See
[../../reference/fips/docs/design/fips-transport-layer.md](../../reference/fips/docs/design/fips-transport-layer.md)
for where transport ends and FMP begins.

The practical implication for the BLE backend: **Bluetooth-layer pairing /
bonding is not used.** Use the *insecure* L2CAP variants on Android
(`listenUsingInsecureL2capChannel`, `createInsecureL2capChannel`). Security
is Noise, end to end and hop to hop; BLE bonding would add a pairing UX and
buy nothing.

## MTU handling

MTU is a **per-link** property, queried per `Stream`, never assumed constant
([io.rs](../../reference/fips/src/transport/ble/io.rs#L31-L35),
[../../reference/fips/docs/design/fips-transport-layer.md](../../reference/fips/docs/design/fips-transport-layer.md)):

| Knob | Source | Notes |
| --- | --- | --- |
| `send_mtu()` | per `Stream`, from the OS-negotiated channel | bytes per outbound packet |
| `recv_mtu()` | per `Stream` | sizes the receive buffer |
| Transport default MTU | `BleConfig` | reference default **2048**; requested at listen/connect time |

Reference behavior the Android backend should mirror:

- The listener and the connecting socket both request the configured receive
  MTU up front (`set_recv_mtu`)
  ([io.rs](../../reference/fips/src/transport/ble/io.rs#L344-L348),
  [io.rs](../../reference/fips/src/transport/ble/io.rs#L371-L373)).
- A send larger than the link's effective MTU is **rejected**, not fragmented
  (`TransportError::MtuExceeded`)
  ([mod.rs](../../reference/fips/src/transport/ble/mod.rs#L308-L316)). FIPS
  sizes packets to the link; the transport never splits.
- Android: report whatever MTU the OS negotiated for each L2CAP channel via
  `send_mtu()`/`recv_mtu()`. Do not hard-code; chipset floors/ceilings vary
  (see [device variance](../design/ble-interop.md#device-and-rom-variance)).
- macOS (the `bluest` CoreBluetooth dev/test backend, `BluestIo`): the
  `CBL2CAPChannel` exposes a byte **stream**, not SeqPacket, so this backend
  restores message boundaries with a **2-byte length-prefix L2CAP framing** on
  top of the stream. That framing is internal to `BluestIo`; the bytes handed
  up to FIPS are the same per-packet boundaries every other backend preserves.

## Per-peer PSM advertisement scheme (all platforms)

> **Proposed (Myco net-new). Not in reference fips-core.** This is the
> **mandatory, symmetric** fix for the [PSM problem](../design/ble-interop.md#the-psm-problem)
> across **Linux, Android, and macOS**; it sits **below** the `BleIo` trait
> and is invisible to FIPS above it.

Because every node's listener PSM is OS-assigned on the platforms that matter
(Android `listenUsingInsecureL2capChannel`, macOS
`CBPeripheralManager.publishL2CAPChannel`) and the standard FIPS advert is
UUID-only, **every** peripheral additionally advertises **its own current
listener PSM** so other devices can dial it. There is no fixed well-known PSM
to fall back on across the mesh; the scheme is symmetric — no "central" role
is privileged.

- **Carrier (proposed):** a 16-bit little-endian PSM in a **service-data**
  field keyed on the FIPS service UUID, alongside the UUID in the same advert.
  Legacy advertising PDUs cap at ~31 bytes, so a 128-bit UUID + 2-byte PSM is
  the tight, legacy-safe layout. Extended advertising (BLE 5.0+) is the
  fallback if a device can't fit it. The alternative / complementary carrier
  is a **readable GATT characteristic** the dialer reads once before the
  L2CAP connect — this is the path macOS (CoreBluetooth) needs, since
  CoreBluetooth advertising payloads are restrictive; either carrier satisfies
  the contract. *(Exact field encoding TBD pending hardware tests.)*
- **Consume side:** each scanner parses the PSM (from the service-data field
  or the GATT read) and maintains a short-lived `BleAddr → PSM` map. When FIPS
  calls `BleIo::connect(addr, psm)`, the backend **substitutes the learned
  PSM** for `addr` and dials that. FIPS is unaware.
- **Lifetime:** the map is keyed on `BleAddr`, which **rotates** with MAC
  randomization, so it must be **re-learned each scan cycle**, not cached
  durably. A stale entry simply causes a dial failure and a re-probe on the
  next scan tick (the `scan_probe_loop` cooldown already retries).
- **All backends, symmetric:** advertising-own-PSM and reading-peer-PSM apply
  uniformly to `AndroidBleIo`, the macOS `BluestIo`, and — via a small
  fips-core patch teaching the same field to `BluerIo` — Linux too. This is
  what removes any "Android must be the central" constraint; the
  smaller-`node_addr` cross-probe tiebreaker then works normally on every
  pair. The Linux side is the fixed-`0x0085`-compat that is **intentionally
  dropped** in favor of discovery.

### Dial-target decision table

| Peer kind | Advert / read seen | PSM dialed |
| --- | --- | --- |
| Any peer (this scheme), PSM learned | UUID + PSM (service-data or GATT read) | learned PSM for that `BleAddr` |
| PSM not yet learned | UUID only so far | dial deferred to next scan tick |
| Legacy fixed-PSM peer (last resort) | UUID-only, advertises no PSM | `0x0085` (legacy default) |

### fips touch points

The seam already exists in fips-core; the patch only adds advertising-own-PSM
and reading-peer-PSM around it, on **all** backends:

- The advert **carries** the PSM (service-data field or readable GATT
  characteristic); discovery **captures** it into the `BleAddr → PSM` map.
- `BleIo::connect(addr, psm)` already takes a **per-call PSM**, so the learned
  value flows straight through — no trait change.
- The existing config override `psm() = self.psm.unwrap_or(DEFAULT_BLE_PSM)`
  stays as the fallback: an explicit configured PSM wins, otherwise the
  learned-or-legacy `DEFAULT_BLE_PSM` (`0x0085`) applies.

## Constants summary

| Name | Value | Source |
| --- | --- | --- |
| `DEFAULT_PSM` (legacy default / last resort) | `0x0085` (133) | [mod.rs#L49](../../reference/fips/src/transport/ble/mod.rs#L49) |
| Per-peer PSM carrier (proposed) | 16-bit LE in FIPS-UUID service-data, or a readable GATT characteristic | Myco net-new |
| FIPS service UUID | `9c90b790-2cc5-42c0-9f87-c9cc40648f4c` | [io.rs#L131](../../reference/fips/src/transport/ble/io.rs#L131) |
| Advert local name | `"fips"` | [io.rs#L397](../../reference/fips/src/transport/ble/io.rs#L397) |
| Advert interval | 400–600 ms | [io.rs#L398-L399](../../reference/fips/src/transport/ble/io.rs#L398-L399) |
| `PUBKEY_EXCHANGE_PREFIX` | `0x00` | [mod.rs#L666](../../reference/fips/src/transport/ble/mod.rs#L666) |
| `PUBKEY_EXCHANGE_SIZE` | 33 bytes | [mod.rs#L669](../../reference/fips/src/transport/ble/mod.rs#L669) |
| Pubkey-exchange timeout | 5 s | [mod.rs#L676](../../reference/fips/src/transport/ble/mod.rs#L676) |
| Default transport MTU | 2048 | `BleConfig` default |
| L2CAP mode | SeqPacket (no framing) | [mod.rs#L5](../../reference/fips/src/transport/ble/mod.rs#L5) |
| Min Android API | 29 | platform L2CAP requirement |

## See also

- [../design/ble-interop.md](../design/ble-interop.md) — strategy, option A,
  the PSM problem, foreground service, bitchat contrast.
- [../../reference/fips/docs/design/fips-transport-layer.md](../../reference/fips/docs/design/fips-transport-layer.md)
  — the transport-layer contract this backend satisfies.
- [diagrams/03-offline-propagation.svg](../design/diagrams/03-offline-propagation.svg)
  — where this transport sits in the offline story.
