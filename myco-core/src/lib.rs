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
mod identity_store;
mod ip_source;
mod runtime;
mod state;

#[cfg(target_os = "android")]
mod jni_abi;

#[cfg(target_os = "android")]
mod ble_bridge_jni;

pub use action::NativeAppAction;
pub use runtime::AppRuntime;
pub use state::{AppState, IdentityView, NodeStatus};
