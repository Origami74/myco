NAP-INTENT in Myco
==================

Open Another App by Role
------------------------

`draft` · **implementation notes**

**NAP ID:** NAP-INTENT (with the part of NAP-INC it needs)
**Domain:** `intent`, `inc`
**Spec:** `reference/naps/naps/NAP-INTENT.md`, `NAP-INC.md`, `ARCHETYPES.md`
**Web binding (NIP-5D):** `window.napplet.intent`, `window.napplet.inc` ·
`shell.supports("intent")`, `shell.supports("inc")`

> How Myco implements the upstream NAP-INTENT. The spec is authoritative; this
> page records Myco's choices where the spec leaves room, and what is not built.
> Implementation: `myco-napplet-runtime/src/nap/intent.rs`, `nap/inc.rs`;
> `myco-core/src/intent.rs`; `NappletActivity.kt`, `IntentChooser.kt`,
> `DefaultAppsSettings.kt`.

## What it does

A napplet asks Myco to open *a role*, not an app:

```js
await napplet.intent.open("profile", { pubkey }, { convention: "napplet:profile/open" });
```

Myco finds the installed apps that fill the role, picks one the way the user
said to, opens (or brings forward) its window, and hands it the payload. The
caller never names or learns another app, except through the answer.

## The catalog

- The catalog is the **Library**. Each installed napplet's entry keeps the
  `["archetype", <slug>, <convention>]` tags its served version declares
  (`LibraryItem.archetypes`).
- It is written from the verified manifest at install, and again whenever the
  served version moves (the pin in `Content::pin`), so it follows the version
  that opens.
- Entries written before NAP-INTENT have no recorded roles. The catalog fills
  them lazily from the pinned manifest the first time it is read.
- A napplet's **actions** are its conventions' `<intent>` parts
  (`napplet:profile/open` → `open`).
- **Myco is a handler too**: the `nsite` role, convention
  `napplet:nsite/open`, titled "Myco". It opens the site the payload names
  (`host` label, else `naddr` of kind 35128/15128, else `url`) through the
  same path a `myco://app/<host>` link takes.

`available()` and `handlers()` answer from this catalog, so an app that is
not running is found.

Candidates carry the napplet's `dTag` and title. The host's own key for a
handler (`<npub>:<d>`) never reaches a napplet.

## Resolution

In order:

1. No candidate for the archetype → `"no handler"`.
2. Candidates, none with the action → `"unsupported action"`; none accepting
   the convention → `"unsupported convention"`.
3. `handler: "choose"` → the chooser.
4. `handler: "<dTag>"` → **also the chooser.** The spec wants the user to
   have authorised cross-app targeting first; Myco has no such permission, so
   the name decides nothing and the user picks.
5. The user's **default** for the archetype, if it can take the request. A
   default that cannot is refused (`unsupported action` / `convention`)
   rather than stepped around. A default whose app was removed is ignored.
6. The **only** candidate that can take it.
7. Otherwise the chooser.

With no `convention`, the payload goes on `napplet:<archetype>/<action>` if
the handler accepts it, else its first convention with that action.

Rules 1–7 are pure functions in the runtime (`nap::intent::resolve`); the host
supplies the catalog.

## The chooser and defaults

- **"Open with…"** is a sheet over the calling window, listing each candidate
  with its tile colour and title, and an **"Always use this"** box.
- Picking answers it. "Always use this" also saves the pick as the role's
  default. Closing the sheet answers `"user cancelled"`.
- **Settings › Default apps** lists every role something installed can
  handle, with its default ("Ask every time" when none). The user can change
  or clear each.
- Defaults live in `settings.json` as `intentDefaults`
  (`{ "<archetype>": "<npub>:<d>" | "myco" }`). Only these two screens write
  them. There is no wire message that could.

## Delivery

The payload reaches the handler as an ordinary NAP-INC topic event:

```text
<- { "type": "inc.event", "topic": "napplet:profile/open", "sender": "chronofeed",
     "payload": { "pubkey": "…" } }
```

`sender` is the caller's `dTag`, taken from its session.

Timing is the hard part. Myco's prelude sends `shell.ready` before any napplet
code runs, and the shim drops an `inc.event` for a topic nobody subscribed to.
So "established" does not mean "listening". Myco waits for the handler to
subscribe:

