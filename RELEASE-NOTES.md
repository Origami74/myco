# Myco v0.6.0

**Released**: 2026-08-19

v0.6.0 is the release where the previous one's instrumentation pays off. v0.5.0
could tell you *that* peers were failing; this one identifies why the fastest
lane kept collapsing and fixes it. Alongside that, Myco's data stops being locked
inside the app: you can point it at a Nostr relay you run and keep using it
exactly as before.

**This release changes the mesh wire format.** Two phones must both be on v0.6.0
to exchange anything — an older build and this one will not pass events or pair.
Everything on the device upgrades in place with no data loss.

## At a glance

- **Wi-Fi Aware links stop dying every minute.** The fast lane was being torn
  down by phone firmware on a 64-second cycle. It turned out to be Myco's own
  doing, not radio interference, and teardowns are now roughly one in seven
  minutes.
- **Your data can live on a relay you run.** Settings → Storage → Advanced takes
  a relay URL — [Citrine](https://github.com/greenart7c3/Citrine) on the same
  phone, or a relay on your own network — and Myco uses it as its store. A
  Blossom server for app files can be set the same way. Both are optional and off
  by default.
- **Pairing has its own door.** Devices you have not paired with can no longer
  reach the port that serves your apps and messages at all.
- **Nobody can add themselves to your Circle.** A pairing acceptance is only
  acted on if it answers an invitation you actually sent.
- **Apps open without waiting on a slow phone in the room.**

## The 64-second death

Wi-Fi Aware is the fast lane: two phones talking directly over Wi-Fi with no
router and no internet. It had a habit of coming up, carrying traffic for about a
minute, and being killed by the phone's own firmware — then repeating, forever.

Radio interference was the obvious suspect. Wi-Fi and Bluetooth share one chip
and one antenna on these phones, so a Bluetooth scan drowning out a Wi-Fi data
path is exactly the sort of thing that happens. Backing the Bluetooth scan right
off changed nothing at all: the teardowns kept coming on the same cycle.

The real cause was Myco arguing with itself. It kept re-establishing the same
peer alternately over Bluetooth and over Wi-Fi Aware, and the firmware answered
that churn by ending the data path. Myco now leaves a peer alone on Aware instead
of also dialling it over Bluetooth — and with *both* radios scanning harder than
before, teardowns dropped from one a minute to one in seven. That the fix works
while Bluetooth is busiest is what rules interference out for good.

Some churn remains in the first few minutes after launch, when Bluetooth
legitimately connects a peer before Aware is ready.

## Your data, your relay

Myco keeps everything in a small Nostr relay and blob store built into the app.
That is still the default and still what most people should use. But it meant
your apps and messages lived somewhere only Myco could reach.

The reason it had to be Myco's own relay was that Myco wrote its mesh
bookkeeping — how far a message should travel, which query it belongs to — *into*
the messages themselves. Any relay holding that data had to understand Myco. That
bookkeeping now travels alongside messages rather than inside them, so what gets
stored is ordinary Nostr, and an ordinary relay can hold it.

Under **Settings → Storage → Advanced** you can now point Myco at a relay you
run. It has been confirmed working with Citrine on the same phone. If the relay
becomes unreachable, Myco says so plainly — with a warning on the Settings screen
rather than apps that silently refuse to load.

Two things are worth knowing before you switch. Myco trusts a relay you choose to
check signatures, so only use one you control. And *Delete* only ever clears
what is on this device; a relay you run is not Myco's to empty, and it says so.

## Pairing became its own thing

Pairing is what creates your Circle, and your Circle is what grants access to
everything else. Until now it arrived on the same port that serves your apps —
which meant that port had to stay open to strangers, and every pairing request
was written into your event store as a side effect.

Pairing now has a service of its own, and it is the only thing an unpaired device
can reach. The ports that serve your content refuse anyone you have not paired
with before a connection is even established.

While separating it, one weakness became obvious and is fixed here: a pairing
*acceptance* used to be taken at face value, so a device could send one unasked
and land in your Circle. Myco now only acts on an acceptance that answers an
invitation you actually sent, and that was addressed to your device.

## Known issues

- **A phone in your pocket finds nobody.** Myco winds its radios down when it is
  not on screen, so two idle phones in a room will not discover each other until
  one is opened. This is the largest remaining gap between how the mesh behaves
  in a test and how it behaves in a day.
- **Wi-Fi Aware is shut off entirely by deep Doze** on Android 13 and later after
  a long idle period —
  [#30](https://github.com/Origami74/myco/issues/30), separate from the teardown
  fixed above.
- **Aware links still churn for the first few minutes** after launch.
- **A custom Blossom server has not been tested against a third-party
  implementation.** The relay side has been confirmed with Citrine; the blob side
  has only been run against Myco's own.
- **A relay shared with another Nostr app will not deliver its messages live.**
  They arrive when a screen is reopened rather than as they happen. Only affects
  a relay something else also writes to.
- Phones still do not always connect to every peer around them.
- The interface can lag while the mesh is syncing.
- Exit-node mode still covers proxy-aware apps only; other apps and QUIC/UDP
  traffic keep using the phone's normal connection.

## Getting it

- **Android**: install the APK from the
  [v0.6.0 release](https://github.com/Origami74/myco/releases/tag/v0.6.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.6.0 tag. See
  [CONTRIBUTING.md](https://github.com/Origami74/myco/blob/main/CONTRIBUTING.md)
  for build prerequisites.

**Update every phone together.** The mesh wire format changed, so a v0.6.0 phone
and an older one cannot exchange events or pair.

The full per-release change history lives in
[CHANGELOG.md](https://github.com/Origami74/myco/blob/main/CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone running builds on real phones — the Wi-Fi Aware teardown was
found and narrowed by watching two devices rather than by any test — and to
[@Origami74](https://github.com/Origami74) for maintaining the project.

<!--
This file is published verbatim as the GitHub Release body by
.github/workflows/release.yml — the leading `# Myco vX.Y.Z` heading and
`**Released**:` line are stripped, and the auto-generated "What's Changed"
section is appended below. Two consequences when writing the next one:
  1. Keep the version in the H1 matching the tag, or the workflow falls back
     to generated notes rather than publishing stale text.
  2. Use absolute links — relative paths 404 on a release page.
-->
