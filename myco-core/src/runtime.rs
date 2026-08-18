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

/// The node runs **two** UDP transport instances, not one, and everything
/// below names them.
///
/// One socket cannot serve both lanes. Android routes by the network a socket
/// is *marked* with, not by destination alone: `Network.bindSocket` pins a
/// descriptor to one `android.net.Network`, and a socket pinned to
/// infrastructure Wi-Fi cannot reach a Wi-Fi Aware peer, whose NDP is a
/// separate `Network` with its own routing table. With a single shared socket
/// the AP lane's bind won, so Aware discovery worked end to end (match, npub
/// exchange, NDP up) and then every dial over it timed out in the handshake.
///
/// So: one instance per lane, each pinned by its own Kotlin radio (see
/// [`crate::udp_fd_bridge`]), and peer addresses qualified with the instance
/// name (`"udp/lan"`, `"udp/aware"`) so fips dials down the lane the peer was
/// actually observed on rather than whichever transport id sorted lowest.
const LAN_UDP_INSTANCE: &str = "lan";
const AWARE_UDP_INSTANCE: &str = "aware";

/// UDP port for the LAN / `!FIPS` AP lane. Unchanged at 4871: this lane talks
/// to desktop fips nodes today and works, and desktop peers are discovered by
/// mDNS advert, so moving it would break a working lane for nothing.
const LAN_UDP_PORT: u16 = 4871;

/// UDP port for the Wi-Fi Aware bulk lane. Both peers bind it on the NDP
/// interface and exchange over it — symmetric, no listener/dialer roles. A
/// fixed app constant (we bind our own port), so there is no PSM-style
/// discovery problem. UDP is fips's native transport and the LAN-discovery
/// path (which this reuses) is already UDP + scoped link-local IPv6.
/// See docs/design/wifi-aware-interop.md.
///
/// **Not a flag day.** Each phone advertises this port to its peers in the
/// Aware identity exchange and is dialled at the port it advertised, so a
/// phone on a build from before this port existed — which advertises no port
/// and listens on [`LAN_UDP_PORT`] — is still reachable. `AwareRadio` formats
/// `"[fe80::x%ifindex]:<peer's port>"`; see its `parsePeer`.
const AWARE_UDP_PORT: u16 = 4872;

/// Which UDP transport instance a Kotlin radio's lane rides.
///
/// `lane` is the label the radio itself pushes (`AwareRadio` sends `"aware"`,
/// `ApRadio` sends `"udp"`); this is the single place it is turned into a fips
/// instance name, so the name a socket is pinned by and the name a dial is
/// routed by cannot drift apart. Anything unrecognised is the AP lane, which
/// is the one that behaves as UDP always has.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn udp_instance_for_lane(lane: &str) -> &'static str {
    match lane {
        "aware" => AWARE_UDP_INSTANCE,
        _ => LAN_UDP_INSTANCE,
    }
}

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