1. The host stores the payload under a random **token** and sends the caller's
   window `open-napplet { pointer, title, token }`.
2. The window host starts the handler's task with the token. A new window
   passes it to `nappletOpen`; a window already open gets it in
   `onNewIntent` and calls `nappletBindIntent`.
3. Binding checks the window really is the napplet the request resolved to,
   and answers the caller:

   ```text
   <- { "type": "intent.invoke.result", "id": "i1",
        "result": { "ok": true, "archetype": "profile", "action": "open",
                    "handled": true, "handler": "profiles",
                    "windowId": "napplet-7", "convention": "napplet:profile/open" } }
   ```

   `windowId` is the handler's session id. For Myco's nsite opener it is the
   site's host label, and the result comes at once.
4. The payload is delivered after the handler's `inc.subscribe` for the
   convention's topic (right after the `inc.subscribe.result`), or at once if
   it already subscribed.

Rules:

- Only a window of the resolved napplet can bind a token, and only once.
- A payload is delivered once.
- A token not bound within **60 s** fails the caller (`"invoke failed"`). A
  bound payload whose napplet never subscribes is dropped then. Both go with
  the handler's window if it closes.
- An open that fails (the napplet does not verify) answers `"invoke failed"`.
- Payloads are capped at **64 KiB** of JSON, and at most 32 are held at once.

The shim's invoke times out after 30 s. A user who takes longer on the chooser
still gets the app they picked; the caller has stopped waiting.

## Gestures and rate limits

- Opening another window takes the screen, so it is gated like a web link: a
  touch on the calling window in the last 5 s (or a pick on the chooser)
  vouches for it. Otherwise the user confirms ("Open Profiles?"). Declining a
  napplet handler answers `"user cancelled"`. Declining an nsite leaves the
  caller's earlier `handled: true` as it was.
- A window may invoke at most once per 750 ms, and not while its chooser is
  up (`"busy: try again in a moment"`).
- `behavior` hints are accepted and ignored: a napplet has one window per
  task, so `newWindow` cannot be honoured and `reuse` / `focus` always hold.

## `intent.changed`

Pushed to every open napplet granted `intent`, one message per archetype
whose availability changed, when the Library changes (install, update,
removal) or a default is set or cleared.

## NAP-INC: what is built

- `inc.subscribe` / `inc.unsubscribe`: built. Topics are exact strings, at
  most 64 per session and 256 bytes each.
- `inc.emit`: **accepted and dropped.** Routing one napplet's emits to
  another is a channel the user never set up, and nothing needs it yet.
- Channels: `inc.channel.open` answers an error; `inc.channel.list` answers
  an empty list.

## Grants

`intent` and `inc` are default grants, listed on the install screen like the
rest ("Other apps", "App to app"). Routing stays with the user: a default the
user set, the only installed app, or the user's pick. The payload goes only
to the app that was opened. Both can be switched off per app.

## Not built

- Cold start with an inbound Android intent (`myco://napplet/<naddr>?…`) and
  the `nostr:` scheme: the resolver is ready for them; the entry points are not.
- Falling through to Android's chooser when no napplet handles a role.
- Per-caller permission for `handler: "<dTag>"`.
- Redacting candidates from `available()` (a fingerprinting surface the spec
  lets a shell narrow).
- Icons: the chooser shows the Apps grid's letter tile.

## Implementations

- Runtime: `nap/intent.rs` (validation, `resolve`, `IntentCatalog` seam on
  `NapContext`, `Outcome::Intent`), `nap/inc.rs`, `Session` topics, host
  commands `open-napplet`, `open-nsite`, `choose-intent-handler`.
- Core: `intent.rs` (`LibraryIntents`, backfill, `nsite_host`, the pending
  queue on `NappletHost`), `LibraryItem.archetypes`, settings
  `intentDefaults`, actions `set_intent_default`, `answer_intent_chooser`,
  `cancel_intent`, state `intentHandlers`, JNI `nappletOpen(…, token)` and
  `nappletBindIntent`.
- Android: `NappletActivity` (host commands, `onNewIntent`, the gesture
  gate), `IntentChooser.kt`, `DefaultAppsSettings.kt`.
