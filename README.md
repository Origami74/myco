

# Myco
![](docs/myco-banner.png)

> **Hand a useful app to the people around you.** Phone to phone, and it
> keeps opening when the internet is gone.

Someone in the room has an app that helps: a schedule, a checklist, a game, a
way to ring each other. With Myco they bump phones with you, and the app is on
your phone too. It opens full-screen like any other app, and it still opens
with no signal. Once you have it, you can hand it on to the next person.

|  |  |  |
| :--: | :--: | :--: |
| ![Bump phones to pair](docs/images/zapstore-02-connect-circle.png)<br>**Bump phones to pair** | ![Hand over an app by bumping again](docs/images/zapstore-03-share-app.png)<br>**Hand over an app** | ![People in a park using an app over Bluetooth](docs/images/zapstore-01-mesh.png)<br>**Use it together, offline** |

|  |  |  |  |  |
| :--: | :--: | :--: | :--: | :--: |
| ![Tap to pair over NFC](docs/images/01-nfc-pairing.png)<br>**Bump phones to pair** | ![Your Circle of paired people](docs/images/02-circle.png)<br>**Your Circle** | ![Share an app with someone](docs/images/03-app-sharing.png)<br>**Share an app** | ![Installed apps on the home screen](docs/images/04-home.png)<br>**Your apps** | ![An installed app running full-screen](docs/images/05-bitchat.png)<br>**Apps run full-screen** |

## What you can do with it today

With every phone in airplane mode and Bluetooth on
([the runbook](docs/how-to/run-two-device-demo.md)):

- **Pair** by bumping phones (NFC), tapping someone Myco found nearby over
  Bluetooth, or scanning a QR code.
- **Hand over an app.** The other phone pulls it straight from yours.
- **Open it with no internet**, full-screen, from the Apps grid or a
  home-screen icon.
- **Pass it on.** The phone that received it can share it with someone else,
  even when you have left.

What Myco does not do yet:

