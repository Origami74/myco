# fips local patches

`fips` is consumed from a **local checkout** at `reference/fips` (gitignored — see
[docs/how-to/build.md](../docs/how-to/build.md) §4). Since the checkout isn't
committed here, the local modifications are tracked one of two ways: older
baseline changes as **patch files** in this directory, and ongoing work as
**branches on the fips fork we maintain** (`github.com/jmcorgan/fips`). The
branch model is preferred for active work — it carries history and is what the
build checks out — so new changes go on a branch, not a new patch file.

## `fips-mobile-features.patch`

Makes `fips` cross-compile for mobile (Android now, iOS later). Upstream pulls
desktop-only code unconditionally — the **raw-socket Ethernet transport**
(Linux `AF_PACKET` / macOS BPF) and **system-TUN device management** — which has
no Android/iOS implementation.

The patch puts both behind **default-on Cargo features** so mobile builds opt out
with `default-features = false`:

- **`ethernet`** — gates the Ethernet transport (module, `TransportHandle` variant
  + arms, node construction, and `resolve_ethernet_addr`).
- **`system-tun`** — gates the real `tun.rs` platform ops (Linux/macOS). When off,
  a no-op `platform` stub stands in: the TUN is **app-owned** (the embedder, e.g.
  the Android `VpnService`, provides the fd), so FIPS does no system-TUN ops.
- one `dns.rs` libc fix (`ipi6_ifindex` is `i32` on Android, `u32` on macOS).

`myco-core` depends on `fips` with `default-features = false`, so both the host and
Android builds drop ethernet + system-tun.

### Apply

```sh
git -C reference/fips apply ../../patches/fips-mobile-features.patch
# or, from the repo root:
git -C reference/fips apply "$PWD/patches/fips-mobile-features.patch"
```

The intent is to land these as a feature PR upstream (`k0sti/fips` already has a
`feature/mobile-support` branch); until then they ride the local checkout.

## BLE v2 — the `ble-v2` branch (no patch file)

The "BLE v2" work — universal per-peer PSM discovery, plus compiling the BLE
transport on **macOS and Android**, not just Linux — lives as the **`ble-v2`
branch** on the fips repo (`github.com/jmcorgan/fips`), branched off
`patch-android`. We maintain that fork, so the branch (not a patch file) is the
source of truth: it carries history and is what the build checks out. Build
against it with:

```sh
git -C reference/fips fetch && git -C reference/fips checkout ble-v2
```

What's on the branch so far:

- **`ble/psm.rs`** — the platform-agnostic per-peer PSM core shared by every
  `BleIo` backend: a 16-bit little-endian service-data PSM codec and a
  short-lived `BleAddr → PSM` map. Every node advertises its OS-assigned L2CAP
  listener PSM and every dialer reads a peer's advertised PSM before
  `connect()`, replacing the fixed `DEFAULT_PSM = 0x0085` that only BlueZ could
  bind (Android and macOS listener PSMs are OS-assigned). See
  [docs/reference/ble-wire.md](../docs/reference/ble-wire.md).
- **`ble_available` cfg** (`build.rs`) — un-gates the BLE transport module from
  linux-only to linux/macos/android. The pool/discovery/PSM logic and the
  generic `BleTransport<I>` are platform-agnostic; only the concrete `BleIo`
  backend is platform-specific (`BluerIo` on linux-glibc, else the in-memory
  `MockBleIo` fallback). The Android `AndroidIo` and macOS `BluestIo` backends —
  and the per-backend wiring of the PSM map into advertise/scan/`connect` — land
  on this branch in later phases.

Intended for upstream contribution (per-peer PSM across all backends also
unblocks Linux↔Android/macOS interop).
