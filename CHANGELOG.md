# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

### Changed

- Shared nsites keep the status bar by default. Most nsites are ordinary pages
  written for a browser that supplies its own top chrome, and drawing them
  full-bleed put their header underneath the Android clock and battery icons. A
  page that wants the full height opts in with `viewport-fit=cover`, which is
  already the standard way a page says it handles safe areas itself.

### Fixed

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
  [docs/design/ap-lane.md](docs/design/ap-lane.md).

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
