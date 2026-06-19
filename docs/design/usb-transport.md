# Plan: USB as a FIPS transport (high-throughput phone-to-phone)

**Status:** proposed / not started. Self-contained handoff for an implementing agent.

**Author context (read first):** This plan came out of a long on-device BLE
throughput debug. The mesh now works but BLE tops out at **~22 KB/s** (peripheral→
central uplink), capped by Android's connection-interval floor and the BLE
peripheral asymmetry — *not* by our code (see the commits in "Background"). USB
(AOA) targets **tens of MB/s** — ~1000× — for the case where two phones are
physically together and a cable is acceptable (e.g. seeding a large nsite).

The goal is a **real FIPS transport**, peer to BLE — *not* USB/IP tethering. FSP /
Noise / MMP ride on top unchanged; the content layer (relay/Blossom over the
`fd00::/8` mesh ULA) needs **zero changes** — USB just provides a faster *link* for
the same IPv6 routing.

---

## 0. Background & why this is tractable

The FIPS transport seam is clean and **USB is simpler than BLE** — it drops
scanning, advertising, PSM discovery, and the cross-probe tiebreaker. A cable *is*
the discovery. We mirror the BLE transport's structure and the proven Kotlin↔Rust
JNI bridge pattern.

**Repos (both edited):**
- **fips** — `/Users/gump/Documents/development/fips/fips` (symlinked as
  `fips-pop/reference/fips`; the build uses `MYCO_FIPS_REPO_PATH=reference/fips`).
  Work on the **`ble-v2`** branch (where the BLE transport + recent fixes live).
- **fips-pop** — this repo. The Android app (`UsbRadio`/`UsbService`) + the
  `myco-core` JNI bridge + wiring.

**Reference implementation to clone from (BLE):**
- `fips/src/transport/ble/io.rs` — the `BleStream` / `BleAcceptor` / `BleScanner` /
  `BleIo` traits (the byte-stream Io seam).
- `fips/src/transport/ble/android_io.rs` — `AndroidBleIo` + `AndroidBleBridge` +
  the `AndroidRadio` trait it calls into; the JNI-facing `deliver_*` / `next_send`.
  **This is the closest template for the USB Android bridge.**
- `fips/src/transport/ble/mod.rs` — `BleTransport: Transport`, MTU reporting,
  start/stop/connect/send.
- `fips/src/transport/mod.rs` — the `Transport` trait + `TransportHandle` enum +
  `TransportType`.
- `fips/src/config/transport.rs` — `BleConfig` + `TransportsConfig` + the
  `TransportInstances<T>` (single/named) pattern.
- `fips/src/node/mod.rs` — `create_transports()` (where each transport is built
  from config and pushed as a `TransportHandle`).
- App side: `fips-pop/android/.../ble/BleRadio.kt` + `ble/BleService.kt`;
  `myco-core/src/ble_bridge_jni.rs` (the JNI exports);
  `android/.../core/NativeCore.kt` (the `external fun` decls).

**Recent relevant commits (BLE throughput fixes — context for how the link
behaves):** fips `ble-v2`: `4edf604` (PSM dial fallback), `668def6` (send-queue
backpressure / bufferbloat). fips-pop `main`: `2551e6d` (BLE radio: 2M PHY,
priority, single-SDU framing, MTU 1500, mesh-only).

---

## 1. Architecture

```
┌─ Phone A (USB host) ──────────┐        ┌─ Phone B (USB accessory) ─────┐
│ Kotlin UsbRadio               │  AOA   │ Kotlin UsbRadio               │
│   host: enumerate + START_ACC │ bulk   │   accessory: openAccessory()  │
│   claim bulk IN/OUT endpoints │◄──────►│   read/write the accessory FD │
│        ▲ JNI bridge           │ pipe   │        ▲ JNI bridge           │
│ Rust AndroidUsbIo / UsbBridge │        │ Rust AndroidUsbIo / UsbBridge │
│ Rust UsbTransport: Transport  │        │ Rust UsbTransport: Transport  │
│   FSP / Noise IK / MMP  ───────────────────────  FSP / Noise / MMP     │
│   fd00::/8 IPv6 over the link │        │   fd00::/8 IPv6 over the link │
└───────────────────────────────┘        └───────────────────────────────┘
```

