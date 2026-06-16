use std::path::Path;

use tokio::runtime::Runtime;

use crate::action::NativeAppAction;
use crate::identity_store;
use crate::state::{AppState, BlePeer, BleStatus, IdentityView, NodeStatus};

/// The app runtime behind the FFI. Owns the device identity, a multi-thread
/// Tokio runtime, and the embedded fips node. A `Mutex<AppRuntime>` is what the
/// opaque JNI handle wraps (see `jni_abi`); on the host it is driven directly.
///
/// The node's background work (BLE accept/scan/probe loops, Noise handshakes)
/// runs on `rt`'s worker threads after `node.start()`, so it keeps progressing
/// between FFI polls. P1 does not drive the node's packet loop (`run_rx_loop`)
/// — that is the TUN/sync path, which arrives in P2.
pub struct AppRuntime {
    app_version: String,
    rev: u64,
    error: String,
    identity: IdentityView,
    ble_enabled: bool,
    node_running: bool,
    node_status: String,
    /// Tokio runtime hosting the node's tasks. `None` only if it failed to build.
    rt: Option<Runtime>,
    /// The embedded fips node. `None` if construction failed.
    node: Option<fips::Node>,
}

impl AppRuntime {
    /// Build the runtime for a data dir. Never panics: a startup failure is
    /// captured into [`AppState::error`] so the UI can surface it, mirroring
    /// nostr-vpn's `error_state`.
    pub fn new(data_dir: &str, app_version: &str) -> Self {
        match Self::try_new(data_dir, app_version) {
            Ok(rt) => rt,
            Err(e) => Self::from_error(app_version, &e.to_string()),
        }
    }

    fn try_new(data_dir: &str, app_version: &str) -> anyhow::Result<Self> {
        let dir = Path::new(data_dir);
        std::fs::create_dir_all(dir)?;

        // Multi-thread runtime so the node's spawned tasks self-drive between
        // FFI polls (see the struct doc).
        let rt = Runtime::new().map_err(|e| anyhow::anyhow!("tokio runtime: {e}"))?;

        // Identity: generate-or-load and persist the nsec.
        let nsec = identity_store::load_or_generate(dir)?;

        // Embed FIPS: construct the node from the persisted key. P1 keeps it
        // (unlike P0, which dropped it) so StartNode can run its transports.
        // No TUN yet; BLE transport instances are added when the Android backend
        // lands (P1 M4) — on the host there is no BLE backend, so the node simply
        // starts with no transports.
        let mut config = fips::Config::new();
        config.node.identity.nsec = Some(nsec);
        config.node.identity.persistent = true;
        config.tun.enabled = false;
        let node = fips::Node::new(config)
            .map_err(|e| anyhow::anyhow!("fips Node::new failed: {e}"))?;

        let identity = IdentityView::from_identity(node.identity());

        Ok(Self {
            app_version: app_version.to_string(),
            rev: 0,
            error: String::new(),
            identity,
            ble_enabled: false,
            node_running: false,
            node_status: "fips node constructed (not started)".to_string(),
            rt: Some(rt),
            node: Some(node),
        })
    }

    fn from_error(app_version: &str, msg: &str) -> Self {
        Self {
            app_version: app_version.to_string(),
            rev: 0,
            error: msg.to_string(),
            identity: IdentityView::default(),
            ble_enabled: false,
            node_running: false,
            node_status: "error".to_string(),
            rt: None,
            node: None,
        }
    }

    /// Reduce one action, mutating state and bumping `rev` for mutations.
    pub fn dispatch(&mut self, action: NativeAppAction) {
        match action {
            NativeAppAction::GetState => {} // pure read, no rev bump
            NativeAppAction::Tick => self.rev += 1,
            NativeAppAction::StartNode => {
                self.start_node();
                self.rev += 1;
            }
            NativeAppAction::StopNode => {
                self.stop_node();
                self.rev += 1;
            }
            NativeAppAction::SetBleEnabled { enabled } => {
                self.ble_enabled = enabled;
                // The radio itself lives in the Android foreground service
                // (P1 M4); here we record the master-switch intent the BLE
                // backend reads. On the host there is no BLE backend.
                self.node_status = if enabled {
                    "ble enabled".to_string()
                } else {
                    "ble disabled".to_string()
                };
                self.rev += 1;
            }
        }
    }

    fn start_node(&mut self) {
        if self.node_running {
            return;
        }
        match (&self.rt, &mut self.node) {
            (Some(rt), Some(node)) => match rt.block_on(node.start()) {
                Ok(()) => {
                    self.node_running = true;
                    self.node_status = "running".to_string();
                }
                Err(e) => {
                    self.error = format!("node start failed: {e}");
                    self.node_status = "start failed".to_string();
                }
            },
            _ => self.error = "no node to start".to_string(),
        }
    }

