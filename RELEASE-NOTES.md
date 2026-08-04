# Myco v0.4.2

**Released**: 2026-08-04

v0.4.2 is a small, cosmetic release: Myco now follows your phone's light or dark
theme, and its dark theme is true AMOLED black. Nothing about the mesh, the
protocol, or your data changed.

It upgrades from v0.4.1 in place with no data loss, and changes no ports or
wire formats.

## At a glance

- **Myco follows the Android system theme** — set your phone to dark and the app
  is dark, with no setting to find inside Myco.
- **Dark is pure black** (`#000000`) for backgrounds, surfaces, elevated
  containers, and the launch screen, so an OLED panel actually turns those
  pixels off.
- **Both themes stay legible.** Fixed light colours were replaced with Material
  3 semantic roles across every screen, so text and controls keep their contrast
  either way.
- **The colours that mean something still mean it** — emerald is still the brand
  accent, and pending and warning states keep their own distinct amber rather
  than collapsing into the theme.
- **System bars adapt**, so the status and navigation icons stay readable
  edge-to-edge in both themes.

## Scanning still works

The QR card stays white in dark mode on purpose. Scanners read a dark code on a
light field far more reliably than the inverse, and pairing is not the place to
lose a scan to aesthetics.

## Known issues

Nothing in this release addresses the following. They are the subject of the next
release, which is about mesh reliability.

- **Phones do not always connect to every peer around them.** Some devices
  connect to none, or to one or two out of many nearby. The suspected cause is a
  failure during handshake role selection; it is being investigated with
  instrumentation rather than guesses.
- **Peering can stall after a phone's Wi-Fi or Bluetooth address rotates**,
  until the other node's previous entry expires — about a minute. Phones that
  rotate their address per connection (GrapheneOS by default) hit this most.
  Tracked upstream as [fips#130](https://github.com/jmcorgan/fips/issues/130).
- **Opening one app from Discover can pin others** into your Library alongside
  it.
- **The interface can lag while the mesh is syncing.**
- Exit-node mode still covers proxy-aware apps only; other apps and QUIC/UDP
  traffic keep using the phone's normal connection.
- On macOS, an exit node's proxy needs allowing through the Application
  Firewall before it accepts mesh connections.

## Getting it

- **Android**: install the v0.4.2 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.4.2),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.4.2 tag. See [CONTRIBUTING.md](CONTRIBUTING.md) for build prerequisites.

The full per-release change history lives in [CHANGELOG.md](CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Dark mode was contributed by [@Datawav](https://github.com/Datawav) in
[#23](https://github.com/Origami74/myco/pull/23). Thanks also to everyone who
contributed testing and bug reports, and to
[@Origami74](https://github.com/Origami74) for maintaining the project.
