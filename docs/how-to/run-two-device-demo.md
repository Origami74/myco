# Run the two-device offline demo

The **v1 target** for Myco: a two-device Android demo over **BLE**, fully
offline. One phone is seeded with an nsite (synced once while online, or
side-loaded); the other QR-pairs to it and browses that site through the in-app
WebView — with **both devices in airplane mode** (BLE on). No internet, no
Wi-Fi, no cell. This is the headline "Pillars of Propagation over crappy links"
scenario. (The app never authors nsites; it stores, serves, and replicates sites
authored elsewhere by external tooling.)

This is a forward-looking runbook. Steps that depend on an unbuilt phase
describe the *intended* procedure and are marked **(once Phase N lands)**.

Companion diagrams:
[02-pairing-transitive-discovery.svg](../design/diagrams/02-pairing-transitive-discovery.svg),
[03-offline-propagation.svg](../design/diagrams/03-offline-propagation.svg),
[04-nsite-browse-flow.svg](../design/diagrams/04-nsite-browse-flow.svg).
For the concepts behind identity, `.fips`/`.nsite`, and the relay+Blossom store,
see [../design/concepts.md](../design/concepts.md),
[../design/identity-pairing.md](../design/identity-pairing.md), and
[../design/propagation.md](../design/propagation.md).

> Design doc for a not-yet-built app. Commands are modelled on the reference
> checkouts and adapted. Unverifiable specifics are marked **TBD / open**.

---

## What you need

- **Two physical arm64 Android handsets**, **API 29+** (Android 10 or newer).
  L2CAP CoC and BLE peripheral support are required and are **not** reliable on
  the standard emulator — this demo is physical-device-only.
- Both running a Myco debug build (see [build.md](./build.md)).
- A USB cable + `adb` on the build host (used only for install and log
  inspection — the demo itself is wireless).
- Bluetooth radios that support **BLE peripheral advertising + L2CAP**. Some
  older or budget chipsets advertise GATT fine but reject L2CAP listeners; if
  pairing never completes, suspect the radio first.

We'll call the two phones **A** (source/holder) and **B** (browser).

### Why Android dials and how that shapes the demo

Android cannot bind the fixed default FIPS PSM `0x0085` as a listener:
`listenUsingInsecureL2capChannel()` returns a *dynamically assigned* PSM, while
`createL2capChannel(psm)` can dial any PSM. So the proposed `AndroidBleIo` makes
Android the **central / dialer**, learning each peer's PSM from its BLE advert
and using it in `connect()`. FIPS identifies peers by the in-band pubkey
exchange (`[0x00][pubkey:32]`, 33 bytes), **not** by MAC, so Android MAC
randomization is harmless. (See
[../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs)
and the BLE specifics in [../design/identity-pairing.md](../design/identity-pairing.md).)

For a two-device demo this means **at least one side must successfully dial the
other's dynamically-assigned PSM**. With both devices being Android, the
cross-probe tiebreaker (smaller `node_addr`'s outbound connection wins) decides
which side's dial is kept. **(once the AndroidBleIo BLE transport lands —
Phase: BLE peering.)**

---

## Step 1 — Install on both devices

From the build host, with each device connected in turn:

```sh
# device A
adb -s <serialA> install -r android/app/build/outputs/apk/debug/app-debug.apk
# device B
adb -s <serialB> install -r android/app/build/outputs/apk/debug/app-debug.apk
```

`adb devices -l` lists serials. The `just install` recipe (see
[build.md](./build.md) §3b) installs to whichever single device is attached;
for two devices, address them explicitly with `-s` as above.

Launch the app on each (replace with the real applicationId; `app.myco` is a
placeholder):

```sh
adb -s <serial> shell am start -n app.myco/.MainActivity
```

---

## Step 2 — First-run identity generation

