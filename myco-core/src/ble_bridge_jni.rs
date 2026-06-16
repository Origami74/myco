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

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{JNIEnv, JavaVM};

use fips::transport::ble::addr::BleAddr;
use fips::transport::ble::android_io::{set_android_ble_bridge, AndroidBleBridge, AndroidRadio};

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
    let Some(vm) = JAVA_VM.get() else { return default };
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

/// Create the bridge over a Kotlin `BleRadio`, inject it into fips so the node's
/// BLE transport picks it up at start, and return an opaque handle. Must be
/// called before dispatching StartNode.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleBridgeNew(
    mut env: JNIEnv,
    _class: JClass,
    _app_handle: jlong,
    radio: JObject,
) -> jlong {
    let global = match env.new_global_ref(&radio) {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let bridge = AndroidBleBridge::new(Arc::new(KotlinRadio { radio: global }));
    // Inject (replacing any prior bridge) so the node — fresh or rebuilt after a
    // BLE off/on cycle — picks up this radio.
    set_android_ble_bridge(Arc::clone(&bridge));
    Box::into_raw(Box::new(bridge)) as jlong
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleBridgeFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: reclaims the Box from bleBridgeNew. The fips global keeps its
        // own Arc clone, so the bridge itself lives as long as the node does.
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
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else { return 0 };
    let Some(ble_addr) = jstring_to_addr(&mut env, &addr) else { return 0 };
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
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else { return 0 };
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
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else { return };
    if let Some(ble_addr) = jstring_to_addr(&mut env, &addr) {
        bridge.deliver_scan(ble_addr, psm.max(0) as u16, rssi);
    }
}

/// Kotlin read one L2CAP packet. Returns 1 if delivered, 0 if the channel is gone.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_bleChannelDeliverRecv(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    ch_id: jlong,
    data: JByteArray,
    len: jint,
) -> jboolean {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else { return 0 };
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
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    ch_id: jlong,
    out: JByteArray,
    timeout_ms: jint,
) -> jint {
    let Some(bridge) = (unsafe { bridge_ref(handle) }) else { return -1 };
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
