---
id: 260809-nyq
status: complete
date: 2026-08-09
branch: feat/deep-links
commits:
  - fc29538312b672eeacfc903c5de94857271d0d71  # gateway SPA fallback
  - de9db3670fc98bb291e742fe51a0e24db8e088e3  # android deep links + deferred open
  - 0bbc70dd1364e90ec154c7218542d0aa453859ad  # docs + gitignore
  - 73f568527e1c8f22c41c41a73c6c735440f3fcc4  # myco-dumplings (its own repo)
---

# Myco deep linking + myco-dumplings — done

`myco://app/<host>/<path>` opens an app at a place inside it. If the app isn't
installed, Myco retrieves it and the path is spent on its **first** open — however
long that takes, across process death and reboots.

## What landed

| Scope | Where |
|-------|-------|
| Gateway SPA fallback | `nsite-deck/src/gateway.rs` — `resolve_hash`, 5 new tests |
| Link parser | `android/…/share/MycoLink.kt` + 9 JVM unit tests |
| Pending-link store | `android/…/share/PendingDeepLinks.kt` |
| Deep path + already-open nav | `android/…/NsiteActivity.kt` (`EXTRA_PATH`, `onNewIntent`) |
| Routing + reconciler | `android/…/MainActivity.kt` |
| Test app | `myco-dumplings/` (standalone repo) |
| Design doc | `docs/design/deep-links.md` |

## Decisions taken during execution

1. **The SPA fallback is limited to navigation-style paths.** The task said "on
   manifest miss, fall back to `/index.html`". Applied literally, a missing
   `/assets/app.js` would answer HTML with a 200 — a broken asset turned silent. The
   fallback now applies only where `normalize_path` produced a `…/index.html` (root,
   trailing slash, extensionless segment). Assets keep their 404.

2. **Query params are forwarded, not rejected.** The plan said to reject links with
   pairing/invite query params. Myco defines no query params at all, so there is
   nothing to reject — the rule is enforced by Myco never *reading* the query, and by
   attaching no holder/secret when it builds a link. The query rides along for the app.

3. **A foreground watcher was added alongside the resume reconciler.** Resume alone
   would miss the common case: tap a link, wait five seconds while a peer in the room
   serves the app, with Myco open the whole time. The watcher is bounded (3 min) and
   de-duplicated; the reconciler remains the durable half.

4. **`myco-dumplings` is its own git repo**, gitignored here like `myco-ics` and
   `myco-bitchat` (user's call, mid-execution — an earlier in-tree commit was rewound).
   Initial commit `73f5685` inside `myco-dumplings/`.

5. **Dumplings builds with `base: "/"`**, unlike the other nsites' `"./"`. Relative
   asset URLs resolve against the current route, so from `/invite/x` they 404. Any
   nsite wanting real routes needs this; documented in both READMEs.

## Verified

- `cargo test -p nsite-deck` — 22 pass (5 new)
- `cargo fmt --all --check` clean; `cargo clippy -p nsite-deck --all-targets` clean
- `./gradlew :app:testDebugUnitTest` — `MycoLinkTest` 9/9
- `./gradlew :app:assembleDebug` — APK built
- `npm run build` in `myco-dumplings` — clean typecheck + build
- bech32 round-trip checked directly: long payload, uppercase accepted, mixed case
  rejected, tampered checksum rejected, wrong hrp rejected

## Not verified

On-device. The end-to-end claim — scan on a phone without the app, wait, land on the
invite — has not been run against hardware:

```sh
adb shell am start -a android.intent.action.VIEW -d "myco://app/<host>/invite/cordn1…"
```

Needs Dumplings deployed to a real nsite host (`npm run deploy`) and two phones.

## Out of scope, still open

**Myco itself not installed.** `myco://` has no handler, so the tap does nothing. Needs
an `https://` App Link landing page plus clipboard hand-off (sideload/zapstore
distribution rules out the Play Install Referrer). Rationale for deferring in
`docs/design/deep-links.md` §5.
