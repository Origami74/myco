# Field Findings — Phase 01 (Make Peering Observable)

Real-hardware defects surfaced *by* the Phase 1 instrumentation while executing Phase 1.
These are peering faults, not instrumentation faults — they belong to **Phase 2**.
Recorded here so they survive the session and feed Phase 2 planning.

Observed 2026-08-05 on two devices, both running the 01-02 Task 1 debug build:

- Pixel 7 Pro (`29131FDH3007HW`)
- Samsung SM-A528B (`R5CR916CDCF`)

---

## F-01 (blocking, demo-critical): an already-running node never drives a newly injected BLE bridge

**Symptom:** After a fresh app start, `BleService` comes up and the Dev tab's new observed
`scanning` fact reads `idle` indefinitely. No BLE scanning, no BLE advertising, `RADIO ADVERTS (0)`.
All peering falls through to the Wi-Fi/LAN (`!FIPS` AP) lane. Two phones in the same room with no
shared network would not connect at all.

**Controlled evidence (Samsung, single process, pid 717):**

| Window | `MycoBleRadio` log lines |
|---|---|
| First 8m29s after app start | **0** |
| After the user restarted the node — *same process, same `BleService`, same bridge handle* | **29**, scanning + `advertising PSM 131` |

The only variable between the two windows is the node restart. Reproduced on the Pixel: its
long-lived prior process was scanning normally; the freshly reinstalled process logged 0
`MycoBleRadio` lines for 2m+ until its node was restarted, then began scanning and advertising.

**Ruled out** (all verified on-device, so none of these is the cause):

- `BleService` not running — `dumpsys activity services app.myco` shows
  `ServiceRecord app.myco/.ble.BleService` active on both phones; it logs `BLE service started`.
- Missing permissions — `BLUETOOTH_SCAN`, `BLUETOOTH_ADVERTISE`, `BLUETOOTH_CONNECT` all
  `granted=true`.
- System Bluetooth off — `settings get global bluetooth_on` = 1 on both.
- App Bluetooth toggle off — on, confirmed by screenshot.
- Radio code broken — after a node restart both phones scan *and* advertise correctly.

**Where to look first —** `android/app/src/main/java/app/myco/ble/BleService.kt:66-99`
(`startBle()`). Kotlin never calls `startScanning()` itself; the fips BLE transport drives the
radio through the injected bridge:

```kotlin
// Inject-then-start: the node's BLE transport picks up the bridge and
// begins driving the radio (listen → advertise → scan).
client.dispatch(NativeActions.setBleEnabled(true))
// Node lifecycle follows the mesh "Enable" master switch, not this
// toggle — and a running node is never bounced to adopt this radio.
// The core resolves the injected bridge per operation, so the fresh one
// above is picked up in place.
```

That last claim — "picked up in place" — is what the evidence contradicts. `startBle()` injects a
fresh bridge and dispatches `setBleEnabled(true)`, but when the node is already running nothing
re-arms the listen → advertise → scan sequence. `startNode()` is deliberately skipped in that
case (`if (meshOn && !client.state().nodeRunning)`), which is exactly the path that leaves BLE dark.

Note the surrounding comment explains that path was chosen *deliberately* to avoid tearing down
every peer and session on a Bluetooth toggle. Any fix must re-arm the BLE transport **without**
bouncing the node, or it will reintroduce that regression.

**Why Phase 1 caught it:** the pre-Phase-1 `scanning` value was computed as
`ble_enabled && node_running`. In this exact state both are true, so the old Dev tab rendered
`active` — confidently, and wrongly — while the radio was dark. The DIAG-05 observed signal is
what made the fault visible.

---

## F-02 (secondary): PSM never resolves for at least one advertising peer

**Symptom:** With scanning healthy, both phones repeatedly log, for minutes, against the same
advertiser:

```
D MycoBleRadio: scan ble0/84:C5:A6:C8:43:F7: PSM not in advert yet (awaiting scan response)
```

The scan filter is `ScanFilter.setServiceUuid(FIPS_PARCEL_UUID)`, so this device *is* a FIPS
advertiser, but its PSM never arrives in either the primary advert or the scan response — so no
L2CAP connection can be attempted. Both phones advertise their own PSM fine
(`advertising PSM 131`), so this is about consuming a peer's advert, not producing one.

Not yet diagnosed. Peer identity is unconfirmed — both phones use resolvable private addresses,
so the two under test cannot be correlated to each other by MAC, and `84:C5:A6:C8:43:F7` may be a
third FIPS device in range rather than either test phone.

**Relevance:** plan 01-03's per-peer attempt log (discovery latency, attempt outcome, drop counts)
is the artefact that will characterise this properly. Revisit F-02 once 01-03 lands rather than
guessing now.

---

## Not a finding

`E BluetoothLeAdvertiser: Legacy advertiser should be only disabled on timeout, but was enabled!`
appears on both phones right after advertising starts. It is emitted by the Android framework's
own advertiser, not by Myco, and advertising works regardless. Noise unless something else
points back at it.
