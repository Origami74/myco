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
    /// Circle peers we've already pulled chat backlog from this connection, so we
    /// pull once per (re)connect rather than every state poll.
    pulled_chat_from: std::sync::Mutex<std::collections::HashSet<String>>,
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
        let mut identity = IdentityView::from_identity(node.identity());
        // FIPS's effective IPv6 MTU (transport_mtu - 77). The VpnService sets this
        // on the TUN and the MSS clamp derives from it, so packets fit the mesh.
        identity.fips_mtu = node.effective_ipv6_mtu();

        // The content layer (relay + Blossom + gateway + Library) lives for the
        // whole process; it is independent of the node's start/stop lifecycle.
        let content = Arc::new(Content::open(Path::new(data_dir))?);

        // The device keypair (same nsec the node uses) is the pairing identity —
        // pair request/accept events are signed with it.
        if let Ok(nsec) = identity_store::load_or_generate(Path::new(data_dir)) {
            content.set_device_keys(&nsec);
        }

        // Install the IP online-fallback pull source so a pasted nsite link can
        // be fetched over normal internet (the P2 content-entry path). Gated by
        // `sync.offline_only` (a P3 setting); on by default in P2.
        content.set_source(Arc::new(crate::ip_source::IpPeerSource::with_defaults()));

        // Re-list Library ("installed") sites as ready/incomplete by checking the
        // persisted stores — the relay + Blossom survive a restart, the in-memory
        // status map does not.
        rt.spawn(content.clone().refresh_library_status());

        // First-run default apps: install the bundled myco-bitchat nsite so a
        // fresh device shows it in Apps without pasting a link. A one-shot marker
        // file makes this idempotent and lets a user who removes it stay removed.
        seed_default_sites(&content, &rt, Path::new(data_dir));

        // Serve the relay + Blossom over the mesh so paired peers can pull this
        // device's nsites at ws://<npub>.fips:4869 / http://<npub>.fips:24242.
        // Bound IPV6_V6ONLY (the mesh is IPv6-only) so `[::]:port` doesn't collide
        // with another app squatting on `127.0.0.1:port`; a port already in use
        // surfaces as a warning. Android-only (the host has no TUN). ports.md.
        #[allow(unused_mut)]
        let mut mesh_warning = String::new();
        #[cfg(target_os = "android")]
        {
            use std::net::SocketAddr;
            let _guard = rt.enter(); // runtime context for TcpListener::from_std
            let blobs = content.blobs();

            // One shared relay hub backs both the mesh socket and a loopback socket,
            // so a chat event a peer pushes over `.fips` reaches the in-app nsite's
            // live subscription on localhost (shared store + live bus + gossiper).
            // The gossiper fans this device's own nsite events out to Circle peers
            // (docs/design/event-gossip.md).
            let gossiper: Arc<dyn myco_relay::server::Gossiper> =
                Arc::new(crate::gossip::MeshGossiper::new(content.clone()));
            // Restrict mesh access to paired (Circle) peers — only the pairing
            // handshake is open, so strangers can request to pair but can't read or
            // push content. Loopback (the in-app WebView) always bypasses the gate.
            let gate: Arc<dyn myco_relay::server::PeerGate> =
                Arc::new(crate::content::CircleGate::new(content.clone()));
            let hub = myco_relay::server::RelayHub::with_gate(
                content.relay(),
                Some(gossiper),
                Some(gate),
            );

            // Mesh socket: IPV6_V6ONLY `[::]:4869` so it doesn't collide with the
            // loopback bind and is reachable by peers at `ws://<npub>.fips:4869`.
            match myco_relay::server::bind("[::]:4869".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    let hub = hub.clone();
                    rt.spawn(async move {
                        if let Err(e) = myco_relay::server::serve_on_hub(hub, listener).await {
                            tracing::error!(error = %e, "mesh relay server exited");
                        }
                    });
                }
                Err(e) => {
                    mesh_warning =
                        format!("relay port 4869 unavailable (another app using it?): {e}");
                }
            }
            // Loopback socket: the in-app nsite WebView talks to `ws://localhost:4869`
            // / `ws://127.0.0.1:4869`; the mesh socket is v6only, so serve loopback
            // explicitly. Connections here are classified as `Origin::Local`.
            match myco_relay::server::bind("127.0.0.1:4869".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    let hub = hub.clone();
                    rt.spawn(async move {
                        if let Err(e) = myco_relay::server::serve_on_hub(hub, listener).await {
                            tracing::error!(error = %e, "loopback relay server exited");
                        }
                    });
                }
                Err(e) => {
                    // Critical: the in-app nsites connect to ws://localhost:4869, so
                    // if another app holds it they'll silently talk to the WRONG
                    // relay (you'd see messages that aren't yours). Flag it loudly;
                    // the UI watches for "port 4869" to pop a warning.
                    if !mesh_warning.is_empty() {
                        mesh_warning.push_str("; ");
                    }
                    mesh_warning.push_str(&format!(
                        "Another app is using port 4869 — Myco's relay couldn't start, \
                         so apps will talk to the wrong relay. Close the other app and \
                         restart Myco. ({e})"
                    ));
                }
            }
            match myco_blossom::server::bind("[::]:24242".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    // Same paired-only gate for blobs: a mesh source must be a
                    // current Circle member (loopback bypasses). Pairing never
                    // touches Blossom, so there's no handshake exception here.
                    let content_for_blob = content.clone();
                    let access: myco_blossom::server::AccessFn =
                        Arc::new(move |ip| content_for_blob.is_paired_ip(ip));
                    rt.spawn(async move {
                        if let Err(e) =
                            myco_blossom::server::serve_on_guarded(blobs, listener, access).await
                        {
                            tracing::error!(error = %e, "mesh blossom server exited");
                        }
                    });
                }
                Err(e) => {
                    if !mesh_warning.is_empty() {
                        mesh_warning.push_str("; ");
                    }
                    mesh_warning.push_str(&format!("blossom port 24242 unavailable: {e}"));
                }
            }
        }

        Ok(Self {
            app_version: app_version.to_string(),
            data_dir: data_dir.to_string(),
            rev: 0,
            error: mesh_warning,
            identity,
            ble_enabled: false,
            node_running: false,
            node_status: "fips node constructed (not started)".to_string(),
            rt: Some(rt),
            node: Some(node),
            read_handle: None,
            loop_task: None,
            content: Some(content),
            pulled_chat_from: std::sync::Mutex::new(std::collections::HashSet::new()),
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
            config.transports.ble =
                fips::config::TransportInstances::Single(fips::config::BleConfig {
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
            pulled_chat_from: std::sync::Mutex::new(std::collections::HashSet::new()),
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
            NativeAppAction::OpenNsite { link, holder } => {
                self.open_nsite(&link, holder);
                self.rev += 1;
            }
            NativeAppAction::ImportNsite { dir } => {
                self.import_nsite(&dir);
                self.rev += 1;
            }
            NativeAppAction::AddToLibrary { link } => {
                if let (Some(content), Some(addr)) = (&self.content, nsite_deck::parse_link(&link))
                {
                    content.add_to_library(&addr, None, now_secs());
                }
                self.rev += 1;
            }
            NativeAppAction::RemoveFromLibrary { link } => {
                if let (Some(content), Some(addr)) = (&self.content, nsite_deck::parse_link(&link))
                {
                    content.remove_from_library(&addr);
                }
                self.rev += 1;
            }
            NativeAppAction::ForgetNsite { link } => {
                if let (Some(content), Some(addr)) = (&self.content, nsite_deck::parse_link(&link))
                {
                    content.forget_site(&addr);
                }
                self.rev += 1;
            }
            NativeAppAction::CheckNsiteUpdates => {
                // Poll online relays for newer manifests; stage + apply. Non-blocking.
                if let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) {
                    rt.spawn(content.check_updates());
                }
                self.rev += 1;
            }
            NativeAppAction::SearchNsites { .. } => {
                // "nsites around me": query connected Circle peers' mesh relays for
                // their manifests. Spawn-not-block; results land in `discovered`.
                if let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) {
                    rt.spawn(content.discover_from_circle());
                }
                self.rev += 1;
            }
            NativeAppAction::WipeStores => {
                self.wipe_stores();
                self.rev += 1;
            }
            NativeAppAction::WipeCache => {
                self.wipe_cache();
                self.rev += 1;
            }
            NativeAppAction::AddToCircle { npub, name } => {
                if let Some(content) = &self.content {
                    content.add_to_circle(&npub, &name);
                }
                self.rev += 1;
            }
            NativeAppAction::RemoveFromCircle { npub } => {
                if let Some(content) = &self.content {
                    content.remove_from_circle(&npub);
                }
                // Best-effort: tell the peer so they drop us too (if reachable).
                if let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) {
                    rt.spawn(async move { content.send_unpair(&npub).await });
                }
                self.rev += 1;
            }
            NativeAppAction::SendPairRequest { npub, secret, .. } => {
                if let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) {
                    rt.spawn(async move { content.send_pair_request(&npub, &secret).await });
                }
                self.rev += 1;
            }
            NativeAppAction::AcceptPairRequest { npub, name } => {
                if let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) {
                    rt.spawn(async move { content.accept_pair_request(&npub, &name).await });
                }
                self.rev += 1;
            }
            NativeAppAction::DeclinePairRequest { npub } => {
                if let Some(content) = &self.content {
                    content.decline_pair_request(&npub);
                }
                self.rev += 1;
            }
            NativeAppAction::SetOfflineOnly { enabled } => {
                if let Some(content) = &self.content {
                    content.set_offline_only(enabled);
                }
                self.rev += 1;
            }
            NativeAppAction::SetDeviceName { name } => {
                if let Some(content) = &self.content {
                    content.set_device_name(&name);
                }
                self.rev += 1;
            }
        }
    }

    /// Spawn a sync-to-readiness for a pasted link / shared site (spawn-not-block;
    /// readiness is observed via `siteStatus` on `Tick`). `holder` is the mesh
    /// peer to pull from first, if this came from a share QR.
    fn open_nsite(&mut self, link: &str, holder: Option<String>) {
        let Some(addr) = nsite_deck::parse_link(link) else {
            self.error = format!("unrecognized nsite link: {link}");
            return;
        };
        let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) else {
            return;
        };
        rt.spawn(content.open_site(addr, holder));
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

    /// Clear cached content but preserve pinned nsites (the "delete cache" half of
    /// Settings → Storage). Blocks like `wipe_stores` so the next `state()` reflects
    /// the reclaimed space immediately.
    fn wipe_cache(&mut self) {
        let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) else {
            return;
        };
        if let Err(e) = rt.block_on(content.wipe_cache()) {
            self.error = format!("cache wipe failed: {e}");
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
        // `mut` is used only on Android (enable_app_owned_tun); allow on the host.
        #[allow(unused_mut)]
        let mut node = self.node.take().expect("node present after rebuild");
        let rt = match self.rt.as_ref() {
            Some(rt) => rt,
            None => {
                self.error = "no runtime".to_string();
                return;
            }
        };
        // Enable the app-owned TUN before the node moves into the loop task: the
        // Android VpnService owns the fd, so FIPS exchanges IPv6 packet bytes over
        // channels (and skips system-TUN creation). The JNI packet bridge pumps
        // these channels. Android-only (the host has no VpnService).
        #[cfg(target_os = "android")]
        {
            // MSS ceiling from FIPS's effective IPv6 MTU (transport_mtu-77) minus
            // the IPv6+TCP headers — same as the system-TUN path's max_mss.
            let max_mss = node.effective_ipv6_mtu().saturating_sub(60);
            let (tun_outbound_tx, tun_inbound_rx) = node.enable_app_owned_tun();
            crate::tun_bridge::install(tun_outbound_tx, tun_inbound_rx, max_mss);
        }
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
                        psm: 0, // not surfaced in the snapshot yet
                        rssi: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Feed the connected-peer npubs to the content layer so `open_site` can
        // pull from currently-reachable Circle members (and skip offline ones).
        if let Some(content) = self.content.as_ref() {
            let connected: Vec<String> = ble_peers
                .iter()
                .filter(|p| p.connected && !p.npub.is_empty())
                .map(|p| p.npub.clone())
                .collect();
            content.set_connected_peers(connected);

            // Pull chat backlog once per newly-connected Circle peer (the read
            // counterpart of the push flood) so a freshly-opened nsite has history.
            if let Some(rt) = self.rt.as_ref() {
                let conn: std::collections::HashSet<String> =
                    content.connected_circle_npubs().into_iter().collect();
                let mut pulled = self.pulled_chat_from.lock().unwrap();
                for npub in &conn {
                    if pulled.insert(npub.clone()) {
                        // First poll where this Circle peer is reachable: pull chat
                        // backlog (the read counterpart of the push flood).
                        let content = content.clone();
                        let npub = npub.clone();
                        rt.spawn(async move { content.pull_recent_chat(&npub).await });
                    }
                }
                pulled.retain(|n| conn.contains(n)); // re-pull on reconnect
                drop(pulled);

                // Retry not-ready downloads while any Circle peer is reachable.
                // open_site(_, None) tries every connected peer, and
                // `retriable_library_addrs` skips sites already syncing — so this
                // re-tries about once per attempt-duration (not every poll), and
                // keeps trying as a flaky just-connected session settles, instead of
                // firing once on the connect edge and going quiet.
                if !conn.is_empty() {
                    for addr in content.retriable_library_addrs() {
                        let content = content.clone();
                        rt.spawn(async move { content.open_site(addr, None).await });
                    }
                }
            }
        }

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
                adapter_name: if self.node_running {
                    "ble0".to_string()
                } else {
                    "—".to_string()
                },
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
            circle: self
                .content
                .as_ref()
                .map(|c| c.circle_snapshot())
                .unwrap_or_default(),
            pending_pair_requests: self
                .content
                .as_ref()
                .map(|c| c.pending_pairs_snapshot())
                .unwrap_or_default(),
            discovered: self
                .content
                .as_ref()
                .map(|c| c.discovered_snapshot())
                .unwrap_or_default(),
            offline_only: self
                .content
                .as_ref()
                .map(|c| c.is_offline_only())
                .unwrap_or(false),
            update_check: self
                .content
                .as_ref()
                .map(|c| c.update_check_snapshot())
                .unwrap_or_default(),
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
                    .map(|a| BleAdvert {
                        addr: a.addr,
                        psm: a.psm,
                        rssi: a.rssi,
                    })
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

/// nsites installed by default on first run (the bundled myco-bitchat app).
const DEFAULT_SITES: &[&str] =
    &["4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivbitchat.nsite.lol"];

/// Pin + start a download for the default apps, once per install. The marker
/// file in `data_dir` keeps this idempotent and lets a user who removes a seeded
/// app stay rid of it (we never re-seed). Pinning happens immediately so the app
/// lists in Apps even before its blobs land (offline first run); the spawned
/// `open_site` fetches them, and re-attempts when the user taps the app.
fn seed_default_sites(content: &Arc<Content>, rt: &Runtime, data_dir: &Path) {
    let marker = data_dir.join("seeded-defaults");
    if marker.exists() {
        return;
    }
    for link in DEFAULT_SITES {
        let Some(addr) = nsite_deck::parse_link(link) else {
            tracing::warn!(link, "default site link did not parse; skipping seed");
            continue;
        };
        content.add_to_library(&addr, None, now_secs());
        rt.spawn(content.clone().open_site(addr, None));
    }
    if let Err(e) = std::fs::write(&marker, b"1\n") {
        tracing::warn!(error = %e, "could not write default-seed marker");
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
        assert!(
            s1.identity.own_npub.starts_with("npub1"),
            "npub: {}",
            s1.identity.own_npub
        );
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
        assert!(
            rt.state().ble.enabled,
            "SetBleEnabled true should flip the switch"
        );
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
        assert!(
            !rt.state().node.running,
            "node should be stopped after StopNode"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
