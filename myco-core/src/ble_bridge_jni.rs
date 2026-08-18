//! Android BLE byte-bridge — the JNI side.
//!
//! Kotlin owns the BLE radio; this module is the glue between it and the fips
//! `AndroidBleBridge`:
//!
//! - [`KotlinRadio`] implements fips's `AndroidRadio` trait by calling methods on
//!   the Kotlin `BleRadio` object via JNI (`listen`/`connect`/`advertise`/…).
//!   These are rare control upcalls; the byte hot path never crosses here.
//! - The `Java_app_myco_core_NativeCore_*` exports are what the Kotlin radio
//!   calls to push inbound bytes/events into the bridge and pull outbound bytes
//!   (modeled on nostr-vpn's `mobileTunnelSendPacket` / `mobileTunnelNextPacket`).
//!
//! Compiled only on Android (the host build drives `AppRuntime` directly and the
//! fips bridge logic is unit-tested with a mock radio in fips itself).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{JNIEnv, JavaVM};

use fips::transport::ble::addr::BleAddr;
use fips::transport::ble::android_io::{AndroidBleBridge, AndroidRadio, BleRadioSlot};

/// Process-wide JavaVM, captured in `initializeAndroidContext`. Needed to attach
/// tokio worker threads to the JVM before issuing control upcalls.
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// Capture the JavaVM from any JNI call (called by `initializeAndroidContext`).
pub(crate) fn capture_java_vm(env: &JNIEnv) {
    if let Ok(vm) = env.get_java_vm() {
        let _ = JAVA_VM.set(vm);
    }
}

/// Run `f` with a JNIEnv attached to the current (tokio worker) thread. Returns
/// `default` if the VM is unavailable or attaching fails.
fn with_env<R>(default: R, f: impl FnOnce(&mut JNIEnv) -> R) -> R {
    let Some(vm) = JAVA_VM.get() else {
        return default;
    };
    match vm.attach_current_thread() {
        Ok(mut guard) => f(&mut guard),
        Err(_) => default,
    }
}

// ============================================================================
// KotlinRadio — fips AndroidRadio implemented over JNI
// ============================================================================

/// The Kotlin `BleRadio` object the bridge issues commands to.
struct KotlinRadio {
    radio: GlobalRef,
}

impl AndroidRadio for KotlinRadio {
    fn listen(&self) -> u16 {
        with_env(0, |env| {
            env.call_method(&self.radio, "listen", "()I", &[])
                .and_then(|v| v.i())
                .map(|psm| psm as u16)
                .unwrap_or(0)
        })
    }

    fn connect(&self, connect_id: i64, addr: &BleAddr, psm: u16) {
        let addr_str = addr.to_string_repr();
        with_env((), |env| {
            if let Ok(jaddr) = env.new_string(&addr_str) {
                let _ = env.call_method(
                    &self.radio,
                    "connect",
                    "(JLjava/lang/String;I)V",
                    &[
                        JValue::Long(connect_id),
                        JValue::Object(&jaddr),
                        JValue::Int(psm as i32),
                    ],
                );
            }
        });
    }

    fn start_advertising(&self, psm: u16) {
        with_env((), |env| {
            let _ = env.call_method(
                &self.radio,
                "startAdvertising",
                "(I)V",
                &[JValue::Int(psm as i32)],
            );
        });
    }

    fn stop_advertising(&self) {
        with_env((), |env| {
            let _ = env.call_method(&self.radio, "stopAdvertising", "()V", &[]);
        });
    }

    fn start_scanning(&self) {
        with_env((), |env| {
            let _ = env.call_method(&self.radio, "startScanning", "()V", &[]);
        });
    }

    fn stop_scanning(&self) {
        with_env((), |env| {
            let _ = env.call_method(&self.radio, "stopScanning", "()V", &[]);
        });
    }

    fn close_channel(&self, ch_id: i64) {
        with_env((), |env| {
            let _ = env.call_method(&self.radio, "closeChannel", "(J)V", &[JValue::Long(ch_id)]);
        });
    }
}

// ============================================================================
// Radio installation — matching two independent lifecycles
// ============================================================================

