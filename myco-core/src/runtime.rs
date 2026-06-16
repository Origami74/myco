use std::path::Path;

use crate::action::NativeAppAction;
use crate::identity_store;
use crate::state::{AppState, IdentityView, NodeStatus};

/// The app runtime behind the FFI. Holds the device identity and the reducer
/// state. A `Mutex<AppRuntime>` is what the opaque JNI handle wraps (see
/// `jni_abi`); on the host it is driven directly.
pub struct AppRuntime {
    app_version: String,
    rev: u64,
    error: String,
    identity: IdentityView,
    node_running: bool,
    node_status: String,
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

        // Identity: generate-or-load and persist the nsec.
        let nsec = identity_store::load_or_generate(dir)?;
        let identity = fips::Identity::from_secret_str(&nsec)
            .map_err(|e| anyhow::anyhow!("invalid persisted identity: {e}"))?;

        // Embed FIPS: construct a node from the same key to validate the embed
        // path end-to-end. P0 does not start transports (no TUN, no BLE) — that
        // is P1. The node is dropped here; it is retained and started once
        // StartNode is wired. See docs/roadmap.md (P0 -> P1).
        let mut config = fips::Config::new();
        config.node.identity.nsec = Some(nsec.clone());
        config.node.identity.persistent = true;
        config.tun.enabled = false;
        let node = fips::Node::new(config)
            .map_err(|e| anyhow::anyhow!("fips Node::new failed: {e}"))?;
        // Sanity: the node's npub must match the identity we persisted.
        debug_assert_eq!(node.npub(), identity.npub());
        drop(node);

        Ok(Self {
            app_version: app_version.to_string(),
            rev: 0,
            error: String::new(),
            identity: IdentityView::from_identity(&identity),
            node_running: false,
            node_status: "fips node constructed (transports not started)".to_string(),
        })
    }

    fn from_error(app_version: &str, msg: &str) -> Self {
        Self {
            app_version: app_version.to_string(),
            rev: 0,
            error: msg.to_string(),
            identity: IdentityView::default(),
            node_running: false,
            node_status: "error".to_string(),
        }
    }

    /// Reduce one action, mutating state and bumping `rev` for mutations.
    pub fn dispatch(&mut self, action: NativeAppAction) {
        match action {
            NativeAppAction::GetState => {} // pure read, no rev bump
            NativeAppAction::Tick => self.rev += 1,
            NativeAppAction::StartNode => {
                // TODO(P1): own the fips::Node + a tokio runtime and call
                // node.start().await here.
                self.node_status = "start not yet wired (P1)".to_string();
                self.rev += 1;
            }
            NativeAppAction::StopNode => {
                self.node_running = false;
                self.node_status = "stopped".to_string();
                self.rev += 1;
            }
        }
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
        }
    }

    pub fn state_json(&self) -> String {
        serde_json::to_string(&self.state())
            .unwrap_or_else(|e| format!(r#"{{"error":"serialize failed: {e}"}}"#))
    }
}

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
}
