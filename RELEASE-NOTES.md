# Myco v0.8.1

**Released**: 2026-09-27

v0.8.1 makes apps **fast** and **complete** when the public relays aren't cooperating.

- Apps show what your phone already has straight away, instead of waiting for the slowest relay.
- Apps published on their author's own relays are found.
- Broken relays are left alone for a while instead of slowing every lookup.
- Open apps keep receiving new posts while you look at them.

**No wire-format change from v0.8.0.** v0.8.1 and v0.8.0 phones pair and exchange apps and messages as before. Everything upgrades in place.

## At a glance

- **A smooth first run.** The AppStore that comes with Myco keeps working after it updates itself: preinstalled apps start with the permissions they're known to need, so a same-author update that asks for no more doesn't stop to ask. When an app does ask for something new, it asks over the app itself, not on Myco's main screen, and its Install button is no longer refused because of a question waiting elsewhere.
- **Instant reopen.** An app that reads Nostr data (the AppStore, for one) shows what your phone already holds at once, and waits only a moment for relays that answer quickly, so a newer profile still wins. Relays that answer later aren't wasted: what they send is saved for next time.
- **Apps are found where their author publishes.** Myco now looks on the relays an app's author lists as theirs, and on the index relays where people's relay lists live. "Could not find app" for napplets like Minesweeper is fixed.
- **Broken relays don't slow you down.** A relay that refuses Myco, is down, or has a bad certificate is skipped for a minute at first, up to half an hour if it keeps failing. Losing signal, switching networks or a Wi-Fi login page never counts against a relay.
- **One bad relay can't cut you off.** A single relay answering with an error used to make Myco believe the internet was down and stop using every public relay for half a minute. Now any answer from the internet counts as the internet working.
- **Live views stay live.** Relays stay connected while an app is watching (a feed, a chat, someone's app list), so new events arrive as they're published.
- **Fewer connections.** A view with dozens of people now connects to a handful of relays that reach each person twice, instead of every relay any of them listed (up to forty).

## On your phone

- **Profiles and app listings you've seen are kept.** The next look is instant and works offline, and people in your Circle can get them from your phone. Nothing is installed and no app files are downloaded. "Delete cache" in Storage clears these copies. With a custom relay set, nothing is kept.
- **Installed apps keep the version you have files for.** A newer listing that reaches your phone doesn't replace the version an installed app runs, or the one it shares with your Circle, until its files are here. "Delete cache" no longer breaks an app that has a newer version waiting.

## Accounts

- **A new guest gets better default relays**: relay.damus.io, relay.ditto.pub and relay.primal.net for posting and receiving, plus a direct-message relay list so other Nostr apps know where to send it private messages. Its profile and lists also go to four index relays, where other apps look people up.
- **Guests made before this version keep their relay list.** Log out and start a new guest to get the new defaults. Accounts you log in to with an `nsec` or a signer app keep their own lists; Myco never creates or changes them.

## Known issues

- **Profile pictures and app sizes in AppStore.** Napplets can only load files by their hash (`blossom:`); plain web images, including anything on nostr.build, show a placeholder, and an app page may show "Unknown" for size. `https:` image support is next. [#67](https://github.com/Origami74/myco/issues/67) covers finding Blossom servers properly.
- **Busy apps still read some relays once instead of live.** An app with many open views can reach Myco's limit on live relay connections; past it, relays are read once rather than kept open. A shared connection per relay (roadmap N8) removes the limit.
- **Paid relays.** Relays that require a paid account (nostr.wine, for one) refuse Myco's reads, so apps published only there can't be found.
- **Open nsite windows and updates.** A napplet offers a restart when it's updated while open. An nsite doesn't yet, and an update can mix old and new files in an open nsite window until it reloads: [#71](https://github.com/Origami74/myco/issues/71).
- **No "recently updated" mark on the Apps screen yet:** [#69](https://github.com/Origami74/myco/issues/69).
- **A phone in your pocket finds nobody.** Myco winds its radios down when it isn't on screen.
- **Wi-Fi Aware is shut off by deep Doze** on Android 13 and later after a long idle period: [#30](https://github.com/Origami74/myco/issues/30).
- **A napplet's relay access is all-or-nothing,** and with the outbox grant it may name its own relays. Review the install sheet.

## Getting it

- **Android:** install the APK from the [v0.8.1 release](https://github.com/Origami74/myco/releases/tag/v0.8.1), or via [Zapstore](https://zapstore.dev/apps/app.myco).
- **From source:** run `cd android && ./gradlew assembleDebug` from a checkout of the v0.8.1 tag, with fips on its `feat/multi-path-switchover` branch. See [CONTRIBUTING.md](https://github.com/Origami74/myco/blob/main/CONTRIBUTING.md) for build prerequisites.

Phones don't need updating together.

The full per-release change history lives in [CHANGELOG.md](https://github.com/Origami74/myco/blob/main/CHANGELOG.md). Issues and discussion are at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to [@Origami74](https://github.com/Origami74) for maintaining the project, and to everyone who tested on real phones in real rooms.

<!--
This file is published verbatim as the GitHub Release body by
.github/workflows/release.yml — the leading `# Myco vX.Y.Z` heading and
`**Released**:` line are stripped, and the auto-generated "What's Changed"
section is appended below. Two consequences when writing the next one:
  1. Keep the version in the H1 matching the tag, or the workflow falls back
     to generated notes rather than publishing stale text.
  2. Use absolute links — relative paths 404 on a release page.
-->
