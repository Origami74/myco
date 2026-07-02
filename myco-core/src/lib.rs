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
mod content;
// The mesh gossiper is wired only into the Android relay server (runtime.rs); on
// the host it is exercised only by its own tests, so it reads as dead there.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod gossip;
mod identity_store;
mod ip_source;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod peer_relay;
mod runtime;
mod state;
// The bridge is pumped only by the Android VpnService (via tun_bridge_jni) and
// installed only on Android, so its fns read as dead on the host build.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod tun_bridge;

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
