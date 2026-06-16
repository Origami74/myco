use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fips::control::read_handle::ControlReadHandle;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::action::NativeAppAction;
use crate::content::{CacheView, Content};
use crate::identity_store;
use crate::state::{AppState, BleAdvert, BlePeer, BleStatus, IdentityView, NodeStatus};

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
    /// App-private data dir, kept so the node can be rebuilt on a BLE off→on
    /// cycle (run_rx_loop consumes the node, so restart needs a fresh one).
    data_dir: String,
    rev: u64,
    error: String,
    identity: IdentityView,
    ble_enabled: bool,
    node_running: bool,
    node_status: String,
    /// Tokio runtime hosting the node's tasks. `None` only if it failed to build.
    rt: Option<Runtime>,
    /// The embedded fips node, held until `StartNode` moves it into the loop task.
    node: Option<fips::Node>,
    /// Lock-free read view of the running node's peer state (cloned out of the
    /// node before it moves into the loop task; safe to read while the loop runs).
    read_handle: Option<ControlReadHandle>,
    /// The background task running `node.start()` + `run_rx_loop()`. Aborting it
    /// drops the node and stops its transports.
    loop_task: Option<JoinHandle<()>>,
    /// The content layer (embedded relay + Blossom + gateway + Library). `None`
    /// only on a startup error (no valid data dir).
    content: Option<Arc<Content>>,
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
        std::fs::create_dir_all(Path::new(data_dir))?;

        // Multi-thread runtime so the node's spawned tasks self-drive between
        // FFI polls (see the struct doc).
        let rt = Runtime::new().map_err(|e| anyhow::anyhow!("tokio runtime: {e}"))?;

        let node = Self::build_node(data_dir)?;
        let identity = IdentityView::from_identity(node.identity());

        // The content layer (relay + Blossom + gateway + Library) lives for the
        // whole process; it is independent of the node's start/stop lifecycle.
        let content = Arc::new(Content::open(Path::new(data_dir))?);

        // Install the IP online-fallback pull source so a pasted nsite link can
        // be fetched over normal internet (the P2 content-entry path). Gated by
        // `sync.offline_only` (a P3 setting); on by default in P2.
        content.set_source(Arc::new(crate::ip_source::IpPeerSource::with_defaults()));

        // Re-list Library ("installed") sites as ready/incomplete by checking the
        // persisted stores — the relay + Blossom survive a restart, the in-memory
        // status map does not.
        rt.spawn(content.clone().refresh_library_status());

        Ok(Self {
            app_version: app_version.to_string(),
            data_dir: data_dir.to_string(),
            rev: 0,
            error: String::new(),
            identity,
            ble_enabled: false,
            node_running: false,
            node_status: "fips node constructed (not started)".to_string(),
            rt: Some(rt),
            node: Some(node),
            read_handle: None,
            loop_task: None,
            content: Some(content),
        })
    }

    /// Build a fresh embedded fips node from the persisted identity. Called at
    /// construction and again on a BLE off→on cycle (run_rx_loop consumes the
    /// node, so re-enabling needs a new one).
    fn build_node(data_dir: &str) -> anyhow::Result<fips::Node> {
        let nsec = identity_store::load_or_generate(Path::new(data_dir))?;
        let mut config = fips::Config::new();
        config.node.identity.nsec = Some(nsec);
        config.node.identity.persistent = true;
        config.tun.enabled = false;
        // No control socket: peer state is read via the in-process control read
        // handle (peer_views), not the Unix socket. The tick publishes the
        // snapshot regardless of this flag.
        config.node.control.enabled = false;
        // On Android, configure a BLE transport instance so node.start() brings up
        // the AndroidIo backend (the Kotlin radio drives it via the injected
        // bridge). Host builds have no BLE backend, so this is Android-only.
        #[cfg(target_os = "android")]
        {
            config.transports.ble = fips::config::TransportInstances::Single(fips::config::BleConfig {
                auto_connect: Some(true),
                ..Default::default()
            });
        }
        fips::Node::new(config).map_err(|e| anyhow::anyhow!("fips Node::new failed: {e}"))
    }

    fn from_error(app_version: &str, msg: &str) -> Self {
        Self {
            app_version: app_version.to_string(),
            data_dir: String::new(),
            rev: 0,
            error: msg.to_string(),
            identity: IdentityView::default(),
            ble_enabled: false,
            node_running: false,
            node_status: "error".to_string(),
            rt: None,
            node: None,
            read_handle: None,
            loop_task: None,
            content: None,
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
            NativeAppAction::OpenNsite { link } => {
                self.open_nsite(&link);
                self.rev += 1;
            }
            NativeAppAction::ImportNsite { dir } => {
                self.import_nsite(&dir);
                self.rev += 1;
            }
            NativeAppAction::AddToLibrary { link } => {
                if let (Some(content), Some(addr)) =
                    (&self.content, nsite_deck::parse_link(&link))
                {
                    content.add_to_library(&addr, None, now_secs());
                }
                self.rev += 1;
            }
            NativeAppAction::RemoveFromLibrary { link } => {
                if let (Some(content), Some(addr)) =
                    (&self.content, nsite_deck::parse_link(&link))
                {
                    content.remove_from_library(&addr);
                }
                self.rev += 1;
            }
            NativeAppAction::SearchNsites { .. } => {
                // Discovery needs reachable peer relays — a P3 stub for now.
                self.rev += 1;
            }
            NativeAppAction::WipeStores => {
                self.wipe_stores();
                self.rev += 1;
            }
        }
    }

    /// Spawn a sync-to-readiness for a pasted link (spawn-not-block; readiness is
    /// observed via `siteStatus` on `Tick`).
    fn open_nsite(&mut self, link: &str) {
        let Some(addr) = nsite_deck::parse_link(link) else {
            self.error = format!("unrecognized nsite link: {link}");
            return;
        };
        let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) else {
            return;
        };
        rt.spawn(content.open_site(addr));
    }

    /// Spawn a dev side-load of a bundle directory.
    fn import_nsite(&mut self, dir: &str) {
        let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) else {
            return;
        };
        let dir = dir.to_string();
        rt.spawn(async move {
            match content.import_dir(Path::new(&dir)).await {
                Ok(outcome) => tracing::info!(?outcome, dir, "imported nsite bundle"),
                Err(e) => tracing::error!(error = %e, dir, "import nsite failed"),
            }
        });
    }

    /// Clear local content. Blocks (it is fast: clear maps + remove files) so the
    /// next `state()` reflects the empty stores immediately.
    fn wipe_stores(&mut self) {
        let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) else {
            return;
        };
        if let Err(e) = rt.block_on(content.wipe()) {
            self.error = format!("wipe failed: {e}");
        }
    }

    /// The content layer + a Tokio handle, for the out-of-band `gatewayGet` JNI
    /// path (cloned out so the gateway serves without holding the runtime mutex).
    pub fn gateway_context(&self) -> Option<(Arc<Content>, tokio::runtime::Handle)> {
        let content = self.content.clone()?;
        let handle = self.rt.as_ref()?.handle().clone();
        Some((content, handle))
    }

    fn start_node(&mut self) {
        if self.node_running {
            return;
        }
        // Rebuild the node if a prior stop consumed it (BLE toggled off then on).
        if self.node.is_none() {
            match Self::build_node(&self.data_dir) {
                Ok(n) => self.node = Some(n),
                Err(e) => {
                    self.error = format!("rebuild node: {e}");
                    return;
                }
            }
        }
        let node = self.node.take().expect("node present after rebuild");
        let rt = match self.rt.as_ref() {
            Some(rt) => rt,
            None => {
                self.error = "no runtime".to_string();
                return;
            }
        };
        // Clone the lock-free read handle out before the node moves into the loop
        // task — peer state is then readable while run_rx_loop owns the node.
        let handle = node.control_read_handle();
        let task = rt.spawn(async move {
            let mut node = node;
            if let Err(e) = node.start().await {
                tracing::error!("fips node start failed: {e}");
                return;
            }
            // Runs until the packet channel closes or the task is aborted.
            if let Err(e) = node.run_rx_loop().await {
                tracing::warn!("fips rx loop ended: {e}");
            }
        });
        self.read_handle = Some(handle);
        self.loop_task = Some(task);
        self.node_running = true;
        self.node_status = "running".to_string();
    }

    fn stop_node(&mut self) {
        // Aborting the loop task drops the node, stopping its transports.
        if let Some(task) = self.loop_task.take() {
            task.abort();
        }
        self.read_handle = None;
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
        // Live peers, read lock-free from the node's tick-published snapshot
        // (empty until the loop runs / peers are seen). On Android every peer is
        // a BLE peer (the only transport configured).
        let ble_peers: Vec<BlePeer> = self
            .read_handle
            .as_ref()
            .map(|h| {
                h.peer_views()
                    .into_iter()
                    .map(|p| BlePeer {
                        node_addr_hex: p.node_addr_hex,
                        npub: p.npub,
                        connected: p.connected,
                        psm: 0,    // not surfaced in the snapshot yet
                        rssi: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

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
                scanning: self.ble_enabled && self.node_running,
                adapter_name: if self.node_running { "ble0".to_string() } else { "—".to_string() },
            },
            ble_peers,
            ble_adverts: self.ble_adverts(),
            sites: self
                .content
                .as_ref()
                .map(|c| c.sites_snapshot())
                .unwrap_or_default(),
            library: self
                .content
                .as_ref()
                .map(|c| c.library_snapshot())
                .unwrap_or_default(),
            cache: self
                .content
                .as_ref()
                .map(|c| c.cache_view())
                .unwrap_or_else(CacheView::empty),
        }
    }

    /// Raw scan adverts (address / PSM / RSSI) from the BLE radio bridge. The
    /// radio is Android-only; on the host there is no bridge.
    #[cfg(target_os = "android")]
    fn ble_adverts(&self) -> Vec<BleAdvert> {
        fips::transport::ble::android_io::android_ble_bridge()
            .map(|b| {
                b.advert_views()
                    .into_iter()
                    .map(|a| BleAdvert { addr: a.addr, psm: a.psm, rssi: a.rssi })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(not(target_os = "android"))]
    fn ble_adverts(&self) -> Vec<BleAdvert> {
        Vec::new()
    }

    pub fn state_json(&self) -> String {
        serde_json::to_string(&self.state())
            .unwrap_or_else(|e| format!(r#"{{"error":"serialize failed: {e}"}}"#))
    }
}

/// Seconds since the Unix epoch (Library `added_at` timestamps).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
