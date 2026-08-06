# Device Test Batch — Phase 01

Everything in Phase 01 that is blocked on physical hardware, batched so one
two-phone session clears it. Written 2026-08-06 from the Linux build host, which
has the full Rust + Android toolchain but no devices.

**Read `.continue-here.md`'s anti-patterns 1 and 2 before starting.** Both were
learned the hard way and both silently corrupt results:

1. **Never read `adb logcat` unfiltered.** Resolve the pid first
   (`adb -s <serial> shell pidof app.myco`) and filter `logcat -d --pid=<pid>`.
   A stale line from a dead pre-reinstall process has already been misread once as
   evidence the current build was working. The two phones also have different
   timezones — correlate on the `myco` tracing timestamps (UTC), never logcat
   wall-clock.
2. **Never wipe app data.** Use `adb install -r`, which preserves it. `pm clear`
   or uninstall/reinstall regenerates `identity.nsec`, which gives the device a
   new node identity mid-session, empties its Circle and silently un-pairs the
   two phones. Verify afterwards with
   `run-as app.myco ls -la files` that `identity.nsec` kept its old mtime.

Devices: Pixel 7 Pro `29131FDH3007HW`, Samsung SM-A528B `R5CR916CDCF`.

---

## D-1 — Does the tiebreaker actually agree at runtime? (highest value)

**Why this one matters most.** The roadmap's first sequencing rule is that Phase 2
must not act on inference. The BLE role tiebreaker looks deterministic in source
and is unit-tested for the convention, but until now nothing recorded whether
*both sides* agree in the field. 01-03 built the instrument. This reads it.

**Setup.** Both phones running the 01-03 build, BLE on, in the same room, and let
them connect/drop a few times.

**Read.** Dev tab → expand a peer row. Each attempt shows role, discovery latency
and outcome.

**What confirms healthy behaviour:** for any one connection cycle between the two
phones, exactly one side records `lost-tiebreaker`. The winner records
`connected`.

**What confirms the race hypothesis:** *both* phones record `lost-tiebreaker` for
the same cycle (each yielded, so nobody connected), or *neither* does while both
record `connected` for the same peer (both won, so there are two connections).
Either is the evidence Phase 2 has been waiting for.

**Capture:** screenshot both Dev tabs plus
`adb -s <serial> shell run-as app.myco cat files/ble-attempts.jsonl` from each.
The JSONL is the durable artefact — attach both to the phase record.

---

## D-2 — Attempt history survives a force-stop

Covers 01-03 Task 3's `<human-check>`. The corruption, truncation, cap and
eviction contracts are already pinned by unit tests against real files; this
checks Android process death specifically, which no host test reproduces.

1. Let the two phones connect and drop a few times so attempts accumulate.
2. Note what the Dev tab shows for a peer.
3. Force-stop from Android Settings → Apps → Myco → Force stop.
   (Force stop, **not** clear data — see anti-pattern 2.)
4. Reopen, go to the Dev tab, expand the same peer.

**Pass:** the pre-force-stop attempts are still listed.
**Also check:** `run-as app.myco ls -la files/ble-attempts.jsonl*` — there should
be a `ble-attempts.jsonl` and **no** `.corrupt` sibling. A `.corrupt` file
appearing in normal operation means the write path is producing files the read
path rejects, which is a real bug worth stopping for.

---

## D-3 — F-02: the PSM that never resolves

01-03's attempt log is the instrument built for this. F-02 is the repeating
`PSM not in advert yet (awaiting scan response)` against `84:C5:A6:C8:43:F7`,
for minutes, with scanning otherwise healthy.

With the attempt log live, check whether that address appears in
`ble-attempts.jsonl` at all:

- **Absent entirely** → no connect attempt is ever made, so the failure is
  upstream of the transport, in advert parsing. Narrows F-02 to the scan-response
  path.
- **Present with `connect-error`/`connect-timeout`** → attempts *are* being made
  and failing, which is a different fault than currently recorded.

Also worth settling: is `84:C5:A6:C8:43:F7` even one of the two test phones? Both
use resolvable private addresses so they cannot be correlated by MAC. Turning
BLE off on one phone and seeing whether that address disappears from the scan
answers it.

---

## D-4 — F-04: relay reachability flapping while the route is fine

Already recorded as an observed finding with device evidence (Pixel 2/5, Samsung
2/16, all `ENETUNREACH`, while `ping6` ran 0% loss). This is confirmation for
Phase 2 planning, not discovery.

Correlate the `peer relay disconnected` log line's `reason` field against a
concurrently healthy route:

```
adb -s <serial> shell pidof app.myco
adb -s <serial> logcat -d --pid=<pid> | grep "peer relay disconnected"
```

**Expected:** `reason="no frame within a ping interval (half-open)"` on a peer
that `ping6` shows as reachable at the same moment. That is the socket-derived
vs route-derived gap, captured.

**Note:** 01-03's attempt log will **not** show this — the `ENETUNREACH` dials
never reach a BLE connect attempt. Do not expect it in `ble-attempts.jsonl`.

---

## D-5 — Wi-Fi Aware lane label (carried over from 01-02)

01-02's still-open gap. The lane label was verified by unit test and build, never
by watching a live Aware NDP render `transport: "aware"` on a real peer row.

Enable Wi-Fi Aware on both phones, get an NDP up, confirm the peer row reads
`aware` and not `udp`.

---

## D-6 — 01-04 build + install (once 01-04 is written)

01-04 touches only `DevScreen.kt` and `AppCoreClient.kt`. It can be written on
any host but should be **rendered on a real screen** before the phase closes —
column order, the radio self-check card, expanding rows and the demoted
speedtest are all visual judgements.

Build with `cd android && ./gradlew assembleDebug`, install with
`adb install -r`.

---

## Host-side status (nothing to do here)

For context, all of this already passes on the Linux build host and needs no
device:

| Gate | Result |
|---|---|
| fips full suite | 1402 passed, 0 failed |
| fips `--lib transport::ble` | 62 passed (incl. 9 new attempt-log tests) |
| `cargo test -p myco-core` | 65 passed, 0 failed |
| `cargo fmt --all --check` (both trees) | clean |
| `reference/clippy-gate.sh` | PASS vs 9-entry baseline |
| `just ndk-build` | 22 MB aarch64 `.so` |
| `:app:compileDebugKotlin`, `:app:testDebugUnitTest` | BUILD SUCCESSFUL |
