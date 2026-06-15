# Security & Trust Model

This document describes the security posture proposed for Myco: what is
authenticated, what is authorized, who is trusted, and what is explicitly out
of scope. It is a design document for a not-yet-built app; statements about
behaviour are proposals ("the app will…") unless they cite verified reference
source.

The short version: **Myco trusts data, not peers.** Every artifact it
exchanges — Nostr events and Blossom blobs — is self-authenticating, so any
relay or peer is safe to use as a *source* of that data. A source can withhold
or lie about *availability*, but it cannot forge content. Myco drops the
private-network roster that nostr-vpn used as a membership gate, so there is no
longer an "is this peer a member" question; the only questions left are "is
this packet from who it claims" (yes, by FIPS transport crypto) and "is this
content what it claims" (yes, by signature/hash). Authorization to *transact
data* is therefore implicit in being paired.

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
  ([../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)).
  Every event carries a secp256k1 Schnorr signature over its content and an
  author pubkey. The receiver verifies the signature before trusting the event.
  A forged or tampered manifest fails verification and is discarded.
- **Blossom blobs** (the content layer). Blobs are content-addressed by
  SHA-256 (Blossom BUD-01;
  [../../reference/site-deck](../../reference/site-deck)). The manifest names a
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
([diagrams/03-offline-propagation.svg](diagrams/03-offline-propagation.svg)),
Carl is not trusting *you* — he is verifying Alice's signature and the blob
hashes himself. A new source is as good as the original source. This is the
property that lets relays and blobs hop across "crappy links in all directions"
without a trusted intermediary.

What self-authentication does **not** give you:

- **Freshness / non-equivocation.** A replaceable event (`35128`) can be
  withheld so you keep an older signed version; the signature is still valid,
  so you cannot tell you are behind. There is no global ordering. Mitigation is
  pull-from-many: query every reachable relay and keep the newest valid
  `created_at` (see the "nsites around me" search default). This is best-effort,
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
crypto wholesale by embedding `fips-endpoint` + `fips-core` in-process (the
same embedding nostr-vpn uses,
[../../reference/nostr-vpn/crates/nostr-vpn-cli/src/fips_private_mesh/runtime_send.rs](../../reference/nostr-vpn/crates/nostr-vpn-cli/src/fips_private_mesh/runtime_send.rs)).

- **End-to-end (session layer, FSP): Noise XK.**
  `Noise_XK_secp256k1_ChaChaPoly_SHA256`. The two session endpoints (the two
  npubs actually talking) authenticate each other and encrypt the payload
  end-to-end. Intermediate mesh nodes route on the destination `node_addr` but
  **cannot read the payload**
  ([../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md)).
- **Per-hop (link layer, FMP): Noise IK.**
  `Noise_IK_secp256k1_ChaChaPoly_SHA256`. Each direct link between adjacent
  nodes is independently authenticated and encrypted. A peer you forward
  through sees ciphertext and routing headers, not content
  ([../../reference/fips/docs/design/fips-mesh-layer.md](../../reference/fips/docs/design/fips-mesh-layer.md)).
- **Both layers** use ChaCha20-Poly1305 AEAD, SHA-256 transcript hashing,
  HKDF-SHA256 key schedule, counter-based nonces with a 2048-entry sliding
  replay window, and periodic rekey
  ([../../reference/fips/docs/reference/security.md](../../reference/fips/docs/reference/security.md)).

**BLE links are pubkey-authenticated.** On the offline BLE path
([diagrams/01-system-layering.svg](diagrams/01-system-layering.svg)), after the
L2CAP CoC connect, the peers exchange a pre-handshake pubkey frame
(`[0x00][pubkey:32]` = 33 bytes) and then run Noise IK to authenticate the link
([../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs),
[../../reference/fips/src/transport/ble/mod.rs](../../reference/fips/src/transport/ble/mod.rs)).
Identity is the **pubkey**, never the MAC address — so Android MAC
randomization is harmless, and a spoofed MAC gains nothing because it cannot
complete the Noise handshake. BLE adverts are UUID-only and carry no identity
material, so passive scanners learn only that a FIPS device is nearby, not who.

The crucial inherited principle, carried over verbatim from FIPS:

