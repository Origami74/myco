# Security & Trust Model

This document describes Myco's security posture: what is authenticated, what
is authorized, who is trusted, and what is explicitly out of scope. Where a
section describes something not yet built it says so; everything else describes
the app as it stands.

The short version: **Myco trusts data, not peers — and trusts people, not
radios.** Every artifact it exchanges — Nostr events and Blossom blobs — is
self-authenticating, so any relay or peer is safe to use as a *source* of that
data. A source can withhold or lie about *availability*, but it cannot forge
content. There is no network-membership question: FIPS links to whoever is in
range, and that says nothing. The questions that matter are "is this packet
from who it claims" (yes, by FIPS transport crypto), "is this content what it
claims" (yes, by signature/hash), and "do I have a link with this person"
(yes or no, set up from an invitation two people exchanged) — which is what
admits a peer to the content ports and to neighborhood traffic. A fourth question arrived with napplets: "may this *program* do that"
(yes or no, by a grant the user made; §5).

> **Note on the name "FIPS".** Throughout Myco, *FIPS* is the **Free
> Internet Protocol Suite** (the mesh project this app is built on), **not**
> the U.S. NIST Federal Information Processing Standards. Nothing here is a
> claim of FIPS-140 validated cryptography. The primitives below (secp256k1,
> ChaCha20-Poly1305, SHA-256, Noise) are strong modern choices, but they are
> not running in a FIPS-140 validated module and Myco makes no such
> certification claim.

## 1. Self-authenticating data: trust the artifact, not the channel

Myco moves two kinds of artifact, and both are verifiable independently of
who handed them to you:

- **Nostr events** (the relay layer). nsite manifests are signed Nostr events:
  kind `15128` (root site) and `35128` (named site), whose tags map paths to
  blob hashes, e.g. `["path","/index.html","<sha256>"]`
  (`../../reference/site-deck/docs/nsite-protocol.md` (nsite-deck reference)).
  Every event carries a secp256k1 Schnorr signature over its content and an
  author pubkey. The receiver verifies the signature before trusting the event.
  A forged or tampered manifest fails verification and is discarded.
- **Blossom blobs** (the content layer). Blobs are content-addressed by
  SHA-256 (Blossom BUD-01;
  the nsite-deck reference). The manifest names a
  blob by its hash; the receiver hashes the bytes it got and checks them against
  that name. A substituted or corrupted blob has a different hash and is
  rejected.

The consequence is the central security property of the whole propagation
model:

> **Any relay or peer is trustworthy as a *source*.** It can refuse to serve
> you, serve you stale data, or claim a site exists that does not — a
> *withholding* / *availability* attack — but it **cannot forge** content
> attributed to an author it does not control. Signatures and hashes make
> origin and integrity independent of the transport.

This is what makes the offline-propagation design safe. When your local relay +
Blossom server caches Alice's signed events and content-addressed blobs and
later re-serves them to Carl while Alice is unreachable
([diagrams/03-offline-propagation.svg](../diagrams/03-offline-propagation.svg)),
Carl is not trusting *you* — he is verifying Alice's signature and the blob
hashes himself. A new source is as good as the original source. This is the
property that lets relays and blobs hop across "crappy links in all directions"
without a trusted intermediary.

### 1.1 Where signatures are checked

Transport authenticity and event authorship are different claims, and
store-and-forward pulls them apart. FIPS Noise IK plus the identity-derived
`fd00::` address proves **who sent the frame**. It says nothing about **who
signed the event**, because peers routinely hand us events authored by third
parties they have never met.

Manifests are the sharp case. An nsite manifest is authored by its publisher and
the peer relaying it is only a courier, so without verification any linked peer
could hand us a forged manifest for anyone's nsite and we would stage and
activate it as that publisher's site. Content-addressing does not save us: blob
hashes are checked against the manifest, and the manifest is the forged part.

So verification happens **once, at ingress** — the points where a remote event
enters the process. It used to be a habit repeated at nine scattered call sites,
which is a pattern where a single missed one is a silent forgery hole.

Events read back **out** of our own store are not re-verified. NIP-01 already
makes signature checking mandatory for a relay accepting an `EVENT`, so paying
Schnorr again per event on a phone buys nothing. That trade is worth restating if
Myco is ever pointed at a relay it does not own — see
[nsite-layer.md §2.1](../nsite/nsite-layer.md), where that is a planned setting with a
warning attached.

