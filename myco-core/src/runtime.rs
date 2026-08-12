use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::action::NativeAppAction;
use crate::content::{CacheView, Content};
use crate::control_client::PeerView;
use crate::identity_store;
use crate::state::{
    AppState, BleAdvert, BlePeer, BleStatus, IdentityView, NodeStatus, WifiAwareStatus,
};

/// UDP port for the Wi-Fi Aware bulk lane. Both peers bind it on the NDP
/// interface and exchange over it — symmetric, no listener/dialer roles. A
/// fixed app constant (we bind our own port), so there is no PSM-style
/// discovery problem. UDP is fips's native transport and the LAN-discovery
/// path (which this reuses) is already UDP + scoped link-local IPv6.
/// See docs/design/wifi-aware-interop.md.
const WIFI_AWARE_PORT: u16 = 4871;

/// Consecutive failed `show_peers` queries before the peer feed is reported as
/// broken in [`AppState::error`].
///
/// A failure on the first tick after `StartNode` is normal: fips binds the
/// control socket *inside* `run_rx_loop`, which only begins after
/// `node.start()` completes, so there is a startup window where the node is up
/// and the socket is not yet accepting. Three ticks is ~24s — long past that
/// window, and short enough to be visible while the fault is still on screen.
///
/// Shouting matters because the failure is otherwise invisible at every layer
/// at once: the Dev tab's peer rows come from this same source so they read as
/// "no peers nearby", the relay-reachability rows come from the relay pool and
/// keep populating, and the radios' own diagnostics are Myco-owned and keep
/// reporting "discovering". A bind failure in fips only warns and lets its task
/// exit; the node keeps running.
const PEER_FEED_FAILURES_BEFORE_ERROR: u32 = 3;

