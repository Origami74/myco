# Myco v0.12.0

**Released**: 2026-10-23

v0.12.0 tells you when **a new Myco is out**, lets napplets keep **private
data** and **files you pick**, groups **permissions the way Android does**,
and runs on **Android TV**.

- Myco checks for a signed new release, from Zapstore and from your Circle,
  and tells you once per version. A phone in mesh-only mode hears it from
  its peers.
- Napplets can keep data encrypted to your own key, so relays and other
  people see only ciphertext.
- Napplets can open and save a file you pick, and only that file.
- Permissions come in groups with an icon and a switch, sorted by how much
  they allow, and you can switch things off before adding an app.
- Myco installs on Android TV and works with a remote.

**No wire-format change from v0.11.0.** v0.12.0 and v0.11.x phones pair and
exchange messages as before, and everything upgrades in place. Update notices
start with this version: a phone on v0.11.x isn't told about v0.12.0.

## At a glance

- **Update notices.** Myco looks once a day for a stable release signed by
  Myco's publisher key, and shows a banner and one notification per version.
  - It asks Zapstore and your Circle. Each phone keeps the newest signed
    announcement, so one phone with internet tells the room.
  - In mesh-only mode only the Circle is asked; Zapstore is never contacted.
  - Settings › Software updates has **Check now** and the automatic switch,
    on by default. Zapstore sees your IP address and a query for Myco.
  - **View update** opens Myco's Zapstore page. Installing stays with
    Zapstore and Android.
- **Private data for napplets.** A napplet hands Myco plaintext; Myco
  encrypts it to your own key (NIP-44), signs and publishes it.
  - Your napplets that can read events (relays, outbox or mesh) get it back
    in plaintext, whichever app wrote it. The install sheet says so.
  - Logged in with Amber, Myco asks Amber to encrypt and decrypt. **If you
    logged in with Amber before this version, log in again** so Amber is
    asked for those permissions.
- **Files you pick (NAP-FS).** A napplet with the new **Files** permission
  can ask you to choose a file, or where to save one, in Android's own file
  picker. It reads and writes that file only, until you close it.
  - A napplet's `<input type="file">` opens the picker too.
- **Permissions in groups.** The install sheet and an app's Permissions page
  group what it may do: Post as you, Nearby devices, Files & media, Profile,
  Apps & links. Each has an icon and one switch, and the details behind a tap.
  - Sorted by level: **acts as you** first, then **sensitive**, then the
    **standard** ones most apps need, folded into one row at install.
  - Switch things off before adding an app. A group the app needs is marked;
    turning it off warns but is allowed.
- **A tidier app menu.** Long-press an app: **Open** sits next to its name,
  then one row of icons (Share, Permissions, Home screen, Updates) and Remove.
  - The share sheet shows its link with **Copy** and **Share**, to send by
    message. On the other phone: open it, or **Add app › Paste a link**.
- **Uploads that stay on the phone.** A napplet can upload to this phone
  only (`blossom-local`), with no internet and no signature, for paired
  phones to fetch. An upload to your servers that fails offline keeps a copy
  on the phone. Each app may keep 64 MiB a day this way.
- **`napplet.link` links open the right app.** `https://napplet.link/<role>/<intent>?…`
  opens whichever installed napplet handles that role, or asks which one.
- **Android TV.** On the home row with its own banner, tabs on a side rail,
  and everything works with a remote: focus rings, Menu (or hold OK) for an
  app's menu. 32-bit TV boxes are supported with `-Pmyco.abis=armeabi-v7a`.

## Faster on slow devices

Getting Myco to run well on a cheap Android TV box turned up work it did on
every phone, all the time. Every phone benefits; a slow or low-memory one most.

- **No more redrawing every screen once a second.** While any phone was
  linked, the once-a-second refresh changed peer timings, signal strength and
  storage counters, and every visible screen and app tile redrew with them.
  On a slow TV box with an accessibility service on, that was a stall of
  several seconds. Now only the screens that show those numbers update: the
  Dev tab, the mesh status panel, and Settings › Storage and › Internet.
  Low-memory devices refresh every two seconds instead of every second.
- **The "no peers" dot repaints only itself.** Its pulse rebuilt the whole
  screen header 60 times a second while you had no peers. On low-memory
  phones and on TVs it no longer pulses; it stays solid red.
- **No stalls while a sync stores files.** The Storage figures were recounted
  by listing every stored file each time a new one arrived, about once a
  second during a sync, while the rest of the app waited. They are now
  counted once and kept up to date.
- **The Apps tab stops redrawing after a search.** The search box kept focus
  after the keyboard closed, and its blinking cursor redrew the tab twice a
  second for as long as it stayed open.
- **Less logging.** The core logs at INFO instead of DEBUG, which saves CPU
  and battery. `adb shell setprop debug.myco.log debug` and a restart turn
  debug logs back on.
- **Apps pause when you can't see them, on TV.** A TV has nothing that sends
  a backgrounded app to sleep, so napplet and nsite windows pause while off
  screen there.
- **Room for 32-bit TV boxes.** On 32-bit builds the local databases map
  256 MiB and the cache defaults are smaller, to fit the address space and a
  TV's flash.

## Also

- **A napplet already on this phone installs offline.** Myco looks in its own
  store first, so a napplet kept from an earlier install no longer fails
  with "no source served artifact".
- **Circle peers stay reachable when the network's DNS isn't Myco's.** Some
  devices send every lookup to their own DNS over HTTPS. Myco now finds
  `<npub>.fips` peers by their mesh address itself.
- **Mesh subscriptions get every peer's answer**, including events this phone
  had already seen and let go of.
- **A napplet's Copy button copies.** Writing the clipboard on a tap works;
  reading it stays blocked.
- **A refused publish says `ok: false`**, as NAP-RELAY and NAP-OUTBOX require.

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
- **A pulled USB cable can linger in the status sheet** if the other phone is
  still reachable another way. The link itself is gone.
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
