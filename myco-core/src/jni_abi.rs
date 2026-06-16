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
use jni::sys::{jlong, jstring};
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
    let data_dir = get_string(&mut env, &data_dir);
    let app_version = get_string(&mut env, &app_version);
    let rt = AppRuntime::new(&data_dir, &app_version);
    Box::into_raw(Box::new(AppHandle { rt: Mutex::new(rt) })) as jlong
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