/// The node's radio slot, once a node has been built, and the bridge Kotlin
/// last created. Either can arrive first, so both are kept and re-married
/// whenever one of them changes.
///
/// The slot used to be a fips process-global (`set_android_ble_bridge`); it is
/// node-scoped now, handed out by `Node::enable_app_owned_ble_radio`. That is a
/// better shape but it does not, on its own, span the two lifecycles Myco has
/// to reconcile:
///
/// - The **node** is rebuilt on a BLE off→on cycle, because `run_rx_loop`
///   consumes it. Each rebuild yields a fresh slot, which must be re-populated
///   with the radio that is already running.
/// - The **radio** belongs to `BleService`, which deliberately does *not*
///   bounce the node when it starts a fresh one (bouncing tore down every peer
///   and session). `bleBridgeNew` also runs on the Android service thread,
///   before `StartNode` — so a bridge routinely exists before any slot does.
///
/// Keeping both here, in a static rather than on `AppRuntime`, is the same
/// pattern the TUN and UDP-fd bridges use: the JNI exports have no
/// `AppRuntime` handle, and a JVM thread must never take the reducer lock.
struct Installation {
    slot: Option<Arc<BleRadioSlot>>,
    bridge: Option<Arc<AndroidBleBridge>>,
}

static INSTALLATION: OnceLock<std::sync::Mutex<Installation>> = OnceLock::new();

fn installation() -> &'static std::sync::Mutex<Installation> {
    INSTALLATION.get_or_init(|| {
        std::sync::Mutex::new(Installation {
            slot: None,
            bridge: None,
        })
    })
}

/// Hand this node's radio slot over, replacing any prior node's.
///
/// Called from `AppRuntime::start_node` while it still holds `&mut Node`. If
/// Kotlin already created a radio, it is installed into the new slot
/// immediately — the whole point of the slot being resolvable per operation is
/// that this can happen under a node that is already running.
pub(crate) fn set_radio_slot(slot: Arc<BleRadioSlot>) {
    let mut install = installation().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(bridge) = install.bridge.as_ref() {
        slot.install(Arc::clone(bridge));
    }
    install.slot = Some(slot);
}

/// Record the radio Kotlin just built and install it into the current slot, if
/// there is one. Replaces any prior radio.
fn set_radio_bridge(bridge: Arc<AndroidBleBridge>) {
    let mut install = installation().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = install.slot.as_ref() {
        slot.install(Arc::clone(&bridge));
    }
    install.bridge = Some(bridge);
}

/// Retract a radio Kotlin has shut down, clearing it out of the node's slot.
///
/// Without this the installation kept holding a dead radio, and the next
/// [`set_radio_slot`] — which a mesh toggle reaches, because rebuilding the
/// node yields a fresh slot — installed that dead radio into the new slot.
/// The transport's `listen` and `start_advertising` then ran against closed
/// sockets until Kotlin's next `bleBridgeNew` replaced it, about a second
/// later. Benign in that window, but it is the same shape as the stale-PSM
/// bug: state that outlives the thing it describes. An empty slot parks the
/// backend until a live radio arrives, which is the correct degradation.
///
/// Retracts only the radio it was handed. `bleBridgeNew` may already have
/// installed a newer one — a stop racing a start must not take that one down.
fn clear_radio_bridge(bridge: &Arc<AndroidBleBridge>) {
    let mut install = installation().lock().unwrap_or_else(|e| e.into_inner());
    if !install
        .bridge
        .as_ref()
        .is_some_and(|held| Arc::ptr_eq(held, bridge))
    {
        return;
    }
    install.bridge = None;
    if let Some(slot) = install.slot.as_ref() {
        if slot.current().is_some_and(|c| Arc::ptr_eq(&c, bridge)) {
            slot.clear();
        }
    }
}

// ============================================================================
// Bridge handle + helpers
// ============================================================================

/// SAFETY: `handle` must be a pointer returned by `bleBridgeNew` and not freed.
unsafe fn bridge_ref<'a>(handle: jlong) -> Option<&'a Arc<AndroidBleBridge>> {
    if handle == 0 {
        None
    } else {
        Some(&*(handle as *const Arc<AndroidBleBridge>))
    }
}

fn jstring_to_addr(env: &mut JNIEnv, s: &JString) -> Option<BleAddr> {
    let owned: String = env.get_string(s).ok()?.into();
    BleAddr::parse(&owned).ok()
}

// ============================================================================
// JNI exports (called by the Kotlin BleRadio / BleService)
// ============================================================================