On first launch each device **generates a Nostr keypair (nsec) and persists it**
to the app's `filesDir`. From that one keypair the device deterministically
derives all three addressing forms — `npub`, `node_addr = SHA256(npub)[0:16]`,
and the `fd00::` IPv6 address — so a device's mesh address is stable across
restarts. (Persisting the identity in `filesDir` and deriving the FIPS address
deterministically from the public key are standard FIPS-node behaviour — our
base nostr-vpn does the same; see
[reference/nostr-vpn/README.md](../../reference/nostr-vpn/README.md) and
[../design/concepts.md](../design/concepts.md) "One identity, three derived
forms".)

No account, no server, no sign-up. Confirm each device shows its own `npub` in
the UI (proposed: on the Library/home screen or a settings/identity panel —
exact placement **TBD / open**).

> One identity per device in v1; multi-persona is a later milestone.

---

## Step 3 — Seed a sample nsite on device A

On **A**, seed a small externally-authored nsite so there is something to
browse. The app never authors nsites — it only stores and serves sites authored
elsewhere — so seeding means either **syncing a known public nsite once while
A is still online**, or a one-time **side-load/import** of the externally-created
artifacts (its already-signed manifest event + blobs).
**(once the relay + Blossom + site-entry path lands — Phase: embedded relay +
Blossom.)**

The intended procedure:

1. On A, add the nsite by its **author** identity — `AddNsite { author, dTag? }`,
   which triggers a sync of that site — while A is online; or, for a fully
   self-contained demo, use the dev-only `ImportNsite { … }` side-load to import
   the externally-created artifacts directly. Either way the embedded Blossom
   store ends up holding each file as a **content-addressed blob** (sha256,
   Blossom BUD-01) and the embedded relay holds the author's **nsite manifest
   event** — a Nostr event (kind **15128** root site, or **35128** named site
   with a `d` tag) **signed by the external author**, whose tags map paths to
   blob hashes, e.g. `["path","/index.html","<sha256>"]`. A stores and serves
   that signed event unmodified; it does not author or re-sign it.
   (See [reference/site-deck/docs/nsite-protocol.md](../../reference/site-deck/docs/nsite-protocol.md)
   and [../design/concepts.md](../design/concepts.md).)
2. The site is now reachable on A locally at its nsite host —
   `<npub_author>.nsite` for a root site (resolving to `127.0.0.1`, an A record),
   served by the embedded gateway from the relay (`ws://localhost:4869`) + Blossom
   (`http://localhost:24242`). Note the host is the **author's** npub, not A's
   device npub.
3. **Pin** the site so it is exempt from LRU eviction (sites added to the Library
   are pinned by default; cache cap defaults to 2 GB).

Sanity-check on A *before* going offline, while it can only ever be talking to
its own loopback:

```sh
# from device A's own shell (Termux/adb shell) — loads the site from A's stores;
# host is the nsite AUTHOR's npub, not A's device npub
adb -s <serialA> shell am start -a android.intent.action.VIEW -d "http://<npub_author>.nsite/"
```

(The in-app WebView is the real target; the `am start` above is just a
loopback smoke test. The browser never resolves `.fips` — only relay/Blossom
**sync** traffic uses `.fips`. See [../design/concepts.md](../design/concepts.md)
"`.fips` vs `.nsite`".)

---

## Step 4 — QR-pair the two devices

Pairing exchanges identity so each device can find and authenticate the other
over the mesh. Myco reuses nostr-vpn's QR machinery (CameraX + ML Kit
`BarcodeScanning`, a payload-prefix check, plus a deep-link intent filter; see
[reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt](../../reference/nostr-vpn/android/app/src/main/java/org/nostrvpn/app/QrScannerDialog.kt)).
**(once the pairing UI lands — Phase: QR pairing.)**

1. On **A**, open **Show pairing QR**. A renders a code carrying the proposed
   payload `myco://pair/<base64>` — **npub + memorable name only**. It carries
   **no MAC and no PSM**; those are learned later over BLE adverts.
   (See [../design/identity-pairing.md](../design/identity-pairing.md).)
