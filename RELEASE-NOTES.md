# Myco v0.7.0

**Released**: 2026-09-16

v0.7.0 turns Myco from an nsite viewer into an app runtime. Alongside nsites it
now runs **napplets** — single-file programs published on Nostr — each in its
own sandbox, with the capabilities it uses granted by you and implemented by
Myco. Underneath, the mesh learned to hold more than one link to a phone and
switch between them, and the relay moved to a real database.

**No wire-format change from v0.6.x.** A v0.7.0 phone and a v0.6.1 phone still
exchange apps and messages and pair. The new multi-path link messages are
ignored by an older phone, which keeps linking over a single path as before.
Everything on the device upgrades in place; the event store is migrated on
first open.

## At a glance

- **Napplets.** Add one by pasting an `naddr`, scanning a QR, or bumping a
  friend's phone — the friend's phone is asked first, so it arrives with no
  internet. Review what it asks for, and it lands on the Apps grid with a 🦆
  badge and its own full-screen window. Discover suggests three to start with,
  and DingDong comes preinstalled.
- **Permissions you can see and change.** The install sheet lists everything a
  napplet will be able to do before you agree. Hold a tile → Manage permissions
  to switch each capability off or on; a change is live. An update that asks for
  more than you were shown goes back through the sheet first.
- **Every link to a peer, not just one.** The status panel behind the peers pill
  shows a phone on Bluetooth and Wi-Fi at once, the active lane lit and the
  standbys faded, and the mesh moves traffic between them without re-pairing.
- **Same-Wi-Fi discovery has a switch,** and peers that dropped off the network
  come back on their own.
- **Files that get there.** A transfer no longer sticks when a control message
  is lost, and a large file over Bluetooth no longer fails part-way.
- **A crashed page cannot take Myco down.** A window whose renderer dies closes
  on its own; the mesh, the relay and your other windows keep running.

## Napplets

An nsite is a website Myco serves from the mesh. A napplet is a program: one
HTML file, signed by its author, that asks the phone running it for things —
who you are, what your relays hold, what is on the phones around you, a picture
by its hash. NIP-5D defines the shape; Myco implements the runtime.

Each napplet runs in a sandboxed frame inside a trusted page Myco ships. It has
no network of its own: no fetch, no WebSocket, no storage. Everything it does
goes through a message channel to Myco, which checks the request against what
you granted and does the work on the napplet's behalf. This release implements
five capability families, in the words the install sheet uses:

- **Identity** — a user key, separate from the mesh device key, created the first
  time a napplet opens. Napplets learn who you are socially; the device is never
  named in anything signed by that key.
- **Relays** — read and post as you on the relay pool: this phone's relay and the
  public relays when reachable. Posting excludes your profile, contacts, relay
  list and deletions for now; a napplet that tries gets a refusal, not a silent
  drop.
- **Outbox** — an author's notes from the relays they publish to, whether that is
  a phone across the room or a public relay, with an honest `incomplete` when a
  relay never answered.
- **Mesh** — Myco's own: publish to everyone nearby with a chosen hop count and
  pull what was missed. Settings › App reach caps how far apps may send and look.
- **Pictures and files** — by content hash: this phone first, then a friend's
  phone over the mesh, then the public servers. What is fetched is kept for the
  next app and the next phone in the room.

Versions are pinned to bytes: the napplet you open is always the one whose file
is on your phone, and a newer manifest with nothing behind it cannot take an
app off the air. "Check for updates" refreshes napplets beside the nsite check.

### What was reviewed

The runtime went through two adversarial reviews before this release, the
second with a dedicated look at how a napplet might break out of its sandbox.
Every finding is fixed in this build. The sandbox itself held; what needed work
was around it — a napplet could have pointed one of Myco's own mesh connections
at an internet host through a crafted relay address, reached the phone's local
relay on the loopback interface, or crashed the whole app by exhausting its
page. All three are closed, along with the rest of the list. The design doc now
carries an explicit list of what remains policy rather than enforcement, so it
is visible rather than implied.

## The mesh holds more than one link

A phone next to you is usually reachable more than one way — Bluetooth and
Wi-Fi Aware, or Bluetooth and the local network. Until now Myco used one and
forgot the other, and when that one failed the peer was gone until it was found
again from scratch.

The mesh now keeps every path to a peer, probes the standbys so they are known
to work before they are needed, and switches when the active one degrades. The
status panel shows this directly: one icon per lane, the lit one carrying
traffic. Two connected phones stay connected through a Bluetooth hiccup or a
Wi-Fi Aware teardown without re-pairing or re-discovering.

This is the first release built on fips's multi-path branch. The link messages
it adds are ignored by older nodes, so mixed rooms keep working.

## Storage

The relay Myco keeps inside the app is now an LMDB database. Queries are
indexed, writes are one small transaction per event, and the store is ready for
the mesh sync that comes next. Chat and other expiring messages stay in memory
and never touch disk, as before. A store from an earlier version is migrated on
first open; if any event fails to migrate, the old file is kept and nothing is
lost.

## Known issues

- **A phone in your pocket finds nobody.** Myco winds its radios down when it is
  not on screen, so two idle phones in a room will not discover each other until
  one is opened.
- **Wi-Fi Aware is shut off entirely by deep Doze** on Android 13 and later after
  a long idle period —
  [#30](https://github.com/Origami74/myco/issues/30).
- **A napplet's relay access is all-or-nothing.** The `relay` grant lets a napplet
  read everything your relay holds, including what other napplets stored. A
  finer permission model is next on the roadmap.
- **A napplet may name its own relays.** With the outbox grant it can ask Myco to
  talk to a relay it chooses; that is by the specification, and it is an
  exfiltration channel. Review the install sheet.
- **Napplets do not replicate over the mesh yet.** A friend's phone can hand you
  one at install time; keeping them in sync afterwards is roadmap.
- Phones still do not always connect to every peer around them.
- The interface can lag while the mesh is syncing.
- Exit-node mode still covers proxy-aware apps only; other apps and QUIC/UDP
  traffic keep using the phone's normal connection.

## Getting it

- **Android**: install the APK from the
  [v0.7.0 release](https://github.com/Origami74/myco/releases/tag/v0.7.0),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a checkout of
  the v0.7.0 tag, with fips on its `feat/multi-path-switchover` branch. See
  [CONTRIBUTING.md](https://github.com/Origami74/myco/blob/main/CONTRIBUTING.md)
  for build prerequisites.

Phones do not need updating together, but the multi-path link only forms
between two phones on v0.7.0.

The full per-release change history lives in
[CHANGELOG.md](https://github.com/Origami74/myco/blob/main/CHANGELOG.md).
Issues and discussion at [github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to the napplet authors whose apps were the test bed — Mapplets found
two runtime gaps on the first run — and to
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