/// Create the bridge over a Kotlin `BleRadio`, install it into the node's radio
/// slot (or hold it until a node hands one over), and return an opaque handle.
///
/// No longer has to precede `StartNode`: the transport resolves the slot per
/// operation, so a radio that appears later is picked up without a rebuild.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleBridgeNew(
    env: JNIEnv,
    _class: JClass,
    _app_handle: jlong,
    radio: JObject,
) -> jlong {
    let global = match env.new_global_ref(&radio) {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let bridge = AndroidBleBridge::new(Arc::new(KotlinRadio { radio: global }));
    // Install (replacing any prior radio) so the node — fresh, rebuilt after a
    // BLE off/on cycle, or not yet built — ends up driving this one.
    set_radio_bridge(Arc::clone(&bridge));
    Box::into_raw(Box::new(bridge)) as jlong
}

/// Retract the radio behind `handle` from the node's slot.
///
/// Called by `BleService.stopBle` after it shuts the radio down. Does *not*
/// free the handle — the radio's I/O threads may still be winding down through
/// it — it only stops the core from driving a radio that is gone.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleBridgeClear(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: borrows, without reclaiming, the Box that `bleBridgeNew`
    // leaked. Kotlin holds the handle for the life of its radio and passes it
    // back exactly once, before any `bleBridgeFree`.
    let bridge = unsafe { &*(handle as *const Arc<AndroidBleBridge>) };
    clear_radio_bridge(bridge);
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleBridgeFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: reclaims the Box from bleBridgeNew. The radio slot and the
        // installation record keep their own Arc clones, so the bridge itself
        // outlives this handle.
        unsafe { drop(Box::from_raw(handle as *mut Arc<AndroidBleBridge>)) };
    }
}

/// Kotlin accepted an inbound L2CAP channel. Returns the allocated channel id.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleDeliverInbound(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    addr: JString,
    send_mtu: jint,
    recv_mtu: jint,
) -> jlong {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else {
        return 0;
    };
    let Some(ble_addr) = jstring_to_addr(&mut env, &addr) else {
        return 0;
    };
    bridge.deliver_inbound(ble_addr, send_mtu.max(0) as u16, recv_mtu.max(0) as u16)
}

/// Kotlin finished (ok) or failed an outbound dial. Returns the channel id, or 0.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleDeliverConnectResult(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    connect_id: jlong,
    ok: jboolean,
    addr: JString,
    send_mtu: jint,
    recv_mtu: jint,
) -> jlong {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else {
        return 0;
    };
    let ble_addr = jstring_to_addr(&mut env, &addr).unwrap_or(BleAddr {
        adapter: "ble0".to_string(),
        device: [0; 6],
    });
    bridge.deliver_connect_result(
        connect_id,
        ok != 0,
        ble_addr,
        send_mtu.max(0) as u16,
        recv_mtu.max(0) as u16,
    )
}

/// Kotlin discovered a FIPS peer advertising `psm`.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleDeliverScan(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    addr: JString,
    psm: jint,
    rssi: jint,
) {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else {
        return;
    };
    if let Some(ble_addr) = jstring_to_addr(&mut env, &addr) {
        bridge.deliver_scan(ble_addr, psm.max(0) as u16, rssi_from_jint(rssi));
    }
}

/// Kotlin read a self-advertised display name out of a peer's BLE scan
/// response.
///
/// Separate from [`Java_app_myco_core_NativeCore_bleDeliverScan`] on purpose:
/// the name has no bearing on routing, so it never enters the fips bridge and
/// lands in Myco's own [`crate::advert_names`] record instead. Needs no bridge
/// handle for the same reason — the map is process-global, like the lane
/// record.
///
/// The value is an unauthenticated broadcast anyone in range can forge. It is
/// stored as such; the display layer is what keeps it below every name learned
/// from signed pair traffic.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleDeliverAdvertName(
    mut env: JNIEnv,
    _class: JClass,
    addr: JString,
    name: JString,
) {
    let Ok(addr) = env.get_string(&addr) else {
        return;
    };
    let Ok(name) = env.get_string(&name) else {
        return;
    };
    crate::advert_names::set_name(&String::from(addr), &String::from(name));
}

/// Android's `ScanResult.rssi` in dBm, as an optional signal strength.
///
/// `127` is the platform's "RSSI unknown" sentinel and is not a real reading,
/// so it maps to `None` rather than to an implausibly strong signal. Anything
/// outside `i16` cannot come from the radio and is treated the same way.
fn rssi_from_jint(rssi: jint) -> Option<i16> {
    match rssi {
        127 => None,
        v => i16::try_from(v).ok(),
    }
}

