# Myco v0.8.0

**Released**: 2026-09-26

v0.8.0 is about **you** and **your apps**.

- Every install now has an account from the first launch. You can log in with your own key or with a signer app like Amber.
- Apps find their own updates and share them with your Circle.
- The built-in Discover tab is replaced by an app store that is itself an app.
- The Apps screen now looks like the rest of Android.

**No wire-format change from v0.7.0.** A v0.8.0 phone and a v0.7.0 phone still pair and exchange apps and messages. A v0.7.0 phone passes napplet updates along as plain events. Only v0.8.0 phones download and share them. Everything upgrades in place.

## At a glance

- **Your own account.** Every install has a Nostr identity from the first launch: a guest with a name and a picture. Show your key, log out, or log in with your own `nsec`.
- **Log in with Amber.** Sign in through a signer app (NIP-55). Your key never enters Myco, and every app signs through the signer.
- **Discover is an app.** Browse napplets, read recommendations from people you follow, curate your own stacks, and install from a store-style page. It comes preinstalled and replaces the Discover tab.
- **Updates find you.** Myco checks for app updates when you open it and every few hours. A napplet update one phone gets is passed to the rest of your Circle, even with no internet.
- **A launcher-style Apps screen.** Round icons, five across on a phone and sized properly on a tablet. A small globe marks the apps that are websites.
- **Back works inside apps.** The back gesture goes back a page inside a napplet instead of closing it.

## Your account

Every install now has a Nostr identity from the first launch: a guest named `Myco Guest NNNNN`, with a picture made from the Myco logo in colours drawn from your key. When you're online, the profile goes out to the public relays, so other Nostr apps see you too.

The top of Settings shows who you are. It opens the **Account** page, where you can:

- **Show your secret key**, behind a warning never to share it.
- **Log out.** The key is removed from the phone. Myco offers to show it first, because a guest key you never copied is gone for good.
- **Log in** as a new guest, by pasting an `nsec`, or with a signer app.

**Signer apps (NIP-55, e.g. Amber).** Your key stays in the signer and never enters Myco. Napplets sign through it, in the background once you let the signer remember them, otherwise on its approval screen. Every event it signs is checked against what was asked for before Myco uses it. A napplet that publishes now waits while you approve, instead of timing out after 30 seconds.

A new guest follows three default accounts, so an app's friends feed isn't empty on day one. Open napplets hear about a login or logout straight away.

## Discover, the app store

The Discover tab is gone. In its place is **Discover**, an app store for napplets that is itself a napplet. It comes preinstalled, and it reaches phones that were set up before this release too.

- **Feed.** Napplets published to your relays, newest first, with search. An app's page shows its description, the permissions it asks for, and its details. **Install** opens Myco's own install review, and nothing installs until you tap Add.
- **Stacks.** Curated lists of apps (NIP-51 app sets, the same kind Zapstore uses), shown as a carousel. A person's own recommendations are their default stack. Stacks come from you and the people you follow, plus two featured ones.
- **Recommend.** Tap Recommend on an app to add it to your default stack. Apps your community recommends are marked with who recommended them, and only people you follow count.
- **My stacks.** Create, rename and delete your own stacks, and add any app with **Add to stack**.
- **Profiles.** Tap anyone's name to see their stacks and the napplets they published.
- **Around you.** Napplets the phones near you hold, over the mesh. If you switched the Mesh permission off, Discover shows how to turn it back on.

An installed app's review now says **Already installed**. If it's missing from the phone, it offers **Download again**. Either way, the permissions you changed are kept.

## Updates that find you