What self-authentication does **not** give you:

- **Freshness / non-equivocation.** A replaceable event (`35128`) can be
  withheld so you keep an older signed version; the signature is still valid,
  so you cannot tell you are behind. There is no global ordering. Mitigation is
  pull-from-many: query every reachable relay and keep the newest valid
  `created_at`. This is best-effort,
  not a guarantee.
- **Author intent / key compromise.** A valid signature proves the key signed
  it, not that the human meant to. A stolen **external author** key (the keys
  that author nsites elsewhere — never a Myco device key) can sign valid
  malicious updates that then propagate. A device that caches and re-serves such
  content is still only a *source*, not the author — signatures attribute it to
  the compromised author key, not to the re-serving device. Myco has no
  revocation mechanism for author keys in v1 (TBD / open — Nostr has no native
  key revocation).

## 2. Transport crypto inherited from FIPS

Myco does not invent transport security; it inherits the FIPS two-layer
crypto wholesale by embedding the upstream `fips` crate in-process.

- **End-to-end (session layer, FSP): Noise XK.**
  `Noise_XK_secp256k1_ChaChaPoly_SHA256`. The two session endpoints (the two
  npubs actually talking) authenticate each other and encrypt the payload
  end-to-end. Intermediate mesh nodes route on the destination `node_addr` but
  **cannot read the payload**
  ([../../reference/fips/docs/design/fips-session-layer.md](../../../reference/fips/docs/design/fips-session-layer.md)).
- **Per-hop (link layer, FMP): Noise IK.**
  `Noise_IK_secp256k1_ChaChaPoly_SHA256`. Each direct link between adjacent
  nodes is independently authenticated and encrypted. A peer you forward
  through sees ciphertext and routing headers, not content
  ([../../reference/fips/docs/design/fips-mesh-layer.md](../../../reference/fips/docs/design/fips-mesh-layer.md)).
- **Both layers** use ChaCha20-Poly1305 AEAD, SHA-256 transcript hashing,
  HKDF-SHA256 key schedule, counter-based nonces with a 2048-entry sliding
  replay window, and periodic rekey
  ([../../reference/fips/docs/reference/security.md](../../../reference/fips/docs/reference/security.md)).

**BLE links are pubkey-authenticated.** On the offline BLE path
([diagrams/01-system-layering.svg](../diagrams/01-system-layering.svg)), after the
L2CAP CoC connect, the peers exchange a pre-handshake pubkey frame
(`[0x00][pubkey:32]` = 33 bytes) and then run Noise IK to authenticate the link
([../../reference/fips/src/transport/ble/io.rs](../../../reference/fips/src/transport/ble/io.rs),
[../../reference/fips/src/transport/ble/mod.rs](../../../reference/fips/src/transport/ble/mod.rs)).
Identity is the **pubkey**, never the MAC address — so Android MAC
randomization is harmless, and a spoofed MAC gains nothing because it cannot
complete the Noise handshake. BLE adverts carry the service UUID, the L2CAP
PSM and Nearby's 4-byte use-once secret (random, tied to no key), and no
identity material, so passive scanners learn only that a FIPS device is
nearby, not who.

The crucial inherited principle, carried over verbatim from FIPS:

> **Identity is authenticated; identity is *not* authorization.** Knowing
> cryptographically who sent a packet does not by itself decide whether you
> should act on it
> ([../../reference/fips/docs/design/fips-security.md](../../../reference/fips/docs/design/fips-security.md)).

## 3. No membership gate on the mesh — links gate the content

The mesh is not private. Any FIPS node in range gets a Noise-authenticated
link and may route your encrypted datagrams; there is no roster, no admin, no
join event, and nothing to gate. What *is* gated is the content, by **links**
([nap-neighbor/](../nap-neighbor/README.md)): pairwise relationships two people
set up from a one-time invitation.

What this means:

- **Being a FIPS peer confers nothing.** A phone can hold a transport link to
  yours for an hour and never read a byte of content. It reaches exactly one
  service: the link service on `:4873`, and only to set up a link it holds an
  invitation for, or, directly in reach with Nearby on, a nearby link after
  proving it heard this phone's Bluetooth secret.
