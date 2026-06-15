# BLE Interop Strategy

Myco's offline story rests on one transport: Bluetooth Low Energy. Two
phones in a pocket, no Wi-Fi, no cell — one browses the other's nsite. This
doc explains *how* the app will speak BLE, why it speaks L2CAP (not GATT),
and the one hard problem (the PSM problem) that the strategy has to solve.

The on-wire details — exact UUID, byte layouts, MTU rules — live in the
companion reference, [../reference/ble-wire.md](../reference/ble-wire.md).
This doc is the *why*; that doc is the *what-on-the-wire*.

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
`reference/fips` checkout so we can patch fips-core if the PSM problem forces
it (see [Linux dials Android](#scenario-3-linux-dials-android)).

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
the entire PSM problem.

## The PSM problem

FIPS's Linux backend binds the fixed default `DEFAULT_PSM = 0x0085` (133) as its
L2CAP listener and dials that same PSM
([../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs#L49)).
The whole mesh agrees on one number. Android can't play that game: a
`listenUsingInsecureL2capChannel()` listener is assigned *some* PSM by the
OS, and it generally won't be 133. So a Linux peer that blindly dials 133
will not reach an Android listener.

The adverts don't carry the PSM either. FIPS adverts are **UUID-only**
([../../reference/fips/src/transport/ble/discovery.rs](../../reference/fips/src/transport/ble/discovery.rs))
— deliberately, so no identity or routing material leaks before the Noise
handshake. A scanner learns "a FIPS peer is at this BLE address," nothing
more. There is currently no channel for "…and its listener PSM is N."

The consequences differ by who dials whom:

| Scenario | Listener PSM | Dialer | Works today? | Fix |
| --- | --- | --- | --- | --- |
| **1. Android → Linux** | Linux listens on fixed `0x0085` | Android `createL2capChannel(0x0085)` | **Yes, no change** | — |
| **2. Android ↔ Android** | each side's OS-assigned PSM | the other Android | **No** out of the box | Solved Android-side: an addr→PSM advert map (below) |
| **3. Linux → Android** | Android's OS-assigned PSM | Linux dials fixed `0x0085` | **No** | Make Android the central, or a small fips-core patch (below) |

This is also why **Android is the natural central/dialer** in the v1 demo:
the dialer side has all the freedom, the listener side has none.

### Scenario 1: Android dials Linux

The happy path, and the v1 target. Linux runs `BluerIo`, listens on `0x0085`,
advertises the FIPS UUID. Android scans, finds the UUID, calls
`createInsecureL2capChannel(0x0085)`. Connected. The pubkey exchange and
Noise IK run on top. No code changes to fips-core, no advert tricks. **This
is the path the two-device demo will exercise first** — except the v1 demo is
two *Androids*, which is scenario 2; scenario 1 is the Linux-interop target
that proves wire compatibility against the reference implementation.

### Scenario 2: Android ↔ Android (the v1 demo)

Both phones are Android. Each one's listener got an OS-assigned PSM; neither
knows the other's. Android must be both central *and* peripheral, but only
the central can choose the PSM to dial, and it doesn't know which one.

**Proposed fix, entirely Android-side, no fips-core change: an addr→PSM
advert map.** Alongside the UUID-only FIPS service advert, the Android
peripheral advertises its current listener PSM in a separate, app-private
field — a 16-bit service-data value under the FIPS UUID (or a second
characteristic; the exact carrier is settled in
[../reference/ble-wire.md](../reference/ble-wire.md#android-addrpsm-advertisement-scheme)).
Each Android scanner reads that field, builds a `BleAddr → PSM` map, and when
`BleIo::connect(addr, psm)` is called it **substitutes the learned PSM** for
the `0x0085` that FIPS passes in.

Crucially this stays *below* the trait. FIPS still calls
`connect(addr, 0x0085)`; `AndroidBleIo` quietly looks up the real PSM for
`addr` and dials that. From FIPS's perspective nothing changed. The map is
populated from adverts during scanning, so by the time the scan/probe loop
dials, the PSM is known. (Open question: the learn-then-dial race if a peer's
advert hasn't been seen yet — retry on the next scan tick, which the
`scan_probe_loop` cooldown already provides.)

### Scenario 3: Linux dials Android

The genuinely hard case, and the one we explicitly **defer past v1**. A Linux
peer running stock `BluerIo` dials `0x0085`. Android's listener is on some
other PSM. Linux can't learn Android's PSM because (a) the advert is
UUID-only and (b) `BluerIo` wouldn't parse an extra field even if present.

Two ways out, neither needed for the two-Android demo:

1. **Keep Android as the central.** If every cross-pair link is initiated by
   the Android side, Linux never needs to dial Android. The mesh's
   cross-probe tiebreaker complicates this — the tiebreaker wants the
   *smaller node_addr* to dial — so "Android always dials" can fight the
   tiebreaker. Workable only if we accept that an Android-vs-Linux pair
   always lets Android win the dial, which may need a per-transport tiebreaker
   override.
2. **A small fips-core patch: per-peer PSM in the advert.** Teach `BluerIo`
   to (a) advertise its own listener PSM in the same service-data field
   Android uses, and (b) consume a peer's advertised PSM and pass it to
   `connect()` instead of the hard-coded `0x0085`. This makes the addr→PSM
   scheme symmetric across Linux and Android and removes the "Android must be
   central" constraint. Because we build against the **local** `reference/fips`
   checkout, this patch is ours to make. It is the cleaner long-term fix; it's
   out of scope for v1 only because v1 is two Androids.

**Recommendation:** ship v1 on the Android-side addr→PSM map (scenario 2),
target scenario 1 for Linux-interop validation, and land the fips-core
per-peer-PSM patch (scenario 3, option 2) when Linux↔Android two-way
initiation is on the roadmap.

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

The one wrinkle: the addr→PSM map (scenario 2) is keyed on `BleAddr`, which
*does* rotate. The map must therefore be short-lived (re-learned each scan
cycle) rather than a durable cache — see
[../reference/ble-wire.md](../reference/ble-wire.md#android-addrpsm-advertisement-scheme).

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

- **Scenario 3 final call.** Android-always-central (with a tiebreaker
  override) vs. the fips-core per-peer-PSM patch — decide when Linux↔Android
  two-way initiation is scheduled. Leaning toward the patch.
- **Advert carrier for the PSM.** Service-data field vs. a readable GATT
  characteristic vs. extended advertising — settled in the wire doc, pending
  hardware testing of legacy-advert size limits.
- **addr→PSM map under MAC rotation.** Confirm re-learn-per-scan is fast
  enough that the first dial after discovery rarely misses.
- **Foreground-service FGS types and OEM battery exemptions.** Per-OEM
  behavior is **TBD** until tested.
- **Cross-probe tiebreaker vs. Android-as-central.** Whether the
  smaller-node_addr-dials rule needs a per-transport override on BLE.
- **Device/ROM matrix.** Which phones the v1 demo targets — **TBD**.
