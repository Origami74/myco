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

## F-04: `reachable-via-relay` is socket-derived, not route-derived

**Status: observed on-device.** Both phones, same session as F-01/F-02.

**Symptom:** a peer's diagnostics row flips between `reachable-via-relay` and
`paired-offline` while the mesh path underneath is continuously healthy.

**Observed evidence:**

| Device | Relay dials connected | Failed | Error |
|---|---|---|---|
| Pixel 7 Pro | 2 | 5 | `ENETUNREACH` |
| Samsung SM-A528B | 2 | 16 | `ENETUNREACH` |

The failures included dials to **a peer the UI was simultaneously showing as directly
BLE-connected**. Meanwhile `ping6` across the mesh ran at **0 % loss in both directions**
for the same pair, at the same time. So the route was demonstrably fine and the
reachability row was wrong — which is the whole finding.

**Mechanism.** `AppState::reachable_npubs` (`myco-core/src/state.rs:41`) is documented as
the honest reachability signal — "bytes are flowing to them right now, whether they are a
direct neighbour or many hops away". In practice `Content::reachable_npubs()`
(`myco-core/src/content.rs:962`) derives it from `peer_relays.connected_npubs()` filtered to
Circle membership — i.e. purely from whether a keepwarm WebSocket to
`ws://<npub>.fips:4870` happens to be open. That set tracks **one WebSocket actor's
lifetime**, nothing else:

- inserted once, immediately after a successful dial — `peer_relay.rs:329`
- removed when the actor's loop exits, for *any* reason — `peer_relay.rs:421`

`peer_diagnostics.rs:248-262` then renders membership of that set as
`reachable-via-relay`, and a Circle member absent from it falls through to
`paired-offline`.

The actor exits on: the peer closing the socket, a socket error or EOF, any failed write
(publish, request, or ping), the pool dropping the command channel — and, most relevant
here, **10 seconds of total silence**:

```rust
_ = ping.tick() => {
    // The previous ping went a whole interval unanswered by any frame →
    // treat the connection as dead (this is the half-open catch).
    if awaiting_pong {
        reason = "no frame within a ping interval (half-open)";
        break;
```

`PING_INTERVAL` is 10s (`peer_relay.rs:43`). That threshold is deliberate and its comment
says why — it exists to catch a silent half-open in seconds rather than at the TCP
retransmit horizon. But it means any 10-second gap on a lossy link tears down the socket
and clears the flag, **whether or not a mesh route still exists**. Over BLE L2CAP, ten
seconds of silence is not an exotic condition.

Recovery is not immediate either. The keepwarm loop respawns the actor, but the redial
carries `CONNECT_TIMEOUT` 10s (`:49`) and, on a `Hard` fault, a dial backoff escalating
8s → 180s (`:59`). So a single transient stall can render a peer `paired-offline` for up
to three minutes while the route underneath it is intact and healthy. `classify()`
(`:80-89`) already distinguishes `NoResolver` / `NoRoute` / `Hard` for *backoff* purposes —
that routing knowledge exists at dial time and is simply not carried into the displayed
state.

**Why this belongs to Phase 2, not Phase 1.** It is tempting to file this as an
instrumentation fault, since the visible damage is a wrong label. It is not. The label is
an accurate report of the only fact the code actually has: whether a socket is currently
up. The missing thing is a route-derived reachability signal to report *instead*.

And that is the deeper gap, which is what makes this Phase 2 work rather than a display
fix: **fips publishes no routing or reachability API at all.**
`fips::control::read_handle::ControlReadHandle` exposes exactly one public method —
`peer_views()` — so Myco has nothing to ask about a route. A socket probe is not a
substitute for routing state; it is what you fall back to when no routing state is exposed.
Giving the UI an honest answer therefore means adding that surface to fips, which is a
peering change, not a diagnostics change — exactly the boundary the roadmap draws.

**Note for 01-03.** The attempt log landing in 01-03 will **not** characterise this. That
log records BLE connect attempts at the transport; this fault lives in the relay layer
above it, and the `ENETUNREACH` dials never reach a BLE connect attempt at all. The
supporting trace to correlate on is instead the `peer relay disconnected` line and its
`reason` field (`peer_relay.rs:420`), which already distinguishes the half-open ping
timeout from a clean close.

---

## F-05: the tiebreaker is not racing — it is thrashing against address rotation

**Status: observed on-device**, from the 01-03 attempt log read on the Samsung
(`R5CR916CDCF`) on 2026-08-06. 48 recorded attempts, 0 unparseable.

**This is the first direct reading of the instrument 01-03 was built for, and it
does not confirm the hypothesis it was built to test.**

| Peer node address | Role + outcome | Count |
|---|---|---|
| `c66233c1eb43074e…` | `central` / `connected` | 6 |
| `c66233c1eb43074e…` | `peripheral` / `lost-tiebreaker` | **28** |
| `b4dc20096ff99f1f…` | `central` / `connected` | 13 |
| *(unresolved)* | `peripheral` / `pubkey-exchange-failed` | 1 |

**The tiebreaker race hypothesis is not supported by this sample.** Against
`c66233c1…` this device consistently wins as central and consistently drops the
peer's inbound as peripheral. That is the convention being applied *correctly* on
both paths — the two sides agree. A race would look like both sides recording
`lost-tiebreaker` for one cycle (nobody connects) or both recording `connected`
(two connections); neither appears here.

**What the log does show is different, and was not predicted.** All 28 peripheral
losses are against the same *node* identity but **28 distinct BLE addresses**
(`ble0/42:C3:…`, `ble0/43:0D:…`, `ble0/43:C7:…`, …). The peer uses resolvable
private addresses and rotates them constantly. Every rotation presents as a new
device, dials us inbound, loses the tiebreaker and is dropped — 28 times, against
6 useful outbound connects. Discovery latencies on the dropped inbounds are tiny
(16–189 ms), so this is cheap per event but relentless.

That churn is a plausible cause of the "not always connecting with peers"
complaint in `reference/FIX-TODOS.md`, but by a different mechanism than the one
that was assumed: not a disputed tiebreaker, but repeated rediscovery of a peer
this device already holds a connection to.

**Caveats, stated because this is one sample.** One phone's log, one session,
roughly twenty minutes. The counterpart phone's log has not been read, so the
"both sides agree" claim rests on this device's own two paths agreeing rather
than on comparing two devices — which is what `DEVICE-TEST-BATCH.md` D-1 still
asks for. A different pair, or a pair whose node addresses sort the other way,
could still surface a genuine disagreement. What is now settled is that the
inferred race is not visible in the first real reading, and Phase 2 should not
open by assuming it.

Raw log preserved: 48 records, per-peer capped at 20, survived a force-stop
(48 before, 48 after, no `.corrupt` sibling).

---

## Not a finding

`E BluetoothLeAdvertiser: Legacy advertiser should be only disabled on timeout, but was enabled!`
appears on both phones right after advertising starts. It is emitted by the Android framework's
own advertiser, not by Myco, and advertising works regardless. Noise unless something else
points back at it.
