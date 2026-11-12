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
> Implementation: `myco-napplet-runtime/src/nap/intent.rs`, `nap/inc.rs`,
> `assets/myco-prelude.js`; `myco-core/src/intent.rs`; `NappletActivity.kt`,
> `IntentChooser.kt`, `DefaultAppsSettings.kt`.

## What it does

A napplet asks Myco to open *a role*, not an app:

```js
await napplet.intent.open("napplet:profile/open?pubkey=abc123");
```

Myco finds the installed apps that fill the role, picks one the way the user
said to, opens (or brings forward) its window, and hands it the payload. The
caller never picks another app. It can only recommend one (see
[Recommended handlers](#recommended-handlers)).

## The binding: two call forms

The vendored `@napplet/shim` (0.29.2) has the first draft's API. Myco's
prelude replaces `window.napplet.intent` with a binding that takes both:

| Form | Call |
|------|------|
| Current draft | `invoke(uri, options?)`, `open(uri, options?)`, `launch(naddr, options?)` |
| First draft | `invoke({ archetype, action?, convention?, payload? })`, `open(archetype, payload?, options?)` |

- A string starting `napplet:` is a URI. An archetype is a slug and never
  contains `:`, so the two cannot be confused.
- A URI is normalized exactly as `@napplet/nap` 0.38.1 does: `?name=value`
  becomes a text payload (nothing coerced, `+` stays `+`, an empty `?` is an
  empty payload), and `#naddr1…` becomes `handlerHint`.
- A malformed URI or percent-encoding, a pair without `=`, a repeated name, a
  query (even an empty one) with an explicit payload, or a bad fragment
  rejects the promise before anything is sent.
- `onDelivery` is added. Deliveries are held until a handler registers.
- Invokes wait up to 5 minutes, not 30 s: the answer comes once the handler's
  window is open, which can wait on the user.

Bumping the vendored shim to 0.38.1 would bring this API, but it also moves
every domain onto a `MessagePort` that `shell.html` does not speak yet. That
is its own change.

## The catalog

- The catalog is the **Library**. Each installed napplet's entry keeps the
  roles its served version declares (`LibraryItem.archetypes`): each `["i",
  <convention>, <param>…]` whose archetype a `["z", <role>]` names, with its
  parameter names.
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

### Catalog identifiers

A handler's **catalog identifier** is `<npub>:<d>` (`<npub>` for a root
napplet, `myco` for Myco). It is the candidate's `id`, a result's `handler`,
and a delivery's `sender`. It names the publisher, as the spec requires: a
bare `d` tag would let one author's app stand in for another's.

Candidates carry both drafts' fields:

```text
{ "id": "npub1…:profiles", "dTag": "profiles", "title": "Profiles",
  "actions": ["open"], "conventions": ["napplet:profile/open"],
  "contracts": [{ "convention": "napplet:profile/open", "params": ["pubkey"] }],
  "isDefault": false }
```

Entries recorded before parameter names were kept show `params: []` until the
app's served version next moves.

## Resolution

In order:

0. A recommended app that is not installed is offered for install first,
   unless step 3, 4 or 5 applies. See
   [Recommended handlers](#recommended-handlers).
1. No candidate for the archetype → `"no handler"`.
2. Candidates, none with the action → `"unsupported action"`; none accepting
   the convention → `"unsupported convention"`.
3. `handler: "choose"` → the chooser.
4. `handler: "<id>"` → **also the chooser.** The spec wants the user to
   have authorised cross-app targeting first; Myco has no such permission, so
   the name decides nothing and the user picks.
5. The user's **default** for the archetype, if it can take the request. A
   default that cannot is passed over. A default whose app was removed is
   ignored.
6. The **recommended** app, if it is installed and can take the request.
7. The **only** candidate that can take it.
8. Otherwise the chooser.

A request is refused as `"invalid convention"` when its convention carries a
query or fragment, or names another archetype or action than the request.

With no `convention`, the payload goes on `napplet:<archetype>/<action>` if
the handler accepts it, else its first convention with that action.

Rules 1–8 are pure functions in the runtime (`nap::intent::resolve`,
`hint_to_offer`, `contract_accepts`); the host supplies the catalog.

## Recommended handlers

A caller may recommend the app it means with `#naddr1…` on the URI, or
`handlerHint: { address: "35129:<hex>:<d>", relays }`. Only named napplets
(kind 35129) can be recommended.

**Installed:** it wins over the chooser (step 6), never over the user's
default or an explicit `choose`.

**Not installed**, and no default applies:

1. Myco looks the manifest up where an install review would: the hint's
   relays, the user's relays, nearby phones, under offline-only. It has 8 s.
2. The manifest must be signed by the coordinate's author, of kind 35129 with
   exactly that `d`, and its own `z`/`i` tags must take the request. The hint
   is never evidence.
3. Myco puts the app's **install review** over the caller's window, by
   NAP-LINK's rules (one review at a time, rate-limited).
4. The caller hears `ok: true` at once, with the app's future `id` as
   `handler`. That `handler` is provisional: if the user declines (step 6),
   the spec's fallback can route the request to another app. The payload
   waits for up to **10 minutes**, and it does not depend on the caller's
   window staying open.
5. **Added:** the payload goes onto that app's review (`intentToken`).
   Nothing opens by itself: the sheet's **Open**, over the caller or on the
   Apps screen, starts the app with the token and it receives the payload.
   If that review is no longer the one showing (closed, or replaced by
   another) when the app is installed, the request is dropped.
6. **Closed without adding:** the request is resolved as if there had been no
   hint. The installed handler, or the chooser, gets it. If nothing takes it,
   it is dropped. The caller is not told again; the spec does not report
   failures after acceptance.

If any of steps 1–3 fails, the request is resolved without the hint.

## Launch: one named app

`launch(naddr)` (`intent.launch`, proposed in
[napplet/naps#148](https://github.com/napplet/naps/pull/148)) opens exactly the
app the `naddr` names. No role, default, recommendation or chooser applies,
and no other app is ever opened.

```text
-> { "type": "intent.launch", "id": "l1",
     "target": { "address": "35129:<hex>:chess", "relays": ["wss://…"] } }
<- { "type": "intent.launch.result", "id": "l1",
     "result": { "ok": true, "handler": "npub1…:chess" } }
```

- Myco puts that app's **review** over the caller, by NAP-LINK's rules, and
  answers `ok`. For an installed app (any role or none) the sheet says so and
  offers **Open**; for one that is not, it is the install review, and after
  Add it offers Open too. Opening is always the user's tap, and the caller
  cannot tell which case it was. Nothing is delivered. The answer carries
  the app's catalog identifier.
- A named (35129) or root (15129) app. A malformed target is
  `"invalid target"`.
- Every launch passes NAP-LINK's review gate first, installed target or not:
  one of this window's reviews still up, or within 5 s of the last review or
  launch, is `"launch rejected"`, as is a second launch within 750 ms. That
  error never says whether the app is installed.
- `launch` carries no payload. Data goes through `invoke`.

AppStore's **Install** calls `launch` when `window.napplet.intent.launch`
exists, and NAP-LINK `nostr:<naddr>` otherwise.

**NAP-LINK `nostr:naddr1…` as an install request is deprecated.** Myco still
opens the install review for it: AppStore is fetched from its published
address, so every Myco version runs the newest AppStore, and Myco versions
before `launch` need the link. Each use is logged at `warn`
(`deprecated: link.open("nostr:naddr…") as an install request`), with the
calling napplet's `d` tag, so a napplet still relying on it shows up.

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

The payload reaches the handler two ways, one for each draft:

```text
<- { "type": "intent.deliver",
     "delivery": { "sender": "npub1…:chronofeed", "archetype": "profile", "action": "open",
                   "convention": "napplet:profile/open", "payload": { "pubkey": "…" } } }
<- { "type": "inc.event", "topic": "napplet:profile/open", "sender": "chronofeed",
     "payload": { "pubkey": "…" } }
```

- **`intent.deliver`** (current draft) carries the caller's catalog
  identifier. It is sent once the handler's window has finished its
  handshake: right after its `shell.init`, or at bind if it already has. The
  binding holds it until the napplet calls `onDelivery`.
- **`inc.event`** (first draft) carries the caller's `dTag`. It is sent after
  the handler subscribes to the convention's topic.
- Both senders come from the caller's session, never from the caller.
- A napplet that listens both ways hears the payload twice. Neither draft's
  SDK does that; a napplet built on both handles the duplicate itself. Myco
  does not try to tell which one it uses.

The `inc.event` timing is the hard part. Myco's prelude sends `shell.ready`
before any napplet code runs, and the shim drops an `inc.event` for a topic
nobody subscribed to. So "established" does not mean "listening". Myco waits
for the handler to subscribe:

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
                    "handled": true, "handler": "npub1…:profiles",
                    "windowId": "napplet-7", "convention": "napplet:profile/open" } }
   ```

   Both drafts' validators accept this: the first needs `handled`, the
   current one `handler`.

   `windowId` is the handler's session id. For Myco's nsite opener it is the
   site's host label, and the result comes at once.
4. The payload is delivered after the handler's `inc.subscribe` for the
   convention's topic (right after the `inc.subscribe.result`), or at once if
   it already subscribed.

Rules:

- Only a window of the resolved napplet can bind a token, and only once.
- A payload is delivered once each way.
- A token not bound within **60 s** fails the caller (`"invoke failed"`). A
  bound payload is kept for a first-draft subscribe until then. Both go with
  the handler's window if it closes.
- An open that fails (the napplet does not verify) answers `"invoke failed"`.
- Payloads are capped at **64 KiB** of JSON. At most 32 not yet delivered are
  held at once, plus 32 waiting on an install.

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
- Per-caller permission for `handler: "<id>"`.
- `onDelivery` for napplets that load the first-draft SDK and never call it:
  they still get `inc.event`.
- Redacting candidates from `available()` (a fingerprinting surface the spec
  lets a shell narrow).
- Icons: the chooser shows the Apps grid's letter tile.

## Implementations

- Runtime: `nap/intent.rs` (validation, `LaunchRequest`, `resolve`, `hint_to_offer`,
  `contract_accepts`, `deliver_frame`, `IntentCatalog` seam on `NapContext`,
  `Outcome::Intent`), `nap/inc.rs`, `Session` topics, host commands
  `open-napplet`, `open-nsite`, `choose-intent-handler`.
  The binding: `assets/myco-prelude.js` `installIntent`, tested by
  `tests/browser/intent.mjs`.
- Core: `intent.rs` (`LibraryIntents`, backfill, `nsite_host`, the pending
  queue and awaiting installs on `NappletHost`, `HintResolver`,
  `intent_launch`),
  `runtime.rs` `HintSources`, `review_closed` on `DismissNappletReview`,
  `LibraryItem.archetypes`, settings
  `intentDefaults`, actions `set_intent_default`, `answer_intent_chooser`,
  `cancel_intent`, state `intentHandlers`, JNI `nappletOpen(…, token)` and
  `nappletBindIntent`.
- Android: `NappletActivity` (host commands, `onNewIntent`, the gesture
  gate), `IntentChooser.kt`, `DefaultAppsSettings.kt`.
