# UI

Neighborhoods are managed in native Compose, not in a napplet.

Code: `android/app/src/main/java/app/myco/neighbor/` and
`myco-core/src/neighbor/view.rs` (the `neighborhoods` field of the app
state). Actions are in `myco-core/src/action.rs`; see
[../../reference/ffi-surface.md](../../reference/ffi-surface.md).

## Tabs

Apps · Neighbors · Settings · Dev.

## Neighbors tab

- The mesh bar on top.
- **Here now:** peers reachable in the last two minutes.
- Neighborhood cards, with a list / map toggle.
- Pending additions from peers ("Alice added you to …"), with **Join** /
  **No thanks**.
- **Join** (scanner) and **Invite** at the bottom.

## A person

- What you share with them, with **Remove** per neighborhood.
- Add a neighborhood: they're asked first.
- **Disconnect**: stops sharing every neighborhood with them. What was
  received stays on the phone.

## Neighborhood page

- **Rename** (only on this phone), **Reach**, **Files** (blob push hops).
- **People** in it.
- **Posts reach:** Close / Friends of friends / Wide
  ([policy.md](policy.md#reach-presets)).
- **Install updates from people here** and **Send my updates to people here** switches, for apps, sites and Myco at once ([extensions.md](extensions.md)).
- **Leave** and **Invite**.

## Invite

- Pick one or more neighborhoods, or **+ New**.
- Shows a QR code, a share link and, while open, an NFC tag.
- Says how the other side will see you ("They'll see you as …").
- Waits for them to join.

## Join

- Scan a QR, tap phones, or open a `myco://link/…` link.
- Shows who is inviting and to which neighborhoods. Pick which to accept.
- A **Not private** note: people in a neighborhood can pass posts on.
