---
id: 260809-nyq
slug: myco-deep-linking-myco-app-host-path-def
date: 2026-08-09
mode: quick
branch: feat/deep-links
---

# Myco deep linking (`myco://app/<host>/<path>`) + deferred open + myco-dumplings

## Goal

A `myco://app/<host>/<deep-path>` link opens that nsite at that path. If the nsite
isn't installed yet, Myco retrieves it and — however long that takes, across process
death and reboots — the *first* open of that nsite lands on the deep path.

## Locked decisions

1. **SPA fallback in the gateway** (not hash routing). Real paths, one small
   `nsite-deck` change that stays upstream-generic.
2. **Deep links carry no invite/pairing/holder info.** `myco://app/<host>/<path>` and
   nothing else. Retrieval works holder-less: `Content::open_site` already tries every
   Circle peer, then the public IP fallback, when `holder = None`
   (`myco-core/src/content.rs:585-610`).
3. The invite payload (`/invite/cordn1<bech32>`) is **opaque to Myco** — app-defined,
   forwarded verbatim to the nsite.

## Tasks

### T1 — Gateway SPA fallback (`nsite-deck/src/gateway.rs`)

On a manifest miss, precedence is: `/404.html` → `/index.html` (SPA shell, **200**) →
gateway 404 page.

The `/index.html` fallback applies **only to navigation-style requests** — those where
`normalize_path` produced a `…/index.html` (root, trailing slash, or extensionless last
segment). A miss on `/assets/app.js` still 404s, so a broken asset does not silently
render the app shell with a 200.

Unit tests: extensionless deep path → shell 200; missing asset → 404; `/404.html`
present → keeps 404 precedence; no `/index.html` → gateway 404.

### T2 — `MycoLink` parser (`android/.../share/MycoLink.kt`, new)

Parse `myco://app/<host>/<deep-path…>` → `AppLink(host, path)`.
- `path` keeps its leading slash, plus any query/fragment; `/` when absent.
- Rejects anything with pairing/invite query params — this form is deliberately
  credential-free.
- Rejects a bare `myco://app/<host>` with no tail? No — that is a valid "just open it"
  link, `path = "/"`.
Unit-testable pure Kotlin (no Android types beyond `Uri`; use string parsing so it is
testable off-device).

### T3 — `PendingDeepLinks` store (`android/.../share/PendingDeepLinks.kt`, new)

SharedPreferences-backed `{host → {path, ts}}` JSON map.
- `put(host, path)` / `take(host)` (read-and-clear) / `peek(host)` / `sweep()`.
- Entries expire after 30 days; `sweep()` drops them.
- Survives process death and reboot — this is the whole point of the feature.

### T4 — `NsiteActivity`: deep path + already-open navigation

- New `EXTRA_PATH`; `loadUrl("http://$host.localhost$path")`.
- Data URI stays host-only (`myco://app/<host>`) so `documentLaunchMode=intoExisting`
  still re-surfaces the same Recents card.
- Add `onNewIntent`: an already-open nsite navigates to the new path instead of showing
  the stale page.

### T5 — `MainActivity`: routing + reconciler

- `handleScannedText`: explicit `MycoLink` branch **before** the raw-link fallback.
- Ready now → launch with the path immediately.
- Not ready → `PendingDeepLinks.put`, dispatch `OpenNsite{link: host, holder: null}`,
  toast "Getting <app>…".
- `reconcilePendingLinks()` on `onResume`: for each pending entry, if the site is
  `ready` → launch with the path and clear; if `unreachable` → re-dispatch `OpenNsite`
  (retry, do not give up on the first failure — a holder peer may only now be in range).
- `launchNsite()` consumes a pending path too, so a user tapping the app in the drawer
  themselves also lands on the deep link.

### T6 — `myco-dumplings` nsite (new, `/myco-dumplings`)

Vite + React + TS, same shape as `myco-ics` (no applesauce — no relay use).
- Offline-first bookmarks in `localStorage`: URL + label.
- Share → `myco://app/<own-host>/invite/cordn1<bech32(payload)>` + QR + copy.
- Route `/invite/:payload` → decode → "<who> shared <title> — add to bookmarks?" →
  accept/dismiss. Uses real path routing (depends on T1).
- bech32 hrp `cordn`, payload = compact JSON `{u,t,f}`.
- Own bech32 + tiny router implementation; no runtime deps beyond React + a QR encoder.

### T7 — Docs

`docs/design/deep-links.md`: the URL grammar, why it carries no secrets, the deferred
open state machine, and the gateway fallback rule.

## Verification

- `cargo test -p nsite-deck`
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`
- `npm run typecheck && npm run build` in `myco-dumplings`
- Android: `./gradlew assembleDebug`
- Manual: `adb shell am start -a android.intent.action.VIEW -d "myco://app/<host>/invite/cordn1…"`
  against both an installed and a not-yet-installed nsite.
