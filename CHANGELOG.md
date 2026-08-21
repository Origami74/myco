# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Dev: join a FIPS Wi-Fi hotspot (`!FIPS`) as a second, local-only connection
  while staying on your normal internet Wi-Fi, so the phone can carry mesh
  traffic over the hotspot without giving up its internet. Under Dev → Wi-Fi
  AP. The system shows its usual join prompt.
- Phones on the same Wi-Fi now find each other over the network instead of
  Bluetooth. Myco announces itself on the local network the same way a fips
  node does, so two phones — or a phone and a desktop — on one Wi-Fi connect
  over UDP, which moves a file in seconds rather than minutes.
- Send a file straight to a paired phone. Share anything from another app, pick
  one of your paired phones, and it arrives encrypted over the mesh — no
  hotspot, no internet. The receiving phone is asked first and can say no, and
  the file lands in Downloads/Myco.
- Transfers appear on the Circle tab alongside pairing requests, so a send that
  is still waiting is visible from anywhere in the app and can be cancelled.
  An offer nobody answers gives up after ten minutes instead of waiting forever.

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