- **Automatic checks.** Myco checks installed nsites and napplets about 20 seconds after it comes to the foreground, and every 6 hours while it runs. Automatic checks are quiet and run at most every 30 minutes. **Check for updates** still runs straight away and shows its result.
- **Through your Circle.** A newer version of a napplet you have, heard from a paired phone, is fetched (from that phone first), checked and kept, then passed on. It's the same way nsite updates already travelled. Updates found by the check are shared the same way.
- **Restart when it suits you.** If you come back to an app that was updated while it was open, it offers a restart once. **Restart** opens the new version. **I'll restart later** keeps what you have, and you aren't asked again for that version.
- **Safe to accept.**
  - Only the author's newer versions are taken, and a version never moves backwards.
  - An update never gets a permission you didn't review. Anything new it asks for goes through the review sheet when you next open it.
  - Downloads pushed by a peer are size-capped.

## The Apps screen

The Apps screen now looks like a stock Android launcher:

- Round icons, one size per screen: phone size on a phone, a step larger on a tablet.
- Five across on a phone, and more on a tablet.
- A website favicon sits on a white disc, the way the launcher frames older icons.
- Napplets are the default and carry no mark. An **nsite**, a website Myco serves, has a small globe on the edge of its icon.

## Napplets

- **Back goes back.** Myco delivers the back gesture to the napplet as an Escape key. An app that handles it stays open and goes back itself. One that doesn't is closed, as before. An app can't trap you: after three backs it absorbed without a touch in between, the next back closes it. For app authors: handle Escape and call `preventDefault()` when you went back.
- **Links.** A napplet can ask to open a web link (your browser, after a one-tap confirm unless you just touched it). It can also point you at another napplet, which opens Myco's install review over the running app.
- **Theme.** Napplets can match Myco's light or AMOLED look. A switch while the app is open reaches it without a restart.
- **Who you follow.** Napplets can read your follows and mutes.
- **Faster publishing.** A napplet's publish answers once the event is stored here and two relays have it. The rest finish in the background.
- **Faster adding.** Adding a napplet fetches only its manifest for the review. The app itself downloads when you tap Add.

## Fixes

- Napplets saw nobody logged in: `identity.getPublicKey` answered in the wrong field.
- A removed app that nobody could deliver came back on its own. Remove now closes its window and keeps it gone until you add it again.
- A manifest a napplet published over the mesh ignored the hop budget it chose. It now keeps to it.

## Known issues

- **Discover's "Around you" needs the published Discover to declare `mesh`.** Until it does, the tab stays empty.
- **Profile pictures and app sizes in Discover.** Napplets can only load files by their hash (`blossom:`). A picture on a server Myco doesn't know about, or any plain web image, shows a placeholder. An app page may show "Unknown" for size. [#67](https://github.com/Origami74/myco/issues/67) covers finding servers properly.
- **Open nsite windows and updates.** A napplet offers a restart when it's updated while open. An nsite doesn't yet, and an update can mix old and new files in an open nsite window until it reloads: [#71](https://github.com/Origami74/myco/issues/71).
- **No "recently updated" mark on the Apps screen yet:** [#69](https://github.com/Origami74/myco/issues/69).
- **Nearby nsites aren't listed anymore.** The old Discover tab showed nsites your Circle held. Discover lists napplets only. An nsite still arrives by a share, a scan or a link.
- **A phone in your pocket finds nobody.** Myco winds its radios down when it isn't on screen.
- **Wi-Fi Aware is shut off by deep Doze** on Android 13 and later after a long idle period: [#30](https://github.com/Origami74/myco/issues/30).
- **A napplet's relay access is all-or-nothing,** and with the outbox grant it may name its own relays. Review the install sheet.

## Getting it

- **Android:** install the APK from the [v0.8.0 release](https://github.com/Origami74/myco/releases/tag/v0.8.0), or via [Zapstore](https://zapstore.dev/apps/app.myco).
- **From source:** run `cd android && ./gradlew assembleDebug` from a checkout of the v0.8.0 tag, with fips on its `feat/multi-path-switchover` branch. See [CONTRIBUTING.md](https://github.com/Origami74/myco/blob/main/CONTRIBUTING.md) for build prerequisites.

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
