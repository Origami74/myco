# Myco v0.4.0

**Released**: 2026-07-29

v0.4.0 is about **reaching the mesh by name**. A node's npub is now an address any
app on the phone can use: type `http://<npub>.fips/` into an ordinary browser and
it loads, over Bluetooth or Wi-Fi, with no internet involved. Building on that,
an experimental **exit-node mode** lets a phone with no connection of its own
browse the public web through a mesh node that egresses for it.

It also fixes the Wi-Fi AP lane, which could fail to connect on any phone that
also had mobile data — the node saw every handshake arrive while the phone saw
nothing come back.

It upgrades from v0.3.0 in place with no data loss; your identity (nsec), Circle,
and installed apps are preserved. No ports or wire formats changed, so a v0.4.0
phone and a v0.3.0 phone still sync over the mesh.

## At a glance

- **`<npub>.fips` works everywhere on the device** — any browser or app can open
  a mesh node by name, not just Myco's own gateway.
- **Exit-node mode (developer preview)** — browse the public internet through a
  mesh peer, addressed by its npub. For a phone with no internet at all.
- **Wi-Fi AP lane actually connects** — no longer defeated by having mobile data
  on at the same time, and no longer stuck dialling an unreachable address.

## What's new

### Mesh nodes have names now

Every node's npub doubles as a hostname: `<npub>.fips`. Myco's tunnel advertises
an in-mesh resolver, and answers those names by deriving the address from the
public key — pure computation, no lookup, no upstream server, works fully offline.
Because the answer comes from the tunnel rather than from Myco, **every app on the
phone** gets it. Open `http://<npub>.fips/` in your browser and you are talking to
that node over the mesh.

Names that are not `.fips` are passed to your normal resolvers untouched, so this
does not take over DNS on the device.

### Exit-node mode (developer preview)

A phone with no internet — only a Bluetooth or Wi-Fi link to the mesh — can now
browse the web, by routing through an HTTP proxy on a mesh node that does have a
connection. Set the exit under **Settings → Developer → Exit node**, addressed by
npub:

```
<npub>.fips:8080
```

The exit does the DNS and the egress, so the phone never needs to resolve a public
name or hold a route to one. It also need not be a direct peer — FIPS forwards
multi-hop to it. `.fips` addresses bypass the exit and stay on the mesh.

This covers **proxy-aware apps** — browsers. Other apps and QUIC/UDP traffic keep
using the phone's normal connection. The runbook, including how to run the exit
daemon, is in [docs/how-to/exit-node-demo.md](docs/how-to/exit-node-demo.md).

## Reliability: the Wi-Fi AP lane connects

Two separate faults, either of which was enough to stop it:

The mesh socket **was not bound to the Wi-Fi network**. A local-only access point
never passes the system's internet-validation check, so with mobile data also up
the OS steered the socket to the validated network and quietly discarded the AP's
replies. The symptom was maddening from either end: the node's counters showed
every handshake arriving, while the phone re-sent them forever. The socket is now
pinned to the network the peer is actually on.

Myco also **dialled the wrong address**. A fips node advertises one address per
interface, and only the interface facing you answers — nothing in the advert says
which that is. Myco took the first and could sit retrying an unreachable one
indefinitely. It now keeps every advertised address and works through them until
the peer connects, then stays on the one that worked.

## Known issues

- **Peering can stall after a Wi-Fi reconnect** until the node's previous peer
  entry expires, about a minute. Phones that rotate their Wi-Fi MAC on every
  connection — GrapheneOS does by default — hit this most, because the phone's
  mesh-facing address changes each time and the node keeps answering the old one.
  Turning the mesh off, waiting for the peer to disappear on the node, and turning
  it back on clears it. Tracked upstream as
  [fips#130](https://github.com/jmcorgan/fips/issues/130).
- **Exit mode is browsers-only.** `setHttpProxy` is the only system-wide proxy
  hook available to a VPN app, so non-proxy-aware apps and UDP/QUIC traffic are
  unaffected by it. Capturing everything needs a userspace network stack, which
  is the next step.
- On macOS, an exit node's proxy will not accept mesh connections until the
  binary is allowed through the Application Firewall — inbound TCP is dropped
  silently while ICMP still works, which makes it look like a routing problem.

## Getting it

- **Android**: install the v0.4.0 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.4.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.4.0 tag. See [CONTRIBUTING.md](CONTRIBUTING.md) for build prerequisites.

The full per-release change history lives in [CHANGELOG.md](CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed code, design, testing, or bug reports to this
release — and to [@Origami74](https://github.com/Origami74) for maintaining the
project.
