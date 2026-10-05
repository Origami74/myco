# Myco v0.10.0

**Released**: 2026-10-05

v0.10.0 lets apps **post pictures**, **explains every permission** before
Android asks, and can reach your Circle **over the internet**.

- Apps can upload a file, such as a picture to post, to your Blossom servers.
  They can do this only if you allow it.
- First launch walks you through the mesh setup, one card per Android prompt.
- Opt-in: link to public mesh nodes when you're online, so your Circle can
  reach you when you're not in the same room.
- An app shared as a link installs from phones nearby, even with no internet.
- Wi-Fi and Wi-Fi Aware come back after the mesh restarts, and phones on the
  same Wi-Fi keep finding each other.

**No wire-format change from v0.9.1.** v0.10.0 and v0.9.x phones pair and
exchange apps and messages as before. Everything upgrades in place.

## At a glance

- **Apps can upload files.** An app that asks for it, and that you allow on
  the install sheet ("Upload files"), can put a file on your Blossom servers,
  signed as you. Doodle Duo uses it: after a round it uploads the score
  picture and opens Composer with the link in the note.
  - Myco uses your own server list. Only if you have none does it use public
    servers (Primal, hzrd149). If your list can't be found, nothing is
    uploaded.
  - With a signer app (Amber), you approve each upload there.
  - Up to 16 MiB, one at a time. Nothing is uploaded with offline-only on.
  - Apps you installed before that want it ask you again.
- **A setup popup on first launch.** "Enable mesh?", then a card before each
  Android prompt (nearby devices, then the VPN) saying what it is for, then
  your name. Android asks nothing until you tap the card's button.
  - Until you choose a name, Bluetooth advertises a generated one, not the
    phone's own.
  - A refused step gets a card saying what won't work and how to retry,
    including when another app's VPN is set to always on.
  - Upgrading with a working mesh: no popup.
- **Settings › Permissions.** Nearby devices, the mesh connection,
  notifications and (optional) keeping Myco running in the background, each
  with its state and a Fix or Allow. Notifications are no longer asked for at
  launch; the camera is asked for only when you tap "Allow camera".
- **Mesh over the internet (opt-in).** Settings › Mesh › Internet links this
  phone to up to two public FIPS nodes while it is online. A public node sees
  this phone's IP address and which mesh addresses it talks to, not what you
  send. It never dials with offline-only on.
- **App links work offline.** Opening a napplet link with no internet used to
  fail even with the app on a paired phone next to you. Myco now asks the
  sharer and every Circle phone in reach first, all at once, and takes the
  files from the quickest.
- **Apps open with less black screen.** About 570 ms instead of 670–780 ms for
  a fresh Noris open on a mid-range tablet.
- **Full-tunnel exit (experimental).** Set the exit node as
  `socks5://<exit-npub>.fips:1080` to send all of the phone's TCP traffic, and
  DNS, through the exit, not only apps that honour the HTTP proxy. UDP is
  dropped, so apps fall back to TCP.

## Also

- **Wi-Fi Aware on Android 12.** It can now be allowed: Android 12 ignores a
  request for precise location alone, and Myco now asks for approximate too.

- **Wi-Fi and Wi-Fi Aware after a mesh restart.** A phone could come back
  connected over Bluetooth only until Myco was force-stopped. Both lanes now
  come back.
- **Phones on the same Wi-Fi keep finding each other.** A phone leaving the
  network at the wrong moment could stop new connections over that Wi-Fi
  until Myco was restarted.

## Known issues

- **Mesh over the internet is new.** It hasn't yet been tested with two
  phones on different networks reaching each other through a public node.
  It is off by default.
- **Web pictures in apps.** Napplets load files by their hash (`blossom:`).
  Plain web images, including many profile pictures, show a placeholder (🥀 in
  Chronofeed). `https:` image support is still to come;
  [#67](https://github.com/Origami74/myco/issues/67) covers Blossom server
  discovery.
- **Uploads: no per-upload prompt, no progress.** The install-sheet grant is
  the consent; without a signer app, an allowed app uploads without asking
  each time. Files go up as the app sent them (EXIF and other metadata are
  not stripped). There is no progress bar and no cancel.
- **Composer can't attach a picture itself yet.** An app that uploads one,
  such as Doodle Duo, can hand Composer the link; pasting a Blossom link also
  works.
- **Few apps declare a role yet.** Opening "a profile", "an article" or "a
  composer" needs an app whose manifest declares that archetype, such as
  Simple Profile, Noris or Composer; other napplets don't yet. `article`,
  `highlight` and the `composer` convention are ours, not yet in the NAAT
  registry. Messages between apps other than opening by role (`inc.emit`)
  aren't routed yet.
- **Reading positions in Noris stay on the phone.** Boris publishes them as
  public events; Noris keeps them on the device instead, because Myco can't
  encrypt for an app yet (`relay.publishEncrypted`). Without app storage they
  last for the session.
- **A custom relay is read, not streamed.** With a custom relay set, reads wait
  for it rather than streaming as they arrive.
- **Big files are held in memory** while they load (up to 64 MB each).
- **Paid relays.** Relays that require a paid account (nostr.wine, for one)
  refuse Myco's reads, so apps published only there can't be found.
- **Open nsite windows and updates.** An update can mix old and new files in
  an open nsite window until it reloads:
  [#71](https://github.com/Origami74/myco/issues/71).
- **No "recently updated" mark on the Apps screen yet:**
  [#69](https://github.com/Origami74/myco/issues/69).
