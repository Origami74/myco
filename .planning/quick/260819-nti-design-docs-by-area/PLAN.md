---
task: Group docs/design by area (core / nsite / napplet / fips)
quick_id: 260819-nti
date: 2026-08-19
branch: next
---

# Group `docs/design/` by area

Sixteen design docs sat flat in one directory, mixing the system, the content
layer, the new app runtime, and the transport lanes.

## Mapping

| Folder | Docs |
|--------|------|
| `core/` | concepts, architecture, app-shell, deep-links, identity-pairing, event-gossip, security |
| `nsite/` | nsite-layer, propagation, nsite-updates, nsite-permissions |
| `napplet/` | napplet-runtime |
| `fips/` | ble-interop, wifi-aware-interop, ap-lane, usb-transport |

`diagrams/` and `mockups/` stay at `docs/design/` — both are referenced across areas.

## Scope

Move only. `git mv`, then rewrite every inbound link: relative links inside the
moved docs (all gained one level of depth), and `design/<name>.md` citations from
`docs/`, the Rust and Kotlin sources, `README.md`, `CHANGELOG.md`, `Cargo.toml`
and the `.planning/` tree. No prose edits, no status-banner additions.

## Deferred

`propagation.md` was proposed for deletion as stale ("proposal for a not-yet-built
app"). Held — see SUMMARY.

## Done when

Every doc is under an area folder, no link is broken by the move, and the design
index in `docs/README.md` is grouped to match.
