# Config Model

This is the **proposed** configuration model for Myco. It is adapted from
nostr-vpn's `AppConfig` ([`../../reference/nostr-vpn/crates/nostr-vpn-core/src/config/types.rs`](../../reference/nostr-vpn/crates/nostr-vpn-core/src/config/types.rs))
but **stripped to the Myco scope**: the private-network layer (signed
rosters, join-requests, admin membership, exit-node / WireGuard upstream,
`.nvpn` MagicDNS, LAN-multicast pairing) is removed per the locked decisions.

> Design doc, not-yet-built app. Every field below is **provisional** and may
> change. Defaults are "proposed default, vetoable". Open questions are marked
> **TBD / open**.

For the vocabulary used here (npub / node_addr / fd00::, `.fips` vs `.nsite`,
Library, nsite) see [concepts.md](../design/concepts.md). For identity and pairing
see [identity-pairing.md](../design/identity-pairing.md); for the manifest-flood
TTL and pull-on-demand model see [propagation.md](../design/propagation.md).

---

## Shape and storage

The config is a single TOML file in the app data dir (proposed
`config.toml`, alongside the relay/blossom stores and the LRU blob cache).
nostr-vpn loads/saves TOML through `AppConfig`
([`config.rs`](../../reference/nostr-vpn/crates/nostr-vpn-core/src/config.rs)); Myco keeps that load/save
machinery and the "fill defaults on load" behaviour, but with our own struct.

Following nostr-vpn, the **secret key is not stored in the plain TOML** in the
final design. nostr-vpn splits secrets out via `config_secrets` (hydrate on
load, redact on save). Myco will do the same: the `nsec` lives in Android
secure storage / the Keystore-backed keyfile and the TOML carries only the
`npub`. The sketch below shows `nsec` inline **only to document the field** —
mark this **TBD / open**: exact at-rest secret handling is a security decision
deferred to the identity milestone (see
[identity-pairing.md](../design/identity-pairing.md)).

