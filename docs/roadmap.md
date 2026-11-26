# Roadmap

Where **Myco** is and where it goes next. The first plan (P0–P6, a two-phone
offline BLE demo) is done; this page is the second one. Each item has a
one-line goal and an **exit criterion** — the observable condition that says
it is done. Detail lives in the linked design docs; the day-to-day record is
[CHANGELOG.md](../CHANGELOG.md).

For orientation see [getting-started.md](./getting-started.md); for the doc
map, the [index](./README.md).

---

## Status — 2026-11-12

**Shipped in v0.13.0** (2026-11-12; the [CHANGELOG](../CHANGELOG.md) and
[release notes](../RELEASE-NOTES.md) have the detail):

- **Myco updates from other phones.** Phones keep the newest release APK and
  serve it to other phones over the mesh (in v0.13.0 over the Circle; on
  this branch through neighborhoods' `myco-release` extension); it is checked against Zapstore's
  publisher-signed file event and the release key, and Android asks before
  installing (N11).
- **Share Myco**, over a hotspot page or the Android share menu.
- **No VPN needed.** Myco's own traffic runs on an in-app TCP/IP stack; the
  VPN is opt-in, for other apps and the exit proxy.
- **NAP-GAMEPAD** controller input for napplets.
- Fixes: USB relink without a replug and on quick replugs, fewer USB
  permission prompts, no stale USB lane, desktop fips discovering phones on
  the LAN, napplets surviving rotation.

**Built since v0.13.0, not yet released** (on the `feat/nap-neighbor`
branch; see the CHANGELOG's `[Unreleased]`):

- **Neighborhoods replace the Circle.** Links set up from one-time
  invitations (`https://napplet.link/neighborhood/join#invite=…` by QR, share link or NFC), a link key per
  link, signed offers, the link service on `:4873`, traffic scoped per
  neighborhood and passed on link to link, and per-neighborhood reach —
  [nap-neighbor/](design/nap-neighbor/README.md).
- **Apps and updates through neighborhoods.** `napplet-updates`,
  `nsite-updates` and `myco-release` extensions, each with a Get and a Share
  switch per neighborhood; a missing app is fetched from the neighborhoods
  that get it, then the internet —
  [extensions.md](design/nap-neighbor/extensions.md).
- **One port open to other phones.** The link service on `:4873`. Myco's relay
  is loopback only and its Blossom store has no port.
- **NAP-NEIGHBOR** for napplets
  ([napplet/naps#146](https://github.com/napplet/naps/pull/146)), with
  `neighborhood://` addresses for `relay`, `outbox`, `resource` and `upload`.
- **Removed:** pairing (`myco://pair/`, kinds 9101–9103), the Circle (not
  migrated: people re-invite), general event gossip and backlog resync,
  outbox mesh lanes, NAP-MESH and the App reach setting, manifest forwarding
  between linked phones over their relays, sending files between phones,
  and the Dev peer speed test.

## Earlier releases

**Shipped** (v0.12.0 and v0.12.1 — update notices, private data for napplets, `blossom-local` uploads, NAP-FS, `napplet.link`, Android TV, permission groups, current fips; v0.11.0 — the USB lane (phone to phone and phone to desktop over a cable), a native TUN pump, napplets in the current NIP-5D format and their icons, `https:` resources, `mesh.blobs`, authors' Blossom server lists; v0.10.0 — NAP-UPLOAD, the setup popup and Settings › Permissions, mesh over public internet nodes, app links installing from phones nearby, the SOCKS5 full-tunnel exit; v0.9.1 — Composer and Noris, streamed napplet backlogs, a 22 MB APK; v0.9.0 — NAP-INTENT, the shell cache and NAP-LOCAL, streamed
reads over a shared relay pool, Chronofeed and Simple Profile; v0.8.1 —
local-first napplet reads, author relay lookup, relay skip list and selection; v0.8.0 — accounts, the AppStore napplet, updates over
the Circle; v0.7.0 — the napplet runtime, file sharing, multi-path peering):

- **The mesh.** BLE L2CAP with per-peer PSM discovery, Wi-Fi Aware (several
  phones per lane), the LAN lane (mDNS), TCP when online; multi-path per peer
  with standby links; an app-owned TUN scoped to Myco's uid; `.fips` DNS. The
  Dev tab shows every peer, lane, RTT and connect attempt.
- **Pairing and the Circle** (since replaced by neighborhoods). Mutual,
  signed pairing by NFC bump or QR over the auth service; single-use invite
  secrets; unpairing that reaches the other phone; the Circle gate on the
  relay and Blossom; per-peer permissions stored (no UI yet). Native
  encrypted file sharing between Circle members.
- **nsites.** Paste a link or scan a share; holder-first pull over the mesh,
  then any Circle member, then the internet; staged updates; home-screen pins;
  deep links (`myco://app/<host>/<path>`); a custom relay
  or Blossom instead of the embedded ones.
- **Gossip** (since removed; neighborhoods carry events now). Hop-limited
  push (3) and pull (2) between Circle members, the `MESH` envelope, seen-set
  loop safety, backlog replay on reconnect.
- **Napplets.** NIP-5D manifests fetched by `naddr` or shared by bump;
  verified resolve into a sandboxed iframe; install review and per-app
  permission switches; updates found by an automatic check and forwarded over
  the Circle (download-then-forward, pins never move back); back delivered as
  Escape. NAPs: `shell`, `identity`, `relay` (pool reads, relay-pool publish),
  `outbox` (NIP-65 plans over local/mesh/internet lanes), `mesh` (hop-limited
  publish/subscribe, user-capped — Myco's own NAP-MESH, since removed in
  favour of [NAP-NEIGHBOR](https://github.com/napplet/naps/pull/146)),
  `resource` (`blossom:`, local store first, fetched blobs kept, and public
  `https:`; up to 64 MiB, delivered as bytes), `mesh.blobs` (which Circle
  phones in reach hold a file), `link`, `theme`, `local` (keep and pass on what
  a napplet was shown, [NAP-LOCAL](./design/napplet/NAP-LOCAL.md)), `inc`
  (topic subscriptions), `intent` (open by role —
  [NAP-INTENT](./design/napplet/NAP-INTENT.md)) and `upload` (to the user's
  Blossom servers, signed as them — [NAP-UPLOAD](./design/napplet/NAP-UPLOAD.md)).
- **The shell cache.** A bounded cache for events and blobs apart from what
  the phone keeps; reads stream as results arrive over one pooled connection
  per internet relay.
- **Accounts.** A guest identity from the first launch, `nsec` login and
  logout, and login through a NIP-55 signer (Amber) — N1, N2.
- **AppStore.** The app store is a preinstalled napplet: feed, stacks and
  recommendations from people you follow, profiles, "Around you" over the mesh
  — N5.

**Not built**, from the first plan: NIP-77 negentropy reconcile; eviction of
what the phone keeps (the shell cache evicts to its budget, but the local
relay and Blossom only shrink when the user clears them); transitive
peer-list polling (reach is now each neighborhood's links and hops); Linux interop (P6) as a tested pair; external-browser access
(NAT46). All still on the list below.

---

## Next

Ordered by what unblocks what. Each is its own PR or short series.

### N10 — Connect to public internet mesh nodes

**Done** — built and tested across two networks;
[design/fips/public-mesh-nodes.md](design/fips/public-mesh-nodes.md).
**Goal.** When a phone is online, it can also peer with public FIPS nodes
over the internet, so linked peers who aren't in the same room still reach
each other: neighborhood traffic and app updates over the mesh as if they
were nearby. Today internet reach is only Nostr relays; the mesh itself
stops at the radios and the LAN.

**Shape.**

- A short list of public nodes, shipped and user-editable, dialled over FIPS's
  TCP/UDP transports when the internet is up. They're ordinary peers in the
  mesh routing; they're not linked peers and get no neighborhood traffic.
- **Opt-in, with the trade-off said plainly.** A public node learns this
  phone's IP address and which mesh addresses it talks to, though not the
  contents. A Tor transport is the private alternative where available.
- Battery-aware: connect while online and active, back off in the background,
  never instead of a radio path that works.
- Diagnostics in the Dev tab: which public nodes are up and the path to each
  linked peer.

**Exit criterion.** Two linked phones on different networks, with no radio
path between them, see each other as Here now and exchange a neighborhood
post through a public node; switching the option off drops those links.

### N12 — Permissions held against the manifest

**Goal.** One permission model, worked out from what the installed version
declares. Today a napplet's grants are four hand-kept lists (`granted`,
`denied`, `reviewed`, `unreviewed`); the Permissions sheet lists every domain
Myco implements rather than what the app asked for, and can't say which ones
the app requires. Keeping "never decided" apart from "said no" by hand is
what let switches turned off at install come back on at the next open.

**Shape.**

- Store only the person's **decisions** (domain → allowed or denied). Work out
  everything else from the installed version's **manifest** (`R` / `O`) and
  Myco's **policy** (implemented domains, Basics, the sensitivity table).
- One core function, `permission_view(manifest, decisions, policy)`, gives
  each domain its group, level, required / optional / not declared, decision
  and whether it is granted. The install sheet, the Permissions sheet, the
  open-time check and the update review all use it; Kotlin only draws it.
- A declared domain with no decision is not granted: it is up for review,
  which is the "this update asks for more" case. A decision for a domain the
  current version no longer declares is kept and shown as unused.
- Settle first: a manifest that declares nothing (the toolchain dropped its
  `R` tags), which today gets the default set. Migration: `granted` →
  allowed, `denied` → denied.

**Exit criterion.** The Permissions sheet lists what the installed version
declares, marks what it requires, and warns on switching one off; a switch
turned off at install or later stays off across opens and updates; an update
declaring more puts only the new domains up for review.

### N11 — Share Myco itself

**Built** — update notices: a publisher-signed release event is found on
Zapstore or from a neighborhood with `myco-release` on, and the phone is told (see Status); **Share Myco**
on the Apps tab hands the installed APK to a phone without Myco, over a
hotspot page or the Android share menu; and every phone carries the newest
release APK and shares it into those neighborhoods, so a linked phone updates over the mesh
(`docs/design/core/software-updates.md`). Still to show on two phones: the
exit criterion below.

**Goal.** Hand the Myco app on from inside Myco, so someone without it gets
it from a phone nearby instead of a download. Today every newcomer needs the
APK from GitHub Releases or Zapstore first, which is the one step of joining
a group that needs the internet.

**Shape.**

- **To a phone without Myco.** A **Share Myco** entry that hands over the
  installed APK with what the other phone already has: the Android share
  sheet (Quick Share, Bluetooth), or a QR code to a one-off local download
  served from this phone over the shared Wi-Fi or a local hotspot. The page
  says how to allow the install and how to check it's genuine.
- **Updates from phones nearby.** A phone with a newer Myco shares it into
  its neighborhoods (`myco-release`): the APK as a neighborhood blob,
  announced by a signed release event. The receiver installs only if the APK's signing certificate
  matches its own (Android enforces this for updates anyway) and the release
  is signed by the Myco release key.
- Installing from inside Myco needs `REQUEST_INSTALL_PACKAGES` and the
  PackageInstaller session API. Zapstore and F-Droid builds may need to keep
  that path off, depending on the store's rules.

**Exit criterion.** In airplane mode, a phone without Myco installs it from a
nearby phone and joins a neighborhood; a linked phone on an older version
updates from a
newer one with no internet.

### N13 — File sharing as a napplet

**Goal.** Sharing a file between phones moves out of Myco into a napplet,
filedrop, and Myco keeps only what a napplet can't do itself. Until then
Myco shares files natively with the people in Nearby (the interim share,
built on NAP-NEIGHBOR's `nearby`); this replaces it.

**Shape.** Built on the `feat/share-target-notify` branch, not merged:

- **Share menu into a napplet.** A file shared from another app goes to
  the napplet filling NAP-INTENT's `file` role (`napplet:file/share`), and
  joins its NAP-FS picked files read-only. Design:
  [nap-fs/](design/nap-fs/README.md).
- **Raw uploads.** NAP-UPLOAD bytes reach Rust as raw bytes, not base64,
  up to 64 MiB, as the spec wants: the napplet hands over a Blob and the
  shell never turns it into text
  ([NAP-UPLOAD.md](design/napplet/NAP-UPLOAD.md)).
- **NAP-NOTIFY** ([N4](#n4--notifications)), and delivery while the app is
  closed, so "a file arrived" reaches the user.
- **Filedrop itself:** pick or share a file, send it to chosen
  neighborhoods and/or public Blossom servers, encrypted by default with a
  per-file key that travels only in the link's fragment, and a
  `napplet:file/download` link to fetch it.

**Open first.** Upstream: the `file/share` convention and handing a shared
file into NAP-FS, NAP-UPLOAD's `destination` (napplet/naps#33), delete and
expiry, and saving a fetched file straight to a picked file (raw SVG may
not be delivered to a napplet). Neighborhood blob fetches beyond one hop.

**Exit criterion.** A photo shared from the gallery reaches a phone in a
neighborhood through filedrop, encrypted, with a notification while the
receiver's filedrop is closed; the interim native share is removed.

### N9 — An onboarding flow for permissions

**Built** — `onboarding/MeshSetup.kt` (the decisions, unit-tested),
`ui/onboarding/MeshSetupDialog.kt`, `ui/screens/PermissionsSettings.kt`.

**One code path per prompt.** `onboarding/SystemAsk.kt` is the registry:
each prompt's permissions or intent, whether it is granted, and its
explanation. `SystemAsker.kt` is the only code that launches one.
`ui/onboarding/ExplainCard.kt` is the one explanation; its button ("Allow
nearby devices", "Allow VPN", …) is the only tap that can ask.
`SystemAskTest` fails the build if a prompt is asked anywhere else.

**The popup.** It shows over the app after the intro, with a four-step bar
(Install Myco · Nearby devices · Connection · Name):

- "Enable mesh?", then a "Nearby devices" card, then a "Mesh connection"
  card. Each card says what Android is about to ask before its button puts
  the prompt up; a card is skipped when Android has nothing left to ask.
- The name comes last, edited in place and prefilled with the phone's own
  name. Until it is answered the radios advertise the generated name, not
  the phone's.
- A refusal gets its own card: nearby refused, VPN refused, or another
  app's always-on VPN.

**When it shows.**

- A fresh install sees all of it. "No thanks" leaves the mesh off and goes
  on to the name.
- An upgrade sees the mesh steps only when the mesh is on and something it
  needs is missing. Its grey button is "Not now", which leaves the mesh
  running as it was.
- The mesh switch and every Fix open it on "Enable mesh?". Android is asked
  nothing until "Yes, enable".

**Settings › Permissions** lists nearby devices, the mesh connection,
notifications and the battery exemption. The last two are not part of the
popup.

**Goal.** A first run that explains what Myco needs before Android asks:
Bluetooth, Nearby Wi-Fi devices (Wi-Fi Aware), notifications, and the
one-time VPN consent for the mesh adapter. Before N9 the prompts fired at
launch with no context, and a refused one left a lane silently off until a
Settings warning caught it.

**Exit criterion.** On a fresh install every lane the phone supports comes up
once the user says yes, and nothing is asked without a sentence about why.

### N1 — Account and login (nsec)

**Built** — `account.rs`, `user_key.rs`, `guest_avatar.rs`; the Account page in
`AccountSettings.kt`. See [napplet-runtime.md](./design/napplet/napplet-runtime.md) §7.1.

**Goal.** Every install has a Nostr identity from the first launch, the person
can see it, take its key out, log out, and log in as someone else. What a
napplet publishes is then *them*.

- **First launch.** The user key is generated at startup, not on first napplet
  use; existing installs without one get it on their next launch. It is still
  a separate key from the device key.
- **Guest profile.** A new identity publishes a kind 0 named
  `Myco Guest NNNNN` (the five-digit suffix stays), about
  "I'm a guest user of the Myco app. Join me at https://getmyco.app". Existing
  guests are not re-published.
- **Guest picture.** The Myco logo, bundled in the APK as one compressed base
  image, with its gradient recoloured from the npub so guests look different
  from each other. Generated on device, stored in the local Blossom and
  uploaded to a few public Blossom servers (Primal and others); `picture`
  names it by sha256.
- **Settings header.** The top of Settings shows the logged-in user (avatar,
  name, short npub), like the account row in Android's own Settings. Tapping
  it opens the **Account** page.
- **Reveal the nsec.** On the Account page, as other Nostr apps do. A warning
  dialog comes first: never share this key with anyone; whoever has it *is*
  you. Then show and copy.
- **Logout is a real logout.** The key is removed from the device; napplets
  have no identity until the person logs in again. Before logout, the app
  offers to reveal the nsec, because a guest key that was never copied out is
  gone for good.
- **Login** offers three options: **generate a new identity** (a new guest),
  **log in with nsec** (paste), and **log in with a signer** (Amber, N2).

**Exit criterion.** A fresh install shows a guest account in the Settings
header, and its kind 0 and picture are visible from a public Nostr client.
The nsec can be revealed after a warning. Logout leaves no user key on disk.
Logging in with a pasted nsec makes `identity.getPublicKey()` return that key,
and `relay.publish` signs with it.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) §7.1
(two identities) · [identity.md](./design/core/identity.md) §2 (storage).

### N2 — Login with Amber

**Built** — `external_signer.rs`, `app.myco.signer`; see
[napplet-runtime.md](./design/napplet/napplet-runtime.md) §7.1. Encryption
(`nip44_encrypt` / `nip44_decrypt`) is carried for `relay.publishEncrypted`.

**Goal.** The third login option from N1, straight after it. The key lives in
Amber (NIP-55); it never enters Myco. The runtime's `Signer` gets a second
implementation: `public_key` comes from Amber once, via a `nostrsigner:`
intent. Signing goes through Amber's content resolver with no UI when the
user chose "remember", and through an intent otherwise. `publishEncrypted`
becomes possible the same way. If Amber is uninstalled or refuses, that
signature fails and the napplet is told so; the account stays logged in with
Amber, and Myco never quietly switches back to a guest.

**Exit criterion.** Logged in with Amber, a napplet's `relay.publish`
produces an event signed by the Amber key. No key material is ever on disk
or in memory in Myco. Logout forgets the Amber pubkey.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) §7.1 ·
[NIP-55](https://github.com/nostr-protocol/nips/blob/master/55.md).

### N3 — Drop mesh from nsites

**Built** with neighborhoods (unreleased): local event gossip and REQ
forwarding are gone, so what an nsite publishes on the loopback relay stays
on this phone. Reaching people is a napplet's job, through NAP-NEIGHBOR.
Still to check: the chat nsite in the demo set.

**Goal.** An nsite talks to `ws://localhost:4870` like any relay. Before
neighborhoods, an event it published there was also flooded to paired
phones, and its `REQ`s were recreated against them. Reaching people is now a
*granted* capability (NAP-NEIGHBOR), and an nsite has no grant and no
review screen. Make the loopback relay
socket **local-only**: nsite publishes are stored and shown here, forwarded
nowhere; nsite subscriptions are not replayed to peers. Reaching the room is
what a napplet is for.

**Exit criterion.** An nsite's publish is not seen on a linked phone; a
napplet's publish into a neighborhood still is; the chat nsite in the demo set is either
ported to a napplet or documented as local-only.

**Design docs.** [nap-neighbor/](design/nap-neighbor/README.md) ·
[nsite-permissions.md](./design/nsite/nsite-permissions.md) §3 (the `Origin`
question this closes).

### N4 — Notifications

**Goal.** NAP-NOTIFY for napplets — `notify.show` from a napplet becomes an
Android notification in Myco's channel, tapping it deep-links back into the
napplet — with the grant on the review screen and the permissions sheet. The
Kotlin half that notified of file offers (`FileOfferNotifier`) went with file
transfer; its channel setup and deep link are in git history to start from.
A closed napplet cannot notify (no background execution); a doorbell that
should ring while the app is closed needs a Myco-side subscription. Built,
while the window is open, on the `feat/share-target-notify` branch (design
in its `docs/design/nap-notify/`), for
[N13](#n13--file-sharing-as-a-napplet).

**Exit criterion.** A napplet with `notify` granted posts a notification while
its window is open; without the grant the call is refused; the notification
opens the napplet.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) S3 ·
[NAP-NOTIFY](https://github.com/napplet/naps/pull/11) (registry draft).

### N5 — An app store napplet in place of the Discover tab

**Done** — the AppStore napplet ships preinstalled (`DEFAULT_NAPPLETS`) and
the native Discover tab is gone (v0.8.0, completed since); tested. It lists
napplets from relays and from the phones around you ("Around you": the napplet
manifests held in neighborhoods, through NAP-NEIGHBOR), with stacks
(NIP-51 app sets), recommendations from people you follow and profiles;
Install hands the app's `naddr` to Myco over NAP-LINK, which opens the
install review.

**Goal.** Retire the built-in Discover tab and ship "around me" as a
**napplet** — the first-party app store. It lists what the phones around you
hold, lets you install with one tap,
and surfaces new arrivals — all through the NAPs everyone else gets: `neighbor`
for the room, `outbox` for reach beyond it, `resource` for icons, `intent`
(N6+) to hand an install to Myco. Dogfoods the runtime on the one feature that
needs every mesh capability, and lets the store evolve like any other app —
shared, updated and forked over the mesh — instead of being frozen into a
release.

**Exit criterion.** The Discover tab is gone; the store napplet ships
preinstalled, lists the same holders and apps the tab did, installs from the
list, and works with no internet. Needs an install intent (a napplet asking
Myco to fetch and review an app by pointer) that cannot skip the review
screen.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) S2b
(intents) · [nap-neighbor/](design/nap-neighbor/README.md).

### N6 — Release the napplet runtime

**Done** — v0.7.0 (2026-09-16), followed by v0.8.0 (2026-09-26).

**Goal.** Cut v0.7.0 from `feat/napplet-runtime` after the two-phone checks:
share a napplet by bump with no internet; doorbell rings across phones; a
picture loads by `blossom:` from the other phone's store; permissions switch
live. README and the intro diagrams updated to say "apps", not "sites".

**Exit criterion.** Tagged, on GitHub Releases and Zapstore
([publish.md](./how-to/publish.md)); the demo runbook passes on two phones.

### N7 — NAP-INTENT: open another napplet by role

**Done** — `nap/intent.rs`, `nap/inc.rs`, `myco-core/src/intent.rs` (slices 1
and 2 in v0.9.0) and the Android bridge (slice 3); tested. See
[NAP-INTENT](./design/napplet/NAP-INTENT.md).

**Goal.** A napplet asks Myco to open "a `note` viewer" or "a `profile`" — a
role (archetype), never a specific app — and Myco picks the handler from the
installed napplets (the user's default, or an "Open with…" choice), opens its
window and delivers the payload. The spec is small ([NAP-INTENT](https://github.com/napplet/naps/blob/master/naps/NAP-INTENT.md):
`invoke`/`open`, `available`, `handlers`, `intent.changed`); the work is
around it. Manifests already carry `archetype` tags (`manifest.rs`), and the
design is written ([napplet-runtime.md](./design/napplet/napplet-runtime.md)
D11, §5.4). Sized at roughly 1.5–3k lines across Rust and Kotlin, in three
slices:

1. **Open by role, no payload.** `intent.available`, `intent.handlers`,
   `intent.invoke` for `action: "open"`; a role → installed-napplet index
   rebuilt on install/remove/update; a per-role default only the user can set;
   an "Open with…" chooser sheet; launch or focus the handler's window. Enough
   for "open a note viewer", and for AppStore to offer **Open** on an
   installed app instead of a greyed-out Add.
2. **Payloads over NAP-INC.** The spec delivers the payload over NAP-INC
   topics (or as initial state on a cold start), so this slice brings NAP-INC
   in with it. Payloads reach only the resolved handler; targeting a specific
   napplet (`handler: "<dTag>"`) needs a user grant.
3. **The Android bridge and the rest.** Android intents in and out through the
   same resolver (§5.4), `intent.changed`, more actions and conventions.

**Exit criterion.** A napplet's `intent.open("note", …)` opens the user's
default note napplet (or asks, first time), which receives the payload; a
napplet can't force routing to a napplet the user didn't choose; AppStore's
installed-app page offers **Open**.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md) D11,
§5.4, S2b · [NAP-INTENT](https://github.com/napplet/naps/blob/master/naps/NAP-INTENT.md).

### N8 — A shared relay pool

**Built** in v0.9.0 — `relay_pool.rs` over rustic-applesauce's `RelayPool`.

**Goal.** One WebSocket per internet relay, shared by every napplet,
subscription and lookup, the way other Nostr clients work. Today each one-shot
query and each subscription lane dials its own socket: every read pays DNS, TCP,
TLS and the WS upgrade again, and the per-napplet stream bound (16 lanes) is
spent on lanes, not relays. On a device, a napplet that opens many
subscriptions (the AppStore) hit that bound 41 times in one session, so most of
its reads fell back to a one-shot pull instead of a live stream.

**Shape.**

- A pool keyed by normalised relay URL.
- Each connection multiplexes many REQs by subscription id and fans events back
  to their owners (napplet sessions, subscriptions, lookups).
- Connections are opened lazily. They close after an idle period with no REQs,
  and reconnect with backoff, re-sending live REQs with `since`.
- Limits count sockets (per relay and in total), not subscriptions.
- One-shot queries, manifest lookups, relay-list lookups and account publishes
  go through the same pool, so a warm connection serves all of them.
- The relay skip list and the internet breaker sit in front of the pool.
  Custom relays keep their own connections.

**Exit criterion.** On a device, AppStore at steady state holds at most one
socket per relay it reads. It never logs `stream bound reached`. A reopen
answers from warm connections without new TLS handshakes.

**Design docs.** [napplet-runtime.md](./design/napplet/napplet-runtime.md),
"Local first: reads are streams, not requests" and "Relays that keep failing
are skipped".

---

## Later

Each its own milestone with its own design pass. Roughly in order of pull.

- **Reclaiming event-cache disk sooner.** The shell cache evicts to its
  budget, but LMDB never shrinks its file in place: space freed by eviction
  is reused, and after "Clear cache" the file shrinks only at the next start.
- **Pruning of kept events (profiles, relay lists, manifests).** The local
  relay keeps these kinds when a lookup sees them, and only "Clear local database"
  removes them. Bounded in practice by small, replaceable kinds, but not by a
  limit — [nsite-layer.md](./design/nsite/nsite-layer.md) §2.1, "Events kept
  as they pass".
- **Set reconciliation (NIP-77 negentropy)** between links in a
  neighborhood, so backlog catch-up is a sync rather than a replay —
  [propagation.md](./design/nsite/propagation.md) §5.
- **Larger files in Nearby.** A shared file is capped at 64 MB, as the
  old file transfer was: it is held whole in memory on the way in, out and
  back, a link fetch takes at most 64 MiB, and a neighborhood keeps 256 MiB
  of files. Larger needs streaming end to end (copy into the store while
  hashing, serve from the file, fetch to a temp file checked against the
  hash), Nearby's quota bounded by free disk, progress on the sheet, and
  saying which lane a file goes over: on Bluetooth a few hundred MB is over
  an hour — [proximity.md](design/nap-neighbor/proximity.md#files).
- **Neighborhood follow-ups.** Pushing blobs by the per-neighborhood blob
  hops, history from further than one hop (backlog hops), reading one peer's
  view, `neighbor.invite` for napplets, and Tor or public endpoints in
  offers — [nap-neighbor/](design/nap-neighbor/README.md).
- **Cheaper neighborhood stores.** Each neighborhood has its own LMDB store,
  so replaceable and deletion rules stay inside it. Open ones are never
  closed, an event kept in two neighborhoods is stored twice, and each
  reserves its own map (256 MiB, 32 MiB on 32-bit). Ideas: close stores
  idle for a while, size the map from the quota, share event bodies by id
  with a per-neighborhood index, and one store for `nearby` that is simply
  wiped — [data-plane.md](design/nap-neighbor/data-plane.md#scoped-store).
- **Peer permissions UI.** The per-peer record exists (`relay_write`,
  `relay_read_multihop`, …) with defaults for everyone; a switch per app
  peer — [nsite-permissions.md](./design/nsite/nsite-permissions.md) §2.
- **Your own profile from your own relays.** The Settings profile is fetched
  from the default relays and the indexers, not the user's NIP-65 write
  relays — the last open part of
  [#67](https://github.com/Origami74/myco/issues/67).
- **Sensitive kinds in the permission model.** Answer what a blanket `relay` grant leaves
  open today: a napplet signs any kind as the user — profile (0), contacts
  (3), relay list (10002), deletions (5) — with no prompt. Sensitive
  replaceable kinds want a separate grant or a per-event confirmation; a
  napplet naming its own relays (`options.relay`, conformant under shell
  policy — [napplet-runtime.md](./design/napplet/napplet-runtime.md) S2)
  may want an allowlist or a grant of its own.
- **Neighborhood rate limits.** A napplet granted a neighborhood can
  publish or pull as often as it likes; each publish travels up to the
  neighborhood's hops, and one misbehaving app can saturate the BLE lane for
  the room. A per-session or per-link token bucket is the cheap fix
  ([napplet-runtime.md](./design/napplet/napplet-runtime.md) S3).
- **Open nsite windows and updates.** A napplet window keeps the version it
  opened and offers a restart when the served version moves on (v0.8.0); an
  open nsite window can mix old and new files when an update is applied — the
  §5.2 open-window gate was never built —
  [#71](https://github.com/Origami74/myco/issues/71).
- **Private data with a signer app.** With Amber not set to "always allow",
  reading private data asks for approval per event, and a window waits on
  it: a plaintext cache, and no approval screens from background frames —
  [#161](https://github.com/Origami74/myco/issues/161).
- **`blossom-local` upstream.** Propose the rail for NAP-UPLOAD in
  napplet/naps, so other shells can offer the same mesh-only upload.
- **"Recently updated" on the Apps screen.** Needs a local "activated at" time
  recorded when an app's served version moves —
  [#69](https://github.com/Origami74/myco/issues/69).
- **More NAPs.** `storage` (per-napplet key-value), `config`; `resource`
  beyond `blossom:` and `https:` (`nostr:`, SVG rasterization — and HTML with
  inline SVG, which the sniffer misreads as SVG today). `intent` + `inc` are
  N7; `link` and `theme` shipped in v0.8.0 —
  [napplet-runtime.md](./design/napplet/napplet-runtime.md) S2b–S4.
- **Background subscriptions.** A Myco-side subscription that survives the
  napplet's window closing, so a doorbell can ring with the app closed. Needs
  the notification path (N4) and a battery story.
- **External browsers (NAT46 / `.nsite`).** Let system Chrome reach a site;
  the in-process gateway serves only Myco's own WebViews —
  [ports.md](./reference/ports.md) §3.
- **Linux interop.** An Android phone and a Linux `BluerIo` node as a tested
  BLE pair; the per-peer PSM patch already makes it possible —
  [ble-interop.md](./design/fips/ble-interop.md).
- **Multi-persona.** More than one device key per phone —
  [identity.md](./design/core/identity.md) §3.
- **Public-node peering** over the internet via FIPS discovery kinds —
  [nostr-kinds.md](./reference/nostr-kinds.md).

---

## The first plan, for the record

| Phase | Was | Landed |
| --- | --- | --- |
| P0 | scaffold, FIPS up, identity persisted | v0.1 |
| P1 | BLE peering with per-peer PSM discovery, developer UI | v0.1 |
| P2 | relay + Blossom + gateway, an nsite as a full-screen task | v0.2 |
| P3 | pairing + sync over the mesh; Circle; discovery | v0.3 |
| P3.5 | the consumer UI (bottom-nav shell, sheets, intro, names) | v0.4 |
| P4 | the two-phone airplane-mode demo | v0.4 |
| P5 | propagation at scale | partly: gossip planes, backlog replay (since replaced by neighborhoods); not eviction or negentropy |
| P6 | Linux interop | not as a tested pair |

Beyond it: Wi-Fi Aware and the LAN lane (v0.5–0.6), multi-path (v0.6), file
sharing, NFC, deep links, custom stores, and the napplet runtime (unreleased).
