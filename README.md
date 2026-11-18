

# Myco
![](docs/myco-banner.png)

> **Hand a useful app to the people around you.** Phone to phone, and it
> keeps opening when the internet is gone.

Someone in the room has an app that helps: a schedule, a checklist, a game, a
way to ring each other. With Myco they invite you into a neighborhood and hand
the app over, and it is on your phone too. It opens full-screen like any other app, and it still opens
with no signal. Once you have it, you can hand it on to the next person.

<!-- screenshots of the Neighbors tab: TODO -->

|  |  |
| :--: | :--: |
| ![Hand over an app by bumping again](docs/images/zapstore-03-share-app.png)<br>**Hand over an app** | ![People in a park using an app over Bluetooth](docs/images/zapstore-01-mesh.png)<br>**Use it together, offline** |

|  |  |  |
| :--: | :--: | :--: |
| ![Share an app with someone](docs/images/03-app-sharing.png)<br>**Share an app** | ![Installed apps on the home screen](docs/images/04-home.png)<br>**Your apps** | ![An installed app running full-screen](docs/images/05-bitchat.png)<br>**Apps run full-screen** |

## What you can do with it today

- **Invite people into a neighborhood** ("Household", "Sports club") by
  showing a QR code, sending a link, or bumping phones (NFC). Both screens
  show the same short code, so you can check you're joining the right
  person.
- **Meet the phones around you.** **Nearby** links phones in Bluetooth
  reach by themselves, with no invitation and no names. AppStore's
  **Around you** shows the apps they have. It forgets them once nobody is
  in reach. Turn it off on its card in **Neighbors**.
- **Hand over an app.** The other phone pulls it straight from yours.
- **Open it with no internet**, full-screen, from the Apps grid or a
  home-screen icon.
- **Pass it on.** The phone that received it can share it with someone else,
  even when you have left.
- **Link over a USB cable**, phone to phone or phone to a desktop running
  fips, for the fastest transfer ([how](docs/how-to/connect-over-usb.md)).
- **Run it on an Android TV.** Myco shows up on the TV's home row and works
  with the remote; the TV joins the mesh over Wi-Fi or Ethernet.

All of this is tested with every phone in airplane mode and Bluetooth on
([the runbook](docs/how-to/run-two-device-demo.md)).

What Myco does not do yet:

- It is **Android only** (Android 10 or newer, 64-bit ARM phones).
- Phones find each other **only while Myco is open on screen**. A phone in a
  pocket finds nobody.
- **Mesh over the internet** is opt-in (Settings › Mesh › Internet) and not
  yet tested on two phones on different networks. It links to public FIPS
  nodes, which see your phone's IP address but not what you send
  ([how it works](docs/design/fips/public-mesh-nodes.md)).

## Pick your way in

