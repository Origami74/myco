# Myco v0.11.0

**Released**: 2026-10-13

v0.11.0 opens **napplets in the new NIP-5D format**, shows **their own
icons**, and lets napplets load **pictures from the web**.

- Every app on napplet.soy, published in the format NIP-5D adopted on
  2026-10-07, installs and opens. Older napplets still open.
- A napplet's own icon shows in the install sheet, the Apps grid, Recents and
  on home-screen shortcuts.
- Napplets can load `https:` pictures and files, such as profile pictures on
  someone's server, through Myco and only from the public internet.
- Apps hosted on their author's own Blossom servers install and update.
- The App Store shows "Around you" apps only when a phone nearby can serve
  them.

**No wire-format change from v0.10.0.** v0.11.0 and v0.10.x phones pair and
exchange messages as before, and everything upgrades in place. A v0.10.x
phone can't open napplets in the new format, so update both phones to share
them.

## At a glance

- **Napplets in the current NIP-5D format.** A napplet now names its single
  file by hash (`x`), describes itself in the event text, and lists required
  (`R`) and optional (`O`) capabilities.
  - The install sheet marks optional capabilities, and warns when an app needs
    one Myco can't provide. An update that adds one warns the same way.
  - Napplets in the old format still open, through a fallback that will be
    removed after 2026.
  - A napplet republished in the new format counts as a new version: "Check
    for updates" offers it, and the sheet may ask about capabilities you never
    reviewed. Your grants and the app's saved data are kept.
- **Napplet icons.** PNG, JPEG or WebP, checked against the hash the
  manifest signs and decoded before it is shown. Without a valid icon you see
  the coloured initial as before.
  - Shortcuts pinned before this release get the icon too.
  - The icon is kept with the app, so clearing the cache doesn't lose it.
- **Pictures from the web in napplets (`https:`).** A napplet with the
  resource permission can ask for an `https://` link. Myco fetches it: plain
  GET, no cookies or logins, at most five redirects, 30 seconds and 64 MiB.
  - Only public addresses: never this phone, your Wi-Fi network or a mesh
    name, checked again after every redirect.
  - The server sees your IP address and which link was opened.
  - Nothing is fetched with offline-only on. SVG is refused.
- **Authors' own file servers.** Myco reads the Blossom server list an author
  publishes (BUD-03) and looks there after the servers an app names, and
  before the public defaults.
- **Which nearby phones hold a file (`mesh.blobs`).** A napplet with the mesh
  grant can ask how many Circle phones in reach have each of up to 64 files.
  Myco asks over the mesh only, without downloading anything. The App Store
  uses it for "Around you".

## Also

- **Napplet posts reach all your outbox relays.** A note posted through
  NAP-OUTBOX went to only two of your write relays. It now goes to every one
  you list.
- **Posting waits less.** A post that tags people now waits for this phone,
  two relays and the people's inbox relays, and at most 3 seconds for an
  inbox relay. Slower ones are delivered in the background.
- **AVIF and HEIC pictures show in napplets.** They were taken for videos and
  shown as broken pictures.
- **No more dialling your own network.** A Blossom server named in a link, an
  author's list or an app manifest is used only when it is on the public
  internet. A custom Blossom store you set in Settings still works.
- **Your profile picture in Settings** goes through the same safe fetcher as
  napplet pictures.

## Known issues

- **Mesh over the internet is new.** It hasn't yet been tested with two
  phones on different networks reaching each other through a public node.
  It is off by default.
- **`https:` pictures aren't kept across restarts.** Myco remembers a web
  link for five minutes; after that, or a restart, it fetches it again.
- **Uploads: no per-upload prompt, no progress.** The install-sheet grant is
  the consent; without a signer app, an allowed app uploads without asking
  each time. EXIF and other metadata are not stripped. There is no progress
  bar and no cancel.
- **Composer can't attach a picture itself yet.** An app that uploads one,
  such as Doodle Duo, can hand Composer the link.
- **Few apps declare a role yet.** Opening "a profile", "an article" or "a
  composer" needs an app whose manifest declares that archetype, such as
  Simple Profile, Noris or Composer.
- **Reading positions in Noris stay on the phone.** Myco can't encrypt for an
  app yet (`relay.publishEncrypted`).
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
