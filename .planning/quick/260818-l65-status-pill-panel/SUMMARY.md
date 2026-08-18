---
id: 260818-l65
slug: status-pill-panel
date: 2026-08-18
status: complete
branch: feat/status-pill-panel
commits:
  - c09103456ed5625fc640c4a648672181235919f5
  - 38019e251ffbc6a78137185be5087298a834065b
---

# Quick: status pill status panel — complete

## What changed

**Rust — the ping was already on the wire, Myco was dropping it.**
fips's `show_peers` row carries an inline `mmp` block whose `srtt_ms` is MMP's
smoothed RTT for that link. `control_client.rs` never read it. It now lands on
`PeerView.srtt_ms`, rides `merge_peers` onto `PeerDiagnosticView.srtt_ms`, and
serialises as `srttMs`. Optional the whole way: fips omits the key until MMP has
actually measured the link, and an unmeasured link has to read as "no ping"
rather than a confident `0ms`. `PeerView` drops its `Eq` derive (f64).

**Kotlin — the pill.**
- Grows (18dp icon, `titleSmall` counts, more padding).
- Whole surface goes `errorContainer` when the mesh is off, and the switch's own
  track goes `error` — "off" reads from across the room now.
- The switch's touch target is a 52×48dp box (was 36×20) — Material's minimum;
  the drawn slider is unchanged, since `scale` is a draw transform only.
- Tapping the counts opens the new panel.

**Kotlin — the panel (`MeshStatusSheet.kt`).**
A `ModalBottomSheet` in two sections:
- **Circle** — every paired contact, reachable-now or offline, with the link
  numbers when a direct peer row exists (a member reachable over several hops
  has none, and that is not a fault).
- **Mesh** — one block per lane (Bluetooth / Wi-Fi Aware / Network), each with
  its scan state and the peers it carries. Scan state is tri-state: `off`,
  `scanning`, `idle`, or `unknown` when the app could not observe it. The
  routed lane's "scanning" is the mDNS browse from `ApRadio`.
- Peer rows show `ping · up · seen`. Under ten seconds, last-seen is "now" — a
  1s/2s/3s counter flickering reads as a fault when it is the healthy case.
- The sheet runs its own 1Hz ticker: ages come from the wall clock, not the
  snapshot, so they would freeze whenever two polls compared equal.

**Shared `TransportIcon`** moved from `DevScreen` to `ui/TransportIcons.kt` (now
with a `size` parameter) so the Dev tab and the panel draw the same glyphs.

## Verification

- `cargo test -p myco-core` — pass, including two new tests (`srtt_ms` parsed
  from an observed row; absent `srtt_ms` and absent `mmp` both read as `None`)
  and one merge test (`srtt` rides the `PeerView`, `None` on a circle-only row).
- `cargo fmt --all --check` — clean.
- `./gradlew :app:compileDebugKotlin`, `:app:testDebugUnitTest` — pass.

## Not done

- No on-device check of the panel's numbers against a second phone yet.
