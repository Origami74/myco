# Myco v0.6.1

**Released**: 2026-08-21

A maintenance release with one fix in it. v0.6.0 stopped Wi-Fi Aware links from
dying every minute; this one stops the fast lane from carrying only a single
phone. Everything else in v0.6.0 is unchanged.

**No wire-format change.** A v0.6.1 phone and a v0.6.0 phone pair and exchange
events exactly as before, and a phone that has not updated is still reachable.
Upgrade in place, one device at a time if you like.

## At a glance

- **Wi-Fi Aware reaches every phone in the room, not just one.** In a room of
  three, the fast lane carried one and quietly refused the rest. Each phone now
  gets a connection of its own, as many at once as its chipset supports.

## One phone at a time

Wi-Fi Aware is the fast lane — two phones talking directly over Wi-Fi, no router
and no internet. In a room with several Myco phones it carried exactly one, and
requests for the others came back refused in about three milliseconds while
Android reported seven of eight connections free.

The obvious reading is a hardware limit, and it was wrong. Asking the phones
directly: a Pixel 7 Pro supports eight simultaneous Aware connections and a
Galaxy A52s two. Neither was anywhere near its ceiling.

The limit was Myco's own. Android delivers each Aware connection as a separate
network, and a network socket can be attached to exactly one of them. Myco had a
single socket for the whole lane, so the moment a second phone connected, the
first stopped being reachable — its link still up, carrying nothing. Faced with
that, the app deliberately refused to open a second connection at all, which was
the right call for a design that could not have used one.

Now the lane opens a socket per phone, and asks the chipset how many it can hold
rather than assuming. On a Pixel that is eight; on an A52s, two. Each phone is
told which one to talk to as part of the introduction the two devices already
exchange, so older builds keep working unchanged.

Confirmed on three phones in a room, all connected over the fast lane at once.

## Known issues

- **A phone in your pocket finds nobody.** Myco winds its radios down when it is
  not on screen, so two idle phones in a room will not discover each other until
  one is opened. This is the largest remaining gap between how the mesh behaves
  in a test and how it behaves in a day.
- **Wi-Fi Aware is shut off entirely by deep Doze** on Android 13 and later after
  a long idle period —
  [#30](https://github.com/Origami74/myco/issues/30).
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
  [v0.6.1 release](https://github.com/Origami74/myco/releases/tag/v0.6.1),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.6.1 tag. See
  [CONTRIBUTING.md](https://github.com/Origami74/myco/blob/main/CONTRIBUTING.md)
  for build prerequisites.

**Phones do not have to be updated together this time.** The wire format is
unchanged from v0.6.0.

The full per-release change history lives in
[CHANGELOG.md](https://github.com/Origami74/myco/blob/main/CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone running builds on real phones — the one-peer limit was found
by reading a capability report off two devices and noticing it disagreed with
what the app believed — and to
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
