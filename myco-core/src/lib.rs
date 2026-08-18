//! `myco-core` — the Myco app crate.
//!
//! P0 scaffold: it owns the device **identity** (a single Nostr keypair,
//! generated and persisted on first launch), **embeds FIPS** via
//! [`fips::Node::new`], and exposes a Redux-style **JNI/JSON reducer** FFI to
//! Kotlin (`dispatch(actionJson) -> stateJson`, with a monotonic `rev`).
//!
//! Layers above this (relay, Blossom, gateway, nsite sync, BLE) land in later
//! phases — see `docs/roadmap.md`. The host build compiles everything except the
//! Android-only JNI glue, so [`AppRuntime`] is unit-testable on macOS/Linux.

mod action;
mod attempt_store;
// The auth plane: the only port an unpaired peer can reach. Bound by the Android
// runtime, so it reads as dead on the host outside its own tests.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod auth_service;
// Myco-owned BLE connect-attempt vocabulary. These used to be fips types read
// out of a transport-global log; the restacked fips counts outcomes into
// `BleStats` instead. Nothing produces these yet — see the module doc's
// TODO(stage 2).
mod ble_diag;
mod content;
// Client for the fips node's Unix-domain control socket — the only way to read
// peer state or push a platform-discovered peer into a node whose `run_rx_loop`
// has borrowed it. Polled only by the Android peer-state tick and the platform
// peer drainer, so it reads as dead on the host build (its own tests aside).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod control_client;
// The mesh gossiper is wired only into the Android relay server (runtime.rs); on
// the host it is exercised only by its own tests, so it reads as dead there.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod gossip;
mod identity_store;
mod ip_source;
// The NIP-01 front door: live subscriptions, the mesh fan-out hook, and the
// access gate. Bound to its sockets only by the Android runtime, so on the host
// it reads as dead outside its own tests (and the tests that use it as a plain
// relay). See `reference/thinning-custom-relay.md`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod mesh_relay;
// The `MESH` envelope that carries mesh state alongside — never inside — a
// NIP-01 message on the peer link. See `reference/thinning-custom-relay.md`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod mesh_wire;
// A RelayBackend backed by someone else's NIP-01 relay — the point of the seam.
// Not wired to settings yet, so it reads as dead outside its own tests.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod remote_backend;
// npub -> observed lane record (Wi-Fi Aware vs. LAN/AP), pushed by the
// Android Aware JNI bridge and consumed by `AppRuntime::state()`'s
// lane_by_npub override. Plain, non-JNI logic so it is unit-testable on the
// host; the Android JNI bridge is its only real caller.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod advert_names;
mod lane_observation;
mod peer_diagnostics;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod peer_relay;
// Bounded queue + drainer between the Kotlin radios' callback threads and the
// node's control socket, where pushing a platform-discovered peer now lives.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod platform_peers;
mod runtime;
mod state;
// The bridge is pumped only by the Android VpnService (via tun_bridge_jni) and
// installed only on Android, so its fns read as dead on the host build.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod tun_bridge;
// System-wide `.fips` DNS interception; driven by the TUN pump on Android.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod dns_intercept;
// Surfaces each UDP transport instance's raw fd, keyed by instance name, so
// Android can pin the right socket to the right `Network` (the Aware NDP vs.
// the AP/LAN lane). Android-only consumer.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod udp_fd_bridge;

#[cfg(target_os = "android")]
mod jni_abi;

#[cfg(target_os = "android")]
mod ble_bridge_jni;

#[cfg(target_os = "android")]
mod aware_bridge_jni;

#[cfg(target_os = "android")]
mod tun_bridge_jni;

pub use action::NativeAppAction;
pub use runtime::AppRuntime;
pub use state::{AppState, IdentityView, NodeStatus};
