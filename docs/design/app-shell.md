# The Android Shell & Launch Model

This document proposes the **Android shell and launch model** for Myco: how the
app presents itself on the device, how an nsite is launched and lives as its own
fullscreen "app", and why Myco imposes **no browser chrome** of its own. For the
vocabulary it builds on (nsite, author vs device key, `.nsite` vs `.fips`, the
embedded relay + Blossom, the Library), read [concepts.md](./concepts.md) first;
for where the shell sits in the layer stack, see
[architecture.md](./architecture.md); for what the WebView actually loads, see
[nsite-layer.md](./nsite-layer.md); for the trust boundary around nsite content,
see [security.md](./security.md).

> Design doc for a not-yet-built app, written in proposal voice. Open questions
> are marked **TBD / open**. This doc supersedes the earlier "embedded browser
> with a fixed bottom bar (Back · Reload · Library)" chrome model wherever the
> two disagree — that Myco-imposed chrome is **gone**.

---

## 1. Two kinds of surface: the manager and the apps

Myco has **two distinct UI surfaces**, and keeping them apart is the load-bearing
decision of this doc:

- **Myco the manager app** — its own home-screen icon, its own task. This is
  where you *manage* nsites: install, remove, re-pair, add-to-home-screen, view
  info. Its UI is four areas: **Library** (your installed nsites/apps),
  **Pair** (QR scan + show, see [identity-pairing.md](./identity-pairing.md)),
  **Discover** (nsites your circle holds — "nsites around me"), and
  **Settings** (storage, cache cap, device identity).
- **Each nsite as its own fullscreen app** — launched *by* Myco but running in
  its **own task/instance**, filling the screen, with **no Myco UI around it at
  all**. To an Android user it looks and behaves like a separate app, not a tab
  inside Myco.

The manager is the home for "what do I have and how do I manage it"; the nsite is
the thing you actually use. You never browse an nsite *inside* the Myco manager —
opening one leaves the manager and starts the nsite as its own task.

---

## 2. The nsite as a separate fullscreen task

Each nsite launches as a fullscreen **`NsiteActivity`**: an Android `WebView`
filling the whole screen — **no toolbar, no URL bar, no Myco chrome**. It is its
own entry in Android Recents, switchable independently of the Myco manager.

The proposed mechanics:

- **`NsiteActivity`** is declared `android:documentLaunchMode="always"`, so each
  launch is a *distinct document* rather than reusing one activity instance.
- It is launched with **`FLAG_ACTIVITY_NEW_DOCUMENT`** and **without**
  `FLAG_ACTIVITY_MULTIPLE_TASK`, with the intent **data URI keyed by the nsite host
  `<host>`** (§4) — so Android matches an existing task for that nsite and
  **re-surfaces it** instead of creating a duplicate.
