# Myco v0.5.0

**Released**: 2026-08-09

v0.5.0 is about two things: being able to see *why* phones fail to find each
other, and links that carry you to a place inside an app rather than just to the
app.

The peering work in this release is deliberately diagnostic rather than curative.
Myco could tell you a peer was unreachable but not why, which meant every fix was
a guess. The Dev tab now reports what the radios are actually doing and what each
connection attempt did, so the next release can fix causes instead of symptoms.
Three real bugs fell out of building that instrumentation and are fixed here.

It upgrades from v0.4.2 in place with no data loss, and changes no ports or wire
formats.

## At a glance

- **Deep links reach inside an app.** `myco://app/<host>/<path>` opens an app at
  a particular place in it. If you don't have that app, Myco fetches it from
  whoever nearby is carrying it and then opens it where the link pointed — five
  seconds later if a peer is in the room, or after a reboot next week if nobody
  was.
- **Apps can have their own routes.** A path an app doesn't list as a file now
  reaches the app's own router instead of a 404.
- **The Dev tab answers "is it me or is it them"** before you scroll: a radio
  self-check first, then peers you can expand in place to see why a connection
  failed — the BLE role chosen, how long discovery took, dropped sends, signal
  strength, and recent attempts with outcomes and timestamps.
- **That history survives a force-stop**, so a failure you saw yesterday is still
  there today.
- **Distant peers are reachable again.** Anyone who was not a direct neighbour
  had been unreachable since mesh names were introduced.
- **Opening the Discover tab no longer installs everything in it.**
- **Wi-Fi Aware is on out of the box** — a peering lane nobody switches on is a
  lane that silently never carries anyone.
- **A first-run intro**: a spark, filaments growing into the Myco mark, and a
  pupil you fall through into the app.

## Links that survive the wait

The interesting half of a deep link is what happens when the app isn't installed.
The link is kept — through the sync, through the app being swiped away, through
a reboot — and spent on that app's **first** open, whether Myco opens it for you
the moment it lands or you tap it in the Apps grid yourself a week later.

Deep links deliberately carry **no pairing secret and no sender identity**. They
travel through channels nobody controls — a chat message, a printed QR, someone's
screenshot — so anything inside one is public and replayable, which is the
opposite of what a pairing secret needs to be. Pairing keeps its own face-to-face
carrier. Losing the sender hint costs nothing: Myco already asks every peer in
your Circle in turn, so a link with nobody attached still finds the app through
whoever happens to have it.

Two things to know before you rely on them:

- Android does not make `myco://` links tappable in most chat apps — only
  `http`/`https` and a few others get that treatment. QR, an NFC tap, and Myco's
  own scanner are the channels that work today.
- If Myco itself isn't installed, a `myco://` link does nothing at all. Both need
  an `https://` companion link, which is not in this release.

Design notes are in
[docs/design/deep-links.md](https://github.com/Origami74/myco/blob/main/docs/design/deep-links.md).

## Honest instruments

A fact the app genuinely cannot observe now reads `unknown` rather than guessing
`off`, and a peer with nothing recorded says so rather than showing a fabricated
history. That distinction matters more than it sounds: the previous release's
peering bugs were hunted with inference, and inference is what produced the wrong
theories.

The attempt log is written one JSON record per line, so a truncated or damaged
file costs the damaged lines and not the whole history — and a file that mostly
fails to parse is copied aside before anything is rewritten, rather than being
quietly replaced with a shorter one.

## Known issues

Nothing in this release fixes the following. The next one is about mesh
reliability, and it is what the instrumentation above was built for.

- **Phones still do not always connect to every peer around them.** Some devices
  connect to none, or to one or two out of many nearby. The suspected cause is
  handshake role selection; it is now being investigated with device evidence
  rather than guesses.
- **Peering can stall after a phone's Wi-Fi address rotates**, until the other
  node's previous entry expires — about a minute. The equivalent Bluetooth case
  is fixed in this release; the Wi-Fi side is tracked upstream as
  [fips#130](https://github.com/jmcorgan/fips/issues/130).
- **The interface can lag while the mesh is syncing.**
- **The deep-link round trip is unverified across two devices.** Its parts are
  covered by tests, but the end-to-end path — follow a link on a phone without
  the app, wait for the mesh fetch, land on the right page — has not yet been
  run on hardware.
- Exit-node mode still covers proxy-aware apps only; other apps and QUIC/UDP
  traffic keep using the phone's normal connection.
- On macOS, an exit node's proxy needs allowing through the Application Firewall
  before it accepts mesh connections.

## Getting it

- **Android**: install the APK from the
  [v0.5.0 release](https://github.com/Origami74/myco/releases/tag/v0.5.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.5.0 tag. See
  [CONTRIBUTING.md](https://github.com/Origami74/myco/blob/main/CONTRIBUTING.md)
  for build prerequisites.

The full per-release change history lives in
[CHANGELOG.md](https://github.com/Origami74/myco/blob/main/CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed testing and bug reports — the Discover
install bug and the distant-peer failure were both found by users rather than by
tests — and to [@Origami74](https://github.com/Origami74) for maintaining the
project.

<!--
This file is published verbatim as the GitHub Release body by
.github/workflows/release.yml — the leading `# Myco vX.Y.Z` heading and
`**Released**:` line are stripped, and the auto-generated "What's Changed"
section is appended below. Two consequences when writing the next one:
  1. Keep the version in the H1 matching the tag, or the workflow falls back
     to generated notes rather than publishing stale text.
  2. Use absolute links — relative paths 404 on a release page.
-->
