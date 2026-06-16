# fips local patches

`fips` is consumed from a **local checkout** at `reference/fips` (gitignored — see
[docs/how-to/build.md](../docs/how-to/build.md) §4). These patch files capture the
local modifications so they're reproducible, since the checkout itself isn't
committed here.

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