/// Whether Kotlin has ever pushed a scanning state — until it has, the value is
/// unknown, never a guessed false.
static BLE_SCANNING_KNOWN: AtomicBool = AtomicBool::new(false);
/// The last-pushed scanning value, meaningful only once `BLE_SCANNING_KNOWN`.
static BLE_SCANNING: AtomicBool = AtomicBool::new(false);
/// Whether Kotlin has ever pushed an advertising state.
static BLE_ADVERTISING_KNOWN: AtomicBool = AtomicBool::new(false);
/// The last-pushed advertising value, meaningful only once
/// `BLE_ADVERTISING_KNOWN`.
static BLE_ADVERTISING: AtomicBool = AtomicBool::new(false);

/// The last-observed scanning state, or `None` if Kotlin has never pushed one
/// (radio never started, or a non-Android build) — the caller must render
/// unknown rather than guessing false.
pub(crate) fn ble_scanning() -> Option<bool> {
    if BLE_SCANNING_KNOWN.load(Ordering::Relaxed) {
        Some(BLE_SCANNING.load(Ordering::Relaxed))
    } else {
        None
    }
}

/// The last-observed advertising state, or `None` if Kotlin has never pushed
/// one.
pub(crate) fn ble_advertising() -> Option<bool> {
    if BLE_ADVERTISING_KNOWN.load(Ordering::Relaxed) {
        Some(BLE_ADVERTISING.load(Ordering::Relaxed))
    } else {
        None
    }
}

/// Kotlin reports whether its BLE scan loop is live right now, pushed from the
/// scan callback's own start/stop/retry-failure sites.
///
/// Kept in a Myco-owned atomic rather than read back off fips's
/// `AndroidBleBridge`, which is where it used to live: the flag was only ever
/// Kotlin's own push bouncing off a struct in the wrong crate. Same shape as
/// the Aware lane's `set_aware_discovering`. Diagnostic only — nothing
/// functional reads it.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleDeliverScanningState(
    _env: JNIEnv,
    _class: JClass,
    _handle: jlong,
    on: jboolean,
) {
    BLE_SCANNING.store(on != 0, Ordering::Relaxed);
    BLE_SCANNING_KNOWN.store(true, Ordering::Relaxed);
}

/// Kotlin reports whether its BLE advertiser is live right now, pushed from the
/// advertise callback's own install/clear sites. Same ownership rationale as
/// [`Java_app_myco_core_NativeCore_bleDeliverScanningState`].
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleDeliverAdvertisingState(
    _env: JNIEnv,
    _class: JClass,
    _handle: jlong,
    on: jboolean,
) {
    BLE_ADVERTISING.store(on != 0, Ordering::Relaxed);
    BLE_ADVERTISING_KNOWN.store(true, Ordering::Relaxed);
}

/// Kotlin read one L2CAP packet. Returns 1 if delivered, 0 if the channel is gone.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleChannelDeliverRecv(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    ch_id: jlong,
    data: JByteArray,
    len: jint,
) -> jboolean {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else {
        return 0;
    };
    let n = len.max(0) as usize;
    let mut buf = vec![0i8; n];
    if env.get_byte_array_region(&data, 0, &mut buf).is_err() {
        return 0;
    }
    let bytes: Vec<u8> = buf.into_iter().map(|b| b as u8).collect();
    jboolean::from(bridge.deliver_recv(ch_id, &bytes))
}

/// Kotlin reports a channel closed (EOF / socket gone).
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleChannelClosed(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    ch_id: jlong,
) {
    if let Some(bridge) = unsafe { bridge_ref(handle) } {
        bridge.channel_closed(ch_id);
    }
}

/// Kotlin's per-channel writer thread pulls the next outbound packet, blocking up
/// to `timeout_ms`. Returns: >0 = bytes written into `out`; 0 = timed out (loop
/// again); -1 = channel closed (stop the writer).
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleChannelNextSend(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    ch_id: jlong,
    out: JByteArray,
    timeout_ms: jint,
) -> jint {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else {
        return -1;
    };
    match bridge.next_send(ch_id, Duration::from_millis(timeout_ms.max(0) as u64)) {
        Some(bytes) => {
            let i8buf: Vec<i8> = bytes.iter().map(|&b| b as i8).collect();
            let cap = env.get_array_length(&out).unwrap_or(0).max(0) as usize;
            let n = i8buf.len().min(cap);
            if env.set_byte_array_region(&out, 0, &i8buf[..n]).is_err() {
                return -1;
            }
            n as jint
        }
        None => {
            if bridge.channel_open(ch_id) {
                0
            } else {
                -1
            }
        }
    }
}
