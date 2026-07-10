# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Chat and other mesh events could silently stop reaching a paired peer after a
  Bluetooth link dropped and came back. The reused relay connection went stale —
  a half-open socket the app never noticed — and quietly swallowed every message
  while the mesh still looked healthy. Each peer now holds a single persistent,
  two-way relay connection that detects a dead link (read side + keepalive) and
  reconnects, and manifest fetches share that one connection instead of opening a
  second socket per peer.

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