- **A link confers the neighborhoods it carries.** A linked peer may read and
  write the neighborhoods both sides agreed to on that link, through the
  scoped relay. Nothing else.
- **Apps and updates ride the same neighborhoods.** They travel as
  extensions ([extensions.md](../nap-neighbor/extensions.md)): a peer gets
  only the manifests and files this device shares into a neighborhood it
  carries on that link.
- **Not conferred by a link:** reading your end-to-end payloads to others,
  forging or altering content, learning your identity from a passive BLE scan,
  reaching arbitrary localhost services on your phone (§3.1).

**The FIPS optional peer ACL still exists upstream** (`peers.allow` /
`peers.deny`, evaluated at the Noise IK handshake;
[../../reference/fips/docs/reference/security.md](../../../reference/fips/docs/reference/security.md)).
Myco does not use it: the invitation is the gesture. A "block this peer"
control that re-exposes it is a later item.

### 3.1 Inbound surface on the mesh

FIPS FSP port-multiplexing delivers mesh datagrams to localhost ports, so a peer
can reach your services over `.fips`
([../../reference/fips/docs/design/fips-session-layer.md](../../../reference/fips/docs/design/fips-session-layer.md)).
On Linux, FIPS recommends a default-deny nftables baseline to bound this surface;
**on Android there is no nftables equivalent the app controls.** The app's
mitigation is to expose *only* its own ports over the mesh — the VpnService/TUN
routes only `fd00::/8` and answers `*.fips` DNS; it does **not**
capture `0.0.0.0/0`, and the WebView never resolves `.fips`.

One port is exposed:

| Port | Service | Who may reach it |
| --- | --- | --- |
| `4873` | Link service, `/nh/…` | `/nh/nearby`: any direct peer that proves it heard this phone's Bluetooth secret. `setup`: anyone holding a valid invitation. Everything else: a linked peer, by NIP-98 with its link key |

The relay on `4870` listens on loopback only, for the in-app WebView. The
Blossom store has no port.

**Open question:** confirm that no other localhost service on the phone is
inadvertently reachable via FSP port-multiplexing, and whether the app should
enforce an explicit port allowlist on the inbound mesh side.

### 3.2 Relay and Blossom are not on the mesh

Myco's own relay and Blossom are not reachable from other phones, so there
is no gate to get wrong:

- The relay listens on `127.0.0.1:4870` only. Napplets reach it in-process.
- The Blossom store is not served on any port.
- Nothing a peer sends lands in them directly. Neighborhood events and blobs
  go to their own scoped store, through the link service (§3.3).
- Peers cannot upload blobs: the link service only reads blobs, and only from
  a neighborhood's scope.

### 3.3 The link service

Everything on `:4873` lives under `/nh/{pk}/…`, where `{pk}` is this side's
link key for that link.

- **Setup** needs the invitation's one-time secret: the joiner sends
  `HMAC-SHA256(s, A.pk ‖ B.pk ‖ url)`. The secret is single use and expires
  (15 minutes by default).
- **Everything after setup** carries a NIP-98 header signed by the caller's
  link key, fresh within 60 s, for that exact URL. The signer must be the
  link held on that `{pk}`. Each header has a random `nonce`, a `POST` a
  `payload` hash of its body; an accepted header is remembered for 120 s and
  refused if replayed.
- **Refusals say only `refused`.** A stranger learns nothing about which
  check failed. The exceptions say only how to retry: `429` (slow down, or
  too many sockets), `503` (busy), and Nearby's `409 used`.
- **Bodies are capped at 8 KiB.** A setup or offer is a few signed events.
- **Pacing.** Per source address 2 requests/s, bursts of 20 (`429`); all
  sources together 50/s, bursts of 200; 64 requests in flight (`503`). The
  source table is bounded, least recently used out first.
- **Scoped relay sockets.** 4 per link, 64 KiB messages and frames, 20
  frames/s with bursts of 100. A socket closes when its link ends or stops
  carrying its neighborhood, and after 5 minutes idle with no subscription
  open.
- **Outbound, too.** URLs a peer hands over (in an offer or invitation) are
  dialled only if they are `.fips`, a mesh address or the public internet,
  never loopback, LAN, link-local or another ULA, checked again after DNS.
  Answers are read up to 64 KiB, pulls up to 2000 events.