- **Transport = AOA (Android Open Accessory).** The host phone puts the other into
  accessory mode and they talk over **bulk IN/OUT endpoints** — the standard
  app-to-app-over-USB pipe. No drivers, no root.
- **Roles map to the Io seam:** host enumerating + `START_ACCESSORY` ≈ `connect()`;
  the accessory side handling `USB_ACCESSORY_ATTACHED` + `openAccessory()` ≈
  `listen()`/`accept()`. After the pipe is up it's just bytes — FIPS runs its
  33-byte pubkey pre-handshake + Noise IK exactly as over BLE.
- **No discovery layer.** A USB attach event is the "peer appeared" signal.

---

## 2. Build order (de-risk first)

### Phase 0 — AOA bulk-pipe spike (DO THIS FIRST, ~1–2 h)
The whole transport rests on AOA working phone-to-phone on the **actual devices**
(Pixel 7 Pro `29131FDH3007HW`, Samsung A52s `R5CR916CDCF`). Unknowns: which phone
becomes host under USB-C role negotiation, and whether AOA re-enumeration is
reliable. **Prove the pipe + measure throughput before building anything else.**

Minimal standalone Kotlin spike (can be a throwaway Activity, not wired to FIPS):
- Both apps register: a USB-host path (`UsbManager`, listen for
  `USB_DEVICE_ATTACHED`) **and** an accessory path (`USB_ACCESSORY_ATTACHED` +
  `accessory_filter.xml`). Whichever role the cable gives you, take it.
- **Host:** enumerate device → AOA control transfers (`GET_PROTOCOL=51`,
  `SEND_STRING=52` for manufacturer/model/etc., `START_ACCESSORY=53`) → device
  re-enumerates as VID `0x18D1` PID `0x2D00/0x2D01` → claim interface, get the bulk
  IN/OUT endpoints → `bulkTransfer` loop.
- **Accessory:** `UsbManager.openAccessory()` → `FileInputStream`/`FileOutputStream`
  on the FD → read/write loop.
- Blast a few MB each way with a counter; print MB/s.

**Exit criterion:** a stable bidirectional pipe at **tens of MB/s** in *both*
directions (note any asymmetry). If AOA is flaky or one phone refuses host mode,
record it here and we reconsider (raw USB-host w/ a custom gadget needs
configfs/root — avoid; or fall back to WiFi-Direct).

**Manufacturer/model strings must match** between the host's `SEND_STRING` and the
accessory's `accessory_filter.xml` — pick fixed constants (e.g. manufacturer
`"Myco"`, model `"MycoMesh"`, version `"1"`).

### Phase 1 — FIPS Rust transport (host-compiles, partly host-testable)
Mirror BLE, minus discovery. New files under `fips/src/transport/usb/`:

