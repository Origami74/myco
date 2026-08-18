---
id: 260818-l65
slug: status-pill-panel
date: 2026-08-18
mode: quick
branch: feat/status-pill-panel
---

# Quick: status pill status panel

Make the top-right `PeersPill` a real status affordance: bigger, red when the mesh
is off, an easier switch to hit, and tappable to open a panel that answers "what is
my mesh actually doing right now" — per transport, per peer.

## Scope

**Rust (myco-core)**

1. `control_client.rs` — add `srtt_ms: Option<f64>` to `PeerView`, read from the
   `show_peers` row's `mmp.srtt_ms` (fips emits it only when MMP has a measurement,
   so absent stays `None`; never a fabricated 0).
2. `state.rs` — add `srtt_ms: Option<f64>` to `PeerDiagnosticView` (serialises as
   `srttMs`).
3. `peer_diagnostics.rs` — carry it through `merge_peers` from the matching
   `PeerView`; `None` on advert-only / circle-only rows.

**Kotlin (android)**

4. `AppCoreClient.kt` — parse `srttMs` onto `PeerDiagnostic`.
5. New `ui/TransportIcons.kt` — hoist `TransportIcon` out of `DevScreen` so both
   the Dev tab and the new panel draw the same three glyphs. DevScreen's private
   copy is deleted and it imports the shared one.
6. `MycoApp.kt` — `PeersPill`: larger metrics, `errorContainer` colouring when the
   mesh is off, a 48dp-tall switch hit target, and the rest of the pill clickable
   to open the panel.
7. New `ui/MeshStatusSheet.kt` — a `ModalBottomSheet` with two sections:
   - **Circle** — each paired contact, reachable now or not.
   - **Mesh** — one block per transport (`ble` / `aware` / network), each showing
     whether that lane is scanning right now (tri-state: unknown when the app
     could not observe it) and the peers currently carried on it, with ping
     (srtt), connected time, and last seen ("now" under 10s).

## Non-goals

- No new fips changes — `mmp.srtt_ms` is already on the control-socket wire.
- No change to how peers are discovered or dialled.

## Verification

- `cargo test -p myco-core`
- `cargo fmt --all --check` / `cargo clippy`
- Android: `./gradlew :app:compileDebugKotlin` (or assembleDebug)