- **Bounded state.** Pending neighborhood additions: 8 per link. Names a
  peer supplies: 64 characters. A `links.json` that can't be read is set
  aside as `links.json.bad`, never overwritten.

### 3.4 What a linked peer can still learn

- **A linked peer** sees the app manifests you share into a neighborhood
  (the update extensions), so it learns which apps you have installed. That
  is metadata, not a content leak — manifests are author-signed and public by
  design (see [propagation.md](../nsite/propagation.md)). Turning **Share**
  off for an extension stops it.
- **A linked peer** sees what you publish into the neighborhoods you share
  with them, and what others pass on. Neighborhoods are **not private**: the
  join sheet says so. Anyone in a neighborhood can pass posts on, even
  outside it.
- **Link keys stop correlation by key.** Each link has its own key, so two of
  your peers cannot tie their links to you by comparing keys. Over FIPS the
  transport still shows the device key, so this holds only off the mesh.
- **Nearby shares with strangers.** Phones in reach get the update
  manifests this phone shares in `nearby`, so they learn which apps it has
  installed. That is what AppStore's Around you shows. Turning **Share**
  off on `nearby`, or Nearby off, stops it ([proximity.md](../nap-neighbor/proximity.md#privacy-cost)).

The coarse knobs are ending a link, dropping a neighborhood from a link,
turning extensions off, and the FIPS peer ACL ("block this peer", above).

## 4. Link trust: an invitation handed over in person

Setting up a link is the one moment a human asserts "this is who I think it
is."

- **The invitation is a bearer secret.** `https://napplet.link/neighborhood/join#invite=…` carries the inviter's
  link service URLs, link public key, a one-time 32-byte secret, the
  neighborhoods offered, an expiry and the inviter's name. Whoever holds it
  first can join. Hand it over in person (QR, NFC tap) or over a channel you
  trust.
- **The web host never sees it.** The payload rides in the URL fragment,
  which a browser never sends, so napplet.link's server learns nothing when
  the link opens there.
- **The join sheet helps check.** It says where the invitation came from
  (link, QR code or phone tap) and shows a short code of the inviter's link
  key, which the inviter's sheet shows too. Anyone can make a link that
  names anybody; comparing the code catches that. Expired invitations
  can't be joined.
- **The proof binds both keys and the URL.** A party that relays the
  invitation cannot reuse the joiner's answer at another endpoint or with
  another key.
- **The channel is authenticated.** Over FIPS, `<npub>.fips:4873` is reached
  over Noise; an on-path party cannot impersonate the inviter's endpoint.
- **Both devices must be reachable at setup time.** There is no deferred,
  offline setup.
- **Names are display hints.** The inviter's name rides in the invitation and
  the joiner's in its setup request, to that one person only. Nothing
  advertises a name over Bluetooth.
- **Adding neighborhoods later asks the user.** A peer can propose a new
  neighborhood on an existing link; it waits until this side's user accepts.
  Removals take effect at once.
- **No transitive authority.** Your peer's peers gain nothing from your link.
  They may receive what your peer passes on in a shared neighborhood, within
  each device's hop limits.

## 5. Two sandboxes: the nsite's and the napplet's

### 5.1 The nsite sandbox — pure static, no capabilities

An nsite is served to the in-app WebView at `http://<host>.localhost/` by the
in-process gateway, after manifest and hash verification
([diagrams/04-nsite-browse-flow.svg](../diagrams/04-nsite-browse-flow.svg)).
It is *just signed static files* — HTML, CSS, JS, images — authored elsewhere by
an external author. There is **no capability API**: nsite JavaScript cannot
query peers, reach the store's control surface, or sign anything. The threat
surface is the ordinary web-content surface, scoped down:

- nsite JS runs in the WebView's normal sandbox as untrusted third-party code.
- **Per-nsite origin isolation is automatic.** Each nsite is its own origin —
  `<host>.localhost` — so storage, cookies and scripting are partitioned per
  site; each launches as its own `NsiteActivity` and WebView.
- The WebView never resolves `.fips`, which keeps nsite JS off the sync
  transport. What it *can* reach is `ws://localhost:4870` — the embedded relay,
  as any local web page could. Events published there stay on this phone:
  reaching other people is a *granted* capability (NAP-NEIGHBOR, below), not
  a side effect of a loopback socket.
- No `file://`, no Myco chrome to redirect, no shared navigation surface.

### 5.2 The napplet sandbox — a program behind a permission model

A napplet is a program, and it gets exactly what the user granted. The design
is in [../napplet/napplet-runtime.md](../napplet/napplet-runtime.md); the
security shape is:

- **Two walls.** The napplet runs in a `sandbox="allow-scripts"` `srcdoc`
  iframe with an **opaque origin**: no storage, no network, no same-origin
  access to anything. Around it is a trusted shell page from the APK, and around
  that the WebView's channel to Rust, scoped to the shell's origin. The napplet
  can only `postMessage` to the shell; a nested frame, a popup, or a message
  from any other source is dropped by a `MessageEvent.source` check.
- **Every capability is mediated.** The napplet describes what it wants
  (`relay.publish {event}`, `mesh.subscribe {filters, ttl}`,
  `resource.bytes {url}`); Rust decides, does it, and returns the result. No
  key material, no socket, no file handle ever crosses into the iframe. The
  user key signs on the napplet's behalf; the napplet never sees it (§7.1 of
  the runtime design).
- **The user key at rest.** `user.nsec` is written atomically with mode
  0600 in the app's private data dir (`allowBackup=false`); logout deletes
  it. It leaves Rust only through the Account page's reveal
  (`accountRevealNsec`, outside the reducer so it is in no state snapshot),
  behind a warning, with screenshots blocked and the copy marked sensitive.
- **Grants are the user's, per call.** What a napplet may do is decided at
  install review — in words, on a screen the fetch cannot skip — and can be
  changed per capability on its sheet. The check is made on **every call**
  and every delivery, so revoking a grant stops the next call, not the next
  launch. Nothing an inbound intent carries can widen a grant: grants are read
  from the library, never passed at open.
- **A napplet's identity is its bytes.** The aggregate hash over the manifest
  is the session identity; a different build is a different napplet, and a
  manifest whose files do not hash to it never gets a session.
- **What a grant lets a napplet do to you** is said plainly on the review
  screen because it is real: `relay`, `outbox` and `neighbor` let it publish
  *as you*, to your relays or into any neighborhood (a napplet with
  `neighbor` sees all of them), with no per-event prompt.
- **`neighbor` changes settings for everyone.** `neighbor` is a default
  grant. As NAP-NEIGHBOR requires, `create`, `leave` and a `setOffer` that
  widens access ask the user. A napplet with `neighbor` can mute authors or
  narrow an offer without asking, and that applies to every napplet, since neighborhoods are the
  device's, not the napplet's.
  `resource` lets it fetch content by hash; a fetched blob is kept here, but
  not served to other phones (the runtime design, §7.11).
- **Napplet-supplied addresses are validated.** A relay URL a napplet names
  must be `ws`/`wss` and may not point at loopback or a private network; a
  `.fips` URL is refused. People nearby are reached through `neighborhood://`
  addresses the runtime minted for that napplet, and refused in any other. Blob fetches are content
  addressed, so there is no address to abuse.

## 6. Threats and mitigations

| Threat | What an attacker gains | Mitigation |
| ------ | ---------------------- | ---------- |
| **Malicious / forging relay or peer** | Tries to serve forged content | **Cannot forge.** Signatures + SHA-256 verified locally (§1); bad artifacts are rejected. |
| **Withholding / availability attack** | Refuses to serve, serves stale, hides a newer event | Pull-from-many: query all reachable relays, keep newest valid event; manifests are shared into neighborhoods while large blobs are pulled on demand. Best-effort, no freshness guarantee (§1). |
| **Storage-exhaustion DoS** | Floods your cache with junk blobs/events to evict your data or fill the disk | No peer reaches the relay or Blossom at all, and **peers cannot upload blobs** (§3.2), so a peer cannot push bytes onto your disk. Neighborhood events land in their own scoped store, only from links carrying that neighborhood. Junk that fails verification is never stored (§1). What only passes through lands in the **shell cache**, which evicts to its budget (500 MB of events, 1.5 GB of blobs by default; [architecture.md](./architecture.md), "Kept and cached"). What this phone **keeps** — its own publishes, installed and opened apps, private messages addressed through it — is still unbounded until the user clears it. |
| **Identity / link spoofing** | Pretends to be a linked peer | Noise IK/XK over secp256k1; identity is pubkey not MAC; spoof cannot complete handshake (§2). |
| **Replay** | Re-injects captured datagrams | 2048-entry sliding replay window at both FMP and FSP layers (§2). |
| **Stolen or relayed invitation** | Joins a link meant for someone else | The secret is single use and expires in 15 minutes; the setup proof binds both link keys and the URL, so a relayed answer is useless elsewhere (§4). A stolen, unused invitation still works: it is a bearer secret, so hand it over in person. Out-of-band verification is an open proposal. |
| **DoS on the link service** | Floods `:4873`, the one port open to strangers, to burn a BLE radio | Per-address pacing (2 requests/s, bursts of 20, then 429), a global pace (50/s, bursts of 200), 64 in flight, an 8 KiB body cap, 4 relay sockets per link; Nearby caps 32 ephemeral links and 12 setups a minute. Setup costs one HMAC check, and every other route a Schnorr check before anything is stored (§3.3). |
| **Flooding a neighborhood** | Fills a neighborhood's store to push out real posts | Each neighborhood keeps at most 10 000 events and 256 MiB of blobs here, oldest out first; events a peer returns must match the request; leaving deletes what only that neighborhood held ([policy.md](../nap-neighbor/policy.md#limits)). |
| **Stranger writing to your event store** | Gets data into a store you may not own | Link setup never touches the event store. The relay is loopback only (§3.2); the scoped relay admits only a link carrying the neighborhood named. |
| **Revoked peer keeps reading** | A peer whose link ended keeps streaming | Every link service request is NIP-98-signed by a link this device still holds. An open scoped relay socket closes when its link ends or stops carrying the neighborhood (§3.3). |
| **Malicious nsite content** | Untrusted JS in the WebView | Pure-static, no capability API; per-nsite origin isolation; WebView never resolves `.fips` (§5.1). |
| **Malicious napplet** | Untrusted program asks for more than it should, or tries to go around the seam | Opaque-origin iframe; every capability mediated and checked per call against the user's grants; no key material in the iframe; refused calls are logged (§5.2). |
| **A napplet publishing as you** | A granted `relay`/`outbox`/`neighbor` napplet posts without asking | By design and said in words at install; revocable per capability, effective on the next call; how far a neighborhood post travels is the neighborhood's reach, set by the user. |
| **Napplet-named relay as SSRF** | A napplet points the shell at a private host | Relay URLs validated: `ws`/`wss` only, no loopback or private ranges; `.fips` refused (§5.2). |
| **Malicious Myco update** | Gets a phone to keep, serve or install an APK that isn't the release | The release APK is accepted only as exactly the bytes a publisher-signed file event names (size and SHA-256), from any source; the file event must be named by the newest publisher-signed announcement and name the release key. Android re-checks package, a higher `versionCode` and the signing key before its own install prompt, and refuses another key regardless ([software-updates.md](./software-updates.md), "Carrying the APK"). |
| **Update withholding or stalling** | A linked peer refuses the update, drips it, or serves junk, so others don't update | Links are asked one at a time; each download has a size-based time budget and the carry a 15-minute deadline; the carry never holds the update check. A peer can delay an update, never forge it; any other link or (online) the CDN still serves it. |
| **Propagation-privacy leak** | Observers learn what you host / re-serve | See below — partial mitigation only (open). |
| **Metadata / traffic analysis** | A forwarding peer sees who-talks-to-whom | FIPS routes on `node_addr`, payload is end-to-end encrypted; FIPS rejects onion routing, so traffic-graph metadata is visible to forwarders by design ([../../reference/fips/docs/design/fips-mesh-operation.md](../../../reference/fips/docs/design/fips-mesh-operation.md)). |

### Propagation privacy — "what am I hosting / re-serving?"

The offline-propagation design makes your device a **source** for sites it has
cached, including sites you merely browsed and then re-serve to others. That is
the point of the mesh, but it has a privacy cost:

- A linked peer sees **which apps you share** into a neighborhood it
  carries — i.e. what you have installed. Hosting an app is observable to
  your links. Your relay, Blossom and shell cache are not reachable from
  other phones, so what your apps read is not.
- Re-serving signed content does not implicate you as its author (signatures
  attribute it to the original author, not the re-server), but *possession* is
  still a signal.

Only what this device actively shares travels: an update it installed, in
neighborhoods where it shares that extension. Transient cache is never
re-emitted. The control is the per-neighborhood **Share** switch
([extensions.md](../nap-neighbor/extensions.md)).

### Expiring events are on disk

Events with a NIP-40 `expiration` (chat) used to live only in memory, by design
— "a conversation in the room is not a record on the phone". Since the shell
cache they are stored like other events, in the local relay and the cache, and
swept within a quarter hour of expiry; they are never served past it. That was
a deliberate change — chat survives a restart until it expires — and it costs
the old property:

- Until the sweep, an expired message is still in the database file.
- An LMDB delete frees pages without scrubbing them, so swept messages can be
  recovered forensically from the file until those pages are reused.

Each expiring event's id is written to `relay/expiring.log` **before** the event
is saved, so a crash cannot leave a message the sweep never hears of.

## 7. Explicit non-goals and open questions

- **Not FIPS-140.** Restated: Myco makes **no** US-NIST FIPS-140 validated
  cryptography claim. "FIPS" = Free Internet Protocol Suite (§ note at top).
- **No anonymity / onion routing.** Inherited from FIPS, which deliberately
  rejects onion routing; forwarders see the traffic graph. Myco does not add
  an anonymity layer.
- **No author-key revocation** in v1 (no native Nostr mechanism). A compromised
  **external** author key (never a Myco device key) can sign valid malicious
  updates that propagate until users stop following it; a device re-serving them
  is a source, not the author.
- **No freshness guarantee.** Self-authentication proves origin/integrity, not
  recency; withholding a newer replaceable event is undetectable in general.
- **Per-nsite origin isolation** is automatic: each nsite is its own origin
  (`<host>.localhost`), so WebView storage/cookies/scripting partition per nsite and
  one nsite cannot read another's data (§5). **Open:** CSP enforcement on top to
  bound off-origin exfiltration.
- **Open / TBD — which nsite am I in?** With each nsite launching fullscreen and
  **no URL bar or Myco chrome**, a user has no app-supplied indicator of which
  nsite they are currently in. The Recents card title/icon come from the nsite's
  own `ActivityManager.TaskDescription`, which is **author-controlled** — so a
  malicious nsite can present another nsite's title/favicon/colour, an
  impersonation/spoofing risk. How (or whether) Myco surfaces a trustworthy
  "you are in `<host>`" signal without re-imposing chrome is unresolved.
- **Open:** explicit inbound port allowlist on the FSP-multiplexed mesh surface
  on Android (§3).
- **Open:** optional out-of-band link verification (§4).
- **Open:** whether to re-surface the FIPS peer ACL as a "block peer" control
  (§3).

## See also

- [../../reference/fips/docs/design/fips-security.md](../../../reference/fips/docs/design/fips-security.md)
  — FIPS mesh-interface threat model (identity ≠ authorization, inbound
  exposure on a flat L3 segment).
- [../../reference/fips/docs/reference/security.md](../../../reference/fips/docs/reference/security.md)
  — FIPS cryptographic primitives, rekey/replay defaults, peer ACL format,
  per-transport default exposures.
- [../../reference/fips/docs/design/fips-session-layer.md](../../../reference/fips/docs/design/fips-session-layer.md)
  — Noise XK end-to-end session layer and FSP port-multiplexing.
- [../../reference/fips/src/transport/ble/io.rs](../../../reference/fips/src/transport/ble/io.rs)
  — BLE pubkey pre-handshake and the `BleIo` surface Myco implements.
- `../../reference/site-deck/docs/nsite-protocol.md` (nsite-deck reference)
  — nsite manifest event format (signed events, path→hash tags).
- Diagrams:
  [01-system-layering.svg](../diagrams/01-system-layering.svg) ·
  [03-offline-propagation.svg](../diagrams/03-offline-propagation.svg) ·
  [04-nsite-browse-flow.svg](../diagrams/04-nsite-browse-flow.svg).
