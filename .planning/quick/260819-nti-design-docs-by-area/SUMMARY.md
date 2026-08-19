---
task: Group docs/design by area (core / nsite / napplet / fips)
quick_id: 260819-nti
date: 2026-08-19
status: complete
commit: e667f8fc9bb4698355bcabbc34dcdc9a2d36461c
---

# Summary

16 docs moved into `core/` (7), `nsite/` (4), `fips/` (4), `napplet/` (1).
`diagrams/` and `mockups/` stayed shared. 39 files outside `docs/design/` had
citations rewritten — Rust and Kotlin sources, both READMEs, the changelog,
`Cargo.toml`, `AndroidManifest.xml`, and the `.planning/` tree.

Verified: zero links broken by the move. The dangling links that remain all
predate it, pointing into `reference/` checkouts that are not present locally
(nostr-vpn, site-deck, bitchat-android) or at fips sources that moved upstream.
One pre-existing broken link was fixed in passing (`ble-interop.md` cited
`./build.md`, which never existed at that path).

The design index in `docs/README.md` was regrouped into four tables matching the
folders, and gained rows for six docs it had never listed: deep-links,
event-gossip, nsite-updates, nsite-permissions, ap-lane, usb-transport.

## propagation.md — deletion held, not done

It was picked for deletion as stale, and its status banner does read
"proposal for a not-yet-built app. Voice is 'the app will…'".

But it turned out to be the most-cited document in the repository: **36 inbound
references**, many naming specific sections. `config.md` cites §5 for the TTL and
pull-on-demand model. `nostr-kinds.md` cites it three times for manifest flooding
and source discovery. `ports.md` cites it for propagation policy. `event-gossip.md`
opens by defining itself as propagation's sibling. `security.md` cites its open
question on cache-timing.

So the stale part is the voice, not the content — it is the only description of
propagation behaviour that shipped, and no other doc carries those sections.
Deleting it removes working documentation and orphans 36 citations rather than
retiring an obsolete design.

Held pending confirmation. If it should still go, the cheap path is a restatus
(`> Status: BUILT`, present tense) rather than a delete plus 36 repointed links.
