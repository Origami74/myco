# Myco v0.0.3

**Released**: 2026-06-29 (provisional)

v0.0.3 is the pairing release. It makes connecting with people the
headline flow: tap two phones together to pair over NFC, and find,
add, and manage your Circle from a single screen. The separate
"Add to circle" view is gone — nearby people, your circle, and every
way to connect now live on the Circle tab.

It interoperates with v0.0.2 devices: an older peer still pairs over
QR/the mesh and is simply unaware of the new NFC path, the device-name
field, and the unpair signal. No coordinated upgrade is needed.

## At a glance

- NFC tap-to-pair — bump two phones to connect, with single-use codes.
- "Add to circle" folded into the Circle tab as one bubble-based view.
- A persistent Requests inbox; taps auto-accept only while you're on
  the Circle tab, otherwise they prompt.
- An editable, memorable device name that peers actually see.
- Forgetting an online peer now disconnects both sides.
- A developer-mode setting that hides the Dev tab on release builds.

## What's new

### NFC tap-to-pair

While the Circle tab is open, the phone emulates a standard NDEF
Type-4 tag (host card emulation) whose URI record is a `myco://pair`
link. The other phone's OS reads it through normal tag dispatch and
hands the link back to the app — neither side needs an NFC "reader
mode." Both phones present and poll at the same time, so a single bump
pairs symmetrically and both land on "You're connected."

Every emulated (or shown) code carries a fresh, high-entropy secret
that is **single-use**: it's consumed on the first accept and rotated
after every tap, so a captured or replayed code can't pair twice. If
NFC is off, the tap-to-connect item turns into a warning with a
shortcut to the system NFC settings; QR and paste remain as fallbacks.

### One Circle screen

The Circle tab is now the only place you add people. Top to bottom: a
tap-to-connect item with a subtle animated icon; **Nearby** people and
your **Circle** rendered as avatar bubbles (a green ring marks who's
online); and a QR bubble in the corner that opens scan / show / paste.
Your editable name sits at the top so you always know how you appear to
others.

### Requests inbox

Incoming pair requests collect in a persistent inbox, badged on the
Circle tab. A request **auto-accepts only while you're sitting on the
Circle tab** (actively presenting). If one arrives while you're on
another tab — or Myco was just launched by the tap — you get an
accept/ignore prompt instead of pairing silently.

### Editable device name

Your device now has a memorable colour + name (for example
"green sammy"), shown to peers when you pair and editable from the
Circle tab. The chosen name is pushed into the core so it rides along
in the pairing handshake.

### Unpair on forget

Forgetting a peer who is currently online now signals them
(`kind 9103`) to drop you from their Circle too, so the two sides stay
symmetric instead of one end silently keeping a stale entry.

### Developer mode

A new Settings toggle gates the Dev diagnostics tab — on by default for
debug builds, off for release.

## Behavior changes worth flagging

- **Card emulation is scoped to the Circle tab.** The phone only
  advertises itself over NFC while the Circle tab is open, never from
  Apps / Discover / Settings.
- **Auto-accept is intentional, not ambient.** Only a tap that lands
  while you're already on the Circle tab auto-accepts; everything else
  prompts.
- **Pair codes are single-use.** A code is consumed on first use and
  rotated, so re-presenting the same screen mints a new one.

## Notable bug fixes

- **Pair events carry your chosen name.** Outgoing pair requests and
  accepts now include the user's device name; previously the core
  always sent an npub-derived placeholder, so a renamed device still
  showed up under its old generated name on the other side.

## Getting it

- **Android**: install the v0.0.3 APK from the
  [release page](https://github.com/Origami74/myco/releases/tag/v0.0.3),
  or via [zapstore](https://zapstore.dev/apps/app.myco).
- **From source**: `cd android && ./gradlew assembleDebug` from a
  checkout of the v0.0.3 tag builds the app and cross-compiles the Rust
  core into its `jniLibs` (needs the Android SDK + NDK and the Rust
  Android targets). See [CONTRIBUTING.md](CONTRIBUTING.md) for the full
  build prerequisites.

The full per-release change history lives in
[CHANGELOG.md](CHANGELOG.md). Issues and discussion at
[github.com/Origami74/myco](https://github.com/Origami74/myco).

## Contributors

Thanks to everyone who contributed code, design, testing, or bug
reports to this release — and to [@Origami74](https://github.com/Origami74)
for maintaining the project.