    fn stop_node(&mut self) {
        match (&self.rt, &mut self.node) {
            (Some(rt), Some(node)) => {
                if let Err(e) = rt.block_on(node.stop()) {
                    self.error = format!("node stop failed: {e}");
                }
            }
            _ => {}
        }
        self.node_running = false;
        self.node_status = "stopped".to_string();
    }

    /// Parse a JSON action, reduce it, and return the new state as JSON. A bad
    /// action string never crashes the runtime — it is captured into `error`.
    pub fn dispatch_json(&mut self, action_json: &str) -> String {
        match serde_json::from_str::<NativeAppAction>(action_json) {
            Ok(action) => self.dispatch(action),
            Err(e) => {
                self.error = format!("invalid action JSON: {e}");
                self.rev += 1;
            }
        }
        self.state_json()
    }

    pub fn state(&self) -> AppState {
        AppState {
            rev: self.rev,
            error: self.error.clone(),
            app_version: self.app_version.clone(),
            identity: self.identity.clone(),
            node: NodeStatus {
                running: self.node_running,
                status_text: self.node_status.clone(),
            },
            ble: BleStatus {
                enabled: self.ble_enabled,
                role: "peripheral+central".to_string(),
                // Placeholder: the real scan state comes from the BLE transport
                // once the Android backend reports it (P1 M4).
                scanning: self.ble_enabled && self.node_running,
                adapter_name: "—".to_string(),
            },
            // Populated from the BLE transport's discovery/pool once the Android
            // backend exists (P1 M4); empty on the host (no BLE backend).
            ble_peers: Vec::<BlePeer>::new(),
        }
    }

    pub fn state_json(&self) -> String {
        serde_json::to_string(&self.state())
            .unwrap_or_else(|e| format!(r#"{{"error":"serialize failed: {e}"}}"#))
    }
}

/// `AppRuntime` is shared across JVM threads behind a `Mutex` (see `jni_abi`),
/// so it must be `Send`. Assert it at compile time on every target — including
/// the host — so a non-`Send` field is caught here, not only in the Android build.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<AppRuntime>();
};

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("myco-test-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn identity_generates_persists_and_is_stable() {
        let dir = temp_dir("identity");
        let _ = std::fs::remove_dir_all(&dir);

        let first = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");
        let s1 = first.state();
        assert!(s1.error.is_empty(), "startup error: {}", s1.error);
        assert!(s1.identity.own_npub.starts_with("npub1"), "npub: {}", s1.identity.own_npub);
        assert_eq!(s1.identity.own_pubkey_hex.len(), 64);
        assert!(s1.identity.fips_addr.ends_with(".fips"));
        assert!(!s1.ble.enabled, "BLE off until SetBleEnabled");
        assert!(s1.ble_peers.is_empty());

        // Second launch on the same dir must reuse the persisted key.
        let second = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");
        assert_eq!(s1.identity.own_npub, second.state().identity.own_npub);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reducer_rev_and_bad_action() {
        let dir = temp_dir("reducer");
        let _ = std::fs::remove_dir_all(&dir);
        let mut rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        let rev0 = rt.state().rev;
        rt.dispatch(NativeAppAction::GetState);
        assert_eq!(rt.state().rev, rev0, "GetState must not bump rev");
        rt.dispatch(NativeAppAction::Tick);
        assert_eq!(rt.state().rev, rev0 + 1, "Tick must bump rev");

        let json = rt.dispatch_json("not json");
        assert!(json.contains("invalid action JSON"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_ble_enabled_toggles_state() {
        let dir = temp_dir("ble");
        let _ = std::fs::remove_dir_all(&dir);
        let mut rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        assert!(!rt.state().ble.enabled);
        rt.dispatch(NativeAppAction::SetBleEnabled { enabled: true });
        assert!(rt.state().ble.enabled, "SetBleEnabled true should flip the switch");
        rt.dispatch(NativeAppAction::SetBleEnabled { enabled: false });
        assert!(!rt.state().ble.enabled);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn node_starts_and_stops_on_host() {
        let dir = temp_dir("node-start");
        let _ = std::fs::remove_dir_all(&dir);
        let mut rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        // Default config has no transports + no TUN, so start() just sets up the
        // node's internal machinery — no network binding. Verifies the embed.
        rt.dispatch(NativeAppAction::StartNode);
        let s = rt.state();
        assert!(s.error.is_empty(), "start error: {}", s.error);
        assert!(s.node.running, "node should be running after StartNode");

        rt.dispatch(NativeAppAction::StopNode);
        assert!(!rt.state().node.running, "node should be stopped after StopNode");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
