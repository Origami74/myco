# Myco v0.4.1

**Released**: 2026-07-29

v0.4.1 is a follow-up to v0.4.0 that makes the mesh **stay** connected and the
app honest about who it can reach. v0.4.0 gave nodes names; this release fixes
the things that stopped those names being useful in practice — Circle members
being treated as offline unless they were a direct neighbour, `.fips` names
resolving only sometimes, and peering over a Wi-Fi access point dropping every
couple of minutes.

It upgrades from v0.4.0 in place with no data loss, and changes no ports or
wire formats.

## At a glance

- **Circle members are reachable at any distance** — not only when they happen
  to be a direct neighbour.
- **`.fips` names resolve reliably**, and normal browsing keeps working.
- **Wi-Fi AP peering is stable** — under a second to connect, instead of a
  minute and a half followed by a drop every two minutes.
- **Turning Bluetooth on no longer takes the mesh down.**
- **The peer dot tells you how much mesh you have** — red and pulsing at none,
  amber at one, green at two or more.
- **Pairing says what is happening**: invites you are waiting on are visible
  and cancellable, and requests to join appear on the Circle screen itself.

## Reaching people

Myco used to decide for itself who was reachable by intersecting your Circle
with the mesh node's *directly connected* peers. Anyone further away was
treated as offline: their nsites never showed up under "around you", and pulls
skipped them. Chat was the exception — it already targeted the whole Circle,
on the grounds that a peer you have walked away from is still reachable over
the mesh. Everything else now agrees with it.

Peers are also addressed by name (`<npub>.fips`) everywhere instead of by their
raw mesh address. Resolving the name is what tells the node who it is talking
to, so dialling the address directly only ever worked for someone the node had
already met — which is exactly why this looked like a distance problem.

The reachable count in the status pill now means "we hold a live connection to
them", at any hop count.

## Names

v0.4.0 advertised Myco's resolver alongside your network's real ones so that
non-mesh names would still resolve. That cannot work: any ordinary resolver
denies a `.fips` name outright, so whether a mesh name resolved came down to
which resolver the system happened to pick. Myco's resolver now answers every
lookup and passes non-mesh names upstream itself.

## Staying connected

Peering over a Wi-Fi access point re-formed every couple of minutes. Myco was
cycling through a node's advertised addresses faster than a failed attempt
takes to expire, so several attempts were live at once — and whichever finished
last replaced the connection that had already succeeded. It was also trying
them in the wrong order: a node advertises one address per network interface,
and the one on the network you actually joined is the only one certain to reach
it. That one was tried last. Connecting now takes **under a second**.

Turning Bluetooth on used to rebuild the whole mesh node to pick up the radio,
dropping every peer and connection with it — so enabling one transport
interrupted the others. The node's lifecycle is now exactly the mesh switch.

## Discover and pairing

Discover showed the same app once per Circle member hosting it; it now shows
each app once, and leaves out anything you have already pinned or that is
already offered under Suggested.

Pairing gained the state it was missing. An invite that could not be delivered
— bumping two phones that have not met on the mesh yet — used to vanish
silently, so the natural response was to bump again and queue another request.
Invites are now remembered, shown under **Invited** on the Circle screen, and
can be cancelled. Sharing an app with someone already in your Circle no longer
invites them again, and requests waiting on *you* appear on the Circle screen
rather than behind a banner.

## Known issues

- **Peering can stall after a phone's Wi-Fi or Bluetooth address rotates**,
  until the other node's previous entry expires — about a minute. Phones that
  rotate their address per connection (GrapheneOS by default) hit this most.
  Tracked upstream as [fips#130](https://github.com/jmcorgan/fips/issues/130).
- Exit-node mode still covers proxy-aware apps only; other apps and QUIC/UDP
  traffic keep using the phone's normal connection.
- On macOS, an exit node's proxy needs allowing through the Application
  Firewall before it accepts mesh connections.

## Getting it

- **Android**: install the v0.4.1 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.4.1),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.4.1 tag. See [CONTRIBUTING.md](CONTRIBUTING.md) for build prerequisites.

The full per-release change history lives in [CHANGELOG.md](CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed code, design, testing, or bug reports to this
release — and to [@Origami74](https://github.com/Origami74) for maintaining the
project.
