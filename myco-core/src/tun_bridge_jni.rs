//! JNI packet bridge for the Android `VpnService`'s TUN pump — the IPv6
//! counterpart to the BLE byte-bridge. The VpnService reader thread reads IPv6
//! packets from the TUN fd and calls `tunSendPacket` (app → mesh); its writer
//! thread calls `tunNextPacket` (mesh → app) and writes them to the fd. Both
//! operate on the process-global channels in [`crate::tun_bridge`], so the
//! blocking `tunNextPacket` never touches the `AppRuntime` mutex.
//!
//! Android-only; verified by the cargo-ndk build, not host `cargo test`.

use std::time::Duration;

use jni::objects::{JByteArray, JClass};
use jni::sys::{jboolean, jint};
use jni::JNIEnv;

/// Kotlin → Rust: route an IPv6 packet read from the TUN fd into the mesh.
/// Returns `true` if accepted (a TUN is installed and the queue had room).
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_tunSendPacket(
    mut env: JNIEnv,
    _class: JClass,
    packet: JByteArray,
    len: jint,
) -> jboolean {
    let n = len.max(0) as usize;
    let mut buf = vec![0i8; n];
    if env.get_byte_array_region(&packet, 0, &mut buf).is_err() {
        return false as jboolean;
    }
    let bytes: Vec<u8> = buf.iter().map(|&b| b as u8).collect();
    crate::tun_bridge::send_packet(bytes) as jboolean
}

/// Rust → Kotlin: pull the next IPv6 packet for the TUN fd, blocking up to
/// `timeout_ms`. Returns the byte count written into `out`, or 0 on timeout
/// (the writer loops again).
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_tunNextPacket(
    mut env: JNIEnv,
    _class: JClass,
    out: JByteArray,
    timeout_ms: jint,
) -> jint {
    match crate::tun_bridge::next_packet(Duration::from_millis(timeout_ms.max(0) as u64)) {
        Some(bytes) => {
            let i8buf: Vec<i8> = bytes.iter().map(|&b| b as i8).collect();
            let cap = env.get_array_length(&out).unwrap_or(0).max(0) as usize;
            let n = i8buf.len().min(cap);
            if env.set_byte_array_region(&out, 0, &i8buf[..n]).is_err() {
                return 0;
            }
            n as jint
        }
        None => 0,
    }
}