- **`io.rs`** — the byte-stream seam (copy BLE's, delete the BLE-only bits):
  ```
  trait UsbStream { async send(&[u8]); async recv(&mut [u8])->usize; send_mtu; recv_mtu; remote_addr }
  trait UsbAcceptor { type Stream; async accept()->Stream }
  trait UsbIo { type Stream; type Acceptor;
                async listen()->Acceptor;        // accessory role
                async connect(addr)->Stream;      // host role
                local_addr; adapter_name }
  ```
  Drop `BleScanner`, `start/stop_advertising`, `psm` params, `PsmMap`.
- **`mod.rs`** — `UsbTransport` impl `Transport` (clone `ble/mod.rs`, strip
  scan/advertise/PSM/tiebreaker). `TransportType { name:"usb", connection_oriented:
  true, reliable:true }`. `auto_connect`/`accept_connections` from config.
- **`android_io.rs`** — `AndroidUsbIo` + `AndroidUsbBridge` + the `UsbRadio` callee
  trait, cloned from `ble/android_io.rs`. **Keep the `668def6` send-queue
  backpressure design** (shallow `SyncSender` + `try_send`-then-`await`); USB is
  fast so the cap can be larger (e.g. 64), but keep backpressure, not tail-drop.
  Reuse the JNI bridge mechanics (`deliver_inbound`, `channel_deliver_recv`,
  `channel_next_send`, `deliver_connect_result`, `close_channel`).

Edits to existing fips files:
- `src/transport/mod.rs`: `pub mod usb;`; add `TransportHandle::Usb(UsbTransport)`
  variant; delegate it in **every** `match` arm (start/stop/send/state/mtu/
  link_mtu/connect/close/…). Mirror the BLE arms.
- `src/config/transport.rs`: add `UsbConfig` (mtu default large — see §3; plus
  `auto_connect`, `accept_connections`, optional `connect_timeout_ms`) and a
  `usb: TransportInstances<UsbConfig>` field on `TransportsConfig`; update its
  `is_empty()` / `merge()`.
- `src/config/mod.rs`: re-export `UsbConfig`.
- `src/node/mod.rs` `create_transports()`: add a USB instantiation block modeled on
  the BLE/TCP block (allocate transport_id, build `UsbTransport::new(...)`, set
  local pubkey if the pre-handshake needs it, push `TransportHandle::Usb`).
- `Cargo.toml`: no new crate needed if the byte stream comes entirely from the
  Kotlin side via the JNI bridge (recommended — same as BLE's Android path). A
  Linux `libusb`/`nusb` backend is **out of scope** for v1 (Android↔Android only).

Gate USB Android bits behind `#[cfg(target_os = "android")]` like the BLE Android
path. Add a host-side unit test for the `UsbStream` framing/Io contract if feasible
(a loopback `UsbStream` pair) — analogous to existing BLE tests.

### Phase 2 — Kotlin `UsbRadio` + service + JNI
- **`android/.../usb/UsbRadio.kt`** (mirror `ble/BleRadio.kt`): owns the AOA
  connection (both roles, from Phase 0), exposes control methods the Rust core
  calls via JNI (start/connect/listen/close) and pumps bytes:
  - outbound: a writer thread pulls via `NativeCore.usbChannelNextSend` → writes the
    bulk OUT / accessory FD.
  - inbound: a reader thread reads bulk IN / accessory FD → `NativeCore.usbChannelDeliverRecv`.
  - **Framing:** reuse BLE's single-write length-prefix (`[u16 BE len][payload]`,
    one write per packet). AOA bulk is message-ish but treat as a stream to be safe.
- **`android/.../usb/UsbService.kt`** (mirror `ble/BleService.kt`): a foreground
  service holding the USB connection; started on USB attach. Manifest:
  `<uses-feature android:name="android.hardware.usb.host">`,
  `<uses-feature android:name="android.hardware.usb.accessory">`, an
  `<intent-filter>` for `USB_DEVICE_ATTACHED` and `USB_ACCESSORY_ATTACHED` with a
  `resource="@xml/accessory_filter"` meta-data, and `res/xml/accessory_filter.xml`.
- **`myco-core/src/usb_bridge_jni.rs`** (mirror `ble_bridge_jni.rs`): the
  `Java_app_myco_core_NativeCore_usb*` exports wiring Kotlin to `AndroidUsbBridge`.
- **`android/.../core/NativeCore.kt`**: the matching `external fun usb*` decls.

### Phase 3 — Wire into myco-core
- In `myco-core/src/runtime.rs` `build_node()` (the `#[cfg(target_os="android")]`
  block that sets `config.transports.ble`): also set
  `config.transports.usb = Single(UsbConfig{ auto_connect:true, .. })`.
- Inject the `UsbRadio` bridge like the BLE bridge is injected (see how
  `bleBridgeNew` is wired from `BleService`/`MainActivity`).
- **Nothing in the content layer changes.** Once the USB link is up, the node has a
  second transport for the same `fd00::/8` ULA; `open_site` / relay / Blossom work
  unchanged and FIPS routes over whichever link is up. (Confirm FIPS prefers/uses
  the faster link when both BLE + USB exist — if it round-robins or only uses one,
  decide a preference: USB > BLE when present. Likely a small policy in the node's
  transport selection.)

### Phase 4 — On-device end-to-end
- Plug the two phones (USB-C↔USB-C). Confirm the USB link establishes (logcat,
  `adb logcat -s myco` — the Rust→logcat bridge is already in place, tag `myco`).
- Re-run the slow case (Pixel pulling a large nsite from Samsung) over USB; measure
  the MMP goodput (grep `MMP session metrics`). Target: multi-MB/s.
- Verify BLE still works when no cable; both coexist.

---

## 3. Key decisions / risks

- **AOA vs raw USB host:** AOA. Raw host→device app-to-app needs a custom USB
  gadget on the device side (configfs/root) — avoid.
- **Who is host:** USB-C PD negotiates DFP/UFP; not fully app-controllable. **Both
  apps must implement both roles.** Some phones expose a "USB controlled by"
  toggle; document the manual step if auto-negotiation is unreliable (learned in
  Phase 0).
- **MTU:** large. USB bulk handles big transfers; set `UsbConfig.mtu` to e.g.
  **16384** (or tune in Phase 0). Note the TUN MTU is currently 1280 (IPv6 min) and
  the MSS clamp derives from `effective_ipv6_mtu` — with USB present you may want a
  larger TUN MTU for that path, but keep it simple for v1 (the win is link rate, not
  MTU). Don't regress the BLE path's MTU.
- **Framing / bufferbloat:** reuse the single-write length-prefix framing and the
  **backpressure** send queue (not tail-drop) from the BLE fixes. USB is fast, so
  the queue rarely fills, but keep the backpressure semantics.
- **Coexistence with BLE:** both transports active. Decide link preference (USB
  when cabled). Make sure adding `transports.usb` doesn't break the BLE-only path.
- **Security:** same as BLE — Noise IK over the byte stream; no trust placed in USB
  itself. The pubkey pre-handshake (`fips/src/transport/ble/io.rs` lines ~662–714)
  may be needed on USB too; check whether it's transport-generic or BLE-specific.
- **fips is fips-only:** no `myco`/app references in the fips source (per project
  rule). Keep `src/transport/usb/*` transport-generic.

---

## 4. Verification checklist
- [ ] Phase 0 spike: stable AOA pipe, throughput number recorded (both directions).
- [ ] `cargo test -p fips` green (new usb Io/transport unit tests + nothing else
      broken). Build for android: `MYCO_FIPS_REPO_PATH=$PWD/reference/fips
      android/gradlew -p android assembleDebug`.
- [ ] On-device: USB link establishes (logcat), BLE still works without a cable.
- [ ] Large nsite pull over USB at multi-MB/s; compare to the BLE ~22 KB/s baseline.
- [ ] No regression to the BLE transport or the mesh content layer.

## 5. Out of scope (v1)
- Linux/libusb USB backend (Android↔Android only).
- Auto USB-role negotiation beyond "handle whichever role we get."
- Multi-peer over a USB hub.

## See also
- `docs/roadmap.md` — the "WiFi-Direct transport" Later item is the wireless
  sibling of this; consider adding a "USB transport" bullet there.
- `docs/design/ble-interop.md` / `docs/reference/ble-wire.md` — the BLE transport
  this mirrors.
