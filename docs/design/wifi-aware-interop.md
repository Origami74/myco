# Wi-Fi Aware Interop Strategy

> Status: DESIGN / proposal — not yet built. Open questions are marked
> **TBD / open**.

Myco's offline story shipped on one transport: Bluetooth Low Energy. BLE does
its job well — always on, cheap, in every phone — but it cannot move a
Library. Measured on real handsets, the L2CAP link tops out around **~22 KB/s**
(peripheral→central uplink; see [usb-transport.md](./usb-transport.md)), which
makes a 20 MB nsite a fifteen-minute transfer. **Wi-Fi Aware** (NAN, Neighbor
Awareness Networking) is the wireless answer: the same symmetric,
no-access-point, no-internet peering model as BLE — both sides publish, both
sides subscribe, no group owner, no consent dialog — at Wi-Fi throughput. Two
phones discover each other, form a pairwise **data path** (NDP), and each end
gets an IPv6 link-local address on a dedicated `aware_dataN` network
interface. (The Aware API is Android 8+, hardware-gated; the peer-IPv6
plumbing this design relies on — `WifiAwareNetworkSpecifier` /
`WifiAwareNetworkInfo` — is **API 29+**, which is this lane's effective
floor. Conveniently, 29 is already Myco's `minSdk`, set by L2CAP CoC.)

That NDP-ends-in-a-network-interface fact is the whole strategy. BLE needed a
native byte-pipe backend (`AndroidBleIo` in the docs; the shipped fips-core
struct is `AndroidIo`) because L2CAP terminates in app-owned socket objects
that only Kotlin can touch; a Wi-Fi Aware data path terminates in a **kernel
network interface**. So Aware is not a new radio backend below a trait — it is
**fips-core's existing TCP transport pointed at a link-local address**. Kotlin
does discovery and brings the interface up; from the first SYN onward, every
byte moves through the kernel, and FMP/FSP/Noise ride the TCP stream exactly
as they do on any LAN. The hard problem is not addressing — the PSM problem
has no analog here, because we bind our own listener port — but **policy**:
when to raise the Wi-Fi lane, and how a peer moves between BLE and Aware
without flapping.

The on-wire details — the service name, the exact service-info layout,
timeouts — will live in the implementation, as the BLE equivalents now do in
`BleRadio` and the `reference/fips` transport. This doc is the *why*.

BLE stays what it is: the always-on discovery and control plane, the
bottom-most "crappy link" of
[diagrams/03-offline-propagation.svg](./diagrams/03-offline-propagation.svg).
Aware is the **bulk lane** — raised beside an existing BLE peering when both
phones have the hardware, used for blob sync, released when idle. (Google's
Nearby Connections is precedent for exactly this split: BLE for control,
Wi-Fi radios for bulk — we borrow the architecture, not the stack.)

## Option A: the TCP transport over the Aware data path

FIPS already has a connection-oriented, reliable transport with a connection
pool, connect-on-demand, and per-connection receive loops:
[../../reference/fips/src/transport/tcp/mod.rs](../../reference/fips/src/transport/tcp/mod.rs).
It adds no framing of its own — the core's shared reframer recovers packet
boundaries from the 4-byte FMP common prefix
(`[ver+phase][flags][payload_len:2 LE]`,
[../../reference/fips/src/transport/tcp/stream.rs](../../reference/fips/src/transport/tcp/stream.rs))
— the same transparent-byte-pipe model the BLE radio was converged onto. An
Aware NDP is, to the kernel, just another IPv6 link — so the TCP transport
works over it **unchanged**.

**Option A**: Kotlin owns the Aware radio — attach, publish/subscribe, the
data-path request — and, when the NDP comes up, pushes *"peer `npub` is
reachable at `[fe80::…%ifindex]:port`"* into the core. fips-core dials with
the TCP transport it already has. No new transport type, no byte bridge, no
new framing. Two small fips-core patches (a discovery-injection seam and a
link-preference rule — both below) are ours to make, exactly as the PSM patch
was, plus one Myco-side posture change spelled out just after the
alternatives.

The alternatives were rejected:

- **A BLE-style `AwareIo` byte bridge.** Mirror the BLE backend: Kotlin opens
  sockets via the Android `Network` API and pumps bytes across JNI. It works,
  but it hand-copies every byte through the JNI boundary that the kernel
  would otherwise move for free, and it reimplements what TCP already gives
  us (streams, backpressure, teardown). The BLE bridge exists because BLE
  sockets are Java objects; Aware sockets are not.
- **A dedicated `AwareTransport` type in fips-core.** A new
  `TransportHandle` variant that wraps… tokio TCP internals. All duplication,
  no new capability. If per-lane stats or config ever justify it, it can be
  split out later; the wire is identical either way.
- **Google Nearby Connections.** It abstracts the same radios (BLE control,
  Wi-Fi bulk) but owns the whole stack — its own discovery, its own crypto,
  its own framing — none of it FIPS-wire-compatible, and it drags in Play
  Services. A pattern reference only, like bitchat was for BLE.
- **Wi-Fi Direct.** The roadmap's original name for this lane. Rejected as
  the primary — see [Why not Wi-Fi Direct](#why-not-wi-fi-direct) below.

So: one Kotlin radio class, two small fips-core patches, zero new transport
code on the data plane.

One prerequisite the word "reuse" hides: **Myco's Android node runs no TCP
transport today.** `build_node` configures only the BLE instance
(`myco-core/src/runtime.rs`), and fips-core's `TcpConfig.bind_addr` defaults
to `None` — outbound-only. The Aware lane therefore *introduces* a TCP
transport instance on the phone, with a bound listener — a deliberate posture
change, not a free reuse (its exposure is examined under
[Listener exposure](#listener-exposure) below). Lifecycle-wise it follows the
BLE pattern exactly: `set_wifi_aware_enabled` maps onto the same
rebuild-node-on-toggle flow as `set_ble_enabled`. And to keep `offline_only`
semantics honest, the transport is raised for the Aware lane's sake but is a
general-purpose TCP transport once up — v1 keeps it fed **only** by
platform-pushed peers (patch #1 below dials nothing on its own), so enabling
Aware does not quietly turn on internet peering.

## The division of labour

With BLE, Kotlin's job was "small and dumb on purpose": move bytes, surface
adverts. With Aware it is smaller and dumber still — **Kotlin never touches a
byte**. The whole Kotlin surface is control-plane:

| Step | Android API | Kotlin (`AwareRadio`) responsibility |
| --- | --- | --- |
| Attach | `WifiAwareManager.attach()` | Hold the session; rebuild everything on `ACTION_WIFI_AWARE_STATE_CHANGED`. |
| Announce | `publish(PublishConfig)` | Publish the Myco service name; service-specific info carries the TCP listener port (no identity). |
| Find | `subscribe(SubscribeConfig)` | `onServiceDiscovered` → a candidate peer; `onServiceLost` (API 31+) → candidate gone. |
| Identify | `sendMessage()` follow-ups | Exchange device pubkeys post-match (the Aware analog of BLE's in-band `[0x00][pubkey:32]`). |
| Link | `ConnectivityManager.requestNetwork` + `WifiAwareNetworkSpecifier` | Bring up the NDP; read the peer's IPv6 from `WifiAwareNetworkInfo` and the interface from `LinkProperties`. |
| Hand over | JNI push | `"npub P reachable at [fe80::x%ifindex]:port"` → core. Teardown: `"npub P lost"`. |

What fips-core does *above* that hand-over, and what Kotlin must therefore
**not** re-implement:

- **Dial, pool, reconnect.** The TCP transport's connect-on-demand and
  connection pool.
- **Identity & crypto.** The Noise IK link handshake authenticates the peer;
  the pubkey learned over `sendMessage` is only a *hint* (it routes the dial
  and pre-fills the tiebreaker) — nothing trusts it until Noise proves it.
- **Session & routing.** One Noise session per peer pubkey, roaming between
  transports, MMP link metrics — all unchanged.

The FFI surface follows the BLE naming: a `set_wifi_aware_enabled` master
switch beside `set_ble_enabled`, a `wifiAware` status block beside `ble`, and
a `wifiAwarePeers` list beside `blePeers`
([ffi-surface.md](../reference/ffi-surface.md)). But where BLE needed the
whole `ble*` byte-bridge extern family, Aware needs only the control pushes —
there is no `awareChannelNextSend`, because there are no channels to pump.
Config mirrors the `[ble]` precedent: a `[wifi_aware]` table whose only field
is `enabled` ([config.md](../reference/config.md)), with the listener port
living where it already belongs — the fips TCP transport's `bind_addr` —
surfaced as a Myco config knob (proposed default port, vetoable, settled at
implementation). Both reference docs gain their matching `wifi_aware` entries
when this lands.

## Discovery: the service announcement carries no identity

The BLE advert carries no identity — the FIPS service UUID plus the 2-byte
PSM service-data field that universal PSM discovery added (routing plumbing,
not identity) — deliberately, so nothing that names a person leaks before the
Noise handshake. The Aware announcement makes the same promise. The publish
carries:

1. **The Myco service name** (fixed constant — the Aware analog of the FIPS
   service UUID).
2. **The TCP listener port** in the service-specific info, as a small
   little-endian value — the analog of the PSM service-data field. A port
   names a socket, not a person.

A passive scanner learns "a Myco device is nearby, listening on port N" —
exactly the disclosure class of the BLE advert.

Identity comes later and costs interaction, as it does on BLE. After a
subscribe match, the two sides exchange device pubkeys over Aware
`sendMessage` follow-ups — directed, ~255-byte L2 messages the OS delivers
best-effort, once; `AwareRadio` retries the exchange itself. This is the same
disclosure as BLE's pre-handshake `[0x00][pubkey:32]` exchange: an *active*
prober that speaks the protocol can learn the pubkey; a passive one cannot.
The exchanged pubkey then serves two purposes:

- It becomes the `pubkey_hint` the core's discovery drain requires
  (`poll_transport_discovery` silently skips hintless peers —
  [../../reference/fips/src/node/lifecycle.rs](../../reference/fips/src/node/lifecycle.rs)).
- It lets *Kotlin* apply the **cross-probe tiebreaker before spending an
  NDP**: both phones discover each other, but data-path slots are scarce
  (chipset-limited; query `getAvailableAwareResources()`), so only the
  smaller-`node_addr` side initiates the network request, mirroring the rule
  the core applies to BLE probe sockets. The core's own tiebreaker still
  backstops the race.

## The port non-problem

The PSM problem — the defining fight of the BLE strategy — has no Wi-Fi Aware
analog, and it is worth being precise about why.

BLE's listener PSM is **assigned by the OS**; the app cannot choose it, so
every node must advertise its PSM and every dialer must learn it before
dialing — universal per-peer PSM discovery, a real fips-core patch. A TCP
listener is the opposite: **we pick the port** when the transport binds
(`bind_addr` in the TCP transport config). The port rides in the service
announcement as a courtesy (it makes the port a config knob rather than a
protocol constant), but there is nothing to *discover* — the listener is
wherever we put it, on every device, and a wildcard `[::]` bind means the
same listener serves LAN peers and every `aware_dataN` interface alike.
Google's own documented Aware socket pattern binds the same way
(`ServerSocket(0)` — the wildcard address, any interface; Google then ships
its OS-assigned ephemeral port via `setPort()`, the in-band carrier we reject
next — our listener instead binds the configured port).

Android does offer that in-band port carrier —
`WifiAwareNetworkSpecifier.Builder.setPort()`, delivered to the other side
via `WifiAwareNetworkInfo` — and we deliberately **don't use it**. It only
works on the responder side of a *secured* data path (the framework throws
otherwise), and securing the NDP means provisioning a PSK — a second
credential layer under Noise with nothing to authenticate. The framework
explicitly permits **open (unencrypted) data paths** when no security setter
is called, and open is what we want: FIPS authenticates with Noise IK, not
with WPA3, precisely as the BLE strategy chose *insecure* L2CAP over
Bluetooth bonding. Same trust model, new radio:
[security.md](./security.md).

### Listener exposure

The wildcard bind deserves its own paragraph, because it is a genuinely new
posture: today the phone has **zero** IP listeners reachable off-device (the
relay and Blossom are localhost-bound; BLE is the only transport). A `[::]`
TCP listener is reachable from every network the phone joins — the Aware
interfaces, but also home Wi-Fi, the hotel LAN — and the port is broadcast in
the Aware announcement. What holds the line is the same thing that holds it
on the open NDP: **an unauthenticated dialer gets nothing.** The Noise IK
responder handshake gates every inbound connection; failing it yields no
identity, no data, no relay access ([security.md](./security.md)). The
residual surface is honest but small: connection-slot and battery burn from
junk dials (bounded by the transport's `max_inbound_connections`, default
256 — worth lowering for the phone), and the port doubling as a "this device
runs Myco" beacon on any shared LAN — the same fingerprint the BLE advert
already broadcasts to anyone scanning. The alternative — binding per
`aware_dataN` interface as data paths come up and down — closes the LAN
exposure at the cost of chasing dynamic interfaces; whether it is worth that
chase is **TBD / open**.

## Dialing a link-local peer

The one genuinely new piece of plumbing is the address itself. The peer's
address is IPv6 **link-local** (`fe80::…`), which is only meaningful together
with a scope — *which* interface to send from. Two facts make this workable:

- Rust's std parses **numeric**-scope text: `"[fe80::x%3]:4869"` round-trips
  through `SocketAddr` parsing with `scope_id = 3`, so it survives
  fips-core's `resolve_socket_addr` fast path, the TCP pool's
  string-keyed map, and the accept loop, with the kernel receiving the scope
  on connect. Interface-*name* scopes (`%aware_data0`) parse nowhere —
  **Kotlin resolves the interface name from `LinkProperties` to its ifindex
  and pushes numeric-scope text only.**
- fips-core already does exactly this for LAN mDNS discovery: the discovery
  path builds a `SocketAddrV6` with an explicit `scope_id` and refuses
  scope-less link-locals
  ([../../reference/fips/src/discovery/lan/mod.rs](../../reference/fips/src/discovery/lan/mod.rs))
  — the pattern is proven in-tree, just never yet applied to TCP.

One honest caveat: Google's documented dial pattern goes through the Android
`Network` object (`network.getSocketFactory()`), and no official doc blesses
a plain socket to the scoped link-local address — though AOSP itself hands
apps the peer address already scoped to the local Aware interface, so the
plain dial should route correctly in principle. If real hardware disagrees,
the fallback is documented-API-clean: Kotlin dials via the socket factory,
detaches the file descriptor, and the core adopts it into the TCP pool — the
same adopt-a-live-socket move `UdpTransport::adopt_socket_async` already does
for Nostr traversal; TCP lacks only the small public entry point. Scoped
dial first, FD adoption as the fallback — **TBD / open** until tested on
hardware.

## Why address randomization is harmless

The BLE strategy had to argue that Android's MAC randomization costs nothing;
Aware needs the same argument in sharper form, because *everything* about an
Aware peer is ephemeral. The NAN management and data-interface MACs are
randomized and re-randomized by platform mandate; the `PeerHandle` from
discovery is only valid within the discovery session that produced it; and
the peer's link-local IPv6 is derived from the randomized data-interface MAC,
so the pushed `[fe80::x%ifindex]:port` address lives exactly as long as its
NDP.

None of this touches identity, for the same reason it never touched BLE:
**FIPS never identifies a peer by its address.** The pushed address is a
transient dialing handle; identity is the pubkey the Noise IK handshake
proves, and the FIPS peer (keyed on npub) survives any number of address
changes. The practical consequences land entirely in `AwareRadio` and the
injection seam: every Kotlin map keyed on a `PeerHandle` or an NDP address is
session-scoped and rebuilt on the attach cascade, and the peer-lost push is
load-bearing — it is what retracts a dead address before the core's TCP pool
tries to dial it (the Aware twin of BLE's rule that the `BleAddr → PSM` map
must be short-lived, never a durable cache).

## Injecting the discovered peer (fips-core patch #1)

fips-core has no generic "the platform found a peer" entry today. Discovery
is transport-owned buffers drained per tick (`poll_transport_discovery`), the
TCP transport's `discover()` is hardcoded empty, and the closest architectural
twin — the LAN mDNS drain, which delivers exactly our shape, `(npub, scoped
SocketAddr)` — is hardwired to UDP transports. The control-API `connect`
command could dial an arbitrary address at runtime, but as an ephemeral
one-shot with no reconnect policy.

The patch: **generalize the LAN-discovery seam** into a platform peer queue —
`(pubkey_hint, TransportAddr, transport type)` events, drained per tick, fed
to `initiate_connection` against the TCP transport, with the same budgets and
dedup the BLE drain gets. The Kotlin push lands in that queue over JNI;
peer-lost events retract. This is deliberately *not* a new discovery
mechanism — it is the existing one with the transport parameter unhardcoded.
It is also what keeps the new TCP transport passive: with `discover()` empty
and no peer config pointing at it, the queue is the *only* source of TCP
dials, which is how the `offline_only` promise survives the lane.

## Link policy: one peer, two radios (fips-core patch #2)

This is the actual hard problem of the strategy — the Aware counterpart of
the PSM problem, except it lives above the transport instead of below it.

What fips-core does today, precisely:

- A peer holds **one** `(transport_id, current_addr)` pair and **one** Noise
  session. Sessions are keyed per-transport, so a packet arriving on another
  transport cannot authenticate against the existing session — two
  simultaneous live links to the same pubkey are structurally impossible, and
  **switching transports costs a Noise handshake** (the existing
  alternate-path machinery: a new handshake folds into the same peer and
  moves it).
- Roaming is **last-authenticated-packet-wins** with zero hysteresis: every
  AEAD-verified inbound packet stamps its source as the peer's current
  address.

For BLE↔Aware coexistence this means the design must choose cutover points,
because nothing chooses them today. The proposed policy, parameters vetoable:

1. **Raise on discovery, cut over deliberately.** When the Aware link comes
   up for an already-BLE-connected peer, the core runs the alternate-path
   handshake toward the TCP address and the peer moves wholesale. BLE stays
   connected below (the L2CAP channel and adverts persist) but carries no
   FIPS traffic while Aware holds the session — it is the warm standby, not
   a parallel path.
2. **Fall back on loss, not on silence.** Kotlin's peer-lost push (NDP
   `onLost` / `onServiceLost`) triggers the handshake back to the BLE
   address immediately — faster and cleaner than waiting for MMP timeouts.
3. **Hysteresis by asymmetry.** Cut over BLE→Aware eagerly (the lane exists
   to be used) but Aware→BLE only on loss — never on transient packet
   ordering. Because the roaming rule is per-authenticated-packet and the
   sessions are per-transport, the flap window is confined to the rekey
   drain; the asymmetric policy keeps it one-way. Exact guard timers:
   **TBD / open**.

Whether the lane should also be raised *on demand* (core asks Kotlin for an
NDP when a sync is queued, rather than whenever hardware allows) is a power
question — see Open questions.

## Lifecycle, permissions, and the foreground service

Everything Aware is **revocable at any time**: the platform's own guidance on
an availability change is to discard every session and rebuild from
`attach()`. `AwareRadio` treats attach, discovery sessions, and NDPs as a
cascade of disposables (the availability broadcast on all API levels,
`onAwareSessionTerminated` on 33+), and the core sees only peer-found /
peer-lost — the same "the radio may vanish under you" contract the BLE radio
already honors. Wi-Fi must be enabled (Aware runs *beside* an AP association
on most chipsets, but coexistence with SoftAP/Direct/tethering is
device-dependent); pre-Android-13 devices additionally require the Location
toggle. The airplane-mode demo stays BLE's.

Manifest deltas, mirroring the BLE set:

- `NEARBY_WIFI_DEVICES` with `neverForLocation` (API 33+, runtime
  permission), plus `ACCESS_WIFI_STATE`, `CHANGE_WIFI_STATE`,
  `CHANGE_NETWORK_STATE`.
- `ACCESS_FINE_LOCATION`'s `maxSdkVersion` widens from 30 to **32** — BLE
  only needed it through 30, but Aware needs it on API 29–32.
- `<uses-feature android.hardware.wifi.aware required="false"/>` — the
  hardware is optional by design; `FEATURE_WIFI_AWARE` gates the whole radio
  at runtime.
- An `AwareService` foreground service beside `BleService`, FGS type
  `connectedDevice`, hosting the `AwareRadio` on the same
  inject-then-start pattern. (The official Aware docs are silent on
  background lifecycle; the foreground service is field practice, not
  documented mandate — same as BLE.)

Power is the reason the lane is a lane: an active NDP costs materially more
than BLE idle, so Aware should be **released when idle** (tear down the
network request after a sync quiesces; the discovery sessions are cheap and
stay up). Instant communication mode (API 33) can sharpen first-contact
latency but self-disables after 30 seconds by design — an optimization, not
a dependency.

## Hardware and platform variance

Flagged **TBD / open** until we test on real hardware — the Aware ecosystem
is patchier than BLE's:

- **`FEATURE_WIFI_AWARE` is genuinely optional.** Google documents the gate,
  not the market; in practice support skews to flagships (an observation,
  not a spec). The lane must degrade to nothing gracefully — Aware-less
  pairs simply stay on BLE.
- **Capacity is chipset-limited and queryable, not assumable.** Concurrent
  data paths, publish/subscribe session counts, service-info length — all
  runtime-discoverable (`Characteristics`, `getAvailableAwareResources()`
  on API 31+), none guaranteed beyond "≥ 1". Budget NDPs accordingly; the
  mesh degree over Aware is one or two bulk pairs, not seven.
- **Throughput is unmeasured.** Official docs promise only "higher than
  Bluetooth"; credible Aware-specific benchmarks don't exist in the
  literature. Anything from ~20 Mbps (old Wi-Fi Direct handsets) to
  hundreds (modern PHYs) is plausible — two to three orders of magnitude
  above the ~22 KB/s BLE baseline anywhere in that range. The
  `speedtest_peer` harness is the vehicle: measure, don't quote. (And as
  with USB, measured goodput is shaped above the link by the TUN MTU (1280)
  and the MSS clamp — expect the speedtest to reflect mesh-packet overheads,
  not raw NDP capacity; raising the TUN MTU for fast lanes is out of scope
  here.)
- **Open NDP behavior across chipsets.** The API permits unsecured data
  paths; whether every OEM stack honors them equally is untested. If some
  stack proves open-hostile, a fixed app-constant passphrase is the boring
  fallback (plumbing, not security — Noise remains the trust layer).

## The dev/test story (there is no BluestIo this time)

The BLE strategy leaned on a macOS `BluestIo` backend as the buildable
dev/test pair. Aware has no such escape hatch: macOS has no public NAN API
(AWDL stays private); Linux wpa_supplicant has shipped only NAN USD
discovery (v2.11, July 2024) — the synchronized cluster and the NDP data
path never landed, so there is no consumer-hardware data plane; and the
Android emulator does not virtualize Aware at all (its 2026 multi-device
networking stack added Wi-Fi Direct and NSD — Aware is conspicuously
absent). Exercising the real radio takes **two physical Aware-capable
phones**.

The strategy's shape is what softens this. Because the data plane is plain
TCP, everything above the Kotlin radio — the discovery-injection seam, the
scoped link-local dial, the cutover policy, sync throughput — is exercisable
with **no Aware hardware at all**: two devices on the same Wi-Fi LAN (or two
emulators on the virtual network) present the identical interface to
fips-core, down to the scoped-IPv6 dial. Only `AwareRadio` itself — attach,
publish/subscribe, `sendMessage`, the network request — needs the two
handsets. The radio layer is thin by design; the testable surface is
everything else.

Worth noting for the horizon: **iOS 26 shipped a public Wi-Fi Aware
framework** (NAN 4.0, iPhone 12+, entitlement-gated, mandatory user pairing
via DeviceDiscoveryUI). Android↔iOS NAN interop is immature today — Aware
Pairing support on the Android side is nearly nonexistent — but it makes
Aware the only offline transport with a plausible future iOS story, which
Wi-Fi Direct and L2CAP-as-we-use-it lack.

## Why not Wi-Fi Direct

Wi-Fi Direct is the obvious-looking choice — it is the technology the
roadmap's "Later" bullet originally named, it works on effectively every
Android phone back to API 14, and (since the 2026 emulator) it is even
testable between AVDs, which Aware is not. It also genuinely can connect
without a consent dialog on API 29+, if group credentials are shared over a
side channel first. We still reject it as the primary lane, for structural
reasons:

- **It elects a hub.** Every Direct group has a **Group Owner** — one phone
  becomes the access point, holds the DHCP server, and relays or terminates
  everything. That is a star, not a peering: the mesh's symmetric
  pairwise-link model (and the cross-probe tiebreaker that assumes either
  side can dial) maps onto Aware's per-pair NDPs directly, and onto a GO
  election awkwardly.
- **The group dies with its owner.** GO teardown drops every member at once
  — the exact single-point-of-failure shape Myco exists to avoid.
- **Dialog-free needs a side channel anyway.** The API-29 pre-authorized
  join requires shipping SSID+passphrase out-of-band — over BLE, in
  practice. If BLE must broker the credentials regardless, the broker might
  as well introduce a symmetric technology instead of a hub-shaped one.
- **Legacy plumbing.** Direct hands out IPv4 from a NAT-style subnet via the
  GO; Aware hands each pair clean link-local IPv6 — the same address family
  as the mesh's `fd00::/8` world, with no DHCP round-trip.

So Wi-Fi Direct's role mirrors bitchat's in the BLE strategy: **a fallback
candidate, never the design center** — worth revisiting only if Aware's
hardware coverage proves too thin in the field, and even then as a second
implementation of the same "Kotlin raises an IP link, TCP does the rest"
seam. The transport's job still ends at "FIPS bytes crossed the link";
which radio raised the link is a detail the bytes never learn.

## Open questions

- **Scoped dial vs. FD adoption.** Does a plain connect to
  `[fe80::x%ifindex]:port` route over the NDP on real OEM stacks, or do we
  need the socket-factory + adopt-fd fallback? First hardware question to
  answer. **TBD / open.**
- **Wildcard vs. per-interface bind.** Does the LAN-reachable `[::]`
  listener stay (Noise-gated, simple) or do we chase `aware_dataN`
  interfaces to close the exposure? **TBD / open.**
- **Cutover hysteresis parameters.** Guard timers for BLE→Aware and
  Aware→BLE handshakes; whether MMP link metrics should gate the eager
  direction. **TBD / open.**
- **Open NDP on real chipsets.** Does the unsecured data path work across
  the device matrix, or does some stack force the passphrase fallback?
  **TBD / open.**
- **Opportunistic vs. on-demand lane raising.** Raise the NDP whenever an
  Aware peer is discovered, or only when a sync wants bulk? Power vs.
  latency; likely opportunistic for v-next, demand-driven later.
  **TBD / open.**
- **NDP capacity budgeting.** How many concurrent bulk pairs to allow, and
  how to arbitrate when `getAvailableAwareResources()` says one.
  **TBD / open.**
- **Device matrix and measured throughput.** Which Aware-capable pairs to
  test, and what `speedtest_peer` actually reports over an NDP.
  **TBD / open.**