Most fields are also exposed as a runtime **settings patch** over the FFI (see
[ffi-surface.md § Settings patch](./ffi-surface.md#settings-patch)); the TOML is
the persisted form of that same state.

---

## Proposed `config.toml` sketch

```toml
# Myco config — PROVISIONAL. All keys subject to change.

# ---------------------------------------------------------------------------
# [identity] — the DEVICE's one Nostr keypair. It is the FIPS mesh identity and
# the address of THIS device's relay+blossom (<npub>.fips). It is NEVER used to
# author nsites (sites are authored elsewhere by external keys). One per device
# in v1 (multi-persona later). node_addr = SHA256(npub)[0:16];
# fd00:: ULA = fd ‖ node_addr[0:15]. Both are DERIVED, never stored.
# ---------------------------------------------------------------------------
[identity]
# npub is persisted; nsec is held in secure storage in the real build (see note
# above). Generated on first launch if absent.
npub = "npub1…"            # bech32 public key (derived addresses computed from this)
# nsec = "nsec1…"          # TBD/open: secret-at-rest handling; NOT in plain TOML in final design
alias = ""                 # optional human label for this device's <alias>.fips name

# ---------------------------------------------------------------------------
# [node] — the embedded relay + blossom server (Rust: the myco-relay /
#          myco-blossom crates, wired in by myco-core).
# Defaults match the nsite-deck reference ports.
# ---------------------------------------------------------------------------
[node]
relay_port   = 4869        # ws://localhost:4869  (embedded Nostr relay)
blossom_port = 24242       # http://localhost:24242 (embedded Blossom server, ALWAYS embedded)
autostart    = true        # start relay+blossom+FIPS endpoint on app launch

# ---------------------------------------------------------------------------
# [relay] — the relay BACKEND is a pluggable seam. Default = embedded (day one).
# Optional = forward to a local relay app (e.g. Citrine) for devs who already
# run one. Blossom is NOT pluggable: it is ALWAYS embedded (no good Android
# Blossom app to forward to). Embedding is the default and earliest path.
# ---------------------------------------------------------------------------
[relay]
backend = "embedded"       # "embedded" | "local-forward"  (default embedded)
# forward_addr = "ws://127.0.0.1:7777"  # only used when backend = "local-forward"
                                        # (e.g. a local Citrine relay)

# ---------------------------------------------------------------------------
# [cache] — content store: signed events + the Blossom content-addressed blob
# store (sha256 blobs) we retain and re-serve. The blob store is the
# store-and-forward source and what makes us a new SOURCE for offline
# propagation; the cap below governs it. (The htdocs serving cache — a derived,
# path-named cache on top — is DEFERRED to the roadmap and has no config yet.)
# ---------------------------------------------------------------------------
[cache]
cap_bytes       = 2147483648   # 2 GB LRU cap over the Blossom blob store (proposed default)
eviction        = "lru"        # "lru" | "lfu" | "fifo"  (TBD/open: only lru in v1)
pin_library_items  = true         # sites added to the Library are pinned, exempt from eviction

# ---------------------------------------------------------------------------
# [[peers]] — paired peers (npub + memorable name). Equivalent of nostr-vpn's
# peer_aliases, minus all roster/admin/membership semantics. A peer here is
# someone we paired with (QR + BLE adverts); see identity-pairing.md.
# ---------------------------------------------------------------------------
[[peers]]
npub  = "npub1alice…"
name  = "green sammy"      # memorable name (colour + name); optional

[[peers]]
npub  = "npub1bob…"
# name omitted

# ---------------------------------------------------------------------------
# [ble] — offline transport. Android is the CENTRAL/dialer (it cannot bind the
# fixed default PSM 0x0085 as a listener). addr->PSM map is learned at runtime from
# adverts, not persisted here.
# ---------------------------------------------------------------------------
[ble]
enabled = true
role    = "central"        # FIXED for Android in v1 (listenUsingInsecureL2capChannel
                           # yields a dynamic PSM, so we cannot be the listener on the fixed default
                           # listener). "peripheral"/"both" are TBD/open, post-v1.

# ---------------------------------------------------------------------------
# [propagation] — HYBRID default: announce widely, pull content on demand. The
# unit that floods is the author-signed MANIFEST event (kind 15128/35128), small
# and self-authenticating, re-emitted UNMODIFIED by relays (signatures stay
# valid). Only those manifests flood; the large BLOBS stay pull-only.
# ---------------------------------------------------------------------------
[propagation]
fanout       = true        # relay-mesh fanout: re-broadcast accepted events to all
                           # other connected peers (source-excluded). Manifests gossip
                           # both ways once relays connect (roadmap P2). Events only;
                           # blobs stay pull-only. See nsite-layer.md §2.1.
announce_ttl = 5           # max hops for manifest flood/replication (blobs stay pull-only)

# ---------------------------------------------------------------------------
# [dns] — the DNS interceptor / TUN. Routes ONLY fd00::/8 (the mesh ULA) and
# DNS-intercepts the two TLDs system-wide. Does NOT capture 0.0.0.0/0.
# ---------------------------------------------------------------------------
[dns]
tun_enabled       = true        # Android VpnService/TUN owns fd00::/8 routing
intercept_fips    = true        # <npub>.fips / <alias>.fips -> fd00:: AAAA-only; non-.fips REFUSED
intercept_nsite   = true        # *.nsite -> 127.0.0.1 (A record) so any browser (incl. Chromium) works
fips_suffix       = "fips"      # TLD for mesh addressing
nsite_suffix      = "nsite"     # TLD for the localhost nsite gateway
```

---

## Field reference

All **provisional**.

### `[identity]`

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `npub` | string | generated on first launch | bech32 public key; `node_addr` and `fd00::` are derived, never stored. |
| `nsec` | string | generated with `npub` | **TBD/open** — held in secure storage, redacted from plain TOML in the final design (mirrors nostr-vpn `config_secrets`). |
| `alias` | string | `""` | optional label backing `<alias>.fips`. |

One identity per device in v1; multi-persona is a later milestone.

### `[node]`

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `relay_port` | u16 | `4869` | embedded Nostr relay, `ws://localhost:4869`. |
| `blossom_port` | u16 | `24242` | embedded Blossom server, `http://localhost:24242`. **Always embedded** — not pluggable. |
| `autostart` | bool | `true` | start the node (relay + blossom + FIPS endpoint) on launch. |

### `[relay]`

The relay **backend** is a pluggable seam: the default is the embedded relay
(day one), with an optional path to **forward to a local relay app** (e.g.
[Citrine](https://github.com/greenart7c3/Citrine)) for devs who already run one.
Embedding is the default and earliest path. **Blossom is not pluggable** — it is
**always embedded** (there is no good Android Blossom app to forward to).

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `backend` | enum | `"embedded"` | `"embedded"` (default) or `"local-forward"` (forward writes/reads to a local relay app like Citrine). |
| `forward_addr` | string | absent | only when `backend = "local-forward"` — the local relay's address (e.g. `ws://127.0.0.1:7777`). **TBD/open:** exact handshake/sync with the external relay. |

These services are exposed to mesh peers over FIPS FSP port-multiplexing at
`<npub>.fips:4869` and `<npub>.fips:24242` — no separate gateway on a reachable
path (see [concepts.md](../design/concepts.md) and upstream
[fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md)).
The WebView never resolves `.fips`; it loads `npub.nsite` via the localhost
gateway.

### `[cache]`

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `cap_bytes` | u64 | `2147483648` (2 GB) | LRU cap over retained events + the **Blossom content-addressed blob store** (the store-and-forward source). |
| `eviction` | enum | `"lru"` | **TBD/open** — only `lru` in v1; `lfu`/`fifo` reserved. |
| `pin_library_items` | bool | `true` | Library sites are pinned and exempt from eviction. |

Self-authenticating data (signed events, content-addressed blobs) means any
retained item is trustworthy regardless of who served it, so the cache can
become a new source for later, offline peers
(see [propagation.md](../design/propagation.md)).

The **htdocs** serving cache — a derived, path-named cache written on top of the
blob store for fast static serving (nsite-deck's `current/` optimization) — is
**deferred to the roadmap** and has **no config yet**. The cap above governs the
Blossom blob store, which is the store we retain; htdocs would be a regenerable
derivative.

### `[[peers]]`

A flat list replacing nostr-vpn's `peer_aliases` + roster participants. **No**
admin/membership/join-request semantics.

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `npub` | string | — (required) | the paired peer's identity. |
| `name` | string | absent | the peer's memorable name (colour + name). |

Pairing seeds an entry (QR carries npub + memorable name per the locked QR
payload `myco://pair/<base64>`; MAC/PSM arrive later over BLE adverts). See
[identity-pairing.md](../design/identity-pairing.md) and
[diagram 02](../design/diagrams/02-pairing-transitive-discovery.svg).

### `[ble]`

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | master switch for the L2CAP CoC transport. |
| `role` | enum | `"central"` | **fixed** to central on Android in v1; `peripheral`/`both` are **TBD/open**. |

Android requires API 29+ for L2CAP. The `addr -> PSM` map learned from adverts is
runtime-only and not persisted. FIPS identifies peers by the in-band pubkey
exchange, not by MAC, so MAC randomization is harmless. Backed by the native
`AndroidBleIo` implementing fips-core's `BleIo` trait
([`../../reference/fips/src/transport/ble/io.rs`](../../reference/fips/src/transport/ble/io.rs)).

### `[propagation]`

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `announce_ttl` | u8 | `5` | hop limit for flooding/replicating author-signed manifest events (kind 15128/35128), re-emitted unmodified. Blobs stay **pull-only** (HYBRID model). |

### `[dns]`

| Key | Type | Proposed default | Notes |
| --- | --- | --- | --- |
| `tun_enabled` | bool | `true` | Android VpnService/TUN routes **only** `fd00::/8` (never `0.0.0.0/0`). |
| `intercept_fips` | bool | `true` | `<npub>.fips`/`<alias>.fips` -> `fd00::` AAAA-only; non-`.fips` queries **REFUSED** so the system falls through to normal DNS. |
| `intercept_nsite` | bool | `true` | `*.nsite` -> `127.0.0.1` (A record); IPv4/localhost works in any browser including Chromium. |
| `fips_suffix` | string | `"fips"` | mesh-addressing TLD. |
| `nsite_suffix` | string | `"nsite"` | localhost nsite-gateway TLD. |

Behaviour mirrors the reference DNS interceptor
([`../../reference/fips/docs/design/fips-ipv6-adapter.md`](../../reference/fips/docs/design/fips-ipv6-adapter.md)).

---

## Explicitly NOT in Myco config

Stripped relative to nostr-vpn, per locked decisions:

- **`[[networks]]`** — signed rosters (kind 30388), participants, admins,
  inbound/outbound join requests, network mesh ids, invite secrets.
- **Exit-node / WireGuard upstream** — `exit_node`, `wireguard_exit`,
  `advertise_exit_node`, `exit_node_leak_protection`, `advertised_routes`. No
  tunnel-all-internet; the TUN never holds `0.0.0.0/0`.
- **`.nvpn` MagicDNS** — `magic_dns_suffix` and per-peer MagicDNS labels (we use
  `.fips`/`.nsite` instead).
- **LAN-multicast pairing** — invite broadcast / nearby discovery (replaced by
  QR + BLE adverts).
- **Public FIPS bootstrap/transit peers, NAT/STUN** — `fips_bootstrap_peers`,
  `nat` (`stun_servers`, discovery timeout). v1 is a two-device offline BLE
  demo; whether to keep an opt-in bootstrap list for the online path is
  **TBD/open**.

---

## Open questions

- **Secret-at-rest.** Exact `nsec` storage (Keystore-wrapped keyfile vs.
  encrypted blob) and whether the TOML keeps even a redacted placeholder. **TBD/open.**
- **Online relay set.** Whether Myco ships a default Nostr relay list for
  the online discovery path, or stays purely peer-to-peer in v1. **TBD/open.**
- **Eviction policy beyond LRU.** Whether `lfu`/`fifo` are ever needed, or
  whether age/size weighting suffices. **TBD/open.**
- **BLE role.** Whether post-v1 Android can act as peripheral (advertising a
  dynamic PSM peers learn out-of-band) to allow Android↔Android without a Linux
  central. **TBD/open.**
- **Per-peer trust / blocklist.** Whether `[[peers]]` needs a `blocked` flag or
  a separate denylist for propagation. **TBD/open.**
