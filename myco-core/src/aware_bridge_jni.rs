//! Wi-Fi Aware bridge — the JNI side.
//!
//! Unlike the BLE bridge, this is *control-plane only*: there is no byte
//! bridge and no `AndroidRadio` trait to implement. A Wi-Fi Aware data path
//! terminates in a kernel network interface, so the bytes ride an ordinary
//! UDP transport — one of the node's `"aware0"`…`"aware3"` instances, bound at
//! `runtime::AWARE_UDP_BASE_PORT + slot` and pinned by `AwareRadio` to that
//! peer's NDP `android.net.Network`. One socket can be marked for only one
//! network, so the pool is what lets the lane carry more than one peer.
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
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;

fn jstring(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(Into::into)
}

/// Kotlin established a Wi-Fi Aware data path: peer `npub` is reachable at
/// `addr` (`"[fe80::x%ifindex]:port"`). The node reaches it over the UDP
/// transport; the Noise IK handshake authenticates — the pushed npub is only
/// a routing hint.
///
/// `lane` is `"udp"` for the LAN/AP radio, or the Aware radio's slot —
/// `"aware0"`…`"aware3"`. Every lane, and every Aware slot within it, is its own
/// UDP transport instance with its own socket, pinned to its own
/// `android.net.Network`, so the address is qualified with that instance's name
/// (`"udp/aware2"`, `"udp/lan"`). fips's `TransportSpec` routes the dial to the
/// matching socket and, crucially, refuses rather than substituting another one
/// — dialing an Aware peer from the Wi-Fi-pinned socket is unroutable and was
/// the reason Aware never carried a handshake, and dialing peer B's NDP from
/// the socket pinned to peer A's is the same fault one layer down.
///
/// The lane is *also* recorded, separately, in [`crate::lane_observation`], for
/// `merge_peers()`'s `lane_by_npub` override — as the family name (`"aware"`),
/// because which radio saw a peer is one fact regardless of which socket
/// carries it. That record is Myco-owned and never reaches fips.
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
    crate::lane_observation::set_lane(&npub, crate::runtime::lane_family(&lane));
    let transport = format!("udp/{}", crate::runtime::udp_instance_for_lane(&lane));
    crate::platform_peers::push(&npub, &addr, &transport);
}

/// Kotlin observed the Wi-Fi Aware data path to `npub` go away.
///
/// **Nothing is told to the node, deliberately.** This used to call fips's
/// `platform_peer_lost`, which resolved the peer and asked the named transport
/// to close its connection — but the UDP transport does not override
/// `close_connection`; it falls through to the connectionless no-op default.
/// So the call has never had any effect, and the premise it was written on
/// ("the node closes the pooled UDP session so the dead socket is not
/// re-used") was wrong: a connectionless transport has no pooled socket.
/// Falling back to BLE was always the node's ordinary liveness machinery doing
/// its job.
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
    crate::lane_observation::clear_lane(&npub, crate::runtime::lane_family(&lane));
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

/// Rust → Kotlin: the raw socket fd of the UDP transport carrying `lane`, once
/// it has opened and if it is newer than `since_version`. Blocks up to
/// `timeout_ms`.
///
/// `lane` is the caller's own label (`"udp"`, or an Aware slot `"aware0"`…),
/// mapped to a fips instance name by [`crate::runtime::udp_instance_for_lane`].
/// The node binds one socket per instance and fips labels each fd with the
/// instance it belongs to, so a caller can only ever be handed *its*
/// descriptor — never another's, which it would then pin to the wrong
/// `android.net.Network` and black-hole. An instance whose socket did not bind
/// is told nothing instead.
///
/// `AwareRadio` calls this once per slot, holding a pin per instance, because
/// each of its peers needs a socket marked for that peer's own NDP.
///
/// The result packs `(version << 32) | fd`, because JNI has no tuple and two
/// calls could not be made atomic. `fd` is `-1` when nothing newer arrived; the
/// caller passes the returned version back on the next call, and 0 on the
/// first. Versioning rather than plain edge-triggering because a radio is
/// created and destroyed with its lane's toggle while the node keeps running,
/// so it must be able to learn a socket announced before it existed — and
/// because a node restart's replacement socket can reuse the same fd number,
/// which still needs re-binding.
///
/// What the caller does with the fd: `android.net.Network.bindSocket`, pinning
/// it to the local-only network carrying that lane's peers (a Wi-Fi Aware NDP,
/// the `!FIPS` AP). Without it, replies are lost to a competing validated
/// default network (e.g. cellular).
#[no_mangle]
pub extern "system" fn Java_app_myco_core_NativeCore_nextUdpTransportFd(
    mut env: JNIEnv,
    _class: JClass,
    lane: JString,
    since_version: jlong,
    timeout_ms: jint,
) -> jlong {
    let Some(lane) = jstring(&mut env, &lane) else {
        return -1i32 as u32 as jlong;
    };
    let (version, fd) = crate::udp_fd_bridge::next_fd(
        crate::runtime::udp_instance_for_lane(&lane),
        since_version.max(0) as u64,
        std::time::Duration::from_millis(timeout_ms.max(0) as u64),
    );
    ((version << 32) | (fd as u32 as u64)) as jlong
}
