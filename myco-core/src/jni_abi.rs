//! Android JNI surface — `Java_app_myco_core_NativeCore_*`.
//!
//! Modeled on nostr-vpn's `c_abi.rs` Android path: an opaque `jlong` wraps a
//! `Box<AppHandle>`; Kotlin calls `dispatch(actionJson) -> stateJson`. Only
//! compiled when targeting Android (host builds drive `AppRuntime` directly).
//!
//! NOTE: this module is not compiled on the host toolchain, so it is verified by
//! the Android (cargo-ndk) build, not by host `cargo test`.

use std::sync::Mutex;

use jni::objects::{JClass, JObject, JString};
use jni::sys::{jbyteArray, jlong, jstring};
use jni::JNIEnv;

use crate::runtime::AppRuntime;

/// What the opaque handle points at.
struct AppHandle {
    rt: Mutex<AppRuntime>,
}

fn jstr(env: &mut JNIEnv, s: String) -> jstring {
    env.new_string(s)
        .map(|o| o.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn get_string(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|js| js.into()).unwrap_or_default()
}

/// SAFETY: `handle` must be a pointer returned by `appNew` and not yet freed.
unsafe fn handle_ref<'a>(handle: jlong) -> Option<&'a AppHandle> {
    if handle == 0 {
        None
    } else {
        Some(&*(handle as *const AppHandle))
    }
}

fn locked_state<F: FnOnce(&mut AppRuntime) -> String>(handle: jlong, f: F) -> String {
    match unsafe { handle_ref(handle) } {
        Some(h) => {
            let mut guard = h.rt.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut guard)
        }
        None => r#"{"error":"null native handle"}"#.to_string(),
    }
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_initializeAndroidContext(
    env: JNIEnv,
    _class: JClass,
    _context: JObject,
) {
    // Capture the JavaVM so the BLE byte-bridge can attach tokio worker threads
    // before issuing control upcalls into the Kotlin radio.
    crate::ble_bridge_jni::capture_java_vm(&env);
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_appNew(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
    app_version: JString,
) -> jlong {
    init_android_logging();
    let data_dir = get_string(&mut env, &data_dir);
    let app_version = get_string(&mut env, &app_version);
    let rt = AppRuntime::new(&data_dir, &app_version);
    Box::into_raw(Box::new(AppHandle { rt: Mutex::new(rt) })) as jlong
}

/// Route `tracing` (myco-core + fips) to Android logcat under the tag `myco`, so
/// the sync engine / FSP / BLE internals are visible (`adb logcat -s myco`).
/// Idempotent; level DEBUG.
fn init_android_logging() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let writer = paranoid_android::AndroidLogMakeWriter::new("myco".to_owned());
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer),
            )
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .try_init();
        tracing::info!("myco-core logcat bridge active (tag=myco, level=DEBUG)");
    });
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_appFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: reclaims the Box allocated in `appNew`; Kotlin guards against
        // double-free by zeroing its stored handle.
        unsafe { drop(Box::from_raw(handle as *mut AppHandle)) };
    }
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_stateJson(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let json = locked_state(handle, |rt| rt.state_json());
    jstr(&mut env, json)
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_refreshJson(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let json = locked_state(handle, |rt| rt.dispatch_json(r#"{"type":"tick"}"#));
    jstr(&mut env, json)
}

#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_dispatchJson(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    action_json: JString,
) -> jstring {
    let action = get_string(&mut env, &action_json);
    let json = locked_state(handle, |rt| rt.dispatch_json(&action));
    jstr(&mut env, json)
}

/// Serve one nsite request for the in-app WebView's `shouldInterceptRequest`.
///
/// Returns a framed byte array `[u32 BE header-len][header JSON][body]` (see
/// `content::frame_response`). This is the **TUN-independent** in-app serve path:
/// the WebView loads `http://<host>.nsite/…`, its interceptor calls here, and the
/// gateway serves direct from the in-process relay + Blossom. The runtime mutex is
/// only held long enough to clone out an `Arc<Content>` + Tokio handle, so
/// concurrent subresource requests don't serialize on it.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_gatewayGet(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    host: JString,
    path: JString,
    range: JString,
) -> jbyteArray {
    let host = get_string(&mut env, &host);
    let path = get_string(&mut env, &path);
    let range = get_string(&mut env, &range);
    let range_opt = (!range.is_empty()).then_some(range.as_str());

    let ctx = match unsafe { handle_ref(handle) } {
        Some(h) => {
            let guard = h.rt.lock().unwrap_or_else(|p| p.into_inner());
            guard.gateway_context()
        }
        None => None,
    };

    let framed = match ctx {
        Some((content, rt_handle)) => {
            rt_handle.block_on(content.gateway_get_framed(&host, &path, range_opt))
        }
        None => Vec::new(),
    };

    env.byte_array_from_slice(&framed)
        .map(|a| a.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
