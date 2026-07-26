# Exit-node demo — a BLE-only phone loads google.com

Goal: a phone with **only Bluetooth** peering (no Wi-Fi/cell) browses the public
internet by tunnelling its web traffic through a mesh **exit node** that egresses
for it.

This is the *simple* first cut: an **HTTP proxy** on the exit + Android's VPN
`setHttpProxy`. It covers proxy-aware apps (browsers) — enough to load google.com.
A full-tunnel (capture *all* IP traffic via tun2socks) is a later, bigger step.

The exit is addressed by its **npub**: `<exit-npub>.fips`. Myco resolves `.fips`
names system-wide (see [System-wide `.fips` DNS](#system-wide-fips-dns)), so the
exit **does not have to peer the phone** — FIPS forwards multi-hop to it.

```
[BLE-only phone]                              [exit node: anywhere in mesh + internet]
  Chrome                                        tinyproxy :8080 (HTTP + CONNECT)
    │ http proxy = 127.0.0.1:<relay>               ▲
    ▼                                               │ mesh TCP to <exit-npub>.fips:8080
  loopback relay (MycoVpnService.ExitRelay)        │
    │ getByName(<exit-npub>.fips) → fd00:: ────────┤  (multi-hop FIPS forward)
    ▼                                               │
  VPN route fd00::/8 → FIPS → BLE → hop → … → exit's fips0 → tinyproxy → INTERNET
```

Chrome never resolves DNS or touches a public IP itself — it hands the hostname
to the proxy (`CONNECT google.com:443`), and the **exit** does DNS + egress. So a
phone with zero internet still works.

## System-wide `.fips` DNS

Naming the exit by npub relies on a feature Myco already ships: the VPN advertises
an in-mesh sentinel resolver, `fd00::53`, as the DNS server, and the native TUN
pump ([`dns_intercept`](../../myco-core/src/dns_intercept.rs)) answers
`<npub>.fips` AAAA queries by pure computation (npub → `fd00::` address — no
network, no upstream resolver). **Any app** on the phone can address a mesh node
by its npub, which is why the exit can be `<exit-npub>.fips` rather than a raw
`fd00::` literal. Non-`.fips` names return NXDOMAIN — in this demo the browser
reaches the internet through the proxy, which resolves public names on the far
side.

---

## 1. Exit node (any mesh node with internet)

The exit can be **anywhere in the mesh** — FIPS forwards multi-hop, so it need not
peer the phone directly. Simplest to *debug*: a box the phone reaches in one hop
(its BLE peer) that also has internet. Note the exit's **npub** — that is how the
phone addresses it.

### a. Run a FIPS node with a TUN + a transport reachable from the mesh

Use the `reference/fips` checkout. Minimal `fips.yaml` (TUN on so the mesh addr
is a real local interface a proxy can bind behind):

```yaml
node:
  identity:
    nsec: "<exit node nsec>"
  discovery:
    nostr:
      enabled: true
      policy: configured_only
      app: "fips-overlay-v1"

tun:
  enabled: true
  name: fips0
  mtu: 1280

transports:
  ble:
    enabled: true          # so a BLE-only phone can peer it directly
```

Start it, peer the phone (pair as usual), and confirm the session is up.

### b. Note the exit's npub

Its Nostr `npub` (the app shows the same under Settings → Developer as `npub` /
`.fips`). The phone addresses the exit as `<exit-npub>.fips`. A raw `fd00::EXIT`
literal also works if you prefer.

### c. Run the exit proxy bound so mesh peers reach it

Use [`fips-exitnode`](../../fips-exitnode/) — a standalone Rust proxy (HTTP
`CONNECT` + SOCKS5, no auth) built for exactly this. Runs on Linux/macOS:

```bash
cd fips-exitnode
cargo build --release
./target/release/fips-exitnode      # HTTP :8080 + SOCKS5 :1080 on [::]
```

Sanity check on the exit box itself (proxy is local there):

```bash
curl -x http://localhost:8080 https://ifconfig.me   # → the exit's public IP
```

> No-auth open proxy — **demo only**, firewall the ports to the mesh; don't leave
> it on a public box. (`tinyproxy` also works if you'd rather: `Listen ::`, `Port
> 8080`, `Allow ::/0`, `ConnectPort 443`.)

---

## 2. Phone (Myco)

1. Peer the phone to the exit over BLE, mesh **on** (the usual flow).
2. Settings → Developer mode on → **Developer settings → EXIT NODE**.
3. Enter the exit's address + port and tap **Apply** — by npub (preferred):

   ```
   <exit-npub>.fips:8080
   ```

   or a raw literal `[fd00::EXIT]:8080`. Myco re-establishes the VPN: it advertises
   the HTTP proxy + the `.fips` resolver to every app, and stands up the loopback
   relay to the mesh exit.
4. Open Chrome → **google.com** loads over BLE. 🎉
5. **Turn off** clears it (back to mesh-only routing).

---

## 3. What's happening / limits

- **Only proxy-aware apps** (browsers) follow `setHttpProxy`. Non-proxy apps and
  QUIC/UDP won't route — fine for the google.com criterion. Chrome falls back
  from QUIC to proxied TCP when UDP has nowhere to go.
- The relay's upstream socket to `fd00::EXIT` is **not** `protect()`ed on purpose:
  it must ride this same VPN (route `fd00::/8`) into FIPS. Mesh replies return to
  that socket, never back into the listener — no loop.
- Bandwidth is BLE-bound (~200 kbps up / ~500 kbps down). Pages load, slowly.
- Setting or clearing the exit re-establishes the VPN in place (the service
  compares the incoming config against the live one), so there is no need to
  toggle the mesh off and on.

## 4. Next step (real full-tunnel)

Capture `0.0.0.0/0 + ::/0`, terminate every flow in a userspace netstack
(smoltcp / hev-socks5-tunnel embedded in `myco-core`), relay SOCKS to the exit.
`readLoop` classifies `fd00::` → FIPS, everything else → tun2socks. That makes
*all* app traffic exit, not just proxy-aware apps.
