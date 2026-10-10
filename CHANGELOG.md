# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Napplets can load pictures and files from the web (`https:`).** A napplet
  with the resource permission can now ask for an `https://` link — a
  profile picture on someone's server, say — not only a Blossom hash. Myco
  fetches it for the napplet: plain GET, no cookies or logins, at most five
  redirects, 30 seconds and 64 MiB, ten at a time, sixty a minute per app.
  It never reaches this phone, your Wi-Fi network or a mesh name, checked
  again after every redirect and on the address actually dialled. SVG is
  refused, as for Blossom. The server you load from sees your IP address
  and which link was opened. Nothing is fetched with offline-only on.

### Fixed

- **Napplet posts reach all your outbox relays.** A note a napplet posted
  through NAP-OUTBOX went to only two of your NIP-65 write relays, picked
  the way a read picks them; the rest never got it, so people reading from
  those relays didn't see it. It now goes to every write relay you list.
- **Posting waits less for everyone's inbox.** A napplet post that tagged
  people waited for every relay it went to — up to 8 seconds — before
  saying it was out. It now waits for this phone, two relays and the
  people's inbox relays, and for an inbox relay at most 3 seconds: one that
  is slower is reported as not reached yet, and delivery to it and to the
  rest carries on in the background.
- **AVIF and HEIC pictures show in napplets.** Myco took any file starting
  with an ISO media header for an MP4 video, so an AVIF or HEIC avatar or
  photo was handed to napplets as a video and shown as a broken picture.
- **Your profile picture in Settings is fetched safely.** It used a plain
  web request; it now goes through the same fetcher as napplet pictures:
  public addresses only, every redirect checked, a size cap while
  downloading, and nothing with offline-only on.

## [0.10.0] - 2026-10-05

### Added

- **Napplets can upload files (NAP-UPLOAD).** A napplet that asks for it, and
  that you allow on the install sheet, can put a file — a picture to post,
  say — on your Blossom servers, signed as you. Myco uses your server list
  (kind 10063), or public defaults only if you have none; if your list can't
  be found or names nothing usable, nothing is uploaded. It reports only what
  the server confirmed, keeps a copy on the phone for paired phones, and
  hands back the URL and NIP-94 tags. Up to 16 MiB, one upload at a time.
  Nothing is uploaded with offline-only on. Apps installed earlier that
  declared it are asked again. Doodle Duo uses it for its score picture.
- **Mesh over the internet (opt-in).** Settings › Mesh › Internet links this
  phone to public FIPS nodes when it is online, so your Circle reaches you
  when you are not in the same room. Myco lists the nodes advertising on
  Nostr, stars the ones join.fips.network recommends, and ticks up to three of
  those at random (you can change the pick). Nodes on the fips `next`
  protocol are left out. It holds two links, slows down off screen, and never
  dials while mesh-only is on. A public node sees this
  phone's IP address and which mesh addresses it talks to, not what you send.
  The Dev tab shows which public nodes are up.
