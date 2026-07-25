# Myco v0.3.0

**Released**: 2026-07-25

v0.3.0 is about **staying up and staying honest**: the mesh survives the crashes,
stalls, and silent failures that used to bite in the field, and the app now tells
you plainly when a transport is enabled but can't actually run. It also brings the
mesh on/off control and live reachability into the top status pill, and adds an
experimental **Wi-Fi AP lane** for connecting to LAN fips nodes over Wi-Fi.

It upgrades from v0.2.0 in place with no data loss; your identity (nsec), Circle,
and installed apps are preserved. Unlike v0.2.0, this release changes no ports or
wire formats — a v0.3.0 phone and a v0.2.0 phone still sync over the mesh — but
update every device in a Circle to get the reliability fixes on both ends.

## At a glance

- **No more GrapheneOS / secondary-user crash** — a Wi-Fi Aware permission refusal
  no longer kills the whole app; the lane shuts down gracefully and warns instead.
- **Transport warnings** — Settings shows a red dot and a tappable warning when
  mesh, Bluetooth, or Wi-Fi Aware is switched on but can't run (VPN slot taken,
  Bluetooth off, Wi-Fi off).
- **Mesh control in the status pill** — a mesh on/off slider, a live
  `reachable/total` Circle count, and the current peer count, all top-right.
- **Battery drain cut** — background BLE duty-cycles down, GATT priority relaxes
  when idle, and the once-a-second poll stops when the app isn't visible.
- **Relay links self-heal after a stuck rekey** — a wedged mesh session no longer
  kills a Circle link permanently; dials time out and back off, then rebuild.
- **Wi-Fi AP lane (developer preview)** — auto-discover and connect to fips nodes
  on a joined Wi-Fi network over mDNS + UDP. Off unless the network carries a node.

## What's new

### The app stops lying about transports

When a transport was enabled but physically couldn't run, Myco used to look fine
while quietly doing nothing. v0.3.0 surfaces it: a **red dot on the Settings tab**
and a tappable warning row for each broken transport — mesh enabled without the
VPN slot (another VPN app took it), Bluetooth transport on while the phone's
Bluetooth is off, or Wi-Fi Aware on while Wi-Fi is off. Each warning jumps to the
fix. Declining the VPN consent dialog now turns the mesh preference **off** instead
of pretending the mesh is up.

### Mesh control in the status pill

The top-right status pill grows a **mesh on/off slider** and now shows, at a
glance, how many Circle members are reachable right now (`reachable/total`)
alongside the live peer count — so you can see and toggle mesh state without
diving into Settings.

### Wi-Fi AP lane (developer preview)

When the phone joins a Wi-Fi network that carries a FIPS node — such as a router
broadcasting the open `!FIPS` access SSID — Myco discovers the node via its mDNS
advert (`_fips._udp`) and connects to it over UDP automatically. It requires LAN
discovery/rendezvous enabled on the router's fips node. The Developer screen gains
a **Wi-Fi AP** panel (Wi-Fi/SSID state, mDNS browse state, discovered nodes), and
the Wi-Fi Aware panel now lists live data paths. See
[docs/design/ap-lane.md](docs/design/ap-lane.md). Developer preview — treat it as
experimental.

## Reliability: the mesh holds under stress

- **No crash on GrapheneOS / secondary users.** The system can refuse Wi-Fi Aware
  calls for lack of the nearby-devices permission *even after* the app's own check
  passed; the resulting `SecurityException` on the Aware callback thread used to
  kill the whole app. The lane now shuts down gracefully and surfaces a warning.
- **Mesh starts even right after granting VPN access.** Enabling the mesh
  immediately after granting VPN (e.g. reclaiming the slot from another app) no
  longer silently fails when the mesh address isn't ready yet — the VPN start
  retries until the node has published its address.
- **Relay links self-heal after a stuck rekey.** A mesh session wedged mid-rekey
  used to kill a Circle relay link permanently. Peer relay dials now time out at
  10s and back off per peer (8s up to 3min) after consecutive failures, reclaim the
  stale session, and rebuild a fresh one on the next dial.
- **Bluetooth toggle no longer knocks out Wi-Fi Aware.** Turning the Bluetooth
  toggle off used to stop the embedded mesh node out from under the Aware lane. The
  node's lifecycle now follows the mesh **Enable** switch; radio toggles only gate
  their own radios.

## Battery

Background drain is cut substantially: BLE discovery duty-cycles down (low-power
scan with batched delivery) while the app isn't visible, the per-link GATT
connection priority drops to balanced after 30s without bulk traffic, and the
once-a-second state poll no longer runs backgrounded (and no longer walks the blob
cache directory on every read).

## Notable bug fixes

- **App crash on GrapheneOS / non-admin users** — Wi-Fi Aware `SecurityException`;
  see Reliability above.
- **Mesh silently down after VPN grant** — start now retries until the address is
  ready; declining consent turns the preference off.
- **Permanent relay stall after a stuck rekey** — dials now time out and back off.
- **Developer panel reshuffle** — peer/advert rows keep a stable alphabetical
  order instead of reordering on every refresh.

## Getting it

- **Android**: install the v0.3.0 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.3.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.3.0 tag. See [CONTRIBUTING.md](CONTRIBUTING.md) for build prerequisites.

The full per-release change history lives in [CHANGELOG.md](CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed code, design, testing, or bug reports to this
release — and to [@Origami74](https://github.com/Origami74) for maintaining the
project.