/// How long a `StopNode` waits for the node's own graceful teardown before
/// giving up and aborting the loop task.
///
/// fips bounds the drain itself with `node.drain_timeout_secs` (2s by default)
/// and the teardown after it is a handful of `abort()`s plus a best-effort
/// `stop_advertising`, so this only has to be comfortably longer than that.
/// Reaching it means the transports leaked — exactly the failure the graceful
/// path exists to prevent — so it is logged at error level, and the abort is the
/// last resort that keeps the toggle from wedging forever.
const NODE_STOP_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// The custom relay URL as last saved — which is what the settings screen
    /// should show, even though the running store is still the previous one
    /// until the app is restarted.
    pending_relay_url: String,
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
    /// The background task running `node.start()`, the rx loop, and the node's
    /// own graceful teardown. It is *completing* — not aborting — that stops the
    /// transports; see [`AppRuntime::stop_node`].
    loop_task: Option<JoinHandle<()>>,
    /// Fires the rx loop's shutdown signal. Sending on it (or dropping it) makes
    /// `Node::run_rx_loop_with_shutdown` enter fips's bounded drain and return,
    /// after which the loop task calls `Node::finish_shutdown` and the
    /// transports are genuinely down.
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Set at `StopNode` and cleared only once the loop task has actually
    /// finished. [`AppRuntime::start_node`] refuses to build a second node while
    /// it is set: two live nodes fighting over the single Kotlin BLE radio is
    /// the failure this whole dance exists to prevent. Shared and atomic because
    /// the detached watchdog that clears it has no `&mut self`.
    stopping: Arc<AtomicBool>,
    /// A `StartNode` that arrived while `stopping` was still set, replayed by
    /// [`AppRuntime::poll_pending_start`] on the next state read.
    start_pending: bool,
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
        // The backend is chosen before anything else opens, so the setting has to
        // come off disk first. A custom relay means the embedded store stays on
        // disk but stops serving (`reference/thinning-custom-relay.md`, D3).
        let settings = crate::settings_store::load(Path::new(data_dir));
        let custom_relay = settings.relay_url().map(|url| {
            tracing::info!(url, "content: using a custom relay");
            Arc::new(crate::remote_backend::RemoteBackend::new(url))
        });
        let content = Arc::new(Content::open_with_relay(Path::new(data_dir), custom_relay)?);

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
            let gossiper: Arc<dyn crate::mesh_relay::Gossiper> =
                Arc::new(crate::gossip::MeshGossiper::new(content.clone()));
            // Restrict mesh access to paired (Circle) peers — only the pairing
            // handshake is open, so strangers can request to pair but can't read or
            // push content. Loopback (the in-app WebView) always bypasses the gate.
            let gate: Arc<dyn crate::mesh_relay::PeerGate> =
                Arc::new(crate::content::CircleGate::new(content.clone()));
            let hub =
                crate::mesh_relay::RelayHub::with_gate(content.relay(), Some(gossiper), Some(gate));

            // Mesh socket: IPV6_V6ONLY `[::]:4870` so it doesn't collide with the
            // loopback bind and is reachable by peers at `ws://<npub>.fips:4870`.
            match crate::mesh_relay::bind("[::]:4870".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    let hub = hub.clone();
                    rt.spawn(async move {
                        if let Err(e) = crate::mesh_relay::serve_on_hub(hub, listener).await {
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
            match crate::mesh_relay::bind("127.0.0.1:4870".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    let hub = hub.clone();
                    rt.spawn(async move {
                        if let Err(e) = crate::mesh_relay::serve_on_hub(hub, listener).await {
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
            // The auth plane: the one port open to peers we have never met, and
            // the only way into the Circle. It has to bind before the content
            // servers matter — without it a stranger cannot pair at all, and the
            // relay and Blossom gates below have no exceptions to let them in.
            // See `reference/thinning-custom-relay.md` (D6).
            match crate::auth_service::bind(
                format!("[::]:{}", crate::auth_service::AUTH_PORT)
                    .parse::<SocketAddr>()
                    .unwrap(),
            ) {
                Ok(listener) => {
                    let content_for_auth = content.clone();
                    rt.spawn(async move {
                        if let Err(e) =
                            crate::auth_service::serve_on(content_for_auth, listener).await
                        {
                            tracing::error!(error = %e, "auth service exited");
                        }
                    });
                }
                Err(e) => {
                    if !mesh_warning.is_empty() {
                        mesh_warning.push_str("; ");
                    }
                    mesh_warning.push_str(&format!(
                        "Another app is using port {} — Myco can't accept pairing \
                         requests, so new peers won't be able to pair with this \
                         device. ({e})",
                        crate::auth_service::AUTH_PORT
                    ));
                }
            }
            match myco_blossom::server::bind("[::]:24243".parse::<SocketAddr>().unwrap()) {
                Ok(listener) => {
                    // Same paired-only gate for blobs: a mesh source must be a
                    // current Circle member (loopback bypasses). Pairing never
                    // touches Blossom, so there's no handshake exception here.
                    let content_for_blob = content.clone();
                    // Reads are granted to every paired peer; uploads are not, so
                    // the two are answered from different flags on the peer's own
                    // permission record (D10).
                    let access: myco_blossom::server::AccessFn = Arc::new(move |ip, op| match op {
                        myco_blossom::server::BlobOp::Read => content_for_blob.may_read_blobs(ip),
                        myco_blossom::server::BlobOp::Write => {
                            content_for_blob.may_upload_blobs(ip)
                        }
                    });
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
            pending_relay_url: settings.relay_url().unwrap_or_default(),
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
            shutdown_tx: None,
            stopping: Arc::new(AtomicBool::new(false)),
            start_pending: false,
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
        // The built-in `.fips` responder runs, and Myco proxies to it.
        //
        // Android has no system DNS socket to point at the responder — the
        // VpnService aims the OS resolver into the tunnel, so queries surface
        // as IPv6/UDP packets on the app's own fd. Myco still owns the packet
        // plane, but it lifts the DNS payload out and forwards it here instead
        // of resolving in-app: answering a `<npub>.fips` query is what puts
        // that peer's public key in the node's identity cache, and the key
        // cannot be recovered from the mesh address.
        //
        // This is the inversion of the previous fix, not a regression of it.
        // Before, Myco answered and pushed identities *into* the node over a
        // channel that the responder's own start-up then overwrote — so the
        // responder had to be off. Now there is no app-owned channel to
        // clobber, and the responder being off is what would break warming.
        //
        // Port 0: the address is read back off the bound socket via
        // `dns_local_addr()`, so the kernel picks and nothing squats on 5354.
        config.dns.enabled = true;
        config.dns.port = Some(0);
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
        Self::clear_control_socket(&config.node.control.socket_path);
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
        // Two UDP transports, one per lane — see the instance constants at the
        // top of this file for why one socket cannot serve both. UDP is
        // symmetric (no listener/dialer), fips-native, and reuses the proven
        // scoped-link-local path. Peers are supplied only by the platform peer
        // queue — UDP is not advertised on Nostr and no peer config points
        // here — so `offline_only` semantics survive.
        //
        // Both are configured UNCONDITIONALLY on Android (like the BLE
        // transport above), not gated on the Aware toggle: the toggle then
        // controls only the Kotlin radio (whether peers get pushed), never the
        // node's transport set — so flipping Wi-Fi Aware never restarts the
        // node and never disrupts an active BLE link. `wifi_aware` still adds
        // them on the host for the LAN-based dev/test stand-in.
        if wifi_aware || cfg!(target_os = "android") {
            let udp = |port: u16| fips::config::UdpConfig {
                bind_addr: Some(format!("[::]:{port}")),
                ..Default::default()
            };
            config.transports.udp = fips::config::TransportInstances::Named(
                [
                    (LAN_UDP_INSTANCE.to_string(), udp(LAN_UDP_PORT)),
                    (AWARE_UDP_INSTANCE.to_string(), udp(AWARE_UDP_PORT)),
                ]
                .into_iter()
                .collect(),
            );
        }
        fips::Node::new(config).map_err(|e| anyhow::anyhow!("fips Node::new failed: {e}"))
    }

    /// Unlink the control socket file before a node is built, so the next
    /// `ControlSocket::bind` inside `run_rx_loop` always gets the path.
    ///
    /// fips spawns its control accept loop from `run_rx_loop` without keeping
    /// the `JoinHandle`, so the previous node's accept task — and the
    /// `UnixListener` it owns — survives that node's teardown. `bind` treats a
    /// path that still answers `connect()` as "already in use" and refuses,
    /// which would leave the freshly started node with no control socket at all:
    /// no `show_peers`, no platform peer pushes, an empty Dev tab.
    ///
    /// Unlinking first leaves the orphan bound to an unnamed inode — unreachable
    /// by any new client, and harmless — while the new node binds a fresh one.
    /// Safe because Myco is a single process and the path is app-private, so the
    /// "someone else is listening" case `bind` guards against cannot arise; and
    /// because the only caller either has no node running yet (`try_new`) or has
    /// waited for the previous one to finish (`start_node`).
    fn clear_control_socket(socket_path: &str) {
        match std::fs::remove_file(socket_path) {
            Ok(()) => tracing::info!(path = socket_path, "removed the previous control socket"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = socket_path,
                error = %e,
                "could not remove the previous control socket; the node may fail to bind it"
            ),
        }
    }

    fn from_error(app_version: &str, msg: &str) -> Self {
        Self {
            app_version: app_version.to_string(),
            data_dir: String::new(),
            rev: 0,
            error: msg.to_string(),
            pending_relay_url: String::new(),
            identity: IdentityView::default(),
            ble_enabled: false,
            wifi_aware_enabled: false,
            node_running: false,
            node_status: "error".to_string(),
            rt: None,
            node: None,
            node_live: Arc::new(AtomicBool::new(false)),
            loop_task: None,
            shutdown_tx: None,
            stopping: Arc::new(AtomicBool::new(false)),
            start_pending: false,
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
                    content.add_to_library(&addr, None, crate::content::now_secs());
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
            NativeAppAction::SetCustomRelay { url } => {
                let settings = crate::settings_store::Settings {
                    custom_relay_url: Some(url.clone()),
                };
                match crate::settings_store::save(Path::new(&self.data_dir), &settings) {
                    // Takes effect at the next launch; the store in use right now
                    // does not change under the running content layer.
                    Ok(()) => {
                        self.pending_relay_url = settings.relay_url().unwrap_or_default();
                        tracing::info!(url, "settings: custom relay saved");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "settings: could not save the custom relay");
                        self.error = format!("Could not save the relay setting: {e}");
                    }
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
        // A loop task that has already finished on its own — `node.start()`
        // failed, or the packet channel closed — otherwise leaves
        // `node_running` stuck true and the mesh permanently down, with no way
        // back short of a process restart. Treat it as stopped so this call can
        // rebuild. The task tears its own node down before returning, so there
        // is nothing left running to collide with.
        if self.node_running && self.loop_task.as_ref().is_some_and(|t| t.is_finished()) {
            tracing::warn!("fips loop task exited on its own; rebuilding the node");
            self.loop_task = None;
            self.shutdown_tx = None;
            self.node_running = false;
            self.node_live.store(false, Ordering::Relaxed);
        }
        if self.node_running {
            return;
        }
        // Never build a second node while the previous one's transports are
        // still up. They would both drive the one shared Kotlin BLE radio, and
        // the node the UI reads (the new one, which rebinds the control socket
        // last) is not the node doing the work — so the app reports an empty
        // room while BLE is genuinely peered. Queue the start instead;
        // `poll_pending_start` replays it on the next state read, within a
        // second of the drain finishing.
        if self.stopping.load(Ordering::Acquire) {
            self.start_pending = true;
            self.node_status = "waiting for the previous node to stop".to_string();
            return;
        }
        self.start_pending = false;
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
            // Let Android learn each UDP transport's raw fd once it opens, so
            // every lane can pin its own socket to its own local-only network
            // (the Wi-Fi Aware NDP, the `!FIPS` AP) — otherwise handshake
            // replies are lost to a competing validated default network (e.g.
            // cellular), and worse, a socket pinned to one lane's network
            // cannot reach the other lane's peers at all. Deliveries are
            // labelled with the instance name, so a radio can only ever be
            // handed its own descriptor.
            crate::udp_fd_bridge::install(node.enable_app_owned_udp_fd());
            // Hand this node's BLE radio slot to the JNI bridge. The radio
            // itself belongs to `BleService` and may already be running (it
            // deliberately does not bounce the node when it starts a fresh
            // one), so the bridge installs whatever it is holding into the new
            // slot rather than waiting to be handed a radio.
            crate::ble_bridge_jni::set_radio_slot(node.enable_app_owned_ble_radio());
        }
        // The rx loop serves until this fires, then drains in place. Dropping
        // the sender resolves the receiver too, so a runtime torn down without a
        // `StopNode` still asks the node to shut down rather than vanishing.
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let task = rt.spawn(async move {
            let mut node = node;
            if let Err(e) = node.start().await {
                tracing::error!("fips node start failed: {e}");
                // A partial start can still have left children up (a transport
                // bound, the responder listening). Tear down what exists rather
                // than dropping the node on top of them.
                node.finish_shutdown().await;
                return;
            }
            // Publish where the built-in `.fips` responder bound, so the TUN
            // pump can forward queries to it. This has to happen *here*, inside
            // the task: `dns_local_addr()` is only meaningful after `start()`
            // returns, and `run_rx_loop` below borrows the node for the rest of
            // its life, so this is the only moment a `&Node` exists and the
            // value is settled. `None` means the responder never came up (its
            // bind only warns), and `.fips` then resolves to nothing rather
            // than to an address the node has no key for.
            let dns_addr = node.dns_local_addr();
            match dns_addr {
                Some(addr) => tracing::info!(%addr, "fips DNS responder listening"),
                None => tracing::warn!("fips DNS responder is not running; .fips will not resolve"),
            }
            crate::dns_intercept::set_responder_addr(dns_addr);
            // Serves until `StopNode` fires the signal (or the packet channel
            // closes), then runs fips's bounded in-place drain and returns.
            if let Err(e) = node
                .run_rx_loop_with_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                tracing::warn!("fips rx loop ended: {e}");
            }
            // The half that actually stops the radios. The BLE accept loop, the
            // scan+probe loop and the advertiser are separately spawned tasks
            // holding `Arc` clones of the pool, the io and the stats; only
            // `Transport::stop` — reached from here — aborts them, and `Drop`
            // cannot run async teardown to catch them.
            node.finish_shutdown().await;
            // The responder socket is gone with the node's children, so retract
            // its address now (not at `StopNode`: it kept answering for the
            // whole drain window). Nothing can have republished it in between —
            // `start_node` will not build a new node until this task has
            // finished.
            crate::dns_intercept::set_responder_addr(None);
            tracing::info!("fips node stopped and its transports torn down");
        });
        // The control socket is bound inside `run_rx_loop`, so peer queries only
        // start making sense once this flag is up — and even then not for the
        // first tick or two.
        self.node_live.store(true, Ordering::Relaxed);
        self.loop_task = Some(task);
        self.shutdown_tx = Some(shutdown_tx);
        self.node_running = true;
        self.node_status = "running".to_string();
    }

    /// Stop the node — without blocking, and without leaving its transports
    /// running.
    ///
    /// This used to be `loop_task.abort()`, on the belief that dropping the node
    /// stopped its transports. It does not. The BLE accept loop, the scan+probe
    /// loop and the advertiser are *separately spawned* tokio tasks holding
    /// `Arc` clones of the connection pool, the io backend and the stats;
    /// aborting the parent touches none of them, and async teardown cannot run
    /// from `Drop`. The old node kept scanning, advertising and dialling the one
    /// shared Kotlin radio forever, and the next `StartNode` stacked a second
    /// node on top of it — two DNS responders, two BLE transports, one process.
    /// The new node rebound the control socket last, so `show_peers` reported
    /// *its* peers (none) while the old node held the live BLE session.
    ///
    /// The constraint that makes this awkward: `stop_node` runs on the FFI
    /// thread under the reducer mutex, so it must never await the teardown.
    /// Instead it fires the shutdown signal and hands the waiting to a detached
    /// watchdog. `start_node` refuses to build a new node until that watchdog
    /// clears [`Self::stopping`], and `poll_pending_start` replays the queued
    /// start when it does.
    fn stop_node(&mut self) {
        // An explicit stop cancels a start that was queued behind an earlier one.
        self.start_pending = false;
        // Ask the rx loop to drain. Dropping the sender would do it too; sending
        // says so explicitly, and a closed receiver just means the task is
        // already on its way out.
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let task = self.loop_task.take();
        if let (Some(task), Some(rt)) = (task, self.rt.as_ref()) {
            self.stopping.store(true, Ordering::Release);
            let stopping = self.stopping.clone();
            rt.spawn(async move {
                let mut task = task;
                if tokio::time::timeout(NODE_STOP_TIMEOUT, &mut task)
                    .await
                    .is_err()
                {
                    tracing::error!(
                        timeout_secs = NODE_STOP_TIMEOUT.as_secs(),
                        "fips node teardown did not finish; aborting the loop task as a last \
                         resort — its BLE loops may survive and fight the next node for the radio"
                    );
                    task.abort();
                    let _ = task.await;
                    // The task never reached its own retraction.
                    crate::dns_intercept::set_responder_addr(None);
                }
                // Releases the gate in `start_node`: the transports are down (or
                // as down as an abort can make them), so a new node may claim
                // the radio. Ordered after the retraction above so a queued
                // start can only ever publish a fresh responder address.
                stopping.store(false, Ordering::Release);
            });
        } else {
            // Nothing was running (or there is no runtime): no drain to wait
            // for, so retract the responder address here instead.
            crate::dns_intercept::set_responder_addr(None);
            self.stopping.store(false, Ordering::Release);
        }
        // Gates the 8s peer tick and the platform-peer drainer off immediately:
        // the node is on its way out and must not be handed new peers to dial.
        // The control socket does keep answering for the drain window, but
        // nothing should be reading a draining node's peer list.
        self.node_live.store(false, Ordering::Relaxed);
        self.peer_cache.lock().unwrap().clear();
        *self.peer_feed.lock().unwrap() = PeerFeedHealth::default();
        self.node_running = false;
        self.node_status = "stopped".to_string();
    }

    /// Replay a `StartNode` that arrived while the previous node was still
    /// draining. Called from `state_json` — which the UI polls at 1Hz and which
    /// every `dispatch_json` ends with — so a queued start lands within about a
    /// second of the old node actually being down.
    fn poll_pending_start(&mut self) {
        if self.start_pending && !self.stopping.load(Ordering::Acquire) {
            tracing::info!("previous node is down; starting the queued node");
            self.start_node();
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
            &crate::advert_names::snapshot(),
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
                // The drain is a real, visible state: the node is neither
                // running nor yet gone, and a queued start is waiting on it.
                // Saying so beats a flat "stopped" that the toggle contradicts.
                status_text: if self.stopping.load(Ordering::Acquire) {
                    if self.start_pending {
                        "restarting (draining the previous node)".to_string()
                    } else {
                        "stopping (draining)".to_string()
                    }
                } else {
                    self.node_status.clone()
                },
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
                        AWARE_UDP_PORT
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
            relay_backend: self
                .content
                .as_ref()
                .map(|c| c.relay_health())
                .unwrap_or_default(),
            pending_relay_url: self.pending_relay_url.clone(),
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
    /// `(scanning, scanning_known, advertising, advertising_known)`, read from
    /// the BLE bridge's process-global flags rather than derived from other
    /// flags. Each `known` is false until Kotlin has pushed at least once (or
    /// on the host build, where the BLE bridge does not exist) — the UI renders
    /// unknown rather than guessing false.
    ///
    /// Diagnostic only: it decides whether the Dev tab's radio self-check card
    /// renders "active"/"idle" or "unknown", nothing more.
    #[cfg(target_os = "android")]
    fn ble_radio_state(&self) -> (bool, bool, bool, bool) {
        let (scanning, scanning_known) = match crate::ble_bridge_jni::ble_scanning() {
            Some(v) => (v, true),
            None => (false, false),
        };
        let (advertising, advertising_known) = match crate::ble_bridge_jni::ble_advertising() {
            Some(v) => (v, true),
            None => (false, false),
        };
        (scanning, scanning_known, advertising, advertising_known)
    }

    #[cfg(not(target_os = "android"))]
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

    pub fn state_json(&mut self) -> String {
        // The 1Hz UI poll is also the clock a deferred start runs on; see
        // `poll_pending_start`.
        self.poll_pending_start();
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
        content.add_to_library(&addr, None, crate::content::now_secs());
        rt.spawn(content.clone().open_site(addr, None));
    }
    if let Err(e) = std::fs::write(&marker, b"1\n") {
        tracing::warn!(error = %e, "could not write default-seed marker");
    }
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

    /// Wait for the detached stop watchdog to report the old node down.
    /// Generous: the point is to catch "never finishes", not to time it.
    fn await_stopped(rt: &AppRuntime) {
        let deadline = Instant::now() + NODE_STOP_TIMEOUT + Duration::from_secs(5);
        while rt.stopping.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `StopNode` must actually take the node down, and must do it gracefully —
    /// the loop task has to run to completion (rx-loop drain →
    /// `Node::finish_shutdown` → `Transport::stop`), not be aborted.
    ///
    /// Aborting is what the old code did, and it is why two nodes could be live
    /// in one process: the BLE accept / scan+probe / advertiser tasks hold `Arc`
    /// clones and survive their parent, so nothing but `finish_shutdown` stops
    /// them. There is no BLE transport on the host, so what this pins down is
    /// the sequencing: the stop completes on its own well inside
    /// [`NODE_STOP_TIMEOUT`], i.e. the last-resort abort never had to fire.
    #[test]
    fn stopping_the_node_completes_the_loop_task_rather_than_aborting_it() {
        let dir = temp_dir("node-graceful-stop");
        let _ = std::fs::remove_dir_all(&dir);
        let mut rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        rt.dispatch(NativeAppAction::StartNode);
        assert!(rt.state().node.running, "node should be running");

        let stopped_at = Instant::now();
        rt.dispatch(NativeAppAction::StopNode);
        await_stopped(&rt);

        assert!(
            !rt.stopping.load(Ordering::Acquire),
            "the graceful stop never finished — the watchdog would have had to abort"
        );
        assert!(
            stopped_at.elapsed() < NODE_STOP_TIMEOUT,
            "stop took {:?}, which means the last-resort abort fired",
            stopped_at.elapsed()
        );
        assert!(
            rt.loop_task.is_none() && rt.shutdown_tx.is_none(),
            "the loop task and its shutdown signal must be released by StopNode"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `StartNode` that lands while the previous node is still draining must
    /// be queued, not honoured. Honouring it is the bug: two nodes in one
    /// process, both driving the single Kotlin BLE radio, with the UI reading
    /// the idle one.
    ///
    /// The gate is driven directly here rather than by racing a real drain —
    /// on the host, with no transports and no peers, the drain finishes in
    /// microseconds and the window would be untestable.
    #[test]
    fn a_start_during_a_drain_is_queued_until_the_old_node_is_down() {
        let dir = temp_dir("node-restart-gate");
        let _ = std::fs::remove_dir_all(&dir);
        let mut rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        // Stand in for "the previous node is still tearing down".
        rt.stopping.store(true, Ordering::Release);

        rt.dispatch(NativeAppAction::StartNode);
        assert!(
            !rt.node_running && rt.loop_task.is_none(),
            "no second node may be built while the first one's transports are up"
        );
        assert!(
            rt.start_pending,
            "the start must be remembered, not dropped"
        );
        let status = rt.state().node.status_text;
        assert!(
            status.contains("draining"),
            "the UI needs to see the restart in flight, got: {status}"
        );

        // A state read while still draining must not start it either.
        let _ = rt.state_json();
        assert!(!rt.node_running, "still draining — still no node");

        // The watchdog's release edge.
        rt.stopping.store(false, Ordering::Release);
        let _ = rt.state_json();
        assert!(
            rt.node_running && rt.loop_task.is_some(),
            "the queued start must be replayed once the old node is down"
        );
        assert!(!rt.start_pending, "the queued start must be consumed");

        rt.dispatch(NativeAppAction::StopNode);
        await_stopped(&rt);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An off→on cycle must leave exactly one node behind. Same shape as the
    /// mesh toggle on the device: stop, then start again immediately.
    #[test]
    fn an_off_on_cycle_leaves_one_node() {
        let dir = temp_dir("node-off-on");
        let _ = std::fs::remove_dir_all(&dir);
        let mut rt = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");

        rt.dispatch(NativeAppAction::StartNode);
        assert!(rt.state().node.running);

        rt.dispatch(NativeAppAction::StopNode);
        // The toggle's own start, fired before the drain can possibly be done.
        rt.dispatch(NativeAppAction::StartNode);
        assert!(
            rt.node_running || rt.start_pending,
            "the restart is either immediate or queued, never lost"
        );

        // Drive the 1Hz poll the UI would be doing.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !rt.node_running && Instant::now() < deadline {
            let _ = rt.state_json();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(rt.node_running, "the node must come back after an off→on");
        assert!(!rt.start_pending);
        assert!(
            !rt.stopping.load(Ordering::Acquire),
            "the old node must be down before the new one exists"
        );

        rt.dispatch(NativeAppAction::StopNode);
        await_stopped(&rt);
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
