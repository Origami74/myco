# The Wi-Fi AP Lane (`!FIPS` access SSID)

FIPS routers can broadcast an open access SSID — `!FIPS` — that phones
join to reach the mesh through the router (see the fips repo's
`docs/how-to/set-up-open-access-ssid.md`). This lane connects Myco's
embedded node to the router's fips node over the ordinary UDP
transport whenever the phone is on such a network.

## Mechanism

The lane is Kotlin-side control plane only (`app.myco.ap.ApRadio`), a
sibling of the Wi-Fi Aware radio and reusing its core seam:

1. A passive `ConnectivityManager.NetworkCallback` on `TRANSPORT_WIFI`
   watches for infrastructure Wi-Fi. Permissionless.
2. While any Wi-Fi is up, an `NsdManager` browse runs for
   `_fips._udp` — the DNS-SD advert fips LAN discovery publishes. The
   TXT record carries the node's `npub`; the SRV carries the port the
   node's UDP transport is bound to (routers default to `2121`).
3. Each resolve pushes `(npub, "[addr]:port")` into the core's
   platform peer queue (`NativeCore.awarePeerFound`, transport
   `"udp"`) — identical to the Wi-Fi Aware path. The node dials from
   its own UDP socket and Noise IK authenticates; the pushed npub is
   only a routing hint, exactly as with mDNS adverts elsewhere in
   fips.
4. On service loss or Wi-Fi loss, `awarePeerLost` closes the pooled
   UDP session so a dead socket is never re-used.

There is no identity-less "dial an address" path in fips: Noise IK's
msg1 is encrypted to the responder's static key, so the router's npub
must arrive before the first packet. Over IP, the mDNS advert is that
delivery channel (the raw-Ethernet transport's beacons serve the same
role at L2, but an unprivileged Android app cannot receive raw
frames). **The router must therefore have fips LAN
discovery/rendezvous enabled** so it answers `_fips._udp` queries; the
access-SSID firewall zone already admits mDNS (UDP 5353).

## Why the browse is not gated on the literal `!FIPS` SSID

API 33+ redacts the SSID unless the app holds location permission, so
a name gate would disable the lane on modern devices. Browsing other
LANs is harmless (nothing answers) and useful: a desktop fips node
with LAN discovery enabled on a home network exercises the identical
path. The SSID is still read best-effort and shown on the Dev screen.

## Address selection

The access SSID intentionally provides no internet, so Android keeps
**cellular as the default network** — and Myco's own VpnService routes
`fd00::/8` into the mesh TUN. Both would swallow a naive dial. The
resolver output is therefore ranked:

1. **Link-local IPv6 with a numeric scope** (`[fe80::x%3]:2121`) — the
   scope pins the egress interface, bypassing both traps; same proven
   form the Wi-Fi Aware lane dials.
2. Global/ULA IPv6, **skipping `fd00::/8`** (TUN capture).
3. IPv4, rewritten as v4-mapped IPv6 (`[::ffff:a.b.c.d]:port`) — the
   node's UDP socket is a dual-stack `[::]` bind and fips's transport
   selection is family-strict, so a bare V4 address would match no
   socket. May still lose to cellular default-routing; last resort.

## Re-push cadence

NSD fires `onServiceFound` once per service appearance, so a dropped
UDP session would never get a fresh dial hint. While the browse is
live, every resolved node is re-pushed each 60 s; the core's
alternate-path gates make redundant pushes no-ops for healthy peers.

## Observability

The Dev screen's **WI-FI AP (!FIPS)** panel shows Wi-Fi/SSID state,
browse state, and each discovered node (npub, address, pushed). The
**WI-FI AWARE** panel lists live NDP links from the same change.
`adb logcat -s MycoApRadio` traces the lane.