> **Identity is authenticated; identity is *not* authorization.** Knowing
> cryptographically who sent a packet does not by itself decide whether you
> should act on it
> ([../../reference/fips/docs/design/fips-security.md](../../reference/fips/docs/design/fips-security.md)).

## 3. Dropping the roster: no membership gate

nostr-vpn was a *private network*. Membership was a signed roster (kind
`30388`), join-requests promoted nodes to members, and an admin gated who could
participate. Myco **strips all of that** (a locked decision): there is no
roster, no admin, no membership event, no `.nvpn` MagicDNS, no exit node.

What this changes:

- **There is no "is this peer authorized to be on the network" check.** The
  network is not private. Anyone you pair with (Section 4) becomes a peer and,
  by virtue of being a peer, a data source and a data sink. "Paired" is the
  only relationship.
- **Authorization collapses to data semantics.** Because all data is
  self-authenticating (Section 1), there is little a peer is "authorized" to do
  beyond exchange verifiable artifacts. A peer cannot impersonate an author,
  cannot forge content, cannot read your end-to-end payloads to third parties.
  What a peer *can* do is: offer you sites (which you verify), request sites
  from you (which it verifies), and observe traffic metadata it forwards (it
  routes on `node_addr`, sees ciphertext only).
- **What is therefore "authorized" vs not.** Authorized by being paired:
  flooding you author-signed manifest events (kinds `15128`/`35128`,
  re-emitted unmodified — see the propagation model), pulling content you host,
  forwarding your encrypted mesh datagrams. *Not* conferred by pairing: reading
  your end-to-end payloads to others, forging or altering content, learning your
  identity from a passive BLE scan, or reaching arbitrary localhost services on
  your phone (see below).

**The FIPS optional peer ACL still exists upstream** (`peers.allow` /
`peers.deny`, evaluated at the Noise IK handshake;
[../../reference/fips/docs/reference/security.md](../../reference/fips/docs/reference/security.md)).
Myco's v1 stance is *default-allow* — pairing is the gesture, and we do not
ship a roster-like allowlist UI. Re-exposing the ACL as a "block this peer"
control is a candidate later feature (TBD / open).

**Inbound surface on the mesh.** FIPS FSP port-multiplexing delivers mesh
datagrams to localhost ports, so a paired peer can reach your services at
`<npub>.fips:4869` (relay) and `<npub>.fips:24242` (Blossom)
([../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md)).
On Linux, FIPS recommends a default-deny nftables baseline to bound this
surface; **on Android there is no nftables equivalent the app controls.** The
app's mitigation is to expose *only* the relay and Blossom ports over the mesh
and nothing else — the VpnService/TUN routes only `fd00::/8` and DNS-intercepts
`*.fips`/`*.nsite`; it does **not** capture `0.0.0.0/0`, and the WebView never
resolves `.fips`. So the only inbound app surface a peer sees is "fetch signed
events and content-addressed blobs," which is exactly the self-authenticating
surface from Section 1. **Open question:** confirm that no other localhost
service on the phone is inadvertently reachable via FSP port-multiplexing on a
paired path, and whether the app should enforce an explicit port allowlist on
the inbound mesh side.

## 4. Pairing trust: QR is trust-on-first-use

Pairing is the one moment a human asserts "this is who I think it is."

- The QR payload is `myco://pair/<base64>` carrying an **npub + optional
  memorable name only** — no MAC, no PSM (those are learned later over BLE adverts; see
  [diagrams/02-pairing-transitive-discovery.svg](diagrams/02-pairing-transitive-discovery.svg)).
  Myco reuses nostr-vpn's existing QR machinery (CameraX + ML Kit
  BarcodeScanning, payload-prefix check, deep-link intent filter;
  [../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt)).
- **The trust model is TOFU (trust-on-first-use).** Scanning a QR binds an npub
  to "the device the person across the table showed me." The cryptographic
  guarantee afterward is strong: every later BLE/mesh contact with that peer is
  Noise-authenticated against that exact pubkey, so a man-in-the-middle who did
  not have the private key cannot impersonate the paired peer on subsequent
  connections.
- **What QR does not protect against** is a malicious QR at pairing time. If you
  scan an attacker's npub believing it is your friend's, every later
  authenticated session is faithfully authenticated *to the attacker*. There is
  no out-of-band identity verification (no safety-number comparison) in v1.
  Because npubs are public and self-asserted, the security of pairing rests
  entirely on the human seeing the right screen. **Open:** whether to add an
  optional verification step (e.g. comparing a short authentication string /
  memorable-name confirmation) for higher-assurance pairing.
