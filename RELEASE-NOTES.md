# Myco v0.13.0

**Released**: 2026-11-12

v0.13.0 **updates Myco from your Circle**, **shares Myco itself** with a phone
that doesn't have it, and **no longer needs a VPN**.

- A phone with the new release passes it to paired phones over the mesh, so
  a room updates with no internet. Android asks before installing.
- **Share Myco** gives the app to a phone without it, over a hotspot or the
  share menu.
- Myco reaches the mesh without taking Android's VPN slot, so your own VPN
  keeps working beside it.
- Napplet games can read a game controller.
- The USB lane comes back by itself after a restart or a quick replug.

**Myco's own messages are unchanged from v0.12**: v0.13.0 and v0.12.x phones
pair and exchange messages as before, and everything upgrades in place. A
v0.12 phone is told about this release; from v0.13 on, phones also carry the
update itself.

## At a glance

- **Updates from your Circle.** Every phone keeps the newest Myco release APK
  and serves it to paired phones over the mesh.
  - It comes from the phone's own install when it is that release, from a
    paired phone, or, on unmetered Wi-Fi outside mesh-only, from the release
    download.
  - It is accepted only as the exact bytes Zapstore's publisher-signed file
    event names, and installed only if it is signed with the release key.
    Android asks before installing.
  - It travels zstd-compressed (25 MB to about 11 MB). Only the newest
    release is kept.
- **Share Myco.** A **Share Myco** bubble on the Apps tab.
  - **Over a hotspot** needs no internet: the other phone joins this phone's
    Wi-Fi, opens the page and downloads Myco. The page shows the APK's
    SHA-256.
  - **Send the app file** hands the APK to the Android share menu (Quick
    Share, Bluetooth, a messenger).
  - The file-share hotspot page on the Circle tab offers Myco too.
- **No VPN needed.** Myco's own relay, Blossom and pairing traffic runs on a
  TCP/IP stack inside the app.
  - The mesh is on from the first launch. Setup asks only for nearby devices
    and your name. After updating, Myco says once what changed.
  - The VPN is still there, off by default, for letting other apps on the
    phone reach the mesh: **Settings › Mesh › Allow other apps to use mesh**.
    The exit proxy still uses it.
- **Controllers for napplet games (NAP-GAMEPAD).** A napplet with the new
  permission reads controllers through `window.napplet.gamepad`, or through
  `navigator.getGamepads()` as on the web.
  - Only the app in front gets live input; others see an idle controller.
- **Smaller APK.** The Myco mark is a WebP: 1.3 MB to 0.3 MB.

## USB and the LAN

- **USB relinks without a replug.** Restarting Myco, or switching the mesh
  off and on, with the cable in brings the link back by itself.
- **Quick replugs work.** Plugging two phones together again and again links
  them every time.
- **Fewer permission prompts.** "Allow Myco to access …" no longer appears on
  every re-enumeration. On stock Android it is once per plug; elsewhere,
  after "Always", not at all.
- **No stale USB after unplugging.** A peer reached only over the LAN no
  longer shows USB.
- **A desktop fips finds phones on the LAN**, not only the other way round.
- **A restarted peer is dialled again over the network**, instead of only
  over the link it had.

## Also

- **Apps survive rotation.** Turning the phone, or resizing the window, no
  longer restarts an open app: it keeps its place and lays itself out for the
  new size.

## Known issues

- **Private data with Amber asks a lot.** Unless Amber is set to "always
  allow", reading private data asks per event, and the app waits:
  [#161](https://github.com/Origami74/myco/issues/161).
- **"Needs this" only at install.** An app's Permissions page doesn't yet
  mark what the app requires, or warn when you switch it off.
- **`blossom:` links are for Myco phones.** Few other Nostr clients open
  them, so a napplet should only post one when there is no `https:` link.
- **Uploads: no per-upload prompt, no progress.** The install-sheet grant is
  the consent; there is no progress bar and no cancel. Metadata such as EXIF
  is the napplet's to strip.
- **`https:` pictures aren't kept across restarts.** Myco remembers a web
  link for five minutes; after that, or a restart, it fetches it again.
- **USB and adb share the cable.** Use wireless debugging to install builds
  while the USB lane is in use.
- **A custom relay is read, not streamed**, and **big files are held in
  memory** while they load (up to 64 MB each).
- **Paid relays** (nostr.wine, for one) refuse Myco's reads, so apps
  published only there can't be found.
- **Open nsite windows and updates.** An update can mix old and new files in
  an open nsite window until it reloads:
  [#71](https://github.com/Origami74/myco/issues/71).
- **No "recently updated" mark on the Apps screen yet:**
  [#69](https://github.com/Origami74/myco/issues/69).