- Each launch becomes its **own card in Android Recents**, showing the nsite's
  **title + favicon + colour** via
  [`ActivityManager.TaskDescription`](https://developer.android.com/reference/android/app/ActivityManager.TaskDescription),
  populated from the manifest (`["title", …]`, the `/favicon.ico` mapping).
- The activity loads exactly one URL: `http://<host>.nsite` (see §7). It never
  loads anything else; cross-device fetching is native code over `.fips`, which
  the WebView never sees.

So "open an nsite" = start **or re-surface** that nsite's `NsiteActivity` task. Two
*different* nsites open at once are two Recents cards; **opening the same nsite
again — from the Library, a home-screen shortcut, or a cross-nsite link —
re-surfaces the existing task** rather than spawning a second instance. There is
**one live instance per nsite**, keyed by `<host>`; this is achieved by omitting
`FLAG_ACTIVITY_MULTIPLE_TASK` and keying the document by the `<host>` intent data.
(This resolves the earlier multiple-task open question.)

---

## 3. No Myco-imposed chrome

Myco adds **no** browser chrome to an nsite. There is no reload button, no back
bar, no "home" affordance painted over the page. Concretely:

- **Refresh, in-app navigation, and pull-to-refresh are the nsite developer's
  responsibility**, implemented *inside* the nsite (it is a normal web app; it
  can put its own nav, its own reload, its own pull-to-refresh in its HTML/JS).
  Myco does not supply them.
- **Android Back** walks the WebView history; when history is exhausted it
  **finishes the task** (the nsite closes, like any app).
- **Android Recents** is how you switch between running nsites and the Myco
  manager — it replaces the old in-shell "Library" button.

This is a deliberate inversion of the superseded model. Previously Myco wrapped
every site in a fixed bottom bar (Back · Reload · Library) and owned navigation;
now the platform (Back + Recents) owns task-level navigation and the **nsite
author owns everything inside the frame**. The shell's job shrinks to "launch the
right task and describe it to Recents."

> **Open question — which nsite am I in?** With no URL bar and no Myco chrome,
> the only on-screen signals of *which* nsite is running are the
> `TaskDescription` title/icon/colour in Recents and whatever the nsite itself
> renders. Both are **author-controlled**, so a hostile nsite can present a title
> and favicon impersonating another site. The shell has nowhere trusted to show
> the verified author npub once the activity is fullscreen. How to give the user
> a trustworthy "you are in site X, authored by npub1…" signal — without
> reintroducing chrome — is **TBD / open**. (This is the launch-model facet of
> the nsite-sandbox trust question in [security.md §5](./security.md).)

---

## 4. Intents and deep links are the glue

Launching is mediated entirely by intents, so the same mechanism serves the
Library, one nsite opening another, and a shared link:

- The scheme is **`myco://app/<host>`**, resolving to `NsiteActivity`, where
  `<host>` is the nsite's canonical single-token label — **`npub1…`** for a root
  site (kind 15128) or **`<pubkeyB36><dTag>`** for a named site (kind 35128),
  exactly as the nsite spec encodes it (base36 50-char pubkey directly followed by
  the 1–13-char `dTag`, no separator — the same single label used by
  `<host>.nsite`). One token, not an `npub/dTag` split.
- **Myco's Library** launches an nsite by firing this intent.
- **One nsite can open another** by the same intent (a link to
  `myco://app/<other-host>` starts — or re-surfaces (§2) — *that* nsite as its
  **own separate task**, never nested in the current WebView) — subject to the
  origin-isolation and capability constraints in [security.md](./security.md).
- **A shared link** (`myco://app/…` handed to you out of band) opens the site
  directly, the same way.

This `myco://app/…` launch scheme is distinct from `myco://pair/…` (QR pairing,
see [identity-pairing.md](./identity-pairing.md)) — same custom scheme, different
host. Both are deep-link intent filters Myco registers.

---

## 5. Home-screen icons: user-confirmed, best-effort presence

An nsite can be pinned to the device home screen so it launches like any installed
app, but Myco **cannot** silently place icons.

- Pinning uses
  [`ShortcutManager.requestPinShortcut()`](https://developer.android.com/reference/android/content/pm/ShortcutManager#requestPinShortcut(android.content.pm.ShortcutInfo,%20android.content.IntentSender)),
  which is **always user-confirmed** — Android shows a system dialog per pin.
  Myco can *offer* "Add to home screen" from the Library, but the user (and the
  launcher) get the final say.
- Myco also publishes **dynamic app-shortcuts** (long-press the Myco icon →
  recent/pinned nsites) for the sites you use most.

**Knowing whether an nsite is on the home screen is best-effort, not reliable.**

- [`getPinnedShortcuts()`](https://developer.android.com/reference/android/content/pm/ShortcutManager#getPinnedShortcuts())
  is the only signal, but **removal is under-reported** by non-stock launchers,
  there is **no removal callback**, and `LauncherApps` (which *could* enumerate
  pinned shortcuts) is **launcher-only** — a normal app cannot use it.
- Therefore **the Library is the source of truth for "installed."** Home-screen
  presence is a *soft hint* shown in the UI, never a fact the design relies on.
  Nothing must break if the hint is wrong (e.g. the user removed the icon and we
  never heard): the Library entry persists regardless, and re-pinning is always
  available.

---

## 6. Per-nsite origin isolation is automatic

Because each nsite is served under its own host (`<host>.nsite`, where `<host>`
is the author npub or `<pubkeyB36><dTag>` — see [concepts.md](./concepts.md) and
[nsite-layer.md §3.2](./nsite-layer.md)), each nsite is its **own web origin**.
The WebView therefore **partitions storage, cookies, and `localStorage` per
nsite automatically** — one nsite's data is isolated from another's by the
browser's same-origin policy, with no extra bookkeeping in the shell.

This reinforces the "separate app" framing at the data layer: an nsite in its own
task, in its own origin, with its own storage. It does **not** by itself solve the
cross-origin *capability* concerns (reaching `.fips`, exfiltration, sibling-site
tampering) — those remain the gateway/CSP responsibilities tracked in
[security.md §5](./security.md). Origin isolation is a necessary foundation, not
the whole sandbox.

---

## 7. How the shell relates to the gateway

The shell launches tasks; the **gateway** (see [nsite-layer.md](./nsite-layer.md))
makes them resolvable. The seam between them is one URL:

- `NsiteActivity` resolves its intent (`myco://app/<host>`) to a host
  label and loads **`http://<host>.nsite`** in its WebView — root site →
  `npub1…`, named site → `<pubkeyB36><dTag>`.
- `*.nsite` is the localhost gateway (`127.0.0.1`, IPv4), so the load works in the
  Android `WebView` exactly as in any browser. The gateway resolves the manifest,
  fetches blobs from the local Blossom, and serves the bytes; on a cache miss it
  drives a sync over `.fips` (all native, invisible to the WebView).
- The WebView **never** resolves `.fips` (a locked decision, see
  [concepts.md](./concepts.md) and [architecture.md](./architecture.md)) — the
  shell hands it only the `.nsite` URL.

So the division of labour is: **the shell decides *which* task to start and
describes it to Android; the gateway decides *what bytes* that task's one URL
serves.** Neither owns the other's concern.

---

## 8. Open questions

- **Impersonation without chrome (§3).** No trusted, always-visible signal of
  which verified author npub a fullscreen nsite belongs to; `TaskDescription`
  and in-page content are author-controlled. **TBD / open.**
- **Home-screen presence accuracy (§5).** `getPinnedShortcuts()` is unreliable
  for removal on non-stock launchers; we treat the Library as truth, but the
  exact UX for a stale "on home screen" hint is **TBD / open.**
- **Cross-nsite launch policy (§4).** When one nsite opens another via
  `myco://app/…`, what (if any) confirmation or origin check the shell interposes
  is **TBD / open** (ties into [security.md §5](./security.md)).
- **Cold-start latency.** Launching a fresh `NsiteActivity` task that immediately
  needs a sync (uncached site) versus showing the gateway loading page inside the
  new task — the perceived-launch experience is **TBD / open.**

---

## See also

- [concepts.md](./concepts.md) — terminology, the Library, `.nsite` vs `.fips`,
  the embedded relay + Blossom.
- [architecture.md](./architecture.md) — the six-layer stack the shell sits atop.
- [nsite-layer.md](./nsite-layer.md) — the gateway that serves `http://<host>.nsite`.
- [security.md](./security.md) — nsite sandbox, origin isolation, the
  impersonation/capability trust boundary.
- [identity-pairing.md](./identity-pairing.md) — QR pairing (`myco://pair/…`),
  the other deep-link surface.
