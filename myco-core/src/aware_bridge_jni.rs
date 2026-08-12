//! Wi-Fi Aware bridge — the JNI side.
//!
//! Unlike the BLE bridge, this is *control-plane only*: there is no byte
//! bridge and no `AndroidRadio` trait to implement. A Wi-Fi Aware data path
//! terminates in a kernel network interface, so the bytes ride the ordinary
//! UDP transport (bound at `runtime::WIFI_AWARE_PORT` on the NDP interface).
//! The Kotlin `AwareRadio` runs discovery autonomously and only pushes
//! "peer reachable" events into Myco's own bounded queue
//! ([`crate::platform_peers`]), which a tokio task drains onto the node's
//! control socket. The push itself never touches the socket: it arrives on the
//! radio's single `HandlerThread`, which must not be held.
//!
//! Kotlin passes the peer's link-local address already formatted with a
//! *numeric* scope (`"[fe80::x%3]:4871"`, ifindex resolved from
//! `LinkProperties`) — interface-name scopes do not parse (see
//! docs/design/wifi-aware-interop.md § "Dialing a link-local peer").
//!
//! Compiled only on Android; the host build exercises the same seam directly
//! through [`crate::platform_peers`].

use std::sync::atomic::{AtomicBool, Ordering};

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint};
use jni::JNIEnv;

const TRANSPORT_TYPE: &str = "udp";

fn jstring(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(Into::into)
}

/// Kotlin established a Wi-Fi Aware data path: peer `npub` is reachable at
/// `addr` (`"[fe80::x%ifindex]:port"`). The node reaches it over the UDP
/// transport; the Noise IK handshake authenticates — the pushed npub is only
/// a routing hint.
///
/// `lane` is `"aware"` or `"udp"` — which Kotlin radio (Wi-Fi Aware vs. the
/// LAN/AP lane) observed this peer. Both ride fips's plain UDP transport, so
/// `TRANSPORT_TYPE` below is unchanged and still `"udp"` for both; `lane` is
/// recorded separately, in [`crate::lane_observation`], for `merge_peers()`'s
/// `lane_by_npub` override and never reaches fips.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_awarePeerFound(
    mut env: JNIEnv,
    _class: JClass,
    npub: JString,
    addr: JString,
    lane: JString,
) {
    let (Some(npub), Some(addr), Some(lane)) = (
        jstring(&mut env, &npub),
        jstring(&mut env, &addr),
        jstring(&mut env, &lane),
    ) else {
        return;
    };
    crate::lane_observation::set_lane(&npub, &lane);
    crate::platform_peers::push(&npub, &addr, TRANSPORT_TYPE);
}

/// Kotlin observed the Wi-Fi Aware data path to `npub` go away.
///
/// **Nothing is told to the node, deliberately.** This used to call fips's
/// `platform_peer_lost`, which resolved the peer and asked the named transport
/// to close its connection — but the name pushed here is always `"udp"` for
/// both lanes, and the UDP transport does not override `close_connection`; it
/// falls through to the connectionless no-op default. So the call has never
/// had any effect, and the premise it was written on ("the node closes the
/// pooled UDP session so the dead socket is not re-used") was wrong: a
/// connectionless transport has no pooled socket. Falling back to BLE was
/// always the node's ordinary liveness machinery doing its job.
///
/// The control socket's `disconnect` is not a replacement. It keys on npub
/// alone, with no transport parameter, and does a full teardown — notify the
/// peer, drop every session, index and link, and suppress auto-reconnect. Aware
/// data paths are fragile and `onLost` fires often, so wiring it here would let
/// a routine NDP drop tear down a live BLE session to the same peer. That is a
/// direct hit on the one thing the product has to do.
///
/// What remains is the Myco-owned Dev-tab label: `lane` names which radio
/// observed the loss, and [`crate::lane_observation`] clears the recorded lane
/// for `npub` only if it still matches, so a stale loss from one lane cannot
/// erase a fresher record pushed by the other.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_awarePeerLost(
    mut env: JNIEnv,
    _class: JClass,
    npub: JString,
    lane: JString,
) {
    let (Some(npub), Some(lane)) = (jstring(&mut env, &npub), jstring(&mut env, &lane)) else {
        return;
    };
    crate::lane_observation::clear_lane(&npub, &lane);
}

// ============================================================================
// Observed discovering state (developer diagnostics only)
// ============================================================================

/// Whether Kotlin has ever pushed a discovering state — until it has, the
/// value is unknown, never a guessed false.
static AWARE_DISCOVERING_KNOWN: AtomicBool = AtomicBool::new(false);
/// The last-pushed discovering value, meaningful only once
/// `AWARE_DISCOVERING_KNOWN` is true.
static AWARE_DISCOVERING: AtomicBool = AtomicBool::new(false);

/// Record whether the Aware publish/subscribe session pair is live right now
/// — the Aware analogue of a BLE scan. Called from `awareSetDiscovering`.
pub(crate) fn set_aware_discovering(on: bool) {
    AWARE_DISCOVERING.store(on, Ordering::Relaxed);
    AWARE_DISCOVERING_KNOWN.store(true, Ordering::Relaxed);
}

/// The last-observed discovering state, or `None` if Kotlin has never pushed
/// one (radio never started, or a non-Android build) — the caller must render
/// unknown rather than guessing false.
pub(crate) fn aware_discovering() -> Option<bool> {
    if AWARE_DISCOVERING_KNOWN.load(Ordering::Relaxed) {
        Some(AWARE_DISCOVERING.load(Ordering::Relaxed))
    } else {
        None
    }
}

/// Kotlin reports whether the Aware publish/subscribe session pair is live
/// right now, pushed after publish/subscribe install and on teardown. The
/// observed radio state for the developer diagnostics UI only.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_awareSetDiscovering(
    _env: JNIEnv,
    _class: JClass,
    on: jboolean,
) {
    set_aware_discovering(on != 0);
}

/// Kotlin → Rust: the underlying network's real DNS servers, comma-separated
/// (`"8.8.8.8,1.1.1.1"`; a port may be appended as `addr:53`). The sentinel is
/// the tunnel's only advertised resolver, so these are where non-`.fips`
/// queries get relayed — without them nothing but `.fips` resolves.
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_setUpstreamDns(
    mut env: JNIEnv,
    _class: JClass,
    servers: JString,
) {
    let Some(list) = jstring(&mut env, &servers) else {
        return;
    };
    let parsed = list
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            // Accept a bare address (default :53) or an explicit socket address.
            s.parse::<std::net::SocketAddr>()
                .ok()
                .or_else(|| s.parse::<std::net::IpAddr>().ok().map(|ip| (ip, 53).into()))
        })
        .collect();
    crate::dns_intercept::set_upstream(parsed);
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
