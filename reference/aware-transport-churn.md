# Wi-Fi Aware teardowns: transport churn

State of play as of 2026-08-18. Read this before touching the Aware or BLE
radios — several plausible theories are already disproven and re-testing them
costs hours.

## The symptom

Wi-Fi Aware data paths between the Pixel 7 Pro and the Galaxy A52s died on a
regular 61–65s cycle. The path came up, established a FIPS session in ~3s,
carried steady traffic, then the firmware ended it (`NAN_EVENT_DATA_END`) with
no application activity beforehand. Recovery took ~3s, then the cycle repeated.

Only the Pixel logs the vendor event — it is a Broadcom BCM4389 (`bcmdhd4389`
for Wi-Fi, Bluetooth over `hci_uart`, one die). The A52s is Qualcomm and only
ever follows ~100ms later. After correcting for a 0.5s clock offset between the
phones, the Pixel is always first, so the teardown originates there.

## The cause

The core re-establishes the same peer alternately over BLE and Aware. In every
window containing teardowns, `Connection initiated` for one peer alternates
between transports seconds apart. The single teardown-free window in hours of
logs was the one where it settled on a single transport and stopped re-dialling.

Suppressing our outbound BLE dials to a peer that Aware is already carrying
takes teardowns from one per 64s to zero in steady state, with both radios
scanning harder than before. That is the direct experiment, run three times.

## What is implemented

`AwareRadio` publishes the `node_addr` prefixes of peers it is carrying;
`BleRadio` refuses to dial them and withholds their adverts from the core.

`node_addr` is the only identity both radios can compute. BLE reads a 6-byte
prefix of it from the peer's scan response. Aware derives it from the peer's
npub as `SHA-256(x-only pubkey)[..16]`, mirroring `NodeAddr::from_pubkey` in
fips. Verified on device: each phone refuses dials to exactly the other's
prefix.

Inbound connections, advertising and scanning are untouched.

## Remaining issues

### 1. Startup churn (the live one)

Teardowns still happen for the first ~3 minutes after launch, then stop
completely. Consistent across three builds: 1, 2 and 3 teardowns respectively,
each followed by 4+ minutes clean.

The gate only prevents *new* dials. At launch Aware is not up yet, so BLE
legitimately connects the peer first; when the NDP then arrives the core holds
that peer on two live transports and flip-flops until it settles.

The fix is the other half of "drop BLE and use only Aware": actively close the
existing BLE channel when Aware takes over a peer, not just decline future
dials. Needs a chId → MAC association in `BleRadio`; the MAC → prefix map is
already there.

### 2. Unidentified peers are dialled anyway

The `node_addr` prefix rides the scan response, and the advertiser only builds
one when both the display name and node addr are known. A peer with no name set
never sends it. So unidentified addresses are held back for only 4 sightings
(~1–2s) and then reported regardless, rather than being withheld indefinitely —
otherwise unnamed peers would be permanently undiscoverable over BLE.

That leaves a small window on every RPA rotation where a dial can slip through.
Rotation is otherwise handled: the prefix is stable across it, and the new MAC
re-learns the mapping from the next scan response. The map is TTL-bounded
(30 min, pruned at 128 entries) so rotations do not accumulate.

### 3. Silent scanner is never re-armed

Independent bug, hit repeatedly during this investigation and it invalidates any
BLE measurement while active.

The scanner can stop delivering with no `onScanFailed` callback. `logScanSummary`
counts empty windows and sets `BleHealth.scannerConfirmedSilent`, but the only
consumer of that flag is a location-services warning gated on
`!locationServicesEnabled`. With location on, a dead scanner produces no warning
and no recovery — observed dead for 4+ minutes with no attempt to restart.

`scheduleScanRetry()` already handles the throttle correctly; it is simply
unreachable from this path. Call it when `emptyScanWindows >= 2`.

### 4. Dialling the default PSM

57 consecutive dials to one peer at `psm 133`, all failing `No PSM available`.
133 is `DEFAULT_PSM` (`0x0085` in fips `transport/ble/mod.rs`), the fallback used
when the real PSM is unknown. Peers on air were advertising 180 and 228. Caused
by (3): with no adverts arriving, stale cache entries get retried indefinitely.

### 5. fips: transport-blind cross-connection tie-break

`cross_connection_winner` picks the surviving connection from node addresses and
direction only:

```rust
if we_are_smaller { this_is_outbound } else { !this_is_outbound }
```

It is globally consistent — both sides agree, no split-brain — but only the
*smaller* node's outbound connection can ever win. If the larger node is the one
that initiates a transport upgrade, it is rejected by both sides every time and
re-dials indefinitely. For this pair the Samsung is smaller (`a16d353c…` vs the
Pixel's `c66233c1…`) while the Pixel initiates the Aware NDP.

Not firing today — sessions die before they live long enough to be upgraded — so
it is latent. It will surface once the churn is fixed.

### 6. Aware is disabled by Doze on Android 13/14/15

See [#30](https://github.com/Origami74/myco/issues/30). `WifiAwareStateManager`
calls `disableUsage()` on device idle, tearing down every session and NDP, unless
some Aware client app is exempt from battery optimizations. A foreground service
does not help — the check is the battery-opt whitelist. Removed in Android 16.
Unrelated to the churn above, but it will end Aware sessions in the field.

## Disproven — do not re-test

- **Radio coexistence / BLE scan duty cycle.** The obvious theory, given one die
  shares a 2.4GHz front end. Backing the scan off from `LOW_LATENCY` to
  `BALANCED` while a path was live changed nothing: teardowns continued at the
  same 64s cadence. The counter-example is decisive — with dials suppressed,
  teardowns stop while BLE scans *harder* than ever (77 and 137 adverts/30s).
  A dose-response that appeared to support coexistence (31 → 16 → 0 adverts as
  teardowns tapered) was the scanner of issue (3) dying, not causation.
- **Firmware reaping an idle path.** `aware_data0` counters showed steady
  mirrored traffic throughout, right up to each teardown.
- **NDP slot exhaustion.** `availableDataPathsCount` is the *free* count and read
  8 of a maximum 8 (`NAN_MAX_NDP_PEER`) every time.
- **Discovery match expiry.** `NAN_EVENT_MATCH_EXPIRY` did not precede the
  teardowns.
- **Our own release causing it.** The firmware event precedes our
  `releaseNetworkFor` by ~108ms.
- **Duplicate network requests.** `Aware NDP up` is logged from
  `onCapabilitiesChanged`, which fires repeatedly — a logging artifact, not two
  requests. Requests and releases balance.
- **The tie-break causing it.** See (5): the loser path never executes today.

## Loose artifact

A tested fips change making session lookup transport-agnostic — `pending_outbound`,
`peers_by_index`, `decrypt_registered_sessions` and the decrypt worker keyed on
the session index alone instead of `(TransportId, u32)` — is saved at
`reference/fips-transport-agnostic-session-key.patch`. It is **not** committed to
either repo and `reference/` is gitignored, so it will be lost if that file is
deleted.

It applies to fips `integration/platform` and passed 1830 tests, clippy and fmt.
It fixes a real defect: a msg2 arriving on a different transport than the msg1
that created the entry cannot match, so a cross-transport reply can never
complete a handshake. It did not measurably change the churn, which is why it was
reverted rather than kept. It becomes relevant if we ever want a transport change
to be free instead of a full re-handshake.
