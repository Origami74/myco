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

Runtimes that don't know a Myco-only extension never receive it: see
*Agreement* below.

## Switches

- One switch per extension, per neighborhood. On unless the user turns it
  off, including extensions a later build adds.
- Stored as the set turned off: `policy.extensionsOff` in
  `neighborhoods.json`.
- Shown on the neighborhood page under **Share updates**: App updates, Site
  updates, Myco updates.

## Agreement

Each side lists the extensions it has on, per neighborhood, in its signed
offer (kind 37516):

```json
{ "services": [ … ], "extensions": { "<neighborhood id hex>": ["napplet-updates", "myco-release"] } }
```

Something is shared over a link only when **both** ends have that
extension on in that neighborhood. An offer without `extensions` (an older
build) counts as none on. A peer learns a change at its next presence check
(every 30 s), when it fetches the offer again.

## napplet-updates

Code: `myco-core/src/neighbor/updates.rs`.

**Pushing.** When an installed napplet is pinned at a newer version (the
update check, or a push), the device, in every neighborhood with
`napplet-updates` on:

1. puts the version's files (`index.html` and the icon) in the
   neighborhood's scope — read from the device's own store, not copied;
2. keeps the manifest there as a neighborhood event;
3. sends it, with no hops left, to each link where the peer has the
   extension on too.

**Receiving.** A napplet manifest from such a link is kept in the
neighborhood like any event. Then:

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
  APK, a neighbor, an app peer or the internet), it shares the file events
  with the APK in scope, then the announcement.
- **Receiving.** Events from the pinned publisher are kept in the phone's
  own relay, where the release check finds them. Nothing is downloaded
  then: the APK is fetched when the phone carries the update, from the
  neighborhoods with `myco-release` on first (`release_apk.rs`,
  `from_neighborhoods`), then app peers, then the internet.

## Status

All three extensions are built. Myco's updates still also travel to app
peers over the mesh relay ([app-peers.md](app-peers.md)) until that path
is removed.