- **[Someone is handing me an app](#join-a-group)** — what to do, and why.
- **[I want to get an app to my group](#organize-a-group)** — setup, and what
  keeps working offline.
- **[I build apps](#build-apps-for-myco)** — what to build, and how it
  travels.

---

## Join a group

**Someone near you uses an app that helps with what you are doing together.
Myco lets them hand it to you in person.**

1. **[Get Myco](#get-myco)** on your phone. This step needs a download.
   Android asks whether your browser may install apps. Allow it for this
   install.
2. **Open it.** The mesh is on from the start. Android asks for **nearby
   devices**, after a card that explains it — tap **Allow nearby devices**.
   Myco uses it to find nearby mesh devices. On Android 12 and older this
   includes **location**: Android requires it to scan for Bluetooth. Myco does
   not record where you are.

   Myco doesn't use a VPN, so a VPN you already run keeps working. To let
   *other* apps on your phone reach the mesh too, turn on **Settings › Mesh ›
   Allow other apps to use mesh**. That one does use Android's VPN slot.

   Myco gives you a generated name, like "teal juno". It is what the people
   you invite see; change it in **Settings** if you like. It is never
   broadcast over Bluetooth, and phones that meet nearby never see it. The
   **camera** is asked for only when you scan a code.
   **Notifications** are optional: turn them on in **Settings › Permissions**.
3. **Join their neighborhood**, in the **Neighbors** tab. They tap
   the **invite** button (the person icon); you tap the **scan** button
   (bottom right) and scan the code on their screen, or hold
   your phone back to back with theirs, or open the link they sent. Pick
   which neighborhoods to join.
4. **Receive the app.** They long-press the app and tap **Share**. You bump
   phones again, or scan the code on their screen from the **scan** button on **Apps**.
5. **Open it.** It lands in your **Apps** grid. Some apps first list what they
   want to use, such as nearby phones or your pictures. You choose.

After that the app is yours. It opens without internet, though what it can
show offline depends on the app. You can share it with the next person the
same way.

### What joining a neighborhood lets the other person do

A neighborhood works both ways. The people you share one with can:

- get your apps and their updates, and Myco updates, while the
  neighborhood's **Share** switches under **Updates** are on (the default),
- receive what your apps post into that neighborhood, and pass it on.

You get theirs the same way, under the **Get** switches.

**Nearby** is a neighborhood too. With it on, phones in reach see which apps
you have, and get your updates, unless you turn its **Share** switches off.

A neighborhood is not private: people in it can pass posts on, even outside
it. Nothing else on your phone is open to them, and people you share no
neighborhood with get nothing. To undo it, **Leave** the neighborhood, or tap
the person in **Neighbors** and choose **Disconnect**.

## Organize a group

**Can you get a useful tool to the people here, and keep using it without the
internet? Yes. Here is one full run you can repeat.**

### Example: a workshop app

You run a half-day workshop in a hall with poor signal. The group uses a
[napplet](#build-apps-for-myco) (a small app), say a checklist everyone ticks
off together. It is passed between the phones in the room, and each phone
picks up the latest ticks from the phones near it, even after joining late.

**Before the day, with internet:**

1. Install [Myco](#get-myco) on your own phone.
2. Add the app. Find it in the preinstalled **AppStore** app. Or, if its
   maker sent you a link (it starts with `naddr`), go to the **scan** button on **Apps** and
   paste it. Review what it asks for and tap **Add to my apps**. Every file
   is now on your phone.
3. Ask people to install Myco before they come. Send them
   [Join a group](#join-a-group): it explains the permission prompts.

**On the day, no internet needed:**

1. Keep Myco open on your screen.
2. Invite the first few people into a neighborhood for the day (**Neighbors ›
   Invite**), then share the app: open its **Share** screen and let them scan
   the code or bump phones.
3. **Let it spread.** Everyone who has the app can invite the next person
   and share it the same way. You
   don't need to reach every person yourself, and people who arrive late can
   get it from whoever is next to them.
4. Each person reviews what the app asks for, and opens it.

### What keeps working offline

| Works with no internet | Needs the internet |
| --- | --- |
| Joining a neighborhood, while both phones have Myco open | Installing Myco itself |
| Handing an app to someone in your neighborhood | Adding an app nobody here has yet |
| Opening any app already on your phone | Public Nostr content, like profiles and the AppStore feed |
| Apps built to talk over nearby phones, while those phones are in reach | Anything an app loads from the web |

### Know before you rely on it

- Phones must be close, and Myco must be on screen, to find each other.
- **Bluetooth is slow for big apps.** Keep what you hand out small, or add it
  before the day. Phones on the same Wi-Fi network use it, and that is faster.
- Myco is not an emergency radio. It does not promise that a message gets
  through, and it cannot confirm that a message came from an organizer.

Try it on two phones first:
[the runbook](docs/how-to/run-two-device-demo.md).

## Build apps for Myco

**What can you build, what can it use, and how does it reach people?**

### Napplets

A **napplet** is a small program in a sandbox
([napplet.run](https://napplet.run), NIP-5D). It is what Myco is built to
run. It asks Myco for what it needs, and gets only what the person grants:

- an identity to sign as,
- Nostr relays: this phone's own relay, and public ones when online,
- neighborhoods: publish to and read from the people the user shares a
  neighborhood with, through NAP-NEIGHBOR
  ([napplet/naps#146](https://github.com/napplet/naps/pull/146)),
- pictures and files by content hash, from this phone or public servers,
- game controllers through `window.napplet.gamepad` or `navigator.getGamepads()`,
  with live input only while the app has focus,
- other apps, by role: "open this profile" opens the person's profile app,
  the one they chose as the default, or the one they pick when asked.

### nsites, as a bonus

An **nsite** is a static website published on [Nostr](https://nostr.com).
Myco stores its signed files and shows them, so any nsite works offline once
it is on the phone. It asks for no permissions and gets no neighborhoods.

### Sharing an app is not syncing its data

Myco copies an app's files from phone to phone. It does not make an app
collaborative on its own. For people to work together offline, a napplet uses
**neighborhoods** (NAP-NEIGHBOR). It gets a `neighborhood://` address and
hands it to its relay calls:

- **Publish** stores a note on this phone and sends it along the
  neighborhood's links, passed on link to link up to the neighborhood's reach.
- **Subscribe** returns matching notes this phone already holds, then asks
  linked phones in the neighborhood for theirs. So a phone that joins late
  catches up from the people it is linked with.
- **Ephemeral notes** (with a NIP-40 `expiration`, such as chat) disappear
  once they expire, so catch-up is only possible until then.

A napplet can also **keep** a note or picture it was shown on this phone, and
**pass on** a note it was shown to a neighborhood or public relays again — only
ever what it was shown, never signed as you.

With your OK on the install sheet, a napplet can also **upload** a file, such
as a picture to post, to your Blossom servers, signed as you. With no
internet it can keep the file on your phone instead. It can also
**open or save a file you pick** in Android's file picker, and only that file.

A napplet can also keep **private data**: Myco encrypts it to your own key
(NIP-44) before it is stored or sent, so relays and other people see only
ciphertext. Your apps that can read your events (relays, outbox or neighborhoods) get
it back in plaintext, whichever app wrote it; the install sheet says so.

### From a small example to two phones

1. **Build** a single-file napplet, following
   [napplet.run](https://napplet.run). (Or a static site, for an nsite.)
2. **Publish** it to Nostr. (An nsite: `nsite-cli upload dist`.)
3. **Add it** on one phone while online: the **scan** button on **Apps**, then paste its
   `naddr` (or the nsite's link).
4. **Hand it over** to a second phone in airplane mode, and open it there.
   Follow [the two-phone runbook](docs/how-to/run-two-device-demo.md).
5. **Test the offline part on real phones.** Bluetooth, NFC and the mesh
   need real hardware. Emulators are not supported.

### How it works underneath

Every phone runs its own Nostr relay and Blossom file store, and keeps the
signed files of every app it holds. That is why a phone that got an app can
hand it on. Phones connect over a [FIPS](https://github.com/jmcorgan/fips)
mesh: Bluetooth (L2CAP), Wi-Fi Aware, a USB cable, the local network, or the
internet when there is one. Mesh links are encrypted. Your relay and file store are never open to other
phones: apps and updates travel only through your neighborhoods, to the
people you set up a link with from an invitation, and with Nearby on, to
phones in Bluetooth reach.

The reusable content layer (relay, Blossom, gateway, sync) is the
`nsite-deck` crate. `myco-core` wires it to FIPS and the Android app.

Read on:

- [Concepts & glossary](docs/design/core/concepts.md) — start here
- [Architecture](docs/design/core/architecture.md)
- [Napplet runtime](docs/design/napplet/napplet-runtime.md) · [NAP-NEIGHBOR](https://github.com/napplet/naps/pull/146) · [NAP-INTENT](docs/design/napplet/NAP-INTENT.md) · [NAP-UPLOAD](docs/design/napplet/NAP-UPLOAD.md) · [NAP-FS](docs/design/napplet/NAP-FS.md)
- [The nsite layer](docs/design/nsite/nsite-layer.md) · [Propagation](docs/design/nsite/propagation.md)
- [Neighborhoods](docs/design/nap-neighbor/README.md) · [Identity](docs/design/core/identity.md) · [Security](docs/design/core/security.md)
- [Build from source](docs/how-to/build.md) · [All docs](docs/README.md) · [Roadmap](docs/roadmap.md)

---

## Get Myco

- **Download:** the APK from the
  [latest release](https://github.com/Origami74/myco/releases/latest), or
  through [Zapstore](https://zapstore.dev/apps/app.myco).
- **From a phone nearby:** someone who has Myco taps the **share** button
  (above the scan button) on the Apps tab. **Over a hotspot** works with no internet: join their Wi-Fi, open
  the page they show you and tap **Download Myco**. **Send the app file**
  sends the APK through Quick Share, Bluetooth or a messenger.
- **Updates:** Myco checks daily for publisher-signed stable releases, including
  while the UI is closed (turn it off in Settings → Software updates). It asks
  `wss://relay.zapstore.dev`, which sees your IP address and a
  query for Myco releases; no user identity or private signing key is sent.
  Release announcements also reach you through your neighborhoods (**Myco
  updates**), so one phone with internet tells the rest, even in mesh-only
  mode.
- **Updates from your neighborhoods:** every phone also keeps the newest
  release APK (from its own install, a neighborhood, or, on unmetered Wi-Fi,
  the release download) and shares it into its neighborhoods. When it's on your phone, **Install** updates Myco with no internet:
  Myco checks it is signed by the publisher, and Android asks before
  installing. Otherwise **View update** opens the Zapstore listing.
- **Installing the APK:** Android asks whether your browser (or file app) may
  install apps. Allow it, install Myco, and you can turn it off again.
- **Needs:** Android 10 or newer, on a 64-bit ARM phone. NFC is optional; QR
  works everywhere.

## Status

Myco is released and under active development: see the
[releases](https://github.com/Origami74/myco/releases),
[CHANGELOG.md](CHANGELOG.md) and the [roadmap](docs/roadmap.md). Every install
also has a Nostr account from the first launch: a guest you can keep, or
replace with your own `nsec` or a signer app such as Amber.

> Built on the [FIPS](https://github.com/jmcorgan/fips) mesh, with an embedded
> Nostr relay and Blossom store in Rust and a Compose shell in Kotlin.
