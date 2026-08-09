# Deep links

A link that names an app **and a place inside it**, and works whether or not the
receiving phone has that app yet.

```
myco://app/<host>/<path>

myco://app/5rnyv42gd5hia53curyrl58o9vnouebwyt4gxdice14ialcs4vcahmls/dumpling/dmpl1…
         └────────────────────── host ──────────────────────┘└──── path ────┘
```

- `<host>` is the ordinary nsite label — an `npub1…` root, or `<pubkeyB36><dTag>`.
  Myco resolves it, and retrieves the app if it is missing.
- Everything after it is **opaque to Myco** and handed to the nsite verbatim, query
  and fragment included. `/dumpling/dmpl1…` means nothing here; it means whatever the
  app says it means.
- A bare `myco://app/<host>` is valid: "just open it".

Parsed by `MycoLink.parseAppLink` (`android/…/share/MycoLink.kt`), which is a pure
function with JVM unit tests. The host is lowercased (a QR in alphanumeric mode is
uppercase-only) and must be a usable DNS label, because it becomes `<host>.localhost`
in the WebView. The path is left exactly as written.

## §1 It carries no secrets

Deliberately: **no holder npub, no pairing secret, no token Myco interprets.**

A deep link goes places nobody controls — a chat message, a printed QR on a wall, a
URL bar, a screenshot in someone's camera roll. Anything inside it is public, durable
and replayable, which is the exact opposite of what a one-time pairing secret needs to
be. Pairing keeps its own carrier: the scanned or tapped `myco://pair/…` and
`myco://share/…` payloads (`docs/design/identity-pairing.md`), exchanged face to face,
one-shot, and short-lived.

Dropping the holder hint costs a retry and nothing else. `Content::open_site` with
`holder = None` already walks every Circle peer in turn and then the public IP source
(`myco-core/src/content.rs`), so a link with no sharer attached still finds the app
through whoever nearby happens to have it.

## §2 The deferred open

The hard requirement: **the first open of a deep-linked app lands on the deep link** —
no matter how long "first" takes.

```
tap myco://app/<host>/dumpling/…
        │
        ├── app ready ──────────────► open it, at that path. done.
        │
        └── not ready
              │  remember {host → path} in SharedPreferences
              │  dispatch OpenNsite{link: host, holder: null}
              │
              ├── foreground watcher (3 min, 1s poll) ──► ready ──► open at path
              ├── every MainActivity.onResume ──────────► ready ──► open at path
              │                                     └─► unreachable ──► retry OpenNsite
              └── user taps the app in the Apps grid ──► launchNsite spends the path
```

Three consumers, one store (`PendingDeepLinks`), and the path is spent by whichever
gets there first:

- **The watcher** covers the common case — a peer is right there in the room and the
  sync finishes in seconds while Myco is still open.
- **The resume reconciler** is the durable half. The watcher dies with the process;
  this doesn't. A link followed before a reboot opens the first time Myco comes back up
  with the app in hand. It also **re-dispatches** an `unreachable` app rather than
  writing it off: nobody in range had it *then*, and someone who does may have walked
  in since.
- **`launchNsite`** consumes a pending path too, so someone who opens the app from the
  Apps grid themselves still lands on the link they followed days ago.

Entries expire after 30 days. Past that the moment has passed, and landing someone on a
month-old link is more confusing than opening the app.

## §3 Recents, and an app that is already open

The nsite task is keyed by a **host-only** document URI (`myco://app/<host>`), and the
path travels in an intent extra. Folding the path into the document URI would make
every route a separate document, so each route would get its own Recents card for the
same app.

The flip side is that a deep link into an already-open nsite re-surfaces the existing
task, so `NsiteActivity.onNewIntent` navigates the WebView to the new path — otherwise
the user would be handed back the page they left instead of the one they just tapped.

## §4 What the gateway had to give up

`/dumpling/dmpl1…` is not a file any manifest lists. The gateway now resolves a miss as:

1. the site's own `/404.html`, if it ships one — it owns that answer;
2. `/index.html` with a **200** — the app shell, so a client-side route reaches the
   router;
3. otherwise the gateway's 404 page.

Step 2 applies **only to navigation-style requests**: those where `normalize_path`
produced a `…/index.html` (root, trailing slash, or an extensionless last segment). A
missing `/assets/app.js` still 404s. Answering a script request with HTML and a 200 is
the classic SPA-fallback footgun — it turns a broken asset into a silent one.

Two consequences for anyone writing an nsite with real routes:

- **Route segments must not contain a dot.** A dot makes the segment look like an asset,
  and it 404s instead of reaching the router. (Bech32 payloads have none — one reason
  Dumplings encodes its payloads that way.)
- **Build with `base: "/"`, not `"./"`.** From `/dumpling/x`, a relative
  `./assets/index.js` resolves to `/dumpling/assets/index.js`, which does not exist.

## §5 What is not covered

If **Myco itself** is not installed, `myco://` has no handler and the tap does nothing.
Solving that needs an `https://` App Link landing page plus a deferred-deep-link
mechanism (clipboard hand-off, since sideload and zapstore distribution rule out the
Play Install Referrer). Deferred: while distribution is sideload-first, the person
sending the link is standing next to the person receiving it.

## §6 See it work

`myco-dumplings/` exists for exactly this: a bookmark app whose only real feature is
sharing a link as `myco://app/<host>/dumpling/dmpl1…`.

```sh
adb shell am start -a android.intent.action.VIEW -d "myco://app/<host>/dumpling/dmpl1…"
```

Run it twice: once against a phone that has the app, once against one that has never
seen it.
