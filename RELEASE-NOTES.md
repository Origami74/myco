# Myco v0.9.0

**Released**: 2026-09-30

v0.9.0 turns Myco's apps into something you can live in: a **feed** that
loads instantly and keeps scrolling, **apps that open each other**, and a
**cache** that makes everything you've looked at fast and available offline.

- Chronofeed, a chronological feed of the people you follow, and Simple Profile come with new installs.
- Apps can open each other by role: tap a person and your profile app opens.
- Everything your apps look at is cached, instantly available and shared with your Circle.
- Pictures load fast and feeds scroll smoothly.

**No wire-format change from v0.8.1.** v0.9.0 and v0.8.1 phones pair and
exchange apps and messages as before. Everything upgrades in place.

## At a glance

- **Apps open each other by role.** An app can ask Myco to open "a profile",
  "a note" or "a site" (NAP-INTENT). Myco opens the app you have for that
  role and hands it what to show. With several candidates it asks ("Open
  with…", with "Always use this"). **Settings › Default apps** lists your
  choices. Only you can set them. An app that asks without a recent tap gets a
  confirmation first. Myco itself opens sites, so "Open nsite → Myco" works.
- **A cache for everything your apps look at.** Notes, profiles and pictures
  your apps fetch are kept in a cache apart from what the phone keeps on
  purpose: 500 MB of notes and 1.5 GB of files by default, with what you come
  back to kept longest. Opening them again is instant and works offline, and
  paired phones nearby can get them from you. **Settings › Storage** shows the
  cache, sets its size and clears it.
- **Apps can keep what they showed you.** A napplet can keep a note or
  picture it showed you on your phone, or pass a note on to nearby phones or
  relays. It only works for things the app was actually shown, and it never
  signs someone else's note as you.
- **Chronofeed and Simple Profile come with Myco** on new installs, next to
  DingDong and the AppStore. Tapping a person in Chronofeed opens Simple
  Profile. Existing installs are left as they are; both can be installed from
  the AppStore.

## Faster and more complete

- **Apps get their data much faster.** Reading from relays and nearby phones
  was rebuilt around streams: what the phone holds goes out at once, and each
  relay's and each nearby phone's answer is passed on the moment it arrives,
  never waiting on the slowest. One shared connection per relay (built on
  rustic-applesauce) replaces a new connection per request, so busy relays no
  longer turn Myco away. Names now show up for everyone you follow, and feeds
  keep scrolling back past what the phone holds.
- **Pictures load fast.** Nearby phones and the internet are asked at the
  same time, the server a link names is tried first, missing pictures aren't
  asked for again for ten minutes, and pictures reach apps as raw bytes. Apps
  can load files up to 64 MB (was 10 MB), so short videos play.
- **Less battery.** The mesh adapter no longer keeps a processor core busy
  while the mesh is on.

## Also

- **Wi-Fi Aware is on from the first launch** on phones that support it; its
  permission used to be skipped on a fresh install.
- **Installing an app you already have offers to open it**, or to update it
  when a newer version is available, instead of a greyed-out install button.
- **Apps built with WebAssembly run.** They still can't reach the network on
  their own.
- **Chat survives a restart** until it expires.
- **"Delete cache" is now "Clear local database"**, and asks first.

## Known issues

- **Web pictures in apps.** Napplets load files by their hash (`blossom:`).
  Plain web images, including many profile pictures, show a placeholder (🥀 in
  Chronofeed). `https:` image support is still to come;
  [#67](https://github.com/Origami74/myco/issues/67) covers Blossom server
  discovery.
- **Few apps declare a role yet.** Opening "a profile" needs a profile app
  whose manifest declares the `profile` archetype (`napplet:profile/open`),
  such as Simple Profile; other profile napplets don't yet. Messages between
  apps other than opening by role (`inc.emit`) aren't routed yet.
- **A custom relay is read, not streamed.** With a custom relay set, reads wait
  for it rather than streaming as they arrive.
- **Big files are held in memory** while they load (up to 64 MB each).
- **Paid relays.** Relays that require a paid account (nostr.wine, for one)
  refuse Myco's reads, so apps published only there can't be found.
- **Open nsite windows and updates.** An update can mix old and new files in
  an open nsite window until it reloads:
  [#71](https://github.com/Origami74/myco/issues/71).
- **No "recently updated" mark on the Apps screen yet:**
  [#69](https://github.com/Origami74/myco/issues/69).