/// How the control-socket peer feed is doing. Written by the 8s tick (a
/// detached task with no `&mut self`), read synchronously by `state()`.
#[derive(Clone, Debug, Default)]
struct PeerFeedHealth {
    /// Failed queries since the last success. Reset to zero on any success.
    consecutive_failures: u32,
    /// The most recent failure's reason, for the error banner.
    last_error: String,
}

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
    wifi_aware_enabled: bool,
    node_running: bool,
    node_status: String,
    /// Tokio runtime hosting the node's tasks. `None` only if it failed to build.
    rt: Option<Runtime>,
    /// The embedded fips node, held until `StartNode` moves it into the loop task.
    node: Option<fips::Node>,
    /// Whether the node's loop task is live, shared with the detached 8s tick so
    /// it does not query a control socket that cannot exist yet. Mirrors
    /// `node_running`, which the tick has no `&self` to read.
    node_live: Arc<AtomicBool>,
    /// The background task running `node.start()` + `run_rx_loop()`. Aborting it
    /// drops the node and stops its transports.
    loop_task: Option<JoinHandle<()>>,
    /// The content layer (embedded relay + Blossom + gateway + Library). `None`
    /// only on a startup error (no valid data dir).
    content: Option<Arc<Content>>,
    /// Latest dev-menu peer speedtest result; written by the spawned run task and
    /// read back into `state()`. Shared so the async task can update it in place.
    speedtest: Arc<std::sync::Mutex<crate::state::SpeedtestView>>,
    /// Last peer snapshot the 8s tick pulled off the control socket.
    ///
    /// `state()` runs on the FFI thread holding the reducer mutex and must
    /// never block, so it reads this cache rather than querying. That is a real
    /// change from the lock-free `peer_views()` read it replaces: the Dev tab's
    /// peer rows are now up to 8s stale.
    peer_cache: Arc<std::sync::Mutex<Vec<PeerView>>>,
    /// Whether the peer feed is working, so an unbound control socket surfaces
    /// as an error instead of an empty room. See
    /// [`PEER_FEED_FAILURES_BEFORE_ERROR`].
    peer_feed: Arc<std::sync::Mutex<PeerFeedHealth>>,
    /// Crash-surviving history for the BLE attempt log (D-13). Shared so the
    /// rate-limited flush can be spawned onto the tokio runtime rather than
    /// running on the FFI thread. `None` only on a startup error, in which case
    /// attempts simply have no persistence — never an `AppState.error`.
    attempt_store: Option<Arc<crate::attempt_store::AttemptStore>>,
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

        let node = Self::build_node(data_dir, false)?;
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

        // Peer state now comes off the node's control socket, so the tick needs
        // somewhere to publish it and somewhere to record whether the feed
        // works at all. Both are read synchronously by `state()`.
        let peer_cache: Arc<std::sync::Mutex<Vec<PeerView>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let peer_feed: Arc<std::sync::Mutex<PeerFeedHealth>> =
            Arc::new(std::sync::Mutex::new(PeerFeedHealth::default()));
        let node_live = Arc::new(AtomicBool::new(false));

        // Serve the relay + Blossom over the mesh so paired peers can pull this
        // device's nsites at ws://<npub>.fips:4870 / http://<npub>.fips:24243.
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

            // Mesh socket: IPV6_V6ONLY `[::]:4870` so it doesn't collide with the
            // loopback bind and is reachable by peers at `ws://<npub>.fips:4870`.
            match myco_relay::server::bind("[::]:4870".parse::<SocketAddr>().unwrap()) {
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
                        format!("relay port 4870 unavailable (another app using it?): {e}");
                }
            }
            // Loopback socket: the in-app nsite WebView talks to `ws://localhost:4870`
            // / `ws://127.0.0.1:4870`; the mesh socket is v6only, so serve loopback
            // explicitly. Connections here are classified as `Origin::Local`.
            match myco_relay::server::bind("127.0.0.1:4870".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    let hub = hub.clone();
                    rt.spawn(async move {
                        if let Err(e) = myco_relay::server::serve_on_hub(hub, listener).await {
                            tracing::error!(error = %e, "loopback relay server exited");
                        }
                    });
                }
                Err(e) => {
                    // Critical: the in-app nsites connect to ws://localhost:4870, so
                    // if another app holds it they'll silently talk to the WRONG
                    // relay (you'd see messages that aren't yours). Flag it loudly;
                    // the UI watches for "port 4870" to pop a warning.
                    if !mesh_warning.is_empty() {
                        mesh_warning.push_str("; ");
                    }
                    mesh_warning.push_str(&format!(
                        "Another app is using port 4870 — Myco's relay couldn't start, \
                         so apps will talk to the wrong relay. Close the other app and \
                         restart Myco. ({e})"
                    ));
                }
            }
            match myco_blossom::server::bind("[::]:24243".parse::<SocketAddr>().unwrap()) {
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
                    mesh_warning.push_str(&format!("blossom port 24243 unavailable: {e}"));
                }
            }

            // Keepwarm: proactively hold a live relay connection to every Circle
            // member (respawn a dropped one promptly, not lazily on the next send)
            // and resubscribe on each peer's reconnect edge. This is what restores a
            // Circle relay link *mutually and fast* after a mid-chain node flaps —
            // independent of chat traffic and of where the peer sits in the mesh.
            //
            // The tick also feeds the node's connected-peer view into the content
            // layer and drives not-ready-site retries. state() does the same at
            // 1Hz for foreground snappiness, but its poll pauses when the app is
            // backgrounded — this loop is what keeps peer-driven relay sync alive
            // then.
            let control = crate::control_client::ControlClient::new(
                crate::control_client::socket_path(data_dir),
            );

            // Platform-discovered peers (Wi-Fi Aware, the AP lane) reach the
            // node over the same socket. The Kotlin radios push into a bounded
            // queue from their own callback threads; this task owns the client
            // and issues `connect`. Spawned once for the process, so it spans
            // node rebuilds and the window before the first StartNode.
            crate::platform_peers::spawn_drainer(&rt, control.clone(), node_live.clone());

            {
                let content = content.clone();
                let peer_cache = peer_cache.clone();
                let peer_feed = peer_feed.clone();
                let node_live = node_live.clone();
                rt.spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_secs(8));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        tick.tick().await;
                        // Nothing binds the socket until the node's rx loop is
                        // up; querying before then would only manufacture
                        // failures for the health counter to shout about.
                        if node_live.load(Ordering::Relaxed) {
                            match control.show_peers().await {
                                Ok(peers) => {
                                    let connected: Vec<String> = peers
                                        .iter()
                                        .filter(|p| p.connected && !p.npub.is_empty())
                                        .map(|p| p.npub.clone())
                                        .collect();
                                    *peer_cache.lock().unwrap() = peers;
                                    {
                                        let mut health = peer_feed.lock().unwrap();
                                        health.consecutive_failures = 0;
                                        health.last_error.clear();
                                    }
                                    content.set_connected_peers(connected);
                                    if !content.circle_npubs().is_empty() {
                                        for addr in content.retriable_library_addrs() {
                                            let content = content.clone();
                                            tokio::spawn(async move {
                                                content.open_site(addr, None).await
                                            });
                                        }
                                    }
                                }
                                Err(e) => {
                                    // The last snapshot is deliberately kept:
                                    // stale peer rows plus a visible error beat
                                    // an empty list that reads as a quiet room.
                                    let mut health = peer_feed.lock().unwrap();
                                    health.consecutive_failures =
                                        health.consecutive_failures.saturating_add(1);
                                    health.last_error = e.clone();
                                    let n = health.consecutive_failures;
                                    drop(health);
                                    tracing::warn!(
                                        error = %e,
                                        consecutive_failures = n,
                                        "peer state query failed"
                                    );
                                }
                            }
                        }
                        content.keepwarm_tick();
                    }
                });
            }
        }

        Ok(Self {
            app_version: app_version.to_string(),
            data_dir: data_dir.to_string(),
            rev: 0,
            error: mesh_warning,
            identity,
            ble_enabled: false,
            wifi_aware_enabled: false,
            node_running: false,
            node_status: "fips node constructed (not started)".to_string(),
            rt: Some(rt),
            node: Some(node),
            node_live,
            loop_task: None,
            content: Some(content),
            speedtest: Arc::new(std::sync::Mutex::new(crate::state::SpeedtestView::default())),
            peer_cache,
            peer_feed,
            // Load once here; a missing file is an empty history, and an
            // unreadable one degrades to empty rather than failing the launch.
            attempt_store: Some(Arc::new(crate::attempt_store::AttemptStore::load(
                Path::new(data_dir),
            ))),
        })
    }

    /// Build a fresh embedded fips node from the persisted identity. Called at
    /// construction and again on a BLE off→on cycle (run_rx_loop consumes the
    /// node, so re-enabling needs a new one).
    ///
    /// `wifi_aware` adds a UDP transport instance bound on the NDP interface —
    /// the Wi-Fi Aware bulk lane's data plane (docs/design/wifi-aware-interop.md).
    /// Deliberately not Android-gated: the identical UDP path is the lane's
    /// dev/test stand-in on a plain LAN.
    fn build_node(data_dir: &str, wifi_aware: bool) -> anyhow::Result<fips::Node> {
        let nsec = identity_store::load_or_generate(Path::new(data_dir))?;
        let mut config = fips::Config::new();
        config.node.identity.nsec = Some(nsec);
        config.node.identity.persistent = true;
        config.tun.enabled = false;
        // No built-in DNS responder: we answer `.fips` ourselves in the TUN pump
        // (`dns_intercept`), because on Android there is no system DNS socket to
        // bind — the OS resolver is pointed at the in-mesh sentinel instead.
        //
        // This must be off, not merely unused. `dns.enabled` defaults to *true*,
        // and the responder's start-up in `Node::start` assigns
        // `self.dns_identity_rx`, clobbering the receiver that
        // `enable_app_owned_dns()` installed moments earlier. Our sender in
        // `dns_intercept` is then attached to a dropped receiver, so every
        // `try_send` of a resolved identity fails silently and nothing is ever
        // registered. The name still resolves — the interceptor answers the
        // packet regardless — so the only visible symptom is that the first
        // packet to a freshly-resolved `<npub>.fips` gets ICMPv6 "No route".
        // That looks like a routing/distance bug, but reproduces with no peers
        // at all: direct neighbours mask it because their identity comes from
        // the Noise handshake, never from resolution.
        config.dns.enabled = false;
        // The control socket is now load-bearing, not an operator convenience:
        // it carries peer state (`show_peers`, the 8s tick) and every
        // platform-discovered peer push (`connect`). Without it the Wi-Fi Aware
        // and AP lanes carry no peers at all.
        //
        // The default path resolves `/run/fips` → `$XDG_RUNTIME_DIR` → `/tmp`,
        // none of which an Android app UID can write, so it is pointed at
        // app-private storage. Verified on device under SELinux Enforcing: the
        // socket binds with the app's own `app_data_file` label, and a stale
        // file from a force-stop is removed and rebound on the next launch.
        config.node.control.enabled = true;
        config.node.control.socket_path = crate::control_client::socket_path(data_dir);
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
        // The Wi-Fi Aware bulk lane: a UDP transport bound `[::]:4871`. UDP is
        // symmetric (no listener/dialer), fips-native, and reuses the proven
        // scoped-link-local path. Peers are supplied only by the platform peer
        // queue (`fips::discovery::platform`) — UDP is not advertised on Nostr
        // and no peer config points here — so `offline_only` semantics survive.
        //
        // It is configured UNCONDITIONALLY on Android (like the BLE transport
        // above), not gated on the Aware toggle: the toggle then controls only
        // the Kotlin radio (whether peers get pushed), never the node's
        // transport set — so flipping Wi-Fi Aware never restarts the node and
        // never disrupts an active BLE link. `wifi_aware` still adds it on the
        // host for the LAN-based dev/test stand-in.
        if wifi_aware || cfg!(target_os = "android") {
            config.transports.udp =
                fips::config::TransportInstances::Single(fips::config::UdpConfig {
                    bind_addr: Some(format!("[::]:{WIFI_AWARE_PORT}")),
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
            wifi_aware_enabled: false,
            node_running: false,
            node_status: "error".to_string(),
            rt: None,
            node: None,
            node_live: Arc::new(AtomicBool::new(false)),
            loop_task: None,
            content: None,
            speedtest: Arc::new(std::sync::Mutex::new(crate::state::SpeedtestView::default())),
            peer_cache: Arc::new(std::sync::Mutex::new(Vec::new())),
            peer_feed: Arc::new(std::sync::Mutex::new(PeerFeedHealth::default())),
            // No valid data dir on this path, so there is nowhere to persist to.
            // Attempts still render live; they just do not survive a restart.
            attempt_store: None,
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
            NativeAppAction::SetWifiAwareEnabled { enabled } => {
                // Pure flag, like SetBleEnabled: the UDP transport is always
                // present on Android (see build_node), so the toggle only
                // records intent and gates the Kotlin radio (whether peers are
                // pushed). It never touches the node lifecycle — so enabling or
                // disabling Wi-Fi Aware cannot restart the node or drop an
                // active BLE link.
                self.wifi_aware_enabled = enabled;
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
            NativeAppAction::SendPairRequest { npub, name, secret } => {
                if let (Some(content), Some(rt)) = (self.content.clone(), self.rt.as_ref()) {
                    rt.spawn(async move { content.send_pair_request(&npub, &name, &secret).await });
                }
                self.rev += 1;
            }
            NativeAppAction::CancelPairInvite { npub } => {
                if let Some(content) = self.content.as_ref() {
                    content.forget_outbound_pair(&npub);
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
            NativeAppAction::SpeedtestPeer { npub } => {
                self.start_speedtest(npub);
                self.rev += 1;
            }
        }
    }

    /// Spawn a peer speedtest (spawn-not-block; the result is observed via the
    /// `speedtest` field on the next `state()`). A ~1 MiB Blossom round-trip — big
    /// enough to dominate connection setup, small enough not to bloat the peer's
    /// store. Ignored if a run is already in flight.
    fn start_speedtest(&mut self, npub: String) {
        // Adaptive payload: start small and DOUBLE each run until one takes long
        // enough (>= TARGET) to be a meaningful measurement past connection
        // setup — the last run's result is the reported one. A slow link (BLE,
        // ~tens of KB/s) exceeds TARGET on the first 256 KiB run and stops
        // there; a fast link (Wi-Fi Aware) climbs to a few/tens of MiB. Capped
        // at MAX_BYTES (the Blossom upload limit).
        const START_BYTES: usize = 262_144; // 256 KiB
        const MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MiB (Blossom body cap)
        const TARGET: Duration = Duration::from_secs(5);
        let Some(rt) = self.rt.as_ref() else { return };
        {
            let mut s = self.speedtest.lock().unwrap();
            if s.running {
                return;
            }
            s.running = true;
            s.peer_npub = npub.clone();
            s.error.clear();
        }
        let slot = self.speedtest.clone();
        rt.spawn(async move {
            tracing::info!(peer = %npub, "speedtest: starting");
            let mut any_ok = false;
            let mut last_err: Option<String> = None;
            let mut size = START_BYTES;
            loop {
                let started = Instant::now();
                let result =
                    crate::ip_source::speedtest_peer(&npub, size, Duration::from_secs(120)).await;
                let elapsed = started.elapsed();
                match result {
                    Ok((up, down)) => {
                        any_ok = true;
                        last_err = None;
                        tracing::info!(
                            peer = %npub, size, up_mbps = up, down_mbps = down,
                            elapsed_ms = elapsed.as_millis() as u64, "speedtest: run ok"
                        );
                        {
                            let mut s = slot.lock().unwrap();
                            s.up_mbps = up;
                            s.down_mbps = down;
                            s.bytes = size as u64;
                            // Bump per run so the UI shows the size climbing.
                            s.generation += 1;
                        }
                        // Long enough to be meaningful, or hit the cap → done.
                        // Else the link is fast; double and measure again.
                        if elapsed >= TARGET || size >= MAX_BYTES {
                            break;
                        }
                        size = (size * 2).min(MAX_BYTES);
                    }
                    Err(e) => {
                        tracing::warn!(peer = %npub, size, error = format!("{e:#}"), "speedtest: run failed");
                        last_err = Some(e.to_string());
                        break;
                    }
                }
            }
            let mut s = slot.lock().unwrap();
            s.running = false;
            s.generation += 1;
            match last_err {
                // A failed larger run after a smaller success keeps the good
                // result; only surface an error if nothing succeeded.
                Some(err) if !any_ok => {
                    s.up_mbps = 0.0;
                    s.down_mbps = 0.0;
                    s.error = err;
                }
                _ => s.error.clear(),
            }
        });
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
            match Self::build_node(&self.data_dir, self.wifi_aware_enabled) {
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
            // Wire the app-owned DNS interceptor's identity channel into the node
            // so resolving `<npub>.fips` warms the route (caches the pubkey), the
            // same side effect fips's own DNS responder has. Without this the
            // first packet to a resolved address has no session and is dropped.
            crate::dns_intercept::set_identity_tx(node.enable_app_owned_dns());
            // Let Android learn the UDP transport's raw fd once it opens, so it
            // can pin the socket to whichever local-only network (Wi-Fi Aware
            // NDP, the `!FIPS` AP) carries a platform-pushed peer — otherwise
            // handshake replies can be lost to a competing validated default
            // network (e.g. cellular).
            crate::udp_fd_bridge::install(node.enable_app_owned_udp_fd());
            // Hand this node's BLE radio slot to the JNI bridge. The radio
            // itself belongs to `BleService` and may already be running (it
            // deliberately does not bounce the node when it starts a fresh
            // one), so the bridge installs whatever it is holding into the new
            // slot rather than waiting to be handed a radio.
            crate::ble_bridge_jni::set_radio_slot(node.enable_app_owned_ble_radio());
        }
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
        // The control socket is bound inside `run_rx_loop`, so peer queries only
        // start making sense once this flag is up — and even then not for the
        // first tick or two.
        self.node_live.store(true, Ordering::Relaxed);
        self.loop_task = Some(task);
        self.node_running = true;
        self.node_status = "running".to_string();
    }

    fn stop_node(&mut self) {
        // Aborting the loop task drops the node, stopping its transports.
        if let Some(task) = self.loop_task.take() {
            task.abort();
        }
        self.node_live.store(false, Ordering::Relaxed);
        self.peer_cache.lock().unwrap().clear();
        *self.peer_feed.lock().unwrap() = PeerFeedHealth::default();
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
        // Peers as of the last 8s tick. `state()` holds the reducer mutex on the
        // FFI thread, so it must never query the control socket itself — a
        // connect + write + read with a 5s timeout is not a drop-in for the
        // lock-free snapshot read this replaces.
        let peer_views: Vec<PeerView> = self.peer_cache.lock().unwrap().clone();

        let ble_peers: Vec<BlePeer> = peer_views
            .iter()
            .map(|p| BlePeer {
                node_addr_hex: p.node_addr_hex.clone(),
                npub: p.npub.clone(),
                connected: p.connected,
                psm: 0, // not surfaced in the snapshot yet
                rssi: None,
            })
            .collect();
        let ble_adverts = self.ble_adverts();

        // content.rs snapshot accessors `state()` already calls unconditionally
        // (RESEARCH.md Pitfall 5) — fetched once here and reused for both the
        // peers merge below and the AppState fields further down, so the merge
        // adds no new lock acquisitions.
        let circle = self
            .content
            .as_ref()
            .map(|c| c.circle_snapshot())
            .unwrap_or_default();
        let reachable_npubs = self
            .content
            .as_ref()
            .map(|c| c.reachable_npubs())
            .unwrap_or_default();
        let outbound_pairs = self
            .content
            .as_ref()
            .map(|c| c.outbound_pairs_snapshot())
            .unwrap_or_default();
        let pending_pair_requests = self
            .content
            .as_ref()
            .map(|c| c.pending_pairs_snapshot())
            .unwrap_or_default();

        // Lane-origin overrides (npub → observed lane, e.g. "aware" vs the
        // fips-reported "udp"): both Wi-Fi Aware and the LAN/AP lane ride
        // fips's plain UDP transport and share one JNI push site today
        // (`aware_bridge_jni.rs`'s hardcoded `TRANSPORT_TYPE = "udp"`), so
        // fips cannot tell them apart — only the Kotlin push site can. Read
        // from `lane_observation`'s process-global record of the lane each
        // npub was last pushed on (Android; empty on the host build).
        let lane_by_npub = self.observed_lane_by_npub();

        // Per-peer attempt history (role / discovery latency / outcome / send
        // failures) plus the learned address-to-node-address pairs the merge
        // uses to collapse an advert into its peer row.
        //
        // The live fips log is folded into the persistent store and read back
        // merged, so a freshly launched app shows what was recorded before the
        // last force-stop alongside the newest live attempts. `observe` does no
        // I/O — it runs here on the FFI thread — and the flush is spawned onto
        // the tokio runtime, rate limited to once every few seconds.
        let ble_attempts = match self.attempt_store.as_ref() {
            Some(store) => {
                store.observe(&self.ble_attempts());
                if store.flush_due() {
                    if let Some(rt) = self.rt.as_ref() {
                        let store = Arc::clone(store);
                        let at = now_ms();
                        rt.spawn(async move { store.flush(at) });
                    }
                }
                store.snapshot()
            }
            None => self.ble_attempts(),
        };

        let peers = crate::peer_diagnostics::merge_peers(
            &peer_views,
            &ble_peers,
            &ble_adverts,
            &circle,
            &pending_pair_requests,
            &outbound_pairs,
            &reachable_npubs,
            &lane_by_npub,
            &ble_attempts,
            now_ms(),
        );

        // Feed the connected-peer npubs to the content layer so `open_site` can
        // pull from currently-reachable Circle members (and skip offline ones).
        if let Some(content) = self.content.as_ref() {
            let connected: Vec<String> = ble_peers
                .iter()
                .filter(|p| p.connected && !p.npub.is_empty())
                .map(|p| p.npub.clone())
                .collect();
            content.set_connected_peers(connected);

            // Backlog resync is driven by the keepwarm loop's reconnect edge
            // (`Content::keepwarm_tick`), which recreates each in-app subscription
            // against a Circle peer as it (re)appears — direct *or* multi-hop.
            if let Some(rt) = self.rt.as_ref() {
                // Retry not-ready downloads whenever the Circle is non-empty.
                // open_site(_, None) tries every member — hop count is FIPS's
                // problem, and an unreachable one costs a bounded dial then backs
                // off — and `retriable_library_addrs` skips sites already syncing,
                // so this re-tries about once per attempt-duration (not every
                // poll), and keeps trying as a flaky session settles instead of
                // firing once on the connect edge and going quiet.
                if !content.circle_npubs().is_empty() {
                    for addr in content.retriable_library_addrs() {
                        let content = content.clone();
                        rt.spawn(async move { content.open_site(addr, None).await });
                    }
                }
            }
        }

        AppState {
            rev: self.rev,
            error: self.error_with_feed_health(),
            app_version: self.app_version.clone(),
            identity: self.identity.clone(),
            node: NodeStatus {
                running: self.node_running,
                status_text: self.node_status.clone(),
            },
            ble: {
                let (scanning, scanning_known, advertising, advertising_known) =
                    self.ble_radio_state();
                BleStatus {
                    enabled: self.ble_enabled,
                    role: "peripheral+central".to_string(),
                    scanning,
                    scanning_known,
                    advertising,
                    advertising_known,
                    adapter_name: if self.node_running {
                        "ble0".to_string()
                    } else {
                        "—".to_string()
                    },
                }
            },
            ble_peers,
            ble_adverts,
            wifi_aware: {
                let (scanning, scanning_known) = self.aware_radio_state();
                WifiAwareStatus {
                    enabled: self.wifi_aware_enabled,
                    port: if self.wifi_aware_enabled {
                        WIFI_AWARE_PORT
                    } else {
                        0
                    },
                    scanning,
                    scanning_known,
                }
            },
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
            circle,
            reachable_npubs,
            outbound_pairs,
            pending_pair_requests,
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
            speedtest: self.speedtest.lock().unwrap().clone(),
            peers,
        }
    }

    /// `self.error` plus, once the peer feed has failed
    /// [`PEER_FEED_FAILURES_BEFORE_ERROR`] ticks running, a line saying so.
    ///
    /// The tick is a detached task with no `&mut self`, so it cannot write
    /// `self.error` itself; it records into a shared health slot and the banner
    /// is composed here. Without this the only symptom of an unbound control
    /// socket is an empty peer list, which is exactly what a room with no peers
    /// in it looks like.
    fn error_with_feed_health(&self) -> String {
        let health = self.peer_feed.lock().unwrap();
        if health.consecutive_failures < PEER_FEED_FAILURES_BEFORE_ERROR {
            return self.error.clone();
        }
        let note = format!(
            "peer state unavailable ({} failed queries): {}",
            health.consecutive_failures, health.last_error
        );
        if self.error.is_empty() {
            note
        } else {
            format!("{}; {note}", self.error)
        }
    }

    /// The BLE radio's observed scanning/advertising state, as
    /// `(scanning, scanning_known, advertising, advertising_known)`.
    ///
    /// TODO(stage 2): always reports unknown. The flags used to be read back off
    /// fips's `AndroidBleBridge`, which no longer keeps them — correctly, since
    /// they were only ever Kotlin's own pushes bouncing off a struct in the
    /// wrong crate. Restore by mirroring the Aware lane's two Myco-owned
    /// atomics; the JNI push sites are still there, discarding their argument.
    /// Diagnostic only: it decides whether the Dev tab renders "scanning" or
    /// "unknown", nothing more. Reporting unknown is the honest degradation —
    /// the code has always refused to guess `false`.
    fn ble_radio_state(&self) -> (bool, bool, bool, bool) {
        (false, false, false, false)
    }

    /// The Wi-Fi Aware lane's observed discovering state, read from the Aware
    /// bridge's process-global flag rather than derived from other flags.
    /// `known` is false until Kotlin has pushed at least once (or on the host
    /// build, where the Aware bridge does not exist).
    #[cfg(target_os = "android")]
    fn aware_radio_state(&self) -> (bool, bool) {
        match crate::aware_bridge_jni::aware_discovering() {
            Some(v) => (v, true),
            None => (false, false),
        }
    }

    #[cfg(not(target_os = "android"))]
    fn aware_radio_state(&self) -> (bool, bool) {
        (false, false)
    }

    /// The lane ("aware" vs. "udp") each currently known npub was last
    /// observed reached over, read from `lane_observation`'s process-global
    /// record — the only place that can distinguish Wi-Fi Aware from the
    /// LAN/AP lane, both of which ride fips's plain UDP transport. Empty on
    /// the host build, where the Android Aware JNI bridge never pushes.
    #[cfg(target_os = "android")]
    fn observed_lane_by_npub(&self) -> std::collections::HashMap<String, String> {
        crate::lane_observation::snapshot()
    }

    #[cfg(not(target_os = "android"))]
    fn observed_lane_by_npub(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    /// Raw scan adverts (address / PSM / RSSI) seen by the BLE radio.
    ///
    /// TODO(stage 2): always empty. `AndroidBleBridge::advert_views()` is gone;
    /// the bridge forwards each advert straight into the transport's scanner
    /// channel now and keeps no list. Every advert still crosses
    /// `bleDeliverScan` in `ble_bridge_jni`, so the cheapest restoration is a
    /// small Myco-owned ring populated there. Diagnostic only: this feeds
    /// `AppState.ble_adverts` and the merge step that collapses an advert onto
    /// an existing peer row.
    fn ble_adverts(&self) -> Vec<BleAdvert> {
        Vec::new()
    }

    /// Per-peer BLE connect-attempt history.
    ///
    /// TODO(stage 2): always empty, so every Dev-tab row renders as having no
    /// recorded history. `fips::transport::ble::attempts` is gone; the restacked
    /// transport counts connect outcomes into `BleStats`, readable over the
    /// control socket's `show_transports`. Note the shape gap before wiring it:
    /// those are aggregate counters per transport, and these are per-attempt
    /// records keyed by BLE address. Diagnostic only — verified by tracing every
    /// consumer (`AttemptStore`, `merge_peers`, `AppState.peers`, the Kotlin Dev
    /// tab); nothing branches on it.
    fn ble_attempts(&self) -> Vec<crate::ble_diag::BlePeerAttempts> {
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

/// Milliseconds since the Unix epoch, passed to `merge_peers` (reserved for
/// future staleness-based state work; unused by today's merge logic).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
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

    /// Which of the node's subsystems Myco runs itself, and which it now
    /// depends on the node to run. Getting either wrong is silent.
    ///
    /// The TUN stays app-owned: the VpnService holds the fd, so a system TUN
    /// would be a second, competing packet plane.
    ///
    /// `dns` is now **on**, and that is the inversion. Myco used to answer
    /// `.fips` itself and push each resolved identity into the node over a
    /// channel; the responder's own start-up clobbered the receiver on that
    /// channel, so route warming silently stopped and the first packet to a
    /// freshly-resolved `<npub>.fips` came back "No route". The fix then was to
    /// switch the responder off. The fix now is the opposite: the responder
    /// runs, publishes where it bound, and Myco forwards `.fips` queries to it
    /// — so registering the identity is the responder's own side effect and
    /// there is no app-owned channel left to clobber.
    ///
    /// `control` is on because peer state and every platform peer push ride it.
    #[test]
    fn node_config_matches_who_owns_each_subsystem() {
        let dir = temp_dir("owned-subsystems");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp data dir");

        let node = AppRuntime::build_node(dir.to_str().unwrap(), false)
            .expect("node builds with a fresh identity");
        let config = node.config();

        assert!(
            config.dns.enabled,
            "fips's DNS responder must be on — Myco proxies `.fips` queries to \
             it, and answering them is what warms the route"
        );
        assert!(
            !config.tun.enabled,
            "the TUN is app-owned (VpnService holds the fd)"
        );
        assert!(
            config.node.control.enabled,
            "peer state and platform peer pushes both ride the control socket"
        );
        assert_eq!(
            config.node.control.socket_path,
            crate::control_client::socket_path(dir.to_str().unwrap()),
            "the default path resolves to /run, $XDG_RUNTIME_DIR or /tmp — none \
             writable by an Android app UID"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A broken peer feed must not look like an empty room. Three consecutive
    /// failures is the threshold; below it the banner stays clean, because the
    /// socket is bound inside `run_rx_loop` and the first tick after StartNode
    /// legitimately races it.
    #[test]
    fn a_sustained_peer_feed_failure_reaches_the_error_banner() {
        let dir = temp_dir("peer-feed-health");
        let _ = std::fs::remove_dir_all(&dir);
        let rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        assert!(rt.state().error.is_empty(), "healthy by default");

        {
            let mut health = rt.peer_feed.lock().unwrap();
            health.consecutive_failures = PEER_FEED_FAILURES_BEFORE_ERROR - 1;
            health.last_error = "connect: No such file or directory".to_string();
        }
        assert!(
            rt.state().error.is_empty(),
            "a startup-window failure must not shout"
        );

        rt.peer_feed.lock().unwrap().consecutive_failures = PEER_FEED_FAILURES_BEFORE_ERROR;
        let error = rt.state().error;
        assert!(
            error.contains("peer state unavailable"),
            "sustained failure must be visible, got: {error}"
        );
        assert!(
            error.contains("No such file or directory"),
            "the reason must survive into the banner, got: {error}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
