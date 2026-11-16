# App peers

Myco's own basics go to linked devices sharing an extension with us ([extensions.md](extensions.md)).

Code: `myco-core/src/neighbor/apps.rs`, `Content::set_app_peers` and
`AppPeerGate` in `myco-core/src/content.rs`.

## What rides on app peers

- Fetching an app a neighbor has.
- nsite and napplet manifest update forwarding
  ([../nsite/nsite-updates.md §4](../nsite/nsite-updates.md)).
- The release APK ([../core/software-updates.md](../core/software-updates.md)).
- File transfer.

## Who is an app peer

- Every linked device that shares at least one extension with us (on at both ends) in some neighborhood, once each.
- Identified by the device npub in its offer's `.fips` URL.
- Recomputed every 5 s, and at once on every join, leave or extension toggle.
- Turning every extension off in every shared neighborhood stops that device being an
  app peer.

## The gate

The mesh relay (`:4870`) and Blossom (`:24243`) admit **only app peers**
(`AppPeerGate`). Everyone else is refused. The link service on `:4873` is the
only port a stranger can reach.

## A stopgap

These basics are meant to move to napplets on NAP-NEIGHBOR. Until then they
use the mesh relay and Blossom directly, not the scoped data plane.