- **Transitive discovery.** Once paired, peers may learn of *further* peers
  through the mesh. Discovery is not the same as pairing: a transitively
  discovered npub is still just a data source whose every artifact you verify.
  No transitively discovered peer gains any authority a directly paired one
  lacks, because pairing confers no authority beyond "exchange verifiable data."

## 5. The nsite sandbox

An nsite is served to the in-app WebView from **localhost** via the nsite
gateway: `*.nsite` resolves to `127.0.0.1`, the WebView loads `npub.nsite`, and
the bytes come from the local Blossom store after manifest+hash verification
([diagrams/04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg)).

**v1: pure-static (proposed default).** In v1 an nsite is *just signed static
files* — HTML, CSS, JS, images — authored elsewhere by an external nsite author,
verified against that author's signed manifest, and served from localhost. There
is **no capability API**: nsite JavaScript cannot query peers or reach the
relay/Blossom control surface. (The app itself never authors, signs, or
publishes Nostr events at all — it is a relay + cache + browser + propagator for
events authored externally — so there is no signing capability for nsite JS to
reach in the first place.) The threat surface is therefore the ordinary
web-content surface, scoped down:

- nsite JS runs in the WebView's normal sandbox. It is untrusted third-party
  code (the site author is not you).
- **Per-nsite origin isolation is automatic.** Each nsite is its own web origin —
  its `<host>.nsite` hostname — so the WebView partitions storage, cookies, and
  scripting per nsite: one nsite's JS cannot read another's storage/cookies or
  script it. This falls out of each nsite launching as its own fullscreen app
  (its own `NsiteActivity`/WebView instance) at a distinct `<host>.nsite` origin,
  rather than as a tab inside a shared shell. A Content-Security-Policy that
  blocks off-origin/`.fips` access still belongs on top so a malicious nsite
  cannot exfiltrate to a remote server (CSP enforcement is the remaining open
  item, §7). The WebView must **never** resolve `.fips` (locked decision), which
  already keeps nsite JS off the sync transport.
- No `file://` access, no arbitrary network from nsite JS beyond what CSP
  allows. Each nsite is launched fullscreen with **no Myco chrome** — no URL bar,
  no Back/Reload bar; in-app navigation and refresh are the nsite developer's own
  responsibility. Myco does not host a shared navigation surface a site could
  redirect to an attacker origin.

**Later: capability API (call out the risk now).** A future milestone might
propose giving nsite JS a capability API — query peers, write blobs, or other
host affordances. **This is a large trust escalation** and is explicitly *not*
in v1. Note this is independent of authoring: the app's design is that it never
signs or publishes Nostr events on anyone's behalf, and the device's Nostr
keypair is used for FIPS mesh addressing and BLE/link authentication only, never
to author nsites — so exposing a `sign()`-style capability to nsite JS is *not*
on the roadmap. Any capability API that did land would still need: an explicit
per-capability permission model, user-in-the-loop prompts, per-nsite capability
scoping, and a clear UI for what an nsite is asking to do. Designing that is
deferred; v1 sidesteps it entirely by shipping pure-static.

## 6. Threats and mitigations

