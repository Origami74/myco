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
pub(crate) mod file_transfer;
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
mod intent;
mod ip_source;
// Profiles, relay lists and manifests seen from outside, kept in the local
// relay on the way past.
mod keep_seen;
// The NIP-01 front door: live subscriptions, the mesh fan-out hook, and the
// access gate. Bound to its sockets only by the Android runtime, so on the host
// it reads as dead outside its own tests (and the tests that use it as a plain
// relay). See `reference/thinning-custom-relay.md`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod mesh_relay;
// Wires `myco-napplet-runtime` to this device's relay and Blossom store, and
// holds one session per open napplet window. See
// `docs/design/napplet/napplet-runtime.md`.
mod napplet;
// NAP-MESH `mesh.blobs`: which blobs the Circle's Blossom stores hold, asked
// with a bounded, remembered `HEAD` per peer over the mesh.
mod mesh_blobs;
// NAP-UPLOAD: a napplet's bytes onto the user's Blossom servers, signed as
// the user. Spec: napplet/naps PR #33; Myco's choices in
// `docs/design/napplet/NAP-UPLOAD.md`.
mod blossom_upload;
mod outbox;
// The user key a napplet publishes as — separate from the mesh device key (D3).
mod user_key;
// The account behind the Settings header: login, logout, the guest profile.
mod account;
// The guest profile picture: the logo, tinted from the npub.
mod guest_avatar;
// Signing with a key in a signer app (NIP-55, Amber), carried by Kotlin.
mod external_signer;
// The `MESH` envelope that carries mesh state alongside — never inside — a
// NIP-01 message on the peer link. See `reference/thinning-custom-relay.md`.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod mesh_wire;
// A RelayBackend backed by someone else's NIP-01 relay — the point of the seam.
// Not wired to settings yet, so it reads as dead outside its own tests.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod remote_backend;
// The blob half of the same idea: a BlobStore over someone else's Blossom.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod remote_blobs;
// Settings that must survive a restart, because they decide how the content
// layer is constructed rather than how it behaves.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod settings_store;
// Reading the local stores and the shell cache as one; which a write lands in.
mod tiered;
// npub -> observed lane record (Wi-Fi Aware vs. LAN/AP), pushed by the
// Android Aware JNI bridge and consumed by `AppRuntime::state()`'s
// lane_by_npub override. Plain, non-JNI logic so it is unit-testable on the
// host; the Android JNI bridge is its only real caller.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod advert_names;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod lane_observation;
mod peer_diagnostics;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod peer_relay;
// Internet relays and Blossom servers not worth dialling right now, for the
// whole process: every internet dial path checks it first.
mod relay_health;
mod relay_pool;
// Bounded queue + drainer between the Kotlin radios' callback threads and the
// node's control socket, where pushing a platform-discovered peer now lives.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod platform_peers;
// Public internet mesh nodes (N10): adverts read off Nostr, the recommended
// list, and the dials over the internet lane. The driver runs on Android only.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod public_nodes;
mod runtime;
mod state;
mod update_gate;
// The bridge is pumped only by the Android VpnService (via tun_bridge_jni) and
// installed only on Android, so its fns read as dead on the host build.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod tun_bridge;
// System-wide `.fips` DNS interception; driven by the TUN pump on Android.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod dns_intercept;
// Full-tunnel exit: non-mesh TCP from the TUN, carried to a SOCKS5 proxy on a
// mesh exit node. Turned on by the Android VpnService.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod socks_exit;
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
