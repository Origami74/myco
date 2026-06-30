# BLE Interop Strategy

Myco's offline story rests on one transport: Bluetooth Low Energy. Two
phones in a pocket, no Wi-Fi, no cell — one browses the other's nsite. This
doc explains *how* the app will speak BLE, why it speaks L2CAP (not GATT),
and the one hard problem (the PSM problem) that the strategy has to solve.

The on-wire details — exact UUID, byte layouts, MTU rules — now live in the
implementation itself (the `BleRadio` Android backend and the `reference/fips`
transport). This doc is the *why*.

The offline propagation picture this transport feeds is sketched in
[diagrams/03-offline-propagation.svg](./diagrams/03-offline-propagation.svg):
BLE is the bottom-most hop, the "crappy link" that data crosses one device at
a time.

## Option A: native AndroidBleIo over fips-core's BleIo trait

FIPS already abstracts its BLE transport behind a single platform trait,
`BleIo`, with the BlueZ-backed `BluerIo` as the Linux implementation
([../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs)).
Everything above that trait — the connection pool, the scan/probe loop, the
cross-probe tiebreaker, the Noise IK link handshake, reconnect, MTU
accounting — is medium-agnostic Rust in
[../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs).
The trait is the *only* seam between that logic and the radio.

**Option A**: implement `BleIo` natively for Android. Kotlin owns the radio
(Android's BLE APIs are Java-only) and hands raw bytes across JNI to a thin
Rust `AndroidBleIo` that satisfies the same trait `BluerIo` satisfies. FIPS
keeps owning everything above the trait; we add a new backend below it,
exactly as Linux did.

The alternatives were rejected:

- **Reuse bitchat-android's transport.** Wire-incompatible — see
  [Why not bitchat](#why-not-bitchat-as-a-transport) below.
- **Port BlueZ semantics onto Android.** There is no BlueZ on Android; the
  platform BLE stack is a different API with different constraints (notably
  the PSM problem). A faithful port is more work than a native backend.
- **Bridge through a localhost socket to a separate process.** Adds IPC,
  a second process to keep alive, and buys nothing over a direct trait impl.

So: one new trait implementation, written against the **local**
`reference/fips` checkout so we can patch fips-core for universal per-peer PSM
discovery (see [The PSM problem](#the-psm-problem)).

### macOS BluestIo: the dev/test backend

We also stand up a **macOS** `BleIo` backend, `BluestIo`, built on the
[`bluest`](https://crates.io/crates/bluest) CoreBluetooth crate. Rather than
writing it from scratch, we **reuse the fips branch `macos-ble-rebased`**
(commit `0ae9e01`) behind a `ble-macos` cargo feature; it uses 2-byte
length-prefix L2CAP framing. macOS's listener PSM is OS-assigned by
`CBPeripheralManager.publishL2CAPChannel()`, so it slots straight into
universal PSM discovery alongside Android.

`BluestIo` is framed as the **dev/test backend**: a Mac can build and run the
core, so Android↔Mac becomes a buildable, debuggable test pair for the BLE
link without two physical Android handsets. (Like the app-owned TUN change,
it's a test-oriented patch in the four-patch local-fips set — see
[build.md §4c](./build.md).)

## The BleIo trait surface

FIPS owns everything above this line. `AndroidBleIo` must provide exactly
these eight methods plus three small associated types. Verbatim from
[../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs):

| Method | Signature (async unless noted) | Android responsibility |
| --- | --- | --- |
| `listen(psm)` | `→ Acceptor` | Open an L2CAP server channel. **See PSM problem.** |
| `connect(addr, psm)` | `→ Stream` | Dial a peer's L2CAP channel at `psm`. |
| `start_advertising()` | `→ ()` | Advertise the FIPS service UUID (UUID-only). |
| `stop_advertising()` | `→ ()` | Stop the advertiser. |
| `start_scanning()` | `→ Scanner` | Scan for the FIPS UUID; yield peer addresses. |
| `stop` (via dropping Scanner) | — | Stop the scan. |
| `local_addr()` | `→ BleAddr` (sync) | This adapter's BLE address. |
| `adapter_name()` | `→ &str` (sync) | Adapter label (e.g. `"hci0"`; Android: a fixed tag). |

The three associated types:

- **`Stream`** — one live L2CAP CoC connection. Methods: `send(bytes)`,
  `recv(buf) → n`, `send_mtu()`, `recv_mtu()`, `remote_addr()`. SeqPacket
  mode preserves message boundaries, so **one `send` is one FIPS packet** —
  no framing layer. A zero-length `recv` means the peer closed.
- **`Acceptor`** — yields inbound `Stream`s: `accept() → Stream`.
- **`Scanner`** — yields discovered peers: `next() → Option<BleAddr>`;
  `None` ends the scan.

What FIPS does *above* these methods, and what Android must therefore **not**
re-implement:

- **Connection pool & eviction.** Hardware caps concurrent BLE links
  (typically 4–10); FIPS enforces a max (default 7) with static-peer
  priority.
- **Scan → probe → promote.** The `scan_probe_loop` dials every scan hit,
  runs the pubkey exchange, and promotes the *same* socket into the pool — no
  second connect.
- **Cross-probe tiebreaker.** Both phones advertise and scan, so both may
  dial simultaneously. The smaller `node_addr`'s **outbound** connection
  wins; the loser drops its socket. This is decided after the pubkey
  exchange, in Rust.
- **Identity & crypto.** The pre-handshake `[0x00][pubkey:32]` exchange and
  the Noise IK link handshake are FIPS's, not Android's. Android moves
  opaque bytes.

Android's job is small and dumb on purpose: open/close L2CAP channels,
advertise a UUID, scan for a UUID, copy bytes. Everything that makes it
*FIPS* is above the trait.

## Android L2CAP: the platform APIs

Android exposes L2CAP Connection-oriented Channels from **API 29 (Android
10)**. This is the hard floor for `minSdk` — there is no pre-29 fallback,
because GATT (the only pre-29 option) is wire-incompatible with FIPS.

Two calls matter, both on `BluetoothDevice` / `BluetoothServerSocket`:

- **`listenUsingInsecureL2capChannel()`** — opens a server channel and
  returns a **dynamically assigned PSM**. The app does **not** choose the
  PSM; the OS hands one back. `"Insecure"` = no BLE pairing/bonding
  required, which is what we want: FIPS authenticates with Noise, not with
  Bluetooth bonding.
- **`createL2capChannel(psm)`** — dials a remote channel at a **caller-chosen
  PSM**. The central picks the PSM to dial.

`createInsecureL2capChannel(psm)` is the matching insecure dial. Use the
insecure variants throughout; Bluetooth-layer encryption/bonding adds a
pairing UX we don't want and duplicates Noise.

That asymmetry — **listener can't pick its PSM, dialer can pick any PSM** — is
the PSM problem, and it's shared with Apple's CoreBluetooth (see below); only
BlueZ escapes it.

## The PSM problem

**Dynamically assigned listener PSMs are the general case, not an Android
quirk.** Two of the three platforms we care about hand the listener its PSM
from the OS:

- **Android** — `listenUsingInsecureL2capChannel()` returns an
  OS-assigned PSM; the app cannot pick it.
- **Apple (macOS/iOS)** — `CBPeripheralManager.publishL2CAPChannel()` opens
  a channel and asynchronously delivers an OS-assigned `CBL2CAPPSM`; again the
  app cannot pick it.
- **Linux/BlueZ** — the outlier. It *can* bind a fixed PSM, which is why
  FIPS's Linux backend hard-codes `DEFAULT_PSM = 0x0085` (133) as its L2CAP
  listener and dials that same number
  ([../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs#L49)).

So the fixed-PSM assumption baked into FIPS is a *BlueZ* assumption. The
moment either end is Android or Apple, "everyone agrees on 133" breaks: that
listener is on *some* OS-assigned PSM, and a peer that blindly dials 133 will
not reach it.

Compounding it, the adverts don't carry the PSM. FIPS adverts are **UUID-only**
([../../reference/fips/src/transport/ble/discovery.rs](../../reference/fips/src/transport/ble/discovery.rs))
— deliberately, so no identity or routing material leaks before the Noise
handshake. A scanner learns "a FIPS peer is at this BLE address," nothing
more. There is currently no channel for "…and its listener PSM is N."

### The fix: universal PSM discovery

Rather than special-case Android against a fixed-PSM Linux, we make PSM
discovery **symmetric across all backends**. Every node:

1. **Advertises its own OS-assigned listener PSM** — alongside the UUID-only
   FIPS service advert, in a separate app-private carrier: a 16-bit
   service-data value under the FIPS UUID and/or a readable GATT
   characteristic (the exact carrier is settled in the `BleRadio`
   implementation).
2. **Reads a peer's advertised PSM before it dials** — the scanner builds a
   `BleAddr → PSM` map from adverts, and the dial uses the *learned* PSM, not
   the hard-coded `0x0085`.

Both sides listen and both sides may dial; whoever ends up dialing already
knows the other's PSM. No fixed well-known PSM is required, and the existing
smaller-`node_addr` cross-probe tiebreaker decides the dial direction
normally. `0x0085` survives only as a **legacy default** for the case where no
PSM has been learned yet.

This is a real fips-core change — advertising-own-PSM and reading-peer-PSM —
applied to **all backends** (Linux/Android/macOS), not an Android-only shim
below the trait. Because we build against the **local** `reference/fips`
checkout, the patch is ours to make.

### Fixed-0x0085 Linux wire compat is intentionally dropped

We do **not** preserve interop with a stock, unpatched `BluerIo` that only
listens/dials `0x0085`. A Linux peer must run the patched fips-core that
advertises and reads per-peer PSMs to join the mesh. Trying to keep a
fixed-PSM fallback path alive would force every Android/macOS listener to also
bind 0x0085 (which their OSes won't allow) or force Linux to be
dial-only — neither is worth the asymmetry. Universal discovery is the single
path.

The consequences by who dials whom, under universal discovery:

| Scenario | Listener PSM | Dialer reaches it by… | Works |
| --- | --- | --- | --- |
| **Android ↔ Android** | each side's OS-assigned PSM | reading the peer's advertised PSM | yes (patched) |
| **Android ↔ macOS** | each side's OS-assigned PSM | reading the peer's advertised PSM | yes (patched) — the buildable dev/test pair |
| **Android/macOS ↔ Linux** | Linux's PSM (patched advert) | reading the peer's advertised PSM | yes (patched Linux) |

### Android ↔ Android (the v1 demo)

Both phones are Android. Each one's listener got an OS-assigned PSM; neither
knows the other's out of the box. Universal PSM discovery resolves it: each
phone advertises its current listener PSM (the 16-bit service-data value /
GATT characteristic above), each scanner reads the peer's PSM into its
`BleAddr → PSM` map, and the dial uses the learned PSM. Both phones listen and
both may dial; the smaller-`node_addr` tiebreaker decides which dial survives.
The map is populated from adverts during scanning, so by the time the
`scan_probe_loop` dials, the PSM is known. (Open question: the learn-then-dial
race if a peer's advert hasn't been seen yet — retry on the next scan tick,
which the `scan_probe_loop` cooldown already provides.)

### Android ↔ macOS (the buildable dev/test pair)

macOS runs the reused `BluestIo` backend (below). Its listener PSM is
OS-assigned by `CBPeripheralManager.publishL2CAPChannel()`, exactly as
Android's is — so the *same* universal-discovery mechanism applies with no
special-casing. Each side advertises its PSM, each reads the other's, either
side may dial. Because macOS is a backend we can actually build and run on a
laptop, Android↔Mac is the practical de-risk pair for exercising the BLE link
without needing two physical Android handsets.

### Android/macOS ↔ Linux

A Linux peer running the **patched** `BluerIo` advertises its own listener PSM
(even though BlueZ *could* bind a fixed one, it advertises whatever it bound)
and reads peers' advertised PSMs the same way. There is no "Linux dials a
fixed 0x0085" path and no "Android must be the central" constraint: dial
direction is decided by the ordinary smaller-`node_addr` tiebreaker, the same
as any other pair. A stock, unpatched Linux node that only knows `0x0085` is
out of scope by design (see *fixed-0x0085 compat dropped*, above).

**Recommendation:** implement universal per-peer PSM discovery in fips-core
across all backends, ship the two-Android demo on it, and use Android↔macOS
(`BluestIo`) as the buildable dev/test pair while bringing up the link. Linux
interop comes for free once its `BluerIo` runs the same patched discovery.

## Why MAC randomization is harmless

Android randomizes its BLE address aggressively, and a peer's `BleAddr` can
change between scans. This does **not** break FIPS peering, because **FIPS
never identifies a peer by its MAC.** Identity is the 32-byte pubkey
exchanged over the connected L2CAP channel, *after* the address has already
served its only purpose (dialing the socket).

Concretely, in
[../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs)
the flow is: scan yields a `BleAddr` → dial it → `pubkey_exchange` returns the
peer's `XOnlyPublicKey` → *that* drives the pool, the tiebreaker, and the
Noise IK handshake. The `BleAddr` is a transient dialing handle, discarded
for identity purposes. If the same phone reappears under a new random MAC,
it's simply dialed again and re-identified by the same pubkey; the FIPS peer
(keyed on npub) is untouched — the same session-independence property the TCP
and Tor transports rely on. So Android privacy defaults cost us nothing here.

The one wrinkle: the `BleAddr → PSM` discovery map is keyed on `BleAddr`,
which *does* rotate. The map must therefore be short-lived (re-learned each
scan cycle) rather than a durable cache.

## Device and ROM variance

L2CAP CoC support is API 29+ on paper, but real-world behavior varies:

- **OS-assigned PSM range and stability** differ by stack; never assume a
  fixed value, always read it back from `listenUsingInsecureL2capChannel()`.
- **Advertising-data limits and service-data support** vary; the legacy
  advertising PDU is ~31 bytes, so a UUID + a 16-bit PSM is tight. Extended
  advertising (5.0+) relieves this but isn't uniformly reliable. The wire
  doc specifies the conservative legacy-safe layout.
- **Background scanning throttling.** Android throttles scans started from the
  background; this is the central reason for a foreground service (below).
- **MTU negotiation** floors and ceilings differ per chipset; FIPS already
  treats MTU as per-link (`send_mtu()`/`recv_mtu()` per `Stream`), so we
  report whatever the OS negotiated and never assume a constant.
- **OEM power management** kills background radios aggressively (the usual
  suspects). The foreground service plus an exemption request mitigates but
  cannot fully cure this.

These are flagged as **TBD / open** until we test on real hardware; the demo
hardware matrix is an open question.

## Foreground service requirement

BLE peering must keep running while the user reads an nsite, switches apps, or
pockets the phone. Android will throttle or kill background BLE — both
scanning and the L2CAP channels — without a **foreground service** holding a
`connectedDevice` (and likely `location`/`bluetooth`) FGS type with an
ongoing notification. The native `AndroidBleIo` radio loops live inside that
service; the Tokio runtime and FIPS event loop run alongside.

This mirrors the VpnService lifetime: both are long-lived foreground
components the user can see and stop. Exact FGS types, the runtime
`BLUETOOTH_SCAN`/`BLUETOOTH_CONNECT`/`BLUETOOTH_ADVERTISE` permissions
(API 31+), and battery-exemption UX are **TBD** and tracked separately.

## Why not bitchat as a transport

bitchat-android is the obvious-looking reuse target — it's an offline BLE
mesh — and we **do** mine it, but **not** for transport. The reason is a hard
wire incompatibility:

- **bitchat is GATT-only.** It advertises service UUID
  `F47B5E2D-4A9E-4C5A-9B3F-8E1D2C3A4B5C` and moves data through GATT
  characteristic writes/notifies
  ([../../reference/bitchat-android/app/src/main/java/com/bitchat/android/util/AppConstants.kt](../../reference/bitchat-android/app/src/main/java/com/bitchat/android/util/AppConstants.kt),
  [.../mesh/BluetoothMeshService.kt](../../reference/bitchat-android/app/src/main/java/com/bitchat/android/mesh/BluetoothMeshService.kt)).
- **FIPS is L2CAP-only.** It advertises UUID
  `9c90b790-2cc5-42c0-9f87-c9cc40648f4c` and moves data through L2CAP CoC
  SeqPacket sockets.

GATT characteristic writes and L2CAP channels are different transport
mechanisms at the HCI level. A bitchat node and a FIPS node would discover
each other's adverts and have no way to exchange a byte. Bridging them would
mean reimplementing FIPS's framing-free SeqPacket semantics on top of GATT's
~20-byte-MTU, fragmented, characteristic-write model — strictly worse than
the native L2CAP backend, and it would still require all of FIPS's pool/Noise
logic on top.

So bitchat is **a pattern reference for the propagation layer only** — never a
transport, and its crypto is not used. The patterns worth borrowing live
above the transport, in the nsite/relay store-and-re-serve layer:

- **Store-and-forward cache** (bitchat's `StoreForwardManager`, ~12 h
  retention) → the local relay/blossom retaining peers' signed events and
  sha256 blobs to re-serve later, offline.
- **Set-reconciliation sync** (bitchat's GCS `REQUEST_SYNC`, 16-byte packet IDs)
  → an efficient "what events do you have that I don't" exchange; Myco implements
  this as **negentropy / NIP-77** (events only — blobs stay pull-by-sha256). See
  [propagation.md §5](propagation.md).
- **TTL-bounded flood with probabilistic relay** → the manifest-flood
  propagation TTL (proposed default 5 hops; author-signed manifests flood via
  relay-mesh fanout, blobs stay pull-only).

These belong to the offline-propagation design
([diagrams/03-offline-propagation.svg](./diagrams/03-offline-propagation.svg)),
not to BLE transport. The transport's job ends at "FIPS bytes crossed the
link"; bitchat's lessons begin one layer up.

## Open questions

- **Advert carrier for the PSM.** Service-data field vs. a readable GATT
  characteristic vs. extended advertising — settled in the wire doc, pending
  hardware testing of legacy-advert size limits. (Both carriers may coexist:
  service-data for fast scan-time learning, a GATT characteristic as the
  authoritative read.)
- **`BleAddr → PSM` map under MAC rotation.** Confirm re-learn-per-scan is
  fast enough that the first dial after discovery rarely misses.
- **Foreground-service FGS types and OEM battery exemptions.** Per-OEM
  behavior is **TBD** until tested.
- **Device/ROM matrix.** Which phones the v1 demo targets — **TBD**.
