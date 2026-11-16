# Extensions

An extension is something a device shares **proactively** in a
neighborhood, beyond what napplets publish there. Each one shares one kind
of information and only the files that belong to it — nothing else on the
phone.

Code: `myco-core/src/neighbor/extensions.rs`.

## The extensions

| Name | Scope | What is shared |
| --- | --- | --- |
| `napplet-updates` | NAP extension, any runtime | When this device downloads an update for a napplet it has installed, it pushes the manifest into the neighborhood and serves that version's files there |
| `nsite-updates` | Myco only | The same for nsites in the Library |
| `myco-release` | Myco only | The signed Myco release APK |

An update is an ordinary neighborhood event. A runtime that doesn't know
a Myco-only extension keeps and passes it on like any other event, and
does nothing else with it.

## Get and Share

Two switches per extension, per neighborhood, independent of each other
and of the peers:

| Switch | On means |
| --- | --- |
| **Get** | Updates that arrive in this neighborhood are applied here (fetched, verified, installed or carried) |
| **Share** | Updates this device holds are pushed into this neighborhood |

- Both on unless the user turns them off, including extensions a later
  build adds. Stored as the sets turned off: `policy.getOff` and
  `policy.shareOff` in `neighborhoods.json`.
- One-way is fine: get without sharing, or share without getting.
- Nothing is agreed with peers. A shared update travels the
  neighborhood's publish hops, and devices in between pass it on by their
  own **pass on** setting ([policy.md](policy.md)), whatever their
  switches.
- Shown on the neighborhood page as two switches covering all three
  kinds at once: **Install updates from people here** (Get) and **Send my
  updates to people here** (Share). The per-kind setting stays in the core.

## napplet-updates

Code: `myco-core/src/neighbor/updates.rs`.

**Pushing.** When an installed napplet is pinned at a newer version (the
update check, or a push), the device, in every neighborhood where it
shares `napplet-updates`:

1. puts the version's files (`index.html` and the icon) in the
   neighborhood's scope — read from the device's own store, not copied;
2. keeps the manifest there as a neighborhood event;
3. publishes it to every link carrying the neighborhood, with the
   neighborhood's publish hops.

**Receiving.** A napplet manifest is kept and passed on like any
neighborhood event. Where this device gets `napplet-updates`, then:

- **Installed here and newer:** the files are fetched from that
  neighborhood (its scope, then its links; see
  [data-plane.md](data-plane.md#blobs)), verified and pinned. Pinned, it is
  shared onward by the same push.
- **Not installed, or not newer:** nothing is fetched or passed on. The
  manifest stays in the neighborhood, so an app store reading it sees that
  someone around holds that napplet.

Only the files of versions this device pushed are reachable this way. If
the neighborhood can't supply them, the public servers are tried, unless
the phone is mesh-only.

## nsite-updates

The same, for nsites in the Library: an update activated here is shared
with every file its manifest names; a shared one is applied if the site is
in the Library and the version is newer than the active one.

## myco-release

A release is Zapstore's signed announcement (kind 30063) and file event
(kind 3063, naming the APK's sha256), plus the APK.

- **Sharing.** When the phone keeps a new release APK (from the installed
  APK, a neighborhood or the internet), it shares the file events with the
  APK in scope, then the announcement.
- **Receiving.** Events from the pinned publisher are kept in the phone's
  own relay, where the release check finds them. Nothing is downloaded
  then: the APK is fetched when the phone carries the update, from the
  neighborhoods with `myco-release` on first (`release_apk.rs`), then the
  internet when allowed.

## Getting an app that isn't here yet

Opening a Library nsite that isn't ready, opening or installing from a
share link or `naddr`, and installing a napplet all look in the same
places, in order (`ExtensionSource` in `neighbor/updates.rs`):

1. **This device's own store.**
2. **Neighborhoods where this device gets the matching extension**
   (`nsite-updates` for sites, `napplet-updates` for napplets). The
   manifest is looked up in each neighborhood's scope here, then asked of
   its links over the link relay. Files come from the scope, then the
   links ([data-plane.md](data-plane.md#blobs)).
3. **The public internet** (relays and Blossom servers), unless the phone
   is offline-only.

A neighborhood holds only what someone shared into it: a version a
neighbor updated to and shared. An app a neighbor merely has installed is
not in scope, so it is not found there.

Nobody outside a shared neighborhood is contacted. A share link
(`myco://share/…`) still opens the app, but its holder fields are not
dialled: installing from someone you aren't linked with needs the internet,
or a neighborhood you share with the extension on.

Library sites that aren't ready are retried in the background about every
30 s through the same places.

## Status

All three extensions are built. They are the only way Myco's apps, updates
and release APK travel between phones: Myco's own relay and Blossom are not
reachable from other phones ([../../reference/ports.md](../../reference/ports.md)).
