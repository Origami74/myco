# Run the two-device offline demo

Two Android phones, both in airplane mode with Bluetooth on: pair them, share
an app from one to the other, and open it — no internet, no Wi-Fi, no
servers. This is the check that the whole stack works, and the manual test
every mesh, pairing or BLE change needs before it ships (host `cargo test`
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

Open Myco on each. The intro runs once; then you are asked for a **name** —
the memorable label the other phone will see. Accept the permission prompts
(Bluetooth, nearby devices, notifications, and the VPN consent for the
app-owned TUN). Each phone generates its **device key** on this launch; the
Settings › Identity page shows the npub.

Settings › Mesh should show **Enable** on, **Bluetooth** on. Leave Wi-Fi Aware
on if the phones support it; it only adds a faster lane.

## Step 2 — Seed an app on phone A (online, once)

On **A**, with internet: Apps › **+** › paste a link. An nsite link (`npub1…`,
`<host>.nsite.lol`, or a bare host label) syncs the manifest and blobs from the
public relays and Blossom servers; an `naddr…` fetches a napplet and shows the
install review — tap **Add to my apps**. The tile appears in the grid, with a
progress ring until every file is local.

Long-press the tile and **Add to Home screen** if you want the full effect.

## Step 3 — Pair

On both phones open the **Circle** tab.

- **Bump:** hold the phones back to back. Each presents its invite as an NFC
  tag and reads the other's; both add each other and both show a toast. Done.
- **QR:** on A tap **Show my code**; on B tap **Scan** and point at it. B sends
  a signed pair request over the mesh to A; A, still on the Circle tab, accepts
  it automatically because the code it just showed is the one being answered.
  (Off the Circle tab, A gets an accept/ignore prompt instead.)

Either way the pairing is **mutual**: both phones list the other in Circle.
The row shows the name from Step 1 and, when the mesh link is up, a lane icon.

## Step 4 — Go fully offline

On both phones: enable **airplane mode**, then turn **Bluetooth back on**
(airplane mode switches it off). Wi-Fi and cellular stay off. If you left Wi-Fi
Aware enabled, it does not need Wi-Fi to be on.

Within a few seconds the Circle rows should show the BLE lane lit — the phones
found each other's L2CAP listener from the scan advert and formed a Noise link.
The Dev tab lists the peer with its transport, RTT and every path fips holds.

## Step 5 — Share the app

On **A**: long-press the tile › **Share**. A QR appears — and, while that sheet
is open, the same payload is presented over NFC.

On **B**: Apps › **+** › **Scan**, or hold the phones together. The share
payload names the app *and* A as its holder, so B pulls the manifest and
blobs straight from A over the mesh — the ring fills — and the tile appears.
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
- **Discover works.** On B, the **Discover** tab lists what A holds — the
  "around me" query to A's relay over the mesh — as long as A is reachable.
- **Logs.** `adb -s <B> logcat | grep myco` during Step 5 shows the pull from
  `<npubA>.fips:4870` / `:24243`, and `accepted a mesh event` lines as gossip
  arrives.

For a napplet with `mesh` granted, the doorbell test: ring on A, `mesh.event`
lands on B (logcat: `napplet published to the mesh` on A, `accepted a mesh
event` on B).

---

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| No lane icon on the Circle row | BLE off after airplane mode; phones too far; one phone's radio rejected the L2CAP listener | Toggle Bluetooth on; bring within ~1 m; check the Dev tab's attempt log (`connect-timeout`, `pool-rejected`) |
| Pair request never accepted | A left the Circle tab before B scanned, so the request needs a manual accept | Look for the prompt on A; or re-show the code (it rotates) |
| Share scanned, ring never fills | No mesh route yet — the link came up after the pull started | Wait for the lane icon, then scan again; the pull retries the holder first |
| "Couldn't find this app" on a napplet share | Same as above, or the sharer's link dropped mid-fetch | **Try again** on the sheet |
| App opens but a napplet says a capability was refused | Not granted at install (declared nothing) | Long-press › **Manage permissions** |
| Everything works until the phones are apart | That's the mesh: live-path only. The app stays, the feed does not | Expected — B is now a holder; a third phone can pull from B |
| VPN consent dialog again | The app-owned TUN was revoked (another VPN, or the system) | Accept; Settings › Mesh › Enable re-prompts |

---

## What this exercises

| Step | Layer |
| --- | --- |
| 1 | device identity (4), radios (4) |
| 2 | nsite/napplet sync from the internet (1, 3) |
| 3 | pairing over the auth service, the Circle (2) |
| 4 | BLE L2CAP link, Noise, the TUN (4) |
| 5 | share payload, holder-first pull over `.fips`, install review and grants (1, 2, 3) |
| 6 | store-and-forward: B as a holder; discovery; gossip (3) |
