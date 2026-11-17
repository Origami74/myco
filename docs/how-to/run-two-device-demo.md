# Run the two-device offline demo

Two Android phones, both in airplane mode with Bluetooth on: link them in a
neighborhood, share an app from one to the other, and open it — no internet, no Wi-Fi, no
servers. This is the check that the whole stack works, and the manual test
every mesh, neighborhood or BLE change needs before it ships (host `cargo test`
cannot see any of it).

---

## What you need

- Two Android phones, **Android 10+ (API 29)**, with **BLE L2CAP CoC** support
  (any phone from the last few years). No emulator: BLE, Wi-Fi Aware and NFC
  need hardware.
- The debug APK, built per [build.md](./build.md), on both.
- An app to share. Any nsite reachable from the internet will do for seeding
  (paste its link into phone A while A is online); a napplet by `naddr` works
  the same way.
- Optional: NFC on both, for the bump. QR works everywhere.

---

## Step 1 — Install and first run

```sh
adb devices -l
adb -s <A> install -r android/app/build/outputs/apk/debug/app-debug.apk
adb -s <B> install -r android/app/build/outputs/apk/debug/app-debug.apk
```

Open Myco on each. The intro runs once; the mesh is on from then. The setup popup
opens on the "Nearby devices" card: tap **Allow nearby devices** and allow Android's prompt. There is no VPN step: Myco reaches the mesh without one.
Its last step is the
**name** — the memorable label the other phone will see, prefilled with the
phone's own name and edited in place. Notifications are
separate, in Settings › Permissions. Each phone generates its **device key** on this launch; the
Settings › Identity page shows the npub.

Settings › Mesh should show **Enable** on, **Bluetooth** on. Leave Wi-Fi Aware
on if the phones support it; it only adds a faster lane.

## Step 2 — Seed an app on phone A (online, once)

On **A**, with internet: Apps › the **scan** button › paste a link. An nsite link (`npub1…`,
`<host>.nsite.lol`, or a bare host label) syncs the manifest and blobs from the
public relays and Blossom servers; an `naddr…` fetches a napplet and shows the
install review — tap **Add to my apps**. The tile appears in the grid, with a
progress ring until every file is local.

Long-press the tile and **Add to Home screen** if you want the full effect.

## Step 3 — Invite B to a neighborhood

On both phones open the **Neighbors** tab. Wait until the mesh bar shows the
other phone: B joins over the mesh, so it needs a route to A.

- On **A** tap the **invite** button, tick a neighborhood or **+ New neighborhood** (say "Household"),
  and keep the sheet open. It shows a QR code and a share link, and presents
  the same `https://napplet.link/neighborhood/join?invite=…` invitation as an NFC tag.
- On **B** tap the **scan** button and scan the code, or hold the phones together. B
  shows who is inviting and to what; tick the neighborhood and **Join**.

B sets up a link with A's link service (`<npubA>.fips:4873`). Both phones now
list the neighborhood and each other under **People**, by the names from
Step 1. Leave both update switches on for the neighborhood (the default):
Step 5 fetches the app through it.

## Step 4 — Go fully offline

On both phones: enable **airplane mode**, then turn **Bluetooth back on**
(airplane mode switches it off). Wi-Fi and cellular stay off. If you left Wi-Fi
Aware enabled, it does not need Wi-Fi to be on.

Within a few seconds the mesh bar should show the other phone over BLE — the
phones found each other's L2CAP listener from the scan advert and formed a Noise
link. Within 30 s the person shows under **Here now**.
The Dev tab lists the peer with its transport, RTT and every path fips holds.

## Step 5 — Share the app

On **A**: long-press the tile › **Share**. A QR appears — and, while that sheet
is open, the same payload is presented over NFC.

On **B**: Apps › the **scan** button, or hold the phones together. The share
payload names the app. B looks for it in the neighborhoods where it gets
updates: the manifest and files come from A over the link service
(`<npubA>.fips:4873`) — the ring fills — and the tile appears. Nobody else is
asked, and with the internet off there is no other source.

### What A can hand over

A link serves only what is in the neighborhood's scope. An app gets there
when A **shares a new version** of it (an update A installed after joining).
An app A merely has installed is not in scope, so offline B finds nothing.

To demo the offline path, publish a new version of the app between Step 3 and
Step 4 and let A update to it while both phones are online. Otherwise keep the
internet on for this step.
For a napplet, B sees the install review first; its capabilities are listed in
words. Add it.

Tap the tile on B. It opens full-screen as its own task, served from B's own
relay and Blossom.

## Step 6 — Verify

- **Both phones are offline.** Pull down quick settings on each: airplane mode
  on, Wi-Fi off.
- **B serves from its own store.** Kill Myco on A (or walk it out of range);
  B's app still opens and its pages still load. Settings › Storage on B shows
  the event and blob counts that grew in Step 5.
- **Logs.** `adb -s <B> logcat | grep myco` during Step 5 shows the pull from
  `<npubA>.fips:4873/nh/…`.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| The other phone missing from the mesh bar | BLE off after airplane mode; phones too far; one phone's radio rejected the L2CAP listener | Toggle Bluetooth on; bring within ~1 m; check the Dev tab's attempt log (`connect-timeout`, `pool-rejected`) |
| Join fails ("refused" or no answer) | No mesh route to A yet; the invitation expired (15 min) or was already used | Wait for the mesh bar to show A, then make a fresh invitation |
| Share scanned, but nothing arrives | **Get** is off on B, or **Share** is off on A, for that kind of update in the neighborhoods the phones share | Turn it on for the neighborhood |
| Share scanned, ring never fills | No mesh route yet — the link came up after the pull started | Wait for the lane icon, then scan again |
| "Couldn't find this app" on a napplet share | Same as above, or the link dropped mid-fetch | **Try again** on the sheet |
| App opens but a napplet says a capability was refused | Not granted at install (declared nothing) | Long-press › **Permissions** |
| Everything works until the phones are apart | That's the mesh: live-path only. The app stays, the feed does not | Expected — B now holds the app; a third phone linked to B in that neighborhood can get it from B |
| VPN consent dialog again | "Allow other apps to use mesh" is on and the TUN was revoked (another VPN, or the system) | Accept; Settings › Permissions › VPN for other apps › Fix re-prompts. Myco's own mesh is unaffected |
| "Another VPN is always on" card | "Allow other apps to use mesh" is on, and another app is the always-on VPN | Settings › Network & internet › VPN › that app: turn off Always-on VPN, then Try again — or leave other apps off the mesh |

---

## What this exercises

| Step | Layer |
| --- | --- |
| 1 | device identity (4), radios (4) |
| 2 | nsite/napplet sync from the internet (1, 3) |
| 3 | a link from a one-time invitation over the link service, a neighborhood, its update extensions (2) |
| 4 | BLE L2CAP link, Noise, the TUN (4) |
| 5 | share payload, fetching through a neighborhood over `:4873`, install review and grants (1, 2, 3) |
| 6 | store-and-forward: B as a holder (3) |
