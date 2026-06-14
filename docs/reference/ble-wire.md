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
DEFAULT_PSM = 0x0085  (133 decimal)
```

In the L2CAP **dynamic** PSM range (0x0080–0x00FF)
([../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs#L49)).
The reference Linux backend both **listens** on and **dials** this fixed
value.

Android cannot bind a chosen PSM as a listener:
`listenUsingInsecureL2capChannel()` returns an **OS-assigned** PSM, almost
never 133. `createL2capChannel(psm)` *can* dial any PSM. Myco therefore
keeps `0x0085` as the dial target only for **Linux** peers, and substitutes a
**learned** PSM for Android peers (see
[Android addr→PSM advertisement scheme](#android-addrpsm-advertisement-scheme)
and the [PSM problem](../design/ble-interop.md#the-psm-problem)).

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

## Android addr→PSM advertisement scheme

> **Proposed (Myco net-new). Not in reference fips-core.** This is the
> Android-side fix for the [PSM problem](../design/ble-interop.md#the-psm-problem)
> scenarios 2 and 3; it sits **below** the `BleIo` trait and is invisible to
> FIPS above it.

Because an Android listener's PSM is OS-assigned and the standard FIPS advert
is UUID-only, an Android peripheral additionally advertises **its current
listener PSM** so other devices can dial it:

- **Carrier (proposed):** a 16-bit little-endian PSM in a **service-data**
  field keyed on the FIPS service UUID, alongside the UUID in the same advert.
  Legacy advertising PDUs cap at ~31 bytes, so a 128-bit UUID + 2-byte PSM is
  the tight, legacy-safe layout. Extended advertising (BLE 5.0+) is the
  fallback if a device can't fit it. *(Exact field encoding TBD pending
  hardware tests; alternative carrier = a readable GATT characterstic the
  dialer reads once before the L2CAP connect.)*
- **Consume side:** each scanner parses the PSM field and maintains a
  short-lived `BleAddr → PSM` map. When FIPS calls
  `BleIo::connect(addr, 0x0085)`, `AndroidBleIo` **substitutes the learned
  PSM** for `addr` and dials that instead. FIPS is unaware.
- **Lifetime:** the map is keyed on `BleAddr`, which **rotates** with Android
  MAC randomization, so it must be **re-learned each scan cycle**, not cached
  durably. A stale entry simply causes a dial failure and a re-probe on the
  next scan tick (the `scan_probe_loop` cooldown already retries).
- **Linux symmetry (scenario 3, deferred):** the same field, if taught to
  `BluerIo`, lets Linux advertise and consume per-peer PSMs too — removing the
  "Android must be the central" constraint. That is a **small fips-core
  patch** against the local checkout, out of scope for the two-Android v1.

### Dial-target decision table

| Peer kind | Advert seen | PSM dialed |
| --- | --- | --- |
| Linux (`BluerIo`) | UUID-only | `0x0085` (fixed) |
| Android (this scheme) | UUID + PSM service-data | learned PSM for that `BleAddr` |
| Android, PSM not yet learned | UUID only so far | dial deferred to next scan tick |

## Constants summary

| Name | Value | Source |
| --- | --- | --- |
| `DEFAULT_PSM` | `0x0085` (133) | [mod.rs#L49](../../reference/fips/src/transport/ble/mod.rs#L49) |
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
