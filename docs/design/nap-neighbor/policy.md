# Policy per neighborhood

How far traffic travels, set per neighborhood. Myco settings, not NAP
input.

Code: `Policy` in `myco-core/src/neighbor/store.rs`, stored in
`neighborhoods.json`.

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| Publish hops | 3 | How far a post travels |
| Backlog hops | 2 | How far a history request travels |
| Pass on | on | Whether others' posts are passed on |
| Blob push hops | 0 | How far an upload is pushed (0: stays here) |
| Apps | on | Whether Myco fetches apps and updates from people here ([app-peers.md](app-peers.md)) |

## Reach presets

The neighborhood page shows reach as three presets:

| Posts reach | Publish / backlog hops | Pass on |
| --- | --- | --- |
| Close | 1 / 1 | off |
| Friends of friends | 2 / 1 | on |
| Wide (default) | 3 / 2 | on |

## Blob push hops

Blobs are heavy, so pushing is off by default. It is meant for
neighborhoods like the user's own devices. Links fetch blobs on
demand but push none yet, so the setting has no effect today.

## Mutes

Mutes are per author, per neighborhood. Muted authors' events are hidden
and not passed on.

## Default offer

A new neighborhood offers its own content both ways (relay read and write)
and blob reads. See [links.md](links.md#offers).
