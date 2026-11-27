NAP-CVM in Myco
===============

ContextVM Bridge
----------------

`draft` · **implementation notes**

**NAP ID:** NAP-CVM
**Domain:** `cvm`
**Spec:** [napplet/naps PR #31](https://github.com/napplet/naps/pull/31)
(`naps/NAP-CVM.md`)
**Web binding (NIP-5D):** `window.napplet.cvm` · `shell.supports("cvm")`

> How Myco implements the upstream NAP-CVM. The spec is authoritative; this
> page records Myco's choices where the spec leaves room, and what is not
> built. The spec is a **draft in an open PR, not merged**: re-audit this page
> and the code whenever it changes.
> Implementation: `myco-napplet-runtime/src/nap/cvm.rs`,
> `assets/myco-prelude.js`; `myco-core/src/cvm_relays.rs`.

## What it does

A napplet calls tools on a ContextVM server. ContextVM is MCP over Nostr:
the request is a signed event to the server's key, the answer comes back the
same way. The napplet names the server and the MCP message. Myco does the
rest: keys, signing, relays, initialization, encryption.

API, schemas, wire and error strings: see the spec.

## What Myco implements

- `discover`, from CEP-6 announcements and CEP-17 relay lists
- `request`, and the wrappers on top of it (`listTools`, `callTool`,
  `listResources`, `readResource`)
- `close`, and `onEvent`

Not built:

- **The registry.** `registry.list`, `describe` and `call` answer
  `unsupported method`. `registry.has` answers `false`. Bad input is still
  `invalid request`.
- **Payments.** Myco has no payment rail. See below.

## Keys

Each window gets a fresh key **per server**. It lives in memory only and
dies with the window.

- It is never the user's key, and never shown as one.
- Anything a server ties to the client key belongs to one window: its
  session, its requests to the napplet, any paid authorization.
- So two napplets, or two windows of one, can never answer or spend for
  each other. The spec's isolation rules hold by construction.

## Relays

Where a request goes, in order:

1. **Relays the napplet names** (`server.relays`): used as given, for both
   publishing and listening. `ws(s)://` on a public host only; others are
   dropped, not refused. At most 8.
2. **The server's own relay list** (CEP-17, kind 10002), when the napplet
   names none. Requests go to its unmarked and `read` relays; Myco listens
   there and on its `write` relays too. Looked up on the discovery relays,
   remembered for an hour (ten minutes when it has none).
3. **The default**, `wss://relay.contextvm.org`, for a server with no list.

**Discovery relays** — where `discover` reads, and where relay lists are
looked up: the default, then the relays the user publishes to (their NIP-65
list as the outbox holds it; never waited on). At most 8. A napplet's
`query.relays` replace them.

What `discover` learns of a server's relay list is remembered, so a request
right after it needs no lookup. The memory is the device's, shared by every
napplet, so two rules keep one napplet from steering another:

- Only discovery on the device's own discovery relays is remembered. What a
  napplet's `query.relays` bring back answers that napplet only.
- A list never gives way to an older one, so an old signed list replayed by
  a relay changes nothing. A lookup that finds no list keeps the one known.

Every dial goes through the shared relay pool, under offline-only and the
private-address check at the dial. **Offline:** `request` fails with
`server not found`; `discover` returns no servers.

## Requests

- The listen on the window's key is open **before** the first publish.
  ContextVM messages are ephemeral, so a late subscription misses its answer.
- Answers are matched by the request **event** id (the `e` tag), and only
  when signed by the server named. The napplet's JSON-RPC ids pass through
  untouched.
- **Server requests:** delivered to the window as `cvm.event`. The napplet's
  answer keeps the server's id and `e`-tags the event that carried the
  request. At most 64 wait for an answer per session; what a server sends
  beyond that is dropped.
- **Request ids** are unique per session. An id whose request never reached
  a relay (initialization failed or timed out first) is free to use again.
- **Initialization:** before the first request, unless the napplet sends
  `initialize` itself or sets `initialize: false`. Notifications and
  responses initialize only with `initialize: true`. Myco advertises no
  client capabilities: it cannot know what the napplet will answer. The
  session counts as initialized once `notifications/initialized` went out.
  `clientInfo` is a generic `napplet-shell` / `1.0.0`, not Myco's name and
  version, so the throwaway keys aren't tied back together by it.
- **Timeouts:** 1 to 300 000 ms, default 30 000. Myco's maximum is the
  spec's. The binding waits the requested time plus 5 s, so a 120 s call is
  not cut off at 30 s.
- **Limits:** 32 waiting requests and 16 servers per window. Beyond that:
  `policy denied`.

## Encryption

CEP-4 gift wraps (kind 1059, NIP-44).

- A server whose newest server announcement (kind 11316) carries
  `support_encryption`, as discovery last read it, gets every message
  wrapped, `initialize` included. Remembered under the same rules as relay
  lists.
- Otherwise first contact is plain. Once the server shows support — a
  `support_encryption` tag on anything it sends, or a wrapped message —
  Myco wraps what it sends.
- Wrapped answers are always accepted.

## Payments

Myco pays nothing. It asks for CEP-8 explicit gating on the first message.
A payment gate (`-32042`, `-32043`, or `notifications/payment_required`)
ends the call at once:

| `payment` | Result |
|-----------|--------|
| `prompt` (default) | `payment required` — no payment handler |
| `deny` | `payment denied` |
| `allow` | `payment denied` — no standing allowance exists |

`paymentRequired` in discovery is `true` for a positive `cap` price, and
omitted otherwise. Myco never claims a server is free.

## Windows closing

Closing a window ends its ContextVM work at once: every waiting call answers
`session closed`, every listen closes. Withdrawing the `cvm` grant does the
same. The host does this explicitly
(`Cvm::end_window`), because a running call holds a snapshot of the session
and would otherwise keep it alive until its deadline.

## The binding

The vendored `@napplet/shim` has an older `cvm`. Myco's prelude replaces it
whole:

- a notification or response resolves `null`, not an error
- `readResource` returns every content entry, in order
- calls with `timeoutMs` wait that long, not a fixed 30 s
- `registry.has(selector, options)`; a bare family string still works
- `onEvent(handler)` calls `handler({ server, message })`

## Permission

`cvm` is not a default grant. A napplet asks for it in its manifest; the
install sheet shows it as **Remote tools** (sensitive): what the napplet
sends leaves the phone.

## Not tested on devices

Host tests cover the runtime against a scripted server, and the relay
transport's listen against an in-process relay. Neither the transport nor the
binding has been run against a live ContextVM server yet.