2. On **B**, open **Scan to pair** and point the camera at A's code. B validates
   the `myco://pair/` prefix, decodes A's npub, and adds A as a known peer
   (so `<npubA>.fips` / `<aliasA>.fips` will resolve).
3. Optionally pair the other direction (A scans B) so the relationship is
   mutual. For one-way browse (B reads A's site) only B needs to know A.

Pairing here only seeds **identity**; it does **not** require connectivity. The
actual radio link is brought up in Step 6.

---

## Step 5 — Go fully offline

On **both** devices:

1. Enable **airplane mode**.
2. Re-enable **Bluetooth** (airplane mode turns it off; toggle BT back on while
   leaving Wi-Fi and cellular off).
3. Confirm there is genuinely no other path: Wi-Fi off, mobile data off. This is
   what makes the demo prove *offline* propagation rather than accidental
   internet reachability.

Start the Myco node / VPN on each device if it is not already running (the
app owns the `VpnService`/TUN that routes `fd00::/8` and DNS-intercepts `.fips`
and `.nsite`; it does **not** capture `0.0.0.0/0`).

---

## Step 6 — Bring up the BLE link and browse

**(once the AndroidBleIo BLE transport + offline re-serve land — Phases: BLE
peering, then offline propagation.)**

1. Each device advertises the 128-bit FIPS service UUID (UUID-only adverts, no
   identity material) and scans. Adverts carry the dynamically-assigned L2CAP
   PSM so the peer can dial it.
2. Android (central/dialer) reads the peer's PSM from its advert and
   `createL2capChannel(psm)` dials it. After the L2CAP connect, the pre-handshake
   pubkey exchange (`[0x00][pubkey:32]`) runs, then **Noise IK** authenticates the
   link. The cross-probe tiebreaker (smaller `node_addr` wins) resolves the
   double-dial. (See
   [../../reference/fips/src/transport/ble/io.rs](../../reference/fips/src/transport/ble/io.rs),
   [discovery.rs](../../reference/fips/src/transport/ble/discovery.rs).)
3. Once the link is up, B and A form a one-hop spanning tree. B can now reach A's
   embedded services over the mesh at `<npubA>.fips:4869` (relay) and
   `<npubA>.fips:24242` (Blossom), via FIPS FSP port-multiplexing — no separate
   gateway needed. (See
   [../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md).)
4. On **B**, open the Library and **Search nsites around me** (query the holder A's
   reachable relay for kind 15128/35128, filtering by `authors` and newest-first,
   dedup by author+dTag). The discovered manifests are simply the author-signed
   manifest events B has received via flood or queried — so the site A holds
   appears.
5. Tap the site. The in-app WebView loads `http://<npub_author>.nsite/` (the
   nsite-deck model: no URL bar; bottom bar is Back · Reload · Library) — the host
   is the **author's** npub. Under the hood B queries the **holder** A's relay
   at `<npubA>.fips` with `{kinds:[15128 or 35128], authors:[<author_pubkey>]}`,
   pulls the author's signed manifest event and the referenced blobs over Blossom
   sync — **on demand** (HYBRID default: flood the author-signed manifests
   widely, pull the large blobs on demand) — caches them in B's own relay +
   Blossom stores, and serves the page from `127.0.0.1`. The site you want
   (author npub) and the peer you fetch it from (holder A at `<npubA>.fips`) are
   different keys.

After this, **B has become a new source**: B's local relay/Blossom now hold the
author's signed events and content-addressed blobs, so B could re-serve the site
to a third device later, even if A is gone. Re-emitting an author-signed manifest
relay-to-relay is normal relay behaviour, not authoring — B never signs anything.
Data is self-authenticating (author-signed events, sha256 blobs), so any holder
is a trustworthy source.
(See [../design/propagation.md](../design/propagation.md) and
[03-offline-propagation.svg](../design/diagrams/03-offline-propagation.svg).)

---

## Step 7 — Verify

The demo is a success when all three hold with **both devices still offline**:

### 7a. `.fips` resolves on B

From B's shell (Termux or `adb -s <serialB> shell`), the mesh name resolves to
A's `fd00::` address (AAAA-only); non-`.fips` queries are refused:

```sh
# resolves to A's fd00:: ULA over the mesh DNS interceptor
dig @10.1.1.1 AAAA <npubA>.fips
# expect an fd00::… answer

ping6 <npubA>.fips        # one-hop reachability A<->B over BLE
```

(`.fips` name resolution for bionic `getaddrinfo` clients — `ping6 <alias>.fips`,
`dig @10.1.1.1` — is the FIPS `.fips` DNS feature; see
[fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md).)

### 7b. The site loads from cache

In B's in-app WebView, the site renders. Then **break the link** — turn
Bluetooth off on B (or walk A out of range) — and **Reload**. Because B cached
the author's signed manifest event and blobs (fetched from holder A) in Step 6,
the page **still loads from B's local stores**. This is the load-bearing proof of
local propagation: the content outlives the live path. **(once offline re-serve
lands.)**

### 7c. It works with no internet

Confirm throughout that both devices remain in airplane mode (Wi-Fi + cellular
off, BT on). If you want belt-and-suspenders proof, watch logcat for the node's
transport selection and confirm only the BLE transport is active, no
UDP/TCP/internet path:

```sh
adb -s <serialB> logcat -s "fips:*" "FipsViewModel:*"
# expect BLE link up to A; no internet transport
```

(Log tags are placeholders adapted from nostr-vpn's debug tooling — see
[reference/nostr-vpn/Justfile](../../reference/nostr-vpn/Justfile);
Myco's actual tag names are **TBD / open**.)

---

## Troubleshooting

| Symptom | Likely cause | Try |
| --- | --- | --- |
| BLE link never forms | Radio rejects L2CAP listener, or no advert seen | Confirm both radios support L2CAP CoC; bring devices within ~1 m; ensure BT re-enabled after airplane mode |
| Pairing QR won't scan | Camera permission, or wrong prefix | Grant camera permission; confirm payload starts `myco://pair/` |
| `.fips` won't resolve | Node/VPN not started, or peer not paired | Start the node on both; re-check Step 4 pairing seeded A's npub on B |
| Site won't load in WebView | Browser tried to resolve `.fips`, or blobs not pulled | The WebView must use `.nsite` (127.0.0.1), never `.fips`; confirm relay/Blossom sync completed in Step 6 |
| Both dial, neither sticks | Tiebreaker confusion | The smaller `node_addr`'s outbound connection should win; check logs for the cross-probe resolution |

---

## Phase dependency summary

| Step | Lands with |
| --- | --- |
| 1 — install | available now (build pipeline, [build.md](./build.md)) |
| 2 — identity | Phase: identity persistence (mirrors nostr-vpn, low risk) |
| 3 — seed nsite | Phase: embedded relay + Blossom + site-entry (sync / side-load) |
| 4 — QR pair | Phase: QR pairing (reuses nostr-vpn machinery) |
| 5 — go offline | available once the node/TUN runs |
| 6 — BLE browse | Phase: AndroidBleIo BLE peering, then offline propagation |
| 7 — verify | follows 6 |

---

## Open questions

- **Identity UI placement (Step 2):** where the device shows its own npub.
  **TBD / open.**
- **Seed UX (Step 3):** how a user seeds an externally-authored nsite on-device
  (`AddNsite { author, dTag? }` sync while online? dev-only `ImportNsite` side-load
  from a file? sample bundled with the app?). The app never authors a site.
  **TBD / open.**
- **Two-Android dialing (Steps 4/6):** both phones are central-only dialers;
  confirm the cross-probe tiebreaker reliably yields exactly one kept L2CAP
  connection between two Android peers. **TBD / open.**
- **Log tag names (Step 7c):** placeholders adapted from nostr-vpn.
  **TBD / open.**
- **Throughput:** BLE L2CAP MTU vs. blob sizes — how large an nsite is
  practical to pull over BLE in the demo. **TBD / open.**
