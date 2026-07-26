//! Wi-Fi Aware bridge — the JNI side.
//!
//! Unlike the BLE bridge, this is *control-plane only*: there is no byte
//! bridge and no `AndroidRadio` trait to implement. A Wi-Fi Aware data path
//! terminates in a kernel network interface, so the bytes ride the ordinary
//! UDP transport (bound at `runtime::WIFI_AWARE_PORT` on the NDP interface).
//! The Kotlin `AwareRadio` runs discovery autonomously and only pushes
//! "peer reachable / lost" events into fips's process-global platform peer
//! queue (`fips::discovery::platform`), which the node drains each tick.
//!
//! Kotlin passes the peer's link-local address already formatted with a
//! *numeric* scope (`"[fe80::x%3]:4871"`, ifindex resolved from
//! `LinkProperties`) — interface-name scopes do not parse (see
//! docs/design/wifi-aware-interop.md § "Dialing a link-local peer").
//!
//! Compiled only on Android; the host build exercises the same seam directly
//! through `fips::discovery::platform`.

use jni::objects::{JClass, JString};
use jni::sys::jint;
use jni::JNIEnv;

const TRANSPORT_TYPE: &str = "udp";

fn jstring(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(Into::into)
}

/// Kotlin established a Wi-Fi Aware data path: peer `npub` is reachable at
/// `addr` (`"[fe80::x%ifindex]:port"`). The node reaches it over the UDP
/// transport; the Noise IK handshake authenticates — the pushed npub is only
/// a routing hint.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_awarePeerFound(
    mut env: JNIEnv,
    _class: JClass,
    npub: JString,
    addr: JString,
) {
    let (Some(npub), Some(addr)) = (jstring(&mut env, &npub), jstring(&mut env, &addr)) else {
        return;
    };
    fips::discovery::platform::platform_peer_available(&npub, &addr, TRANSPORT_TYPE);
}

/// Kotlin observed the Wi-Fi Aware data path to `npub` go away. The node
/// closes the pooled UDP session so the dead socket is not re-used;
/// reconnection (e.g. falling back to BLE) is the node's ordinary job.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_awarePeerLost(
    mut env: JNIEnv,
    _class: JClass,
    npub: JString,
) {
    let Some(npub) = jstring(&mut env, &npub) else {
        return;
    };
    fips::discovery::platform::platform_peer_lost(&npub, TRANSPORT_TYPE);
}

/// Rust → Kotlin: the UDP transport's raw socket fd, once it has opened.
/// Blocks up to `timeout_ms`; returns `-1` on timeout (no transport, or it
/// hasn't started yet). The fd is sent once per node lifetime (see
/// [`crate::udp_fd_bridge`]) — callers poll this once at startup, not in a
/// loop, then use the fd with `android.net.Network.bindSocket` to pin the
/// socket to whichever local-only network (Wi-Fi Aware NDP, the `!FIPS` AP)
/// currently carries the platform-pushed peer, so replies aren't lost to a
/// competing validated default network (e.g. cellular).
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_nextUdpTransportFd(
    _env: JNIEnv,
    _class: JClass,
    timeout_ms: jint,
) -> jint {
    crate::udp_fd_bridge::next_fd(std::time::Duration::from_millis(timeout_ms.max(0) as u64))
        as jint
}
