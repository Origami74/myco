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

### Root cause (confirmed on-device)

It is a **startup race, plus a one-shot transport start that is never retried.**

The node's BLE transport calls `io.listen()` before the Android foreground service has injected
the bridge. Both phones logged, at node start:

```
WARN fips::transport::ble: failed to start BLE listener adapter=ble0
     error=io error: BLE radio not available
```

On the Pixel that warn lands at `09:57:34.113` — **620 ms before** `MycoBleService: BLE service
started` at `09:57:34.733`. The Samsung logged the same warn at `10:45:58` and then produced
`INFO fips::transport::ble: BLE transport started adapter=ble0 psm=133` only at `11:02:36`,
i.e. on the node restart, ~17 minutes later.

`reference/fips/src/transport/ble/mod.rs:218-222` handles that failure correctly:

```rust
Err(e) => {
    warn!(adapter = %adapter, error = %e, "failed to start BLE listener");
    self.state = TransportState::Failed;
    return Err(e);
}
```

The transport is not lying about its state — it marks itself `Failed` and returns. The defect is
that **nothing ever restarts it.** `TransportState::Failed.can_start()` returns `true`
(asserted by `src/transport/mod.rs:1396`), so restarting a failed transport is explicitly
permitted — but `grep -rn "TransportState::Failed" src/node/` returns **no matches**. The node
never inspects transport state and never retries. The capability exists; no supervisor exercises it.

The whole listen → advertise → scan sequence at `ble/mod.rs:196-263` runs exactly once, inside
`start()`. When the radio appears half a second later there is no trigger to re-run it.

### Why commit `9121925` did not already fix this

`9121925 feat(ble/android): adopt a radio without rebuilding the node` (2026-07-29, already on
`feat/platform-peer-queue`) was written to solve exactly this class of problem, and its message
claims:

> Operations attempted with no radio present return a transport error instead of being
> unreachable, **and recover on their own once a radio appears.**

That claim holds only for *demand-driven* operations. `AndroidIo` now resolves the process-wide
bridge per operation, so an outbound dial attempted later picks up the new radio and succeeds.
But **listen, advertise and scan are not demand-driven** — they are a one-shot startup sequence
with nothing to retrigger them. `9121925` made dials recover; it never made transport startup
recover. That is the remaining gap.

### Constraint on any fix

`BleService.kt:84-91` deliberately does **not** restart the node when one is already running:

```kotlin
// Node lifecycle follows the mesh "Enable" master switch, not this
// toggle — and a running node is never bounced to adopt this radio.
```

`9121925`'s message records why: bouncing the node "drops every peer, every session, and every
route with it. Turning Bluetooth on took the whole mesh down for as long as re-handshaking took."

So the fix must **re-arm the BLE transport without restarting the node** — most likely a
supervisor that retries `start()` on a `Failed` transport (with backoff), or a signal from
`set_android_ble_bridge()` that wakes a transport sitting in `Failed`. Restarting the node is
what the user did manually to recover, and it is exactly what must not become the shipped fix.

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

## F-03: Wi-Fi Aware has the same one-shot shape, with a partial safety net

Asked during Phase 1 execution: does F-01 also apply to Wi-Fi Aware? **Same shape, smaller blast
radius.** Not observed failing in the field yet — this is a code-read finding, unlike F-01 and F-02.

The fips *transport-level* race does not apply. Aware rides the ordinary UDP transport, which
binds a local socket and has no external radio that can be "not available", so it cannot fail
`start()` the way the BLE transport does.

But the Kotlin layer has the same one-shot pattern.
`android/app/src/main/java/app/myco/aware/AwareRadio.kt:127-129`:

```kotlin
override fun onAttachFailed() {
    Log.e(TAG, "Aware attach failed")
}
```

Failure is logged and nothing else — no retry, no backoff. Compare `BleRadio.scheduleScanRetry()`
(`BleRadio.kt:320-331`), which re-arms with exponential backoff capped at 60s.

What saves Aware is `registerAvailability()` (`AwareRadio.kt:150-169`): a `BroadcastReceiver` on
`ACTION_WIFI_AWARE_STATE_CHANGED` that re-attaches whenever Aware transitions to available:

```kotlin
if (mgr.isAvailable) {
    if (session == null) {
        Log.i(TAG, "Aware became available; attaching")
        attach(mgr)
    }
}
```

So Aware recovers from unavailable → available transitions. It does **not** recover from a bare
`onAttachFailed()` while Aware remains nominally available — that state is terminal until
something else flips availability.

**The architectural point, which is the real finding:** BLE fails in Rust at the fips transport
layer, where there is no supervisor at all; Aware fails in Kotlin, where a broadcast receiver can
rescue it. The generic missing piece is the same in both — *nothing retries a failed start*. A fix
for F-01 that adds transport-start supervision in fips should be checked against whether it also
wants to cover this, rather than bolting a second ad-hoc retry onto `onAttachFailed()`.

---

## Not a finding

`E BluetoothLeAdvertiser: Legacy advertiser should be only disabled on timeout, but was enabled!`
appears on both phones right after advertising starts. It is emitted by the Android framework's
own advertiser, not by Myco, and advertising works regardless. Noise unless something else
points back at it.