| Threat | What an attacker gains | Mitigation |
| ------ | ---------------------- | ---------- |
| **Malicious / forging relay or peer** | Tries to serve forged content | **Cannot forge.** Signatures + SHA-256 verified locally (§1); bad artifacts are rejected. |
| **Withholding / availability attack** | Refuses to serve, serves stale, hides a newer event | Pull-from-many: query all reachable relays, keep newest valid event; manifests flood widely (announce-wide) while large blobs are pulled on demand. Best-effort, no freshness guarantee (§1). |
| **Storage-exhaustion DoS** | Floods your cache with junk blobs/events to evict your data or fill the disk | LRU cache (default cap **2 GB**); Library sites are **pinned** (exempt from eviction); per-source caps / rate limits (TBD). Junk that fails verification is never stored (§1). |
| **Identity / link spoofing** | Pretends to be a paired peer | Noise IK/XK over secp256k1; identity is pubkey not MAC; spoof cannot complete handshake (§2). |
| **Replay** | Re-injects captured datagrams | 2048-entry sliding replay window at both FMP and FSP layers (§2). |
| **Malicious QR at pairing** | Gets you to pair with the attacker's npub (TOFU break) | Human verification at scan time; optional safety-string check is an open proposal (§4). |
| **Malicious nsite content** | Untrusted JS in the WebView | Pure-static v1, no capability API; per-nsite origin isolation + CSP; WebView never resolves `.fips` (§5). |
| **Capability-API abuse** (future) | nsite JS reaches host affordances (query peers, write blobs) | Out of scope for v1; the app never signs/publishes events, so no `sign()` capability exists; needs explicit permission model if any capability is ever introduced (§5). |
| **Propagation-privacy leak** | Observers learn what you host / re-serve | See below — partial mitigation only (open). |
| **Metadata / traffic analysis** | A forwarding peer sees who-talks-to-whom | FIPS routes on `node_addr`, payload is end-to-end encrypted; FIPS rejects onion routing, so traffic-graph metadata is visible to forwarders by design ([../../reference/fips/docs/design/fips-mesh-operation.md](../../reference/fips/docs/design/fips-mesh-operation.md)). |

### Propagation privacy — "what am I hosting / re-serving?"

The offline-propagation design makes your device a **source** for sites it has
cached, including sites you merely browsed and then re-serve to others. That is
the point of the mesh, but it has a privacy cost:

- A peer that queries your relay/Blossom can learn **which sites you hold** —
  i.e. infer what you have browsed or chosen to cache. Hosting a site is
  observable.
- Re-serving signed content does not implicate you as its author (signatures
  attribute it to the original author, not the re-server), but *possession* is
  still a signal.

This is an open area. Candidate directions (all TBD / open): a distinction
between *pinned/propagated* sites (Library sites whose manifests you intend to flood
onward) and *transient cache* whose manifests you do not re-emit to peers; a
setting to disable re-serving entirely; not re-flooding manifests for content you
only transiently fetched. v1 should at minimum make "which manifests you are
replicating as a source" visible and controllable in the UI rather than implicit.
**Open question:** default propagation posture — replicate manifests for
everything cached, or only Library-pinned sites?

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
  (`<host>.nsite`), so WebView storage/cookies/scripting partition per nsite and
  one nsite cannot read another's data (§5). **Open:** CSP enforcement on top to
  bound off-origin exfiltration.
- **Open / TBD — which nsite am I in?** With each nsite launching fullscreen and
  **no URL bar or Myco chrome**, a user has no app-supplied indicator of which
  nsite they are currently in. The Recents card title/icon come from the nsite's
  own `ActivityManager.TaskDescription`, which is **author-controlled** — so a
  malicious nsite can present another nsite's title/favicon/colour, an
  impersonation/spoofing risk. How (or whether) Myco surfaces a trustworthy
  "you are in `<host>.nsite`" signal without re-imposing chrome is unresolved.
- **Open:** explicit inbound port allowlist on the FSP-multiplexed mesh surface
  on Android (§3).
- **Open:** optional out-of-band pairing verification (§4).
- **Open:** propagation-privacy controls and default manifest-replication posture (§6).
- **Open:** whether to re-surface the FIPS peer ACL as a "block peer" control
  (§3).

## See also

- [../../reference/fips/docs/design/fips-security.md](../../reference/fips/docs/design/fips-security.md)
  — FIPS mesh-interface threat model (identity ≠ authorization, inbound
  exposure on a flat L3 segment).
- [../../reference/fips/docs/reference/security.md](../../reference/fips/docs/reference/security.md)
  — FIPS cryptographic primitives, rekey/replay defaults, peer ACL format,
  per-transport default exposures.
- [../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md)
  — Noise XK end-to-end session layer and FSP port-multiplexing.
- [../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs)
  — BLE pubkey pre-handshake and the `BleIo` surface Myco implements.
- [../../reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)
  — nsite manifest event format (signed events, path→hash tags).
- Diagrams:
  [01-system-layering.svg](diagrams/01-system-layering.svg) ·
  [02-pairing-transitive-discovery.svg](diagrams/02-pairing-transitive-discovery.svg) ·
  [03-offline-propagation.svg](diagrams/03-offline-propagation.svg) ·
  [04-nsite-browse-flow.svg](diagrams/04-nsite-browse-flow.svg).