- **A setup popup on first launch.** After the intro, one popup with a
  progress bar sets up the mesh and your name. "Enable mesh?" leads to a
  "Nearby devices" card and then a "Mesh connection" card; each says what
  Android is about to ask for, and only its button ("Allow nearby
  devices", "Allow VPN") shows Android's
  prompt. Last comes your name, filled in with the phone's own name and
  editable right there, with a clear button; "Use this name" saves it. A refused step gets a card that says what won't work
  and how to retry, including when another app's VPN is set to always on and
  holds the slot. "No thanks" leaves the mesh off and still asks your name.
  Switching the mesh on later, or a Fix in Settings, opens the popup on
  "Enable mesh?" again; Android asks nothing until you tap "Yes, enable".
  Phones upgrading with a working mesh don't see the mesh steps; an upgrade
  that is missing something sees them with "Not now", which keeps the mesh
  running. Until you choose a name, Bluetooth advertises a generated one,
  not the phone's own. Permissions
  are no longer all asked for at launch, and notifications are no longer
  asked for with Bluetooth. The camera is asked for only when you tap "Allow
  camera" on the scanner, not when it opens.
- **Settings › Permissions.** One page for nearby devices, the mesh connection,
  notifications and, optionally, keeping Myco running in the background (the
  battery-optimisation exemption), each with its state and a Fix or Allow.
  Every Allow, the file-share hotspot, the camera's "Allow camera" and the
  "Bluetooth is off" warning now show what Android is about to ask first;
  Android asks only after you tap the card's button.
- **Full-tunnel exit over SOCKS5 (experimental).** Entering the exit node as
  `socks5://<exit-npub>.fips:1080` sends all of the phone's TCP traffic through
  a SOCKS5 proxy on the exit, not only the web traffic of apps that honour the
  HTTP proxy. DNS goes through the exit too. UDP (QUIC included) is dropped,
  so apps fall back to TCP. `.fips` addresses and the local network stay off
  the exit. The plain `host:port` HTTP-proxy exit is unchanged.

### Changed

- **Less black screen when a napplet opens.** The WebView renderer process is
  kept running while Myco is on screen instead of being restarted for every
  window, and a napplet's WebView is built while the napplet is being verified
  rather than after. Fresh Noris open on a mid-range tablet: about 570 ms of
  black instead of 670–780 ms. The renderer is let go when Myco leaves the
  screen.

### Fixed

- **Wi-Fi Aware on Android 12.** Its location permission is now asked for
  with approximate location as well as precise. Android 12 ignores a request
  for precise location alone, so Aware could never be allowed there.
- **A napplet link finds the app on phones nearby.** Opening a napplet from a
  link (a bare `naddr`, with no sharer named) asked only the internet relays,
  so a phone without internet could not install it even with the app on a
  paired phone next to it. Review and install now ask the sharer and every
  Circle member in reach, all at once and before the internet — and with
  offline-only on as well. A sharer who has walked off no longer holds up the
  others. The app's files come from one phone, the quickest to answer, not
  from all of them. What a peer sends is still fully verified.
- **Wi-Fi and Wi-Fi Aware keep working after the mesh restarts.** When Myco's
  mesh restarted in the background, a phone could come back connected over
  Bluetooth only: phones on the same Wi-Fi and Wi-Fi Aware links were found
  but never used, until Myco was force-stopped. A restart now brings both back.
- **Phones on the same Wi-Fi keep finding each other.** If another phone left
  the network at the wrong moment, a phone could stop connecting to anyone new
  on that Wi-Fi until Myco was restarted. It now skips the phone that left and
  carries on.

## [0.9.1] - 2026-10-02

### Added

- **Composer for new installs.** Write a Nostr note, a reply or a quote.
  Replies are tagged the NIP-10 way (marked `root`/`reply` `e` tags, the
  parent's people as `p`) so the whole thread is notified; quotes get a
  NIP-18 `q` tag and the `nostr:nevent` in the text; `@` suggests the people
  you follow and inserts NIP-27 mentions; hashtags become `t` tags; there is
  a preview. It fills the `composer` role (`napplet:composer/open`, with
  `{ replyTo?, quote?, content?, mentions? }`), so another app's Reply opens
  it with the note being answered on screen. New installs only.
- **Noris for new installs.** A long-form reader after Boris: NIP-23
  articles, NIP-84 highlights (yours, your follows' and everyone's, painted in
  the text), and reading positions kept on the device. It fills the `article`
  and `highlight` roles, so an article or a highlight opened from Chronofeed
  lands in it. New installs only, like Chronofeed and Simple Profile.

### Changed

- **Napplets open and fill faster.** Measured on a Pixel 7 Pro with Noris:
  from the window starting to articles on screen went from about 650–700 ms
  to 240–330 ms.
  - A subscription's backlog is **streamed**: the subscribe call answers at
    once, and the stored events are pushed to the napplet as frames of their
    own as soon as the store read returns, then `eose`, instead of riding back
    in the call's reply. Each subscription is handed every event once (a
    per-subscription seen-set shared by the backlog and live deliveries), and
    a subscription closed or replaced while its backlog is read gets nothing
    more from it.
  - Fewer copies on the way: events become the napplet's JSON directly,
    without a print-and-parse; frames cross JNI as a `String[]`; Kotlin no
    longer parses frames it only forwards (the napplet itself, ~385 KB for
    Noris, was parsed on every open, and so was every delivered event under
    64 KB); a session's subscription maps are shared copy-on-write rather than
    copied into every read's snapshot.
  - The outbox reads many authors' stored relay lists in one query instead
    of two store reads per author.
- **The APK is about 22 MB, down from 78 MB.** Release builds run R8 code and
  resource shrinking (no renaming, so stack traces stay readable): the dex
  drops from 43 MB, mostly unused material-icons-extended, to 3.4 MB.
  `libmyco_core.so` is built with fat LTO, one codegen unit and stripped
  symbols, from 34 MB to 18 MB. Keep rules in `proguard-rules.pro` cover the
  classes Rust calls over JNI by name (`NativeCore`, `BleRadio`).

### Fixed

- **Tapping an app after Myco restarted no longer lands back in Myco.** A
  napplet's Recents task, restored after an update or a reclaimed process,
  is relaunched without its extras; the window now reads the napplet from
  its `myco://napplet/<pointer>` document address instead of closing.

## [0.9.0] - 2026-09-30

### Added

- **Apps can open each other by role (NAP-INTENT).** A napplet asks for "a
  `profile`" or "a `note`", never a specific app, and Myco opens the user's
  app for that role. `intent.invoke` / `open`, `available`, `handlers` and the
  `intent.changed` push are implemented. Roles come from the `archetype` tags
  of installed manifests, stored on each Library entry and backfilled from the
  pinned manifest. Resolution: the user's default for the role, else the only
  candidate, else an **Open with…** sheet ("Always use this" sets the default;
  **Settings › Default apps** changes it, and napplets can't). A request naming
  a specific app gets the sheet too. The payload is delivered as a NAP-INC
  `inc.event` on the convention topic (`napplet:profile/open`), held under a
  one-time token until the handler has sent `inc.subscribe` for it, since
  Myco's prelude announces readiness before napplet code runs. Only the
  resolved handler's window can bind it; held payloads expire after 60 s.
  Myco itself handles `napplet:nsite/open`. Without a touch in the last 5 s
  the user confirms first. `intent` and `inc` are default grants; NAP-INC is
  minimal (`inc.subscribe` / `unsubscribe`; `inc.emit` is accepted and not
  routed). Design: `docs/design/napplet/NAP-INTENT.md`.
- **A cache for everything apps look at, apart from what the phone keeps.**
  Query answers, pulls, mesh pass-through and napplet blob fetches go to a new
  `myco-cache` crate: a second `nostr-lmdb` store and a blob directory, each
  held to a byte budget by a segmented LRU (500 MB of events and 1.5 GB of
  blobs by default; a second read promotes an entry to the protected
  segment). The local relay and Blossom now hold only what the device keeps:
  its own events, profiles, follow and relay lists, manifests, and private
  messages, which are never cached. `tiered.rs` reads relay and cache as one,
  merged by id and newest per replaceable slot, and routes each write. Circle
  peers read both through the existing gate. Upkeep runs every 15 minutes
  (expiry sweep, eviction, snapshot only when changed). **Settings › Storage**
  shows usage, sets the budgets and has **Clear cache**.
- **Napplets can keep what they were shown, and pass it on (NAP-LOCAL,
  provisional).** `local.publish` keeps an event on the device: a template is
  signed as the user, and a signed event is kept as is. `relay`, `outbox` and
  `mesh.publish` accept a signed event too, sent unmodified. `resource.keep`
  moves a blob from the cache into the device's Blossom. All of these work
  only for events and blobs the napplet was actually delivered. Delivery is
  tracked per napplet in scalable bloom filters of event ids and sha256s
  (about 0.1% false positives, 8 MB cap). Another author's event is never
  re-signed. A signed `mesh.publish` floods again past the seen-set, at most
  once per 30 s per event. Design: `docs/design/napplet/NAP-LOCAL.md`.
- **Chronofeed and Simple Profile come with Myco** on new installs, next to
  DingDong and the AppStore. `DefaultNapplet::new_installs_only`: an install
  that has seeded before records them as seeded without installing them.

### Changed

- **Apps get their data much faster and more completely.** Reads were
  rebuilt around streams:
  - **Local first.** A query answers from the device at once when that
    satisfies it (a full `limit` for each limited filter, every requested id).
    Otherwise it takes the first relay with events.
  - **Plans don't wait on lookups.** Reads plan from the NIP-65 lists held now
    (`plan_stored`); missing lists are looked up in the background, and a
    subscription adds the relays they name to the streams already running.
  - **Streamed pulls.** Mesh pulls stream per event: the peer pool
    (`request_stream`), multi-peer pulls merged across the Circle, and
    forwarded mesh `REQ`s, which send the stored backlog at once and `EOSE`
    after the forwarded pull, without stalling the connection. Resync after a
    peer returns goes through the hub to live subscribers.
  - **Shared relay pool.** Internet relays use one multiplexed connection each
    (`relay_pool.rs`, over [rustic-applesauce](https://github.com/hzrd149/rustic-applesauce)'s
    `RelayPool`, pinned). Myco's connector checks the skip list, records each
    dial in relay health, pings every 30 s, drops a socket silent for 90 s,
    and fails waiting reads at once when a dial fails.
  - **Fixes.** Kinds 0, 3 and 10002 are also asked of the indexer relays. A
    multi-filter query is capped per filter (NIP-01) instead of at the
    smallest limit, which had cut profiles out of feed requests. Follow lists
    (kind 3) are kept like profiles.
- **Pictures in apps load faster, and bytes no longer travel as text.**
  - **Hints:** BUD-10 `blossom:<sha>.<ext>?xs=&as=` URIs are parsed. Servers
    named in `xs`, then the author's kind 10063 servers, are tried before the
    defaults.
  - **Fetching:** the mesh and the internet are raced over one shared HTTP
    client (5 s connect, 15 s read timeout). A confirmed miss isn't retried
    for 10 minutes.
  - **Delivery:** a `resource.*` result carries `blobRef`, and the shell
    fetches `/_blob/<token>/<sha256>`, which the window host serves as bytes
    behind a per-window token. Window frames cross the FFI one per line and
    are sorted off the main thread, and `resource.*` calls get their own
    in-flight slots.
  - **Size cap:** a resource can be up to 64 MiB (was 10 MiB).
- **Installing an app you already have offers to open it.** The review state
  carries `already_installed` and `update_available`, decided in the core for
  every entry point (links, scans, the AppStore, `link.open`). A newer version
  that asks for no new permissions gets **Update**.
- **Apps built with WebAssembly run.** The napplet CSP adds
  `'wasm-unsafe-eval'` (JS `eval` stays off) and `connect-src data: blob:`,
  which reads only bytes already in the page; network access is still only
  through capabilities.
- **The mesh adapter no longer keeps a core busy.** The TUN reader spun on
  `EAGAIN` from the non-blocking fd. It now waits with `poll(2)` and a 1 s
  timeout, so it sleeps until a packet arrives and lets go on a tunnel
  restart.
- **Wi-Fi Aware is on from the first launch** where the phone supports it.
  On a fresh install the Bluetooth and Wi-Fi Aware permission requests were
  launched back to back on one `ActivityResultLauncher`, and Android drops a
  second request while one is on screen, so `NEARBY_WIFI_DEVICES` was never
  asked and the lane stayed off despite defaulting on. Every request now goes
  through one `requestPermissions()`: startup asks for both radios at once,
  and a request made while a dialog is up is queued until it closes.
- **"Delete cache" is now "Clear local database"**, which clears what the
  phone kept (except pinned apps) and the cache, and asks first.
- **Chat survives a restart until it expires.** NIP-40 events are stored in
  LMDB like any other, instead of in memory, and swept once expired.

## [0.8.1] - 2026-09-27

### Fixed

- **The AppStore that comes with Myco keeps working after it updates.** On a
  new phone, AppStore could update itself a few minutes after the first
  launch and then lose access to its app listings, while its Install button
  said only "cannot open install". The apps Myco comes with (AppStore and
  DingDong) now start with the permissions they are known to need, so an
  update from the same author that asks for no more just works. An app that
  asks for something new after an update now asks you over the app itself,
  where you are, instead of on Myco's main screen; tap Allow and it restarts
  with the new permission. Anything you switched off for an app stays off.
  An app's Install button is no longer refused because of a question waiting
  somewhere else, and when it is refused, the app is told why (for example
  "busy: another review is open").
- **Apps open with what your phone already has.** An app that reads Nostr
  data (the AppStore, for one) used to wait for the slowest relay before it
  showed anything, even when your phone already held the answer — so it
  reopened slowly whenever a public relay was down or slow. A lookup now
  waits a moment for the relays that answer quickly (so a newer profile
  still wins over the copy on your phone) and at most about a second and
  a half when only your phone has answered. Relays that answer later are
  not wasted: what they send is saved on your phone for next time, and
  live views get it as it arrives.
- **Apps published only on their author's own relays are found.** Adding an
  app, and checking installed apps for updates, used to look only on a fixed
  set of public relays, so an app its author published elsewhere came back
  as "could not find app" (the Minesweeper napplet from the AppStore, for
  one). Myco now also looks on the relays the author lists as theirs, and
  remembers that list for next time. `relay.ditto.pub`, where many apps are
  published, is now one of the default relays; `relay.nostr.band`, which no
  longer answers, is dropped.
- **One broken relay no longer cuts apps off from the rest.** An app
  asking a single relay that answered with an error could make Myco
  believe the internet was down and stop using every public relay for half
  a minute. Now any answer from the internet counts as the internet
  working.
- **Broken relays stop slowing things down.** A public relay that refuses
  Myco, is down, has a bad certificate, or does not exist is now left alone
  for a while (a minute at first, up to half an hour if it keeps failing)
  instead of being tried again on every lookup. One that keeps failing to
  connect while others answer gets a shorter break. Losing your signal,
  switching networks, or a Wi-Fi login page does not count against any
  relay. Relays in your Circle, and a custom relay or Blossom server you
  set in Storage, are never skipped.
- **Open apps keep receiving new posts.** An app watching for new events
  (a feed, a chat, someone's app list) used to ask each relay once and
  then hear only what came through your Circle. Relays now stay connected
  for as long as the app is watching, and new events arrive as they are
  published; closing the view or the app disconnects them.
- **Profiles show all of a person's apps.** Looking up where someone
  publishes now also asks the index relays and any relays the app
  suggests, so people who publish only on their own relays (hzrd149's
  apps on nostr.wine, for one) are found. A person is no longer marked
  as "nowhere to be found" because a relay was slow to answer.
- **Feeds with many people use far fewer connections.** An app showing
  posts or apps from dozens of people used to connect to every relay any
  of them listed — up to forty for one view. Myco now picks a handful of
  relays that between them reach each person twice, preferring relays it
  is already connected to and avoiding ones that are failing.
- **"Delete cache" no longer breaks an installed app that has a newer version
  waiting.** If a newer version had reached your phone but was not downloaded
  yet, deleting the cache could leave the app with nothing to open. The version
  you run is now kept.

### Changed

- **Profiles and app listings you have seen stay on your phone.** When an
  app looks up a profile, someone's relay list, or an app listing (the
  AppStore, for one), Myco keeps a copy. The next look is instant and works
  offline, and people in your Circle can get it from your phone. It installs
  nothing and downloads no app files, and an app you have keeps running (and
  sharing) the version you have the files for. "Delete cache" in Storage
  clears these copies. With a custom relay set in Storage, nothing is kept.
- **A new guest gets better default relays.** Its public relay list now
  names three general-purpose relays (relay.damus.io, relay.ditto.pub,
  relay.primal.net) for both posting and receiving, instead of the lookup
  relays, which included a directory-only relay and an unreliable one. A new
  guest also gets a direct-message relay list, so other Nostr apps know where
  to send it private messages. A guest made before this version keeps its
  relay list; log out and start a new guest to get the new defaults.
  Accounts you log in to with an `nsec` or a signer app keep their own
  lists; Myco does not create or change them.
- **Profiles and relay lists go to the index relays.** A new guest's profile
  and relay lists are now also sent to four index relays (purplepag.es,
  index.hzrd149.com, indexer.coracle.social, user.kindpag.es), where other
  Nostr apps look people up, and those four are where Myco looks up an app
  author's relay list. purplepag.es is no longer asked for apps, which it
  does not carry.

## [0.8.0] - 2026-09-26

### Added

- **Your own account — the headline of this release.** The top of Settings
  shows who you are — picture, name and npub — and opens an Account page.
  Every install starts as a guest (`Myco Guest NNNNN`) from the first launch,
  with a picture: the Myco logo in a gradient drawn from your npub. The
  profile goes to the public relays and Blossom when you're online. From the
  Account page you can show your secret key (after a warning never to share
  it), log out, and log back in as a new guest or with an `nsec`. Open
  napplets hear about a login or logout at once (NAP-IDENTITY's
  `identity.changed`). A new guest follows three default accounts, so a
  napplet's friends feed is not empty on day one; existing and imported
  identities are left alone.
- **Log in with Amber — your key never enters Myco.** The Account page's "Log
  in with a signer" logs in through a NIP-55 signer app: your key stays there
  and never enters Myco. Napplets sign through it — in the background once you
  let the signer remember, otherwise on its approval screen — and every signed
  event is checked against what was asked for before it is used.
- **AppStore comes preinstalled.** An app store for napplets, itself a
  napplet: a feed of napplets from your relays, community recommendations
  from people you follow, and store-style app pages whose Install opens
  Myco's install review, and an "Around you" tab of napplets the phones nearby
  hold. Pinned like DingDong, with only the default grants.
  It replaces the native Discover tab (see Removed). Phones that were
  already set up get it on upgrade too: the seed now remembers each default
  it pinned, so a new default still arrives and one you removed stays gone.
- **Napplet updates reach your Circle.** A newer version of a napplet you
  have installed, heard from a paired phone, is downloaded (from that phone
  first), checked and kept, then passed on — as nsite updates already were —
  and opens at the app's next launch. One found by "Check for updates" is
  passed on too. Only its author's newer versions are taken, and an update
  never gets a permission you did not review: anything new it asks for goes
  through the review sheet when you next open it. Phones without the app just
  pass the update along.
- **Apps check for updates on their own.** Myco now checks installed nsites
  and napplets when it comes to the foreground and every 6 hours while it
  runs. Automatic checks are quiet and run at most once per 30 minutes;
  "Check for updates" still runs right away and shows its result.
- **An updated napplet offers a restart.** A napplet window keeps the version
  it opened, so re-opening one that sat in the background used to bring back
  the old version after an update. Now, when it comes back and a newer version
  has been installed meanwhile, it asks once: "Restart" opens the new version,
  "I'll restart later" keeps the window as it is and is not asked again for
  that version.
- **Open an app right after adding it.** Tapping "Add to my apps" used to show
  "Adding…" and then the sheet just vanished, as if nothing had happened. Now
  it stays up once the app lands — "<App> was added to your apps" (or
  "downloaded again") with **Open** and **Done**. Open starts the app in its
  own window, also when the sheet was opened from a link inside another
  napplet.
- **Napplets can open links and match your theme.** NAP-LINK: a napplet can
  ask to open a web link (your browser, after a one-tap confirm unless you just
  touched the app) or point you at another napplet — Myco's install review
  opens over the running app, and nothing installs until you tap Add. Repeated
  asks are refused while a review is showing and rate-limited. NAP-THEME:
  `theme.get` answers Myco Light or Myco AMOLED to match the app's dark mode,
  and a switch while the app is open is pushed as `theme.changed` without
  restarting it. Both are granted by default and can be switched off per app.
- **Napplets can read who you follow and mute.** NAP-IDENTITY's `getFollows`
  and `getMutes` answer from your kind 3 and kind 10000 on this phone (empty
  when there are none), where they were always empty.

### Fixed

- **Back inside a napplet goes back, and never closes it.** The back gesture
  always closed the whole napplet, even from a page it had opened —
  AppStore's app page included. Myco now delivers back to the napplet as an
  Escape keydown: a napplet that handles it (`preventDefault()`) stays open
  and goes back itself. One that leaves it unhandled is sent to the
  background — you land where you came from, and the app keeps running with
  its state, in Recents. A napplet can't trap you: after three backs it
  consumed without a touch in between, the next back leaves it. Only a
  crashed or hung window is closed. Plain DOM behaviour, no new NAP — see the
  napplet runtime design doc.
- **Napplets saw nobody logged in.** `identity.getPublicKey` answered in a
  `publicKey` field where NAP-IDENTITY (and the reference shim) use `pubkey`,
  so every napplet read `undefined` — Minesweeper said "Sign in to publish".
  The other identity list queries replied in a generic `result` field the
  shim reads as `undefined`; they now answer in their spec fields (`pubkeys`,
  `entries`, `zaps`, `badges`).
- **A napplet's publish no longer times out while you approve it in your
  signer app.** Publishing (NAP-OUTBOX and NAP-MESH) gave up after 30 s, so a
  slow approval in Amber showed as "outbox.publish timed out" — and the event
  could still go out. The napplet now waits for the answer: the signed event,
  or a clear failure once the signer app has had its two minutes.
- **Slow napplet publishes.** `outbox.publish` waited for every relay (up to
  8 s) before answering. It now answers once the event is stored here and
  two relays have taken it — or the only relay, for someone with one — and
  the rest finish in the background. A publish to other people's inboxes
  still waits for every relay, so a failed delivery is always reported.
- **Adding a napplet was slow and downloaded too early.** "Looking for this
  app" now fetches the manifest only — the review appears as soon as it is
  found and its signature checks out, and the relay wait after the first
  answer is 250 ms, down from 600 ms. The app itself is downloaded only once
  you tap "Add to my apps" (the button shows "Adding…"), verified against the
  manifest you reviewed. The review sheet opens fully and scrolls, with its
  buttons always on screen. The update check keeps its 600 ms wait.
- **A removed app came back.** Removing an app nobody could deliver (stuck on
  loading) had no lasting effect: its open window's loading page reloads every
  second and each reload started a new search, re-creating the tile. Remove now
  closes that window, drops any pending deep link to it, and keeps it gone —
  an in-flight sync can't re-list or re-pin it — until you add it again. The
  loading page also searches at most every 15 s rather than every second.
  Opening the app again — from Add, AppStore, a link or a home-screen
  shortcut — brings it back.
- **A manifest published through NAP-MESH keeps to the hop budget the
  napplet chose.** An nsite or napplet manifest a napplet published over the
  mesh went out at the default budget whatever it asked for; it now respects
  the choice, including 0 for "this phone only", as other NAP-MESH publishes
  do.

### Changed

- **Apps get the Mesh permission by default, for now.** The tools napplet
  authors publish with drop Myco's own permissions from what an app asks
  for, so no app could ask for Mesh — and "Around you" in AppStore, and
  every app that talks to the phones nearby, stayed silent. Mesh is now
  granted like Relays and Identity: still listed on the install sheet,
  capped by App reach, and switchable off per app. Apps you already have
  get it at their next open, without asking, unless you switched it off.
  This goes back to opt-in once the publishing tools keep Myco's permissions.
- **The Apps screen looks like a stock Android launcher.** Round icons, five
  across on a phone and more on a tablet, a step larger on tablets, instead of
  four tiles stretched to fit. Napplets are the default and carry no mark; an
  nsite — a website Myco serves — has a small globe on the icon's edge.
- **The install review knows an app is already installed.** Opening a
  napplet you already have — from a link, a scan, or another app — greys
  out Add and says "Already installed". An installed app that is not on
  this phone ("hold to reload") offers "Download again" instead, and an
  installed app whose update asks for more still offers Add, to agree to the
  new permissions. Either way the permissions you switched since stay as you
  left them.
- The user key napplets publish as is created on first launch, not the
  first time a napplet opens. Settings' "Identity" row is now "Device name",
  to keep it apart from the account.

### Removed

- **The Discover tab.** The bottom bar is now Apps · Circle · Settings · Dev;
  the preinstalled AppStore napplet, on the Apps grid, takes its place. What
  goes with it: the tab's "Around you" list of *nsites* your connected Circle
  members hold, and its Suggested row (bitchat, ICS, Dumplings, Mappy,
  Minesweeper, DingDong). The AppStore napplet lists napplets only. An nsite
  still arrives by a share, a scan or a link. For developers: the
  `search_nsites` action and the `discovered` state field are gone from the
  FFI.

## [0.7.0] - 2026-09-16

### Added

- **Napplets.** Myco runs napplets — single-file NIP-5D programs published
  on Nostr — beside nsites, as its own apps. Add one by `naddr` (paste, QR,
  or a bump from a friend, whose phone is asked first so it arrives with no
  internet), review what it asks for, and it lands on the Apps grid with a
  🦆 badge and its own full-screen window, home-screen shortcut included.
  Every napplet runs in a sandboxed iframe inside a trusted shell page with
  no network of its own; everything it does goes through capabilities Myco
  implements on its behalf.

  Capabilities this version implements, in the words the install sheet
  uses:
  - **Identity** (NAP-IDENTITY) — a user key, separate from the mesh device
    key, generated the first time a napplet opens and seeded with a guest
    profile and a relay list of the configured relays. The device is never
    named in a user-key event.
  - **Relays** (NAP-RELAY) — read and post as you on the relay pool: this
    phone's relay and the public relays when reachable. A granted `relay`
    posts without asking each time. Subscriptions are live; what the pool
    holds streams in behind the local backlog. Posting excludes kinds 0, 3,
    5 and 10000–19999 for now — profile, contacts, deletions, relay list —
    a napplet that tries gets a refusal, not a silent drop.
  - **Outbox** (NAP-OUTBOX) — outbox-model routing: an author's notes from
    their NIP-65 relays, a phone across the room over the mesh or a public
    relay over the internet, deduplicated, with `incomplete` when a relay
    never answered. Inbox delivery is refused rather than misrouted when a
    relay list is missing.
  - **Mesh** (NAP-MESH, Myco's own, in the registry's form) — publish to
    everyone nearby with a chosen hop count and pull what was missed.
    Settings › App reach caps how far apps may send (default 3 hops) and
    look (default 2); zero keeps an app on your phone.
  - **Pictures and files** (NAP-RESOURCE, `blossom:` only) — by content
    hash, this phone first, then a friend's phone over the mesh, then the
    public servers; what is fetched is kept for the next app and the next
    phone in the room, bounded per blob and per request.

  Permissions: the install sheet lists everything a napplet will be able
  to do — what it declares plus the defaults (identity, relays, pictures)
  — before anything is agreed to. Hold an installed napplet → **Manage
  permissions** to switch each capability on or off; a change is live, and
  an open window restarts under the new grants. What you switch off stays
  off. An update that asks for more than you were shown goes back through
  the sheet before it gets it.

  Updates: "Check for updates" refreshes installed napplets beside the
  nsite check, and the version you open is always the one whose bytes are
  on the phone — a newer manifest with nothing behind it cannot take an app
  off the air. A tile dims and says so when its app is not on this phone
  (after "Delete cache", say); "Reload app" fetches it again, the sharer's
  phone first.

- Discover suggests napplets beside its nsites — Mappy, Minesweeper and
  DingDong. A tap fetches the napplet and opens install review on the Apps
  tab; nothing is granted until you say so there.
- DingDong comes preinstalled, like bitchat: pinned on first run with only
  the default grants, and what it declares is put in front of you the first
  time it opens.

- **Send a file to a paired phone.** Share anything from another app, pick
  one of your paired phones, and it arrives encrypted over the mesh — no
  hotspot, no internet. The Circle tab has the same door: tap a contact and
  choose "Send a file". The receiving phone is asked first and can say no;
  received files land in Downloads/Myco. Transfers in flight show on the
  Circle tab beside pairing requests, so a send that is still waiting is
  visible from anywhere in the app and can be cancelled; an offer nobody
  answers gives up after ten minutes. A transfer survives a flaky link: a
  lost offer, accept or "ready" is re-sent until the other side has heard
  it, and a large file over a slow hop is only given up when nothing has
  arrived for thirty seconds.
- Phones on the same Wi-Fi find each other over the network instead of
  Bluetooth. Myco announces itself on the local network the same way a fips
  node does, so two phones — or a phone and a desktop — on one Wi-Fi connect
  over UDP, which moves a file in seconds rather than minutes. A peer the
  phone quietly stops reporting is found again on its own, and one that
  first answered with only a link-local address is re-resolved rather than
  given up on. Settings → Mesh has a "Network (LAN)" switch to turn this on
  or off.

- **Peers keep every link they have.** The mesh holds more than one path
  to a phone — Bluetooth and Wi-Fi Aware, or Bluetooth and the local
  network — probes the standbys so they are known to work, and moves
  traffic when the active one degrades, instead of dropping the peer and
  finding it again from scratch. Two connected phones stay connected
  through the hiccups that used to disconnect them. Built on fips's
  multi-path branch; an older Myco ignores the new link messages and keeps
  linking over one path as before.
- The status panel behind the peers pill shows every link a peer has, not
  just the one carrying traffic: one icon per lane, the active one lit and
  the standbys faded. A phone on Bluetooth and Wi-Fi at once is listed
  under both. Peers that never told us a name are shown by their shortened
  npub there instead of a generated placeholder name.

- Nsite manifests declaring a NIP-5A aggregate hash are checked against it;
  a mismatch is logged and the site is served on its per-blob hashes
  (napplets, whose identity the aggregate is, are refused instead).
- A Nix flake for the toolchain (`nix develop` for the Rust host shell,
  `nix develop .#android` for the Android SDK/NDK/JDK 17/Gradle/adb shell), so a
  NixOS or nix-enabled machine gets a working build environment without a manual
  rustup/SDK-manager install. See `docs/how-to/build.md` §1.

### Changed

- The relay store is an LMDB database (`nostr-lmdb`): indexed NIP-01
  queries, one small write per event, and negentropy items ready for mesh
  sync. Chat and other NIP-40 expiring events stay in memory and never touch
  disk, as before. `events.json` from an earlier version is migrated on
  first open; if any event fails to migrate, the file is kept.

### Fixed

- A page that crashes cannot take Myco down. An nsite window whose renderer
  died used to kill the whole app — mesh, relay and every other window. The
  window now closes on its own and the rest of Myco keeps running.
- The mesh tunnel comes back on its own after another VPN app takes the
  slot and gives it up again. Peers stayed linked over the radios, so the
  mesh looked healthy while nothing could reach anyone; Settings now says
  "Mesh tunnel is down" with a tap to fix, and reopening Myco fixes it too.
- Wi-Fi Aware no longer retries a failed attach thousands of times a minute
  while the phone's Wi-Fi stack refuses it, which got the app killed; it backs
  off from a second to a minute between tries.
- Inviting someone from Nearby no longer labels them with your own device
  name. The tap recorded your name against their npub, so the "invite sent"
  pop-up — and their bubble everywhere else — read back as you. The invite now
  carries the name they told us, and nothing at all when they have told us
  none, so their real name still wins once it arrives.

## [0.6.1] - 2026-08-21

### Fixed

- Wi-Fi Aware carries several phones at once instead of one. The lane ran a
  single UDP socket, and Android lets a socket serve only one Wi-Fi Aware
  connection, so a second phone's link came up and then went quiet — the
  hardware was never the limit. Each phone now gets a socket of its own, as many
  as the phone's chipset says it can hold.

## [0.6.0] - 2026-08-19

### Added

- Myco can store its data on a relay you run instead of on the phone. Settings →
  Storage → Advanced takes a relay URL — Citrine on the same device, or a relay
  on your own network — and everything Myco keeps in its event store lives there
  instead. A Blossom server for the app files themselves can be set the same way.
  Both are off by default and neither is needed to use Myco; the built-in stores
  remain the normal case. Confirmed working against Citrine.
- Settings warns when a store you configured cannot be reached, with a red dot on
  the Settings tab and on Storage. Without it the symptom is apps that will not
  load and nothing to explain why — the same class of invisible failure the radio
  warnings already cover. Changing either store offers to restart, since the
  setting is read when Myco starts.
- Storage says when the built-in store is no longer the one being used, rather
  than showing a usage bar for data nothing reads. Delete now says plainly that
  it clears this device only: a store you run is not Myco's to empty, and
  claiming otherwise about a destructive action is worse than saying nothing.

- The status pill opens. Tapping the counts brings up a panel with the two
  questions people actually have — can I reach my Circle, and what are the
  radios doing. Circle members are listed reachable-first (alphabetically
  inside each group, so the list never reshuffles under your thumb), with the
  offline ones folded behind a single line. Each radio lane says whether it is
  scanning and lists the peers it is carrying, with ping, how long the session
  has held, and when it was last heard from — "now" for anything inside ten
  seconds, because a counter flickering 1s/2s/3s reads as a fault when it is
  the healthy case. A lane whose scan state cannot be observed says `unknown`
  rather than `idle`, and a radio the phone does not have is left out entirely.
- Peers show a ping. FIPS has been measuring a smoothed round-trip time per
  link all along and Myco was discarding it at the boundary. A link that has
  never been timed shows no ping rather than a confident `0ms`.
- Myco asks what to call this device, once, on first run — and defaults to the
  name the phone already has. "Arjen's S21" is far easier to pick out across a
  table than "green sammy", but it usually carries a real name and it travels
  in every pair request, so it is shown before it is used rather than adopted
  silently. The pseudonymous generated name sits beside it as a single tap.
- That chosen name now rides the Bluetooth advert, so people see it in Nearby
  before pairing rather than a name derived from your public key. It is a
  plaintext broadcast anyone in range can forge, so it never displaces a name
  learned from a signed pair request — it only fills the gap where there was no
  name at all.
- Wherever a peer is named it is now the name they chose: Nearby, the Circle,
  the Dev peer list, the speedtest. The key-derived name is the floor rather
  than the default.
- Wi-Fi Aware carries mesh traffic for the first time. The fast lane had been
  negotiating a data path with nearby phones for months and never moving a
  byte over it: the mesh node had one UDP socket, and the LAN lane pinned it to
  the Wi-Fi network, after which nothing addressed to an Aware link could be
  routed. Aware now has its own socket, pinned to its own network, and two
  phones in a room peer over it directly — no access point, no router, no
  internet. On the bench it becomes the busiest link between them, ahead of
  Bluetooth and ahead of the LAN.
- A lost Aware link comes back in seconds rather than minutes. It used to wait
  for the next discovery sweep; it now asks for the path again as soon as it
  drops, backing off if the peer has genuinely gone.
- Settings says so when Bluetooth scanning is deaf because location services
  are off. Some phones refuse to report nearby devices without location even
  when an app asks not to use it for location, and the symptom is an empty peer
  list with nothing to explain it — on one tablet, hours of it. The warning
  appears only once scanning has actually been silent for a while, so a phone
  that scans perfectly well with location off is never nagged, and tapping it
  goes straight to the setting.
- Each peer on the Dev tab now shows which radio carried it, as an icon down
  the left edge: the Bluetooth rune, the Wi-Fi Aware arcs, or a globe for
  anything routed. A peer with no link yet shows nothing rather than a guess.
- Peer rows carry how long the session has been up beside how long ago it was
  heard from. Those answer different questions, and only the second was
  visible: a link re-establishing every few seconds looks perfectly healthy if
  all you can see is that it was heard from a moment ago.
- Share files with **any** phone — no Myco on the other side. A new hotspot
  bubble on the Circle tab (above the QR bubble) opens a local-only Wi-Fi
  hotspot on this phone and a plain web page served from it. The other phone
  scans one QR to join the hotspot, opens the shown address in its browser, and
  can download the files you chose to share and upload files back to you.
  Received files land in `Download/Myco/`, so they show up in the Files app
  like any other download. The hotspot runs in a foreground service with a
  Stop action, so it survives leaving the tab and is always one tap to kill.
- While the hotspot is on, bumping the phones hands the other phone the file
  page directly: the NFC tag Myco already emulates for pairing serves the
  page's address instead, and the other phone's own system opens it in its
  browser — nothing to install, nothing to type. For the whole hotspot
  session NFC does *only* that — pairing by bump is fully disabled, in both
  directions (this phone neither presents a pair code nor acts on one it
  reads), and comes back the moment the hotspot stops.
- Nothing is transferred behind your back: every download and upload a guest
  starts pops an accept-or-decline dialog on your phone — wherever in Myco you
  are, not just on the hotspot sheet — with the file's name and size. The
  guest's browser simply waits for your answer; an unanswered request is
  denied after 90 seconds, and stopping the hotspot denies everything still
  waiting. The notification names the file that is waiting so a request can't
  sit there unseen.
- Sending now works like AirDrop in both directions. "Send a file" on the
  hotspot sheet pushes any document straight at the guest: their browser pops
  an accept-or-decline dialog with the file's name and size, accepting saves
  it as a normal download, and the sheet shows each offer's fate — waiting,
  sent, or declined.
- The file page only ever offers this session's files. Starting a hotspot
  wipes the served list; only what you pick now, or receive now, shows up for
  the guest — files from earlier sessions stay in `Download/Myco/`, visible
  to you alone.

### Changed

- **Devices must be on the same version to exchange messages.** Mesh state — how
  far a message travels, which query it belongs to — used to be written into the
  messages themselves, which meant any relay carrying Myco traffic had to
  understand Myco. It now travels alongside them, so the events and queries Myco
  stores are ordinary Nostr and an ordinary relay can hold them. The cost is a
  clean break: a phone on an older build and a phone on this one will not pass
  events to each other.
- Pairing has its own door. It used to arrive on the same port that serves your
  apps and messages, which meant that port had to stay open to strangers and
  every pairing request was written into your event store as a side effect.
  Pairing now has a service of its own — the only thing an unpaired device can
  reach — and the content ports are closed to anyone you have not paired with,
  refused before a connection is established rather than after.
- A peer you have paired with can no longer upload files to your device by
  default. Nothing in normal use needs it: sharing an app works by the other side
  fetching it from you. The developer speedtest is the only thing that did, and
  it now says the peer declined rather than failing obscurely.
- Nothing starts and nothing is asked for until the intro has played. A cold
  install used to bring up the LAN browse and then stack the Bluetooth prompt,
  the Wi-Fi Aware prompt and the system's "Myco wants to set up a VPN
  connection" dialog over the splash animation, before the app had said what it
  is. Every one of those now arrives after the intro. Later launches are
  unchanged.
- The status pill is bigger, and turns red outright when the mesh is off — a
  grey slider was not enough to notice across a room. Its whole left third
  toggles the mesh rather than the slider alone: the slider swallowed every tap
  that landed beside it, which is what made this fiddly, not the target being
  small.
- The generated device name has 2048 combinations instead of 144, which is why
  duplicates kept turning up — a room of fourteen phones was already even money
  for a collision. The colour and the name are now drawn from independent parts
  of a real hash rather than from correlated bits of one small one.
- The mesh node is rebuilt on current FIPS. The version Myco had been building
  against had drifted a long way behind, and the gap included fixes to path
  MTU, framing, peer identity and the control plane. Everything Myco needs from
  the node is now carried as focused changes on top of that current base rather
  than as a private fork: Bluetooth as a first-class transport on Android,
  per-instance transport addressing, an app-owned socket seam, and two
  control-plane bug fixes. Peer state, peer discovery and `.fips` name
  resolution all moved onto interfaces the node already ships.
- Dev tab peer rows are legibly expandable — a caret says a row opens before
  you tap it — and the screen now leads with your own identity, then peers,
  then the radio self-check, which is the order you read them in.

### Fixed

- Someone can no longer add themselves to your Circle uninvited. A pairing
  acceptance is only acted on if it answers an invitation you actually sent, and
  if it was addressed to your device — a signed acceptance meant for somebody
  else could otherwise be captured and replayed at you. Being in a Circle grants
  access to your relay and files, so this was worth closing properly.
- Wi-Fi Aware links stop dying about once a minute. A data path would come up,
  carry traffic, and be torn down by the phone's firmware on a startlingly
  regular 64-second cycle. Radio coexistence was the obvious suspect and turned
  out to be wrong — backing the Bluetooth scan off changed nothing at all. The
  cause was Myco itself, re-establishing the same peer alternately over Bluetooth
  and Aware; it now leaves a peer alone on Aware instead of also dialling it over
  Bluetooth. Teardowns go from one a minute to one in seven, with both radios
  scanning harder than before. Some churn remains in the first few minutes after
  launch, when Bluetooth legitimately connects first.
- Opening a chat or an app list no longer waits on a slow peer before showing
  anything. A request from an app was answered only once every peer had replied
  or timed out, so a single unreachable phone made the app look hung. What is on
  this device now appears immediately, and anything a peer adds arrives as it
  comes.
- An old message is no longer re-broadcast to everyone each time someone new
  comes into range. Whether a message counted as new was decided by whether the
  store still held it, so a message that had expired and was fetched again looked
  new and started a fresh wave.
- Removing someone from your Circle now closes the connection they already have,
  instead of only refusing the next one. They could otherwise keep receiving
  everything they were already subscribed to.
- Renaming your device changes what goes out over the air. The rename wrote the
  preference and told the mesh node but never told the radio, so the old name
  kept being broadcast until the app was next brought to the foreground — which
  is exactly the surface a rename is usually aimed at.
- A peer that connected to us, rather than being dialled by us, is attributed
  its own Bluetooth adverts again. Only outbound dials were recorded, so an
  inbound peer had no address on file and its signal strength — and now the name
  it advertises — went missing.
- Bluetooth works again after being switched off and on. Turning the radio off
  and back on — or leaving and returning from airplane mode — left the app
  permanently unable to see any Bluetooth peer until it was force-stopped,
  because nothing was watching the adapter. The lane is now rebuilt when the
  radio returns, including the case where the app started with Bluetooth
  already off.
- Failed Bluetooth dials no longer accumulate until nothing can connect. Every
  attempt that timed out abandoned a socket holding a connection slot, and once
  enough had leaked every later attempt hung for its full timeout and failed —
  recoverable only by force-stopping the app, which is exactly the workaround
  this behaviour had been trained into people for months.
- A phone could advertise a Bluetooth port nothing was listening on, which made
  it permanently impossible to dial while looking perfectly healthy from the
  outside. It now advertises the port it actually bound, and re-advertises when
  that changes.
- An unreachable peer is no longer redialled every thirty seconds forever. One
  dead address could absorb most of the connection attempts and block every
  other peer queued behind it; attempts now back off per address.
- Turning the mesh off and on left the previous node running. Two nodes then
  shared one radio, and the one answering questions about peers was not the one
  doing the work — so the app could report no peers while a connection was live.
- The Dev tab reported Bluetooth scanning and advertising as `unknown` on a
  radio that was plainly working, and kept saying `active` after the radio had
  been shut down.
- Scan reports no longer flood the log. A busy room produced thousands of
  lines and pushed anything useful out of the buffer within seconds; a phone
  whose scanner returns nothing at all now says so once per window instead of
  saying nothing.

## [0.5.0] - 2026-08-09

### Added

- The Dev tab answers "is it me or is it them" before you scroll. It now opens
  on a radio self-check — BLE enabled, scanning, advertising; Wi-Fi Aware
  supported, available, discovering — in a fixed order that never changes with
  the data. A fact the app genuinely cannot observe reads `unknown` rather than
  guessing `off`, because a radio that can't be read is reporting honestly, not
  failing.
- Tapping a peer expands it in place onto why a connection failed: the BLE role
  this device chose, how long discovery took, how many sends were dropped, the
  signal strength, and the recent connect attempts with their outcomes and
  timestamps. No debugger, no leaving the list. A peer with nothing recorded
  says so plainly instead of showing a fabricated history.
- That attempt history survives a force-stop. It is written as one JSON record
  per line, so a truncated or damaged file costs the damaged lines and not the
  whole history, and a file that mostly fails to parse is copied aside before
  anything is rewritten rather than being replaced with a shorter one.
- Pending pair requests and your own identity — the npub peers address you by,
  and the Circle name they see you as — are now on screen.
- After a peer shares an app with you, Myco offers to put it on your home
  screen once the download actually finishes. Not while it is still
  transferring, because an icon for an app that never arrived is worse than no
  icon; and only once per app, so declining is respected.
- A link can now point at a place *inside* an app, not just at the app:
  `myco://app/<host>/<path>`. Follow one for an app you don't have and Myco
  fetches it from whoever nearby is carrying it, then opens it on the spot the
  link named — five seconds later if a peer is in the room, or after a reboot
  next week if nobody was. Opening the app yourself from the Apps grid spends
  the link just the same, so the first time you see that app is the time you
  land where you were sent. Deep links deliberately carry no pairing secret:
  they travel through channels nobody controls, so anything inside one is
  public and replayable. Pairing keeps its own face-to-face carrier.
- Apps can serve their own routes. A path an app's manifest doesn't list now
  gets the app's shell instead of a 404, so client-side routing works —
  bounded to navigation-style paths, because answering a missing script with a
  page would turn a broken asset into a silent one. An app that ships its own
  `404.html` still owns that answer.
- Dumplings joins bitchat and ICS in Discover's suggested apps — save a link,
  hand it to whoever is next to you, and it arrives as something they can
  choose to keep.
- A first-run intro. A spark appears, mycelial filaments grow out of it into
  the Myco mark, and the ring closes around them; the mark then breathes while
  it waits. Tapping anywhere opens a pupil in the middle of it, which contracts
  and dilates the way a real one does before the camera falls into it and the
  app is there. The pupil is a hole rather than a black disc, so the app itself
  shows through it: frosted at first, clearing as the dive starts. It plays in
  full on first launch only; later launches take a shorter path straight into
  the dive, and Settings has a developer control to play it again. The mark is
  generated at runtime rather than shipped as an asset — one quadrant of
  branching filaments drawn four times, which is where the logo's fourfold
  symmetry comes from. Geometry is covered by unit tests that run in CI.

### Changed

- Shared nsites keep the status bar by default. Most nsites are ordinary pages
  written for a browser that supplies its own top chrome, and drawing them
  full-bleed put their header underneath the Android clock and battery icons. A
  page that wants the full height opts in with `viewport-fit=cover`, which is
  already the standard way a page says it handles safe areas itself.

### Fixed

- Peers that are not direct neighbours are reachable again. Resolving a
  `<npub>.fips` name is what teaches the mesh node that peer's identity, and
  that step had been silently doing nothing since it was introduced: Myco
  answers `.fips` itself in the tunnel, but left the mesh node's own DNS
  responder switched on as well, and starting it discarded the channel the
  answer travels back on. The name still resolved, so the failure surfaced only
  on the first packet, as "no route" — which read like a distance problem
  because a direct neighbour's identity comes from the connection handshake and
  never needed resolving. Anyone further away was unreachable no matter how
  good the mesh path was.
- Opening the Discover tab no longer downloads and pins every app in it. The
  report was that tapping one app added all of them; the tap turned out to be
  incidental — simply viewing the tab did it, because fetching each tile's icon
  started a full sync for that site, and a completed sync adds the app to your
  library. Icon previews are now served from what is already on the device and
  never start a download.
- Wi-Fi Aware is on out of the box. It is a peering transport, and a lane
  nobody switches on is a lane that silently never carries anyone.
- The QR scanner keeps focusing. It focused once when the camera opened and
  never again, so a code moved closer or further away stayed blurred until you
  left the screen and came back.
- A peer that changes its Bluetooth address — which phones do routinely, for
  privacy — is recognised as the same peer instead of appearing as a stranger
  each time. Previously every change looked like a brand-new device dialling
  in, and with a connection limit of seven those duplicates could crowd out
  peers you were actually talking to.

## [0.4.2] - 2026-08-04

### Added

- System-aware AMOLED dark mode. Myco now follows the Android system theme and
  uses pure black (`#000000`) for dark backgrounds, surfaces, elevated
  containers, and the launch-window handoff — easier on the eyes and on an
  OLED battery. Fixed light colours were replaced with Material 3 semantic
  roles throughout, so both themes stay legible: emerald remains the brand
  accent, and pending and warning states keep their own distinct amber.
  Edge-to-edge system-bar icons adapt to whichever theme is active. The QR
  card deliberately stays white, because scanners are more reliable against
  it. Covered by theme palette unit tests that run in CI.

## [0.4.1] - 2026-07-29

### Fixed

- Circle members are reachable at any distance, not just as direct neighbours.
  Myco decided for itself who was reachable by intersecting your Circle with
  the mesh node's directly-connected peers, so a member two hops away was
  treated as offline: their nsites never appeared under "around me" and pulls
  skipped them. Chat was unaffected — it already targeted the whole Circle.
- Peers are addressed by name (`<npub>.fips`) everywhere rather than by their
  mesh address. Resolving the name is what registers a peer's identity with
  the node, so dialling the raw address only ever worked for someone already
  a direct neighbour — which is why this looked like a distance problem.
- The reachable count in the status pill reflects peers we hold a live mesh
  connection to, at any hop count, instead of only adjacent ones.
- `.fips` names resolve reliably. The tunnel had listed the network's real
  resolvers alongside its own, and any of them will deny a `.fips` name
  authoritatively, so whether a mesh name resolved depended on which resolver
  the system happened to pick. Myco's resolver now answers every lookup,
  relaying non-mesh names to a real one.
- Turning Bluetooth on no longer takes the mesh down. Starting the Bluetooth
  radio rebuilt the embedded mesh node, dropping every peer and session — so
  enabling one transport interrupted the others until everything re-handshook.
- Peering over a Wi-Fi AP no longer flaps. Myco re-announced peers it was
  already connected to and treated a lapsed mDNS advert as a departure, each
  of which tore down a healthy session every few minutes.

- Peering over a Wi-Fi access point stops dropping and re-forming every couple
  of minutes. Myco tried a node's advertised addresses faster than a failed
  attempt takes to expire, so several were live at once and whichever finished
  last replaced the connection that had already succeeded. It also tried them
  in the wrong order — the address on the network you actually joined is the
  one certain to reach the node, and it was tried last. Connecting to an access
  point now takes under a second instead of a minute and a half.
- The same app no longer appears several times under Discover, once per Circle
  member hosting it, and apps you have already pinned or that are already
  offered under Suggested are left out of "Around you".
- Sharing an app with someone already in your Circle no longer sends them
  another invite to accept.
- Bumping two phones that cannot reach each other over the mesh yet no longer
  loses the invite silently, and bumping again no longer queues a second one.

### Added

- A peers overview at the top of the Developer screen: who is connected, over
  which lane (Wi-Fi Aware / LAN / Bluetooth), and for how long.
- The status pill's peer dot now shows how much mesh you have rather than just
  whether you have any: red and pulsing with no peers, amber with one (working,
  but nothing to fall back on), green with two or more.
- Invites you have sent appear on the Circle screen under "Invited", and can be
  cancelled — which is also how you re-invite someone who never accepted.

### Changed

- Requests to join your Circle now appear on the Circle screen itself, under
  "Waiting to join", instead of behind a banner leading to a separate screen.

## [0.4.0] - 2026-07-29

### Added

- **`<npub>.fips` addresses now resolve for every app on the device**, not just
  inside Myco. The mesh tunnel advertises an in-mesh resolver that answers
  `<npub>.fips` from the public key alone — no network, no lookup — so any
  browser or app can open `http://<npub>.fips/` and reach that node over the
  mesh. Previously only Myco's own gateway could address mesh content by name.
- An **exit-node mode** (developer preview): point Myco at an HTTP proxy running
  on a mesh node and every proxy-aware app's web traffic egresses through it, so
  a phone with no internet of its own can browse the web over the mesh. The exit
  is named by npub (`<npub>.fips:8080`), so it does not have to be a direct peer
  — FIPS forwards multi-hop. Set it under Settings → Developer → Exit node; see
  [docs/how-to/exit-node-demo.md](docs/how-to/exit-node-demo.md). `.fips` names
  bypass the proxy and stay on the mesh.

### Fixed

- The **Wi-Fi AP lane now connects reliably** on a phone that also has mobile
  data. A local-only AP never passes internet validation, so the OS steered the
  mesh socket to the validated network and the peer's replies were discarded
  before reaching us — the node received every handshake while the phone saw
  nothing. The socket is now bound to the Wi-Fi network explicitly.
- The AP lane also **dials the right address**. A fips node advertises one
  address per interface and only the one facing us answers; Myco took the first
  and could sit retrying an unreachable one. It now keeps every advertised
  address and rotates through them until the peer connects.

### Known issues

- Peering over the AP lane can stall after the phone's Wi-Fi reconnects, until
  the node's old peer entry expires (roughly a minute). Phones that rotate their
  Wi-Fi MAC per connection — GrapheneOS by default — hit this most often, since
  the phone's mesh-facing address changes each time. Tracked upstream at
  [fips#130](https://github.com/jmcorgan/fips/issues/130).
- Exit-node mode only covers proxy-aware apps (browsers). Other apps, and
  QUIC/UDP traffic, continue to use the phone's normal connection.

## [0.3.0] - 2026-07-25

### Added

- Settings now warns — with a red dot on the Settings tab — when a transport
  is enabled but can't actually run: mesh on without the VPN slot (another
  VPN app took it), Bluetooth transport on while the phone's Bluetooth is
  off, or Wi-Fi Aware on while Wi-Fi is off. Each warning is tappable and
  jumps to the fix.
- The top-right status pill now carries a mesh on/off slider, shows how many
  Circle members are reachable right now (`reachable/total`), and the live
  peer count.
- A **Wi-Fi AP lane** (developer preview): when the phone joins a Wi-Fi network
  that carries a FIPS node — such as a router broadcasting the open `!FIPS`
  access SSID — Myco discovers the node via its mDNS advert (`_fips._udp`) and
  connects to it over UDP automatically. Requires LAN discovery/rendezvous to
  be enabled on the router's fips node. The Developer screen gains a
  **Wi-Fi AP** panel (Wi-Fi/SSID state, mDNS browse state, discovered nodes),
  and the Wi-Fi Aware panel now lists live data paths. See
  [docs/design/fips/ap-lane.md](docs/design/fips/ap-lane.md).

### Fixed

- Crash on GrapheneOS / secondary (non-admin) users: the system can refuse
  Wi-Fi Aware calls for lack of the nearby-devices permission even after the
  app's own permission check passed, and the resulting `SecurityException`
  on the Aware callback thread killed the whole app. The lane now shuts down
  gracefully and surfaces a warning instead.
- Enabling the mesh right after granting VPN access (e.g. when Myco reclaims
  the VPN slot from another app) no longer silently fails when the mesh
  address isn't ready yet — the VPN start now retries until the node has
  published its address. Declining the VPN consent dialog now turns the mesh
  preference off instead of pretending the mesh is up.

- Background battery drain cut substantially: BLE discovery now duty-cycles
  down (low-power scan with batched delivery) while the app is not visible,
  the per-link GATT connection priority drops to balanced after 30s without
  bulk traffic, and the once-a-second state poll no longer runs backgrounded
  (and no longer walks the blob cache directory on every read).
- Circle relay links no longer die permanently after a mesh session gets
  stuck mid-rekey: peer relay dials now time out at 10s and back off per
  peer (8s up to 3min) after consecutive failures, letting the node reclaim
  the stale session and rebuild a fresh one on the next dial.
- Turning the Bluetooth toggle off no longer stops the embedded mesh node
  out from under the Wi-Fi Aware lane — the node's lifecycle now follows
  the mesh "Enable" switch; radio toggles only gate their radios.
- Developer panel peer/advert rows keep a stable alphabetical order instead
  of reshuffling every refresh.

## [0.2.0] - 2026-07-14

### Added

- An experimental **Wi-Fi Aware** transfer lane that runs alongside the Bluetooth
  mesh. When two nearby devices both support it, larger transfers (such as nsite
  blobs) can ride a faster Wi-Fi data path instead of BLE, while pairing and
  discovery stay on the existing mesh. Experimental — see the Wi-Fi Aware section
  in Settings.
- A peer speedtest in the Developer menu that measures upload and download
  throughput to a paired peer over the mesh, for diagnosing slow transfers.
- The Discover tab now shows apps as an icon grid, with a **Suggested** row of
  starter apps (bitchat and ICS, an Incident Command System app for disaster
  response) above the nsites your Circle is hosting. Tapping any app opens it
  just like opening a shared one — it starts syncing and shows its live page.

### Changed

- The in-app Blossom store now accepts uploads up to 64 MiB, so larger nsite
  blobs and the new speedtest payload transfer in a single request.
- The embedded Nostr relay and Blossom server now listen on **4870** and
  **24243** — one above the previous `4869` / `24242`. This stops Myco from
  squatting on the ports a developer's own localhost relay or Blossom may
  already use. The localhost and mesh binds share the same port number, so both
  moved together and peer sync is unaffected. Temporary until the ports become
  configurable. The experimental Wi-Fi Aware lane moves to **4871** so it no
  longer shares 4870 with the relay.

### Fixed

- Chat and other mesh events could silently stop reaching a paired peer after a
  Bluetooth link dropped and came back. The reused relay connection went stale —
  a half-open socket the app never noticed — and quietly swallowed every message
  while the mesh still looked healthy. Each peer now holds a single persistent,
  two-way relay connection that detects a dead link (read side + keepalive) and
  reconnects, and manifest fetches share that one connection instead of opening a
  second socket per peer.
- Bluetooth peer discovery could stop for good after a burst of
  connects/disconnects and stay stuck at zero peers until you toggled the mesh
  off and on. Android throttles BLE scanning (~5 scan starts per 30s); a
  throttled scan was logged and then abandoned. The scanner now re-arms itself on
  a backoff — waiting out the throttle window — and recovers discovery on its own.
- Chat only reached Circle members you were *directly* connected to. Once two
  paired people moved apart and became several hops apart over the mesh, their
  messages stopped flowing — even though the mesh could still route between them.
  Chat now fans out to your whole Circle, so a paired peer keeps receiving your
  messages wherever they are on the mesh, not just when they're a direct neighbour.
- When a device in the middle of a mesh chain dropped and reconnected, the relay
  links between Circle members restored slowly and often only one-way, so messages
  stalled or flowed in a single direction for up to a minute. Each device now
  proactively keeps a live relay connection to every Circle member and re-establishes
  it within seconds of a flap — both directions — and on reconnect it recreates the
  app's open subscriptions against the returned peer to pull back anything missed,
  wherever that peer sits in the mesh.

## [0.1.0] - 2026-06-30

### Added

- Share an app by tapping phones. The share sheet now presents its
  `myco://share` code over NFC, so a bump opens the app and pairs with the
  sharer — the same result as scanning its QR. Receiving a tapped share also
  works from the new *Add an app* sheet.
- A **Storage** settings page with a usage gauge and two deletes: *Delete
  cache* reclaims space while keeping your pinned apps working offline, and
  *Delete all data, including apps* wipes the local relay + Blossom entirely.
  Your identity and Circle survive both.
- A peer **speedtest** in the Dev diagnostics tab: round-trips a small payload
  through a connected, paired peer's mesh Blossom and reports up/down
  throughput, so you can sanity-check a BLE link's speed.

### Changed

- Settings is reorganised into focused pages. Everyday controls stay up front
  (your device-name identity, storage, and the mesh with Bluetooth as a
  sub-toggle); the mesh-only switch and the raw identity fields (npub /
  node_addr / .fips / mesh ULA) move behind a developer-only page.
- The Circle *Nearby* list is always shown — with a hint when no one's around —
  and is sorted by name, so bubbles no longer reshuffle as Bluetooth signal
  strength fluctuates.

- The "Share this app" surface is now a bottom sheet styled like the pairing
  QR — a larger code and a prominent "tap phones together" prompt — and it
  closes itself once the recipient pairs.
- *Add an app* is now a bottom sheet: a live camera scanner, a paste-a-link
  button, and a tap-a-friend's-phone option, replacing the full-screen add
  view.
- Tapping or long-pressing someone in your Circle opens an action sheet
  (avatar, short npub, "Remove from circle") instead of a bare "Forget?"
  dialog; removal stays the last, destructive, confirmed action.

### Fixed

- Bluetooth links are far more reliable. The L2CAP reader and writer assumed
  each socket read returned exactly one whole mesh packet (and added their own
  length framing on top), but `BluetoothSocket` is a byte stream with no packet
  boundaries — so fragmented reads were shipped up as runt packets and coalesced
  reads were truncated, dropping data and thrashing the link. The radio is now a
  transparent, in-order byte pipe; the embedded core recovers packet boundaries
  from the mesh framing header (the same length-prefixed framer the IP transport
  uses), and a dropped inbound chunk now resets the link instead of silently
  corrupting the rest of the connection.
- The main app no longer flips to landscape on a slight tilt — it's locked to
  portrait, matching the QR scanner.

## [0.0.3] - 2026-06-29

### Added

#### Pairing & Circle

- NFC tap-to-pair. While the Circle tab is open the device emulates a
  standard NDEF Type-4 tag (host card emulation) whose URI record is a
  `myco://pair` link; the other phone's OS reads it via tag dispatch and
  hands the link back to the app — no NFC reader mode on either side.
  Both phones present and poll at once, so a single bump pairs
  symmetrically and both show "You're connected". Falls back to QR/paste,
  and warns (with a shortcut to system NFC settings) when NFC is off.
- Single-use pair secrets. Each shown/emulated code carries a fresh
  high-entropy secret that is consumed on first accept and rotated after
  every tap, so a captured or replayed code can't pair twice.
- Persistent Requests inbox (badged on the Circle tab). A tap auto-accepts
  only while you're on the Circle tab; a request that arrives while you're
  elsewhere prompts accept/ignore instead of pairing silently.
- Editable device name — a memorable colour + name (e.g. "green sammy"),
  shown to peers when pairing and editable from the Circle tab.
- Unpair on forget: forgetting a peer who is online now signals them
  (`kind 9103`) to drop you from their Circle too, keeping both sides
  symmetric.

#### App shell

- Developer-mode setting that gates the Dev diagnostics tab — on by
  default for debug builds, off for release.

### Changed

#### Pairing & Circle

- The separate "Add to circle" screen is merged into the Circle tab as a
  single view: a tap-to-connect (NFC) item with a subtle animated icon,
  **Nearby** people and your **Circle** shown as avatar bubbles (a green
  ring marks who's online), and a QR bubble (bottom-right) that opens
  scan / show / paste.

### Fixed

#### Pairing

- Outgoing pair requests and accepts now carry the user's chosen device
  name; previously the core always sent an npub-derived placeholder, so a
  renamed device still showed up under its old generated name.

## [0.0.2] - 2026-06-27

### Fixed

#### Bluetooth

- The throughput-boost GATT connection (opened alongside each L2CAP
  channel purely to request a high-priority connection interval and the
  2M PHY) no longer triggers Android's "<device> wants to access your
  messages" system dialog. The 3-argument `connectGatt` defaulted to
  `TRANSPORT_AUTO`, which on a dual-mode peer can bring up a classic
  BR/EDR link; BR/EDR between two phones makes Android auto-negotiate the
  MAP/PBAP profiles and prompt for message access. The GATT is now pinned
  to `TRANSPORT_LE`, matching the LE-only mesh data path, so no bond or
  classic profile is ever negotiated.

#### nsite rendering

- A chrome-less nsite's bottom content (e.g. the myco-bitchat chat
  composer) no longer hides behind the system navigation bar on devices
  with a 3-button bar, nor behind the soft keyboard when it opens. The
  fullscreen WebView is drawn edge-to-edge and pages are expected to pad
  via `env(safe-area-inset-bottom)` / `interactive-widget`, but older
  Android WebViews map only display cutouts into env() and ignore
  `interactive-widget`/`visualViewport`. The WebView is now hosted in a
  container padded by the larger of the navigation-bar and IME insets,
  which shrinks the WebView's layout (and the page's CSS viewport) so
  bottom content clears the bar and rides above the keyboard on every
  WebView version (`adjustResize` makes the IME inset available on
  Android 10). The reserved strip matches the nsite background; the status
  bar stays full-bleed. Newer WebViews then see no occlusion, so their own
  inset/keyboard handling is a no-op.

## [0.0.1] - 2026-06-27

Initial release: an offline-first, peer-to-peer Android client for nsites
— self-contained web apps served straight from a local relay and Blossom
store, shared with people nearby over a Bluetooth LE mesh.

### Added

#### Mesh networking

- Bluetooth LE mesh via an embedded FIPS node, using L2CAP
  Connection-Oriented Channels (insecure CoC; minSdk 29). Peers are
  discovered over BLE advertising/scanning and auto-connected; each link
  requests a high-priority connection interval and the 2M PHY for
  throughput.
- App-owned TUN over Android's `VpnService` so the device reaches the
  mesh's IPv6 ULA space without a system TUN. On by default; mesh-only
  ("no IP fallback") is an opt-in setting.

#### nsites — host and browse

- Embedded NIP-01 relay + Blossom blob store + gateway that serve an
  nsite's signed manifest and blobs from local storage, over both the
  mesh (`ws://<npub>.fips`) and in-app loopback.
- Fullscreen, chrome-less per-nsite WebView — each nsite opens as its own
  Recents task, served from the in-process gateway with no toolbar,
  TUN-independent.
- IP online-fallback: a pasted nsite link is fetched over normal internet
  (public relays + Blossom) when no mesh holder has it yet.
- nsite update checks with staged activation — a new version is
  discovered and downloaded, then activated atomically.

#### Pairing, Circle, and sharing

- Mutual pairing over the mesh by scanning a peer's QR: a signed pair
  request is dialed point-to-point to their mesh relay, and only a mutual
  accept adds both sides to the Circle. Relay/Blossom mesh access is
  restricted to paired (Circle) peers.
- Share an nsite via QR or a `myco://` deep link; the recipient pairs and
  pulls the app from the sharer over the mesh.
- Pin any nsite to the home screen as an app-like shortcut (favicon +
  title), opening straight into its fullscreen view.
- myco-bitchat (built-in mesh chat) is seeded as a default app on first
  run, so a fresh device has something to open without pasting a link.

#### App shell and identity

- Bottom-navigation shell: Apps, Circle, Discover, Settings, and a Dev
  diagnostics screen.
- Device identity from a persisted nsec (the same key signs pairing
  events).
- Blue mycelium launcher icon and a black Myco splash; edge-to-edge
  system bars.
