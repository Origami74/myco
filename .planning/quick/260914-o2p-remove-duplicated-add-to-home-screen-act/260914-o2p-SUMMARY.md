---
quick_id: 260914-o2p
status: complete
date: 2026-09-14
commit: 3c0bb29adb4ffb8a71c71994973501f1142dadc0
branch: feat/napplet-runtime
---

# Summary: remove duplicated "Add to Home screen" action in nsite AppSheet

Deleted the unconditional `SheetAction(Icons.Filled.Add, "Add to Home screen")`
that 3c9d0cd added to the nsite `AppSheet` above the existing
`site.state == "ready"` guard. One-line deletion in
`android/app/src/main/java/app/myco/ui/screens/AppsScreen.kt`.

Verified by grep: `"Add to Home screen"` now occurs twice in the file — once in
`AppSheet` (guarded), once in `NappletSheet`. No Gradle build run (Rust
cross-compile required; the change removes a duplicate of an identical call).

Root cause worth a follow-up: nsite and napplet sheets/tiles/share/forget paths
are copy-paste twins keyed on different types (`SiteStatus` vs `LibraryItem`)
rather than one `AppEntry` branching on `LibraryKind`; napplet work editing the
nsite sheet is how the duplicate got in.
