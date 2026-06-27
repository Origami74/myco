# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Fixed

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
