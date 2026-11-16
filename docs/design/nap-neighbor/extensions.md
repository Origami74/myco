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

## Status

The switches and the offer field are built. What each extension pushes is
being built next; until then Myco's own updates still travel to app peers
([app-peers.md](app-peers.md)), which are the devices sharing at least one
extension with us.