- It is **Android only** (Android 10 or newer, 64-bit ARM phones).
- **Myco itself** comes from a download, not from another phone. See
  [Get Myco](#get-myco). Sharing Myco from inside Myco is on the
  [roadmap](docs/roadmap.md#later).
- Phones find each other **only while Myco is open on screen**. A phone in a
  pocket finds nobody.
- **Rebroadcasting Nostr events on purpose.** You can't yet pick events and
  send them on again to other phones or relays.

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
2. **Open it and allow what it asks for:** Bluetooth, nearby devices,
   notifications, and a VPN prompt. The VPN stays on your phone. Myco uses it
   to talk to other phones, not to send your traffic anywhere.
3. **Pair.** Open the **Circle** tab and hold your phone back to back with
   theirs. Or tap them under **Nearby**, where Myco lists phones it found over
   Bluetooth, and they accept. Or they tap **Show my code** and you tap
   **Scan**.
4. **Receive the app.** They long-press the app and tap **Share**. You bump
   again, or scan the code from **Apps › +**.
5. **Open it.** It lands in your **Apps** grid. Some apps first list what they
   want to use, such as nearby phones or your pictures. You choose.

After that the app is yours. It opens without internet, though what it can
show offline depends on the app. You can share it with the next person the
same way.

Pairing works both ways. The person you pair with can send you apps and files,
and you can send them yours. Only people you have paired with can pull from
your phone.

## Organize a group

**Can you get a useful tool to the people here, and keep using it without the
internet? Yes. Here is one full run you can repeat.**

### Example: a workshop app

You run a half-day workshop in a hall with poor signal. The group uses a
[napplet](#build-apps-for-myco) — say, a checklist everyone ticks off
together, passed between the phones in the room.

**Before the day, with internet:**

1. Install [Myco](#get-myco) on your own phone.
2. Add the app: **Apps › +**, then paste its `naddr`. Review what it asks
   for and tap **Add to my apps**. Every file is now on your phone.
3. Ask people to install Myco before they come. Tell them it asks for a VPN
   prompt, and why ([Join a group](#join-a-group), step 2).

**On the day, no internet needed:**

1. Keep Myco open on your screen.
2. Pair with each person: a bump, a tap on them under **Nearby**, or a QR
   scan.
3. Share the app. Each person pulls it from your phone, reviews what it asks
   for, and opens it.
4. People who arrive late can pair with anyone who already has it and get it
   from them.

### What keeps working offline

| Works with no internet | Needs the internet |
| --- | --- |
| Pairing, while both phones have Myco open | Installing Myco itself |
| Handing an app to someone you paired with | Adding an app nobody here has yet |
| Opening any app already on your phone | Public Nostr content, like profiles and the AppStore feed |
| Apps built to talk over nearby phones, while those phones are in reach | Anything an app loads from the web |

### Know before you rely on it

- Phones must be close, and Myco must be on screen, to find each other.
- **Bluetooth is slow for big apps.** Keep what you hand out small, or add it
  before the day. Phones on the same Wi-Fi network use it, and that is faster.
- Myco is not an emergency radio. It does not promise that a message gets
  through, and it does not check who is in charge.

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
- the mesh: publish to and read from nearby paired phones, up to a hop limit
  the person sets,
- pictures and files by content hash, from this phone, a paired phone, or
  public servers.

### nsites, as a bonus

An **nsite** is a static website published on [Nostr](https://nostr.com).
Myco stores its signed files and shows them, so any nsite works offline once
it is on the phone. It asks for no permissions and gets no mesh.

### Sharing an app is not syncing its data

Myco copies an app's files from phone to phone. It does not make an app
collaborative on its own. For people to work together offline, a napplet has
to use the **mesh** capability, and the phones must be in reach of each other
at the time. There is no sync of what was missed yet beyond a short replay on
reconnect.

### From a small example to two phones

1. **Build** a single-file napplet, following
   [napplet.run](https://napplet.run). (Or a static site, for an nsite.)
2. **Publish** it to Nostr. (An nsite: `nsite-cli upload dist`.)
3. **Add it** on one phone while online: **Apps › +**, then paste its
   `naddr` (or the nsite's link).
4. **Hand it over** to a second phone in airplane mode, and open it there.
   Follow [the two-phone runbook](docs/how-to/run-two-device-demo.md).
5. **Test the offline part on real phones.** Bluetooth, NFC and the mesh
   need real hardware. Emulators are not supported.

### How it works underneath

Every phone runs its own Nostr relay and Blossom file store, and keeps the
signed files of every app it holds. That is why a phone that got an app can
hand it on. Phones connect over a [FIPS](https://github.com/jmcorgan/fips)
mesh: Bluetooth (L2CAP), Wi-Fi Aware, the local network, or the internet when
there is one. Links are encrypted. The Circle — the people you paired with —
decides who may pull from your phone.

The reusable content layer (relay, Blossom, gateway, sync) is the
`nsite-deck` crate. `myco-core` wires it to FIPS and the Android app.

Read on:

- [Concepts & glossary](docs/design/core/concepts.md) — start here
- [Architecture](docs/design/core/architecture.md)
- [Napplet runtime](docs/design/napplet/napplet-runtime.md) · [NAP-MESH](docs/design/napplet/NAP-MESH.md)
- [The nsite layer](docs/design/nsite/nsite-layer.md) · [Propagation](docs/design/nsite/propagation.md)
- [Identity & pairing](docs/design/core/identity-pairing.md) · [Security](docs/design/core/security.md)
- [Build from source](docs/how-to/build.md) · [All docs](docs/README.md) · [Roadmap](docs/roadmap.md)

---

## Get Myco

- **Download:** the APK from the
  [latest release](https://github.com/Origami74/myco/releases/latest), or
  through [Zapstore](https://zapstore.dev/apps/app.myco).
- **Needs:** Android 10 or newer, on a 64-bit ARM phone. NFC is optional; QR
  works everywhere.

Myco cannot pass itself on from phone to phone yet
([on the roadmap](docs/roadmap.md#later)). Each person needs the APK
before they can receive apps.

## Status

Myco is released and under active development: see the
[releases](https://github.com/Origami74/myco/releases),
[CHANGELOG.md](CHANGELOG.md) and the [roadmap](docs/roadmap.md). Every install
also has a Nostr account from the first launch: a guest you can keep, or
replace with your own `nsec` or a signer app such as Amber.

> Built on the [FIPS](https://github.com/jmcorgan/fips) mesh, with an embedded
> Nostr relay and Blossom store in Rust and a Compose shell in Kotlin.
