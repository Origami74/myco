use serde::Serialize;

/// One JSON snapshot per `state()` call. `rev` lets the UI skip no-op redraws;
/// `error` is empty when healthy. Mirrors `docs/reference/ffi-surface.md`,
/// narrowed to the P1 surface (identity + node + BLE).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub rev: u64,
    pub error: String,
    pub app_version: String,
    pub identity: IdentityView,
    pub node: NodeStatus,
    /// BLE adapter/transport status (the developer-UI control plane).
    pub ble: BleStatus,
    /// Peers seen/connected over the radio. Identified by `node_addr` from the
    /// in-band pubkey exchange, never by MAC. Empty until the BLE backend runs
    /// (Android, P1 M4).
    pub ble_peers: Vec<BlePeer>,
    /// Raw scan adverts (address / PSM / RSSI) — the radio-level "discovered
    /// devices" view, distinct from the mesh-level `ble_peers`.
    pub ble_adverts: Vec<BleAdvert>,
    /// Wi-Fi Aware bulk-lane status (the control plane beside `ble`).
    pub wifi_aware: WifiAwareStatus,

    // --- content layer (P2) ---
    /// Per-site sync/readiness, keyed by `<host>` label. Kotlin polls this after
    /// `OpenNsite` to know when to launch the fullscreen NsiteActivity.
    pub sites: Vec<crate::content::SiteStatusView>,
    /// Pinned/opened sites.
    pub library: Vec<crate::content::LibraryItem>,
    /// Local relay/Blossom counts (for the developer screen + cache view).
    pub cache: crate::content::CacheView,
    /// The user's **Circle**: paired peers we pull nsites from over the mesh.
    pub circle: Vec<crate::content::CircleContact>,
    /// Circle members we currently hold a live mesh relay connection to. This
    /// is the honest "reachable" signal: it means bytes are flowing to them
    /// right now, whether they are a direct neighbour or many hops away. The
    /// direct-peer table ([`AppState::ble_peers`]) is *not* a substitute — it
    /// only ever names adjacent nodes.
    pub reachable_npubs: Vec<String>,
    /// Invites we sent that nobody has accepted yet. Kept so the UI can say an
    /// invite is *waiting* — a bump with no mesh route between the two phones
    /// cannot be delivered at the time it is made.
    pub outbound_pairs: Vec<crate::content::OutboundPairView>,
    /// Incoming pair requests awaiting accept/decline (the UI shows a pop-up).
    pub pending_pair_requests: Vec<crate::content::PairRequestView>,
    /// nsites discovered on Circle peers' relays (`SearchNsites` — "around me").
    pub discovered: Vec<crate::content::DiscoveredNsite>,
    /// "Mesh-only": the IP online fallback is disabled (pull only over the mesh).
    pub offline_only: bool,
    /// The configured custom relay and whether it can be reached. Empty `url`
    /// means the built-in store; a non-empty `error` is what the Storage screen
    /// warns about, the same way it warns about a radio being off.
    pub relay_backend: crate::remote_backend::BackendHealth,
    /// The custom relay URL as last saved, which may differ from the one in use
    /// until the app restarts.
    pub pending_relay_url: String,
    /// The configured custom Blossom and whether it can be reached.
    pub blob_backend: crate::remote_backend::BackendHealth,
    /// The custom Blossom URL as last saved.
    pub pending_blossom_url: String,
    /// Status of the latest nsite update check (feedback for "Check for updates").
    pub update_check: crate::content::UpdateCheckView,
    /// Result of the dev-menu peer speedtest (a Blossom upload+download round-trip).
    pub speedtest: SpeedtestView,
    /// Native paired-peer file transfers, including incoming offers awaiting
    /// the user's accept/deny decision.
    pub file_transfers: Vec<crate::file_transfer::FileTransferView>,
    /// The merged per-identity peer diagnostics view (DIAG-01/03/04/06):
    /// one row per known peer, covering every state from `connected` through
    /// `unreachable`, built once here rather than re-derived client-side from
    /// `ble_peers` / `ble_adverts` / `circle` / `reachable_npubs`.
    pub peers: Vec<PeerDiagnosticView>,
}

/// One merged, npub-or-address-keyed peer diagnostics row for the Dev tab
/// (DIAG-01/03/04/06). `key` is the `npub` when resolved, else the
/// `node_addr_hex`, else the raw BLE address — so a device seen only as an
/// unresolved advert still gets a stable row (D-09). This is a rendering
/// surface: it deliberately does NOT carry the one-time pairing credential
/// value that [`crate::content::PairRequestView`] holds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerDiagnosticView {
    /// Stable row identity: `npub` when resolved, else `node_addr_hex`, else
    /// the raw BLE address of an unresolved advert.
    pub key: String,
    /// Resolved npub; empty when not yet identified.
    pub npub: String,
    /// Resolved `node_addr` hex; empty when only a raw advert has been seen.
    pub node_addr_hex: String,
    /// Raw BLE address (`adapter/AA:BB:..`); empty unless this row was seen or
    /// attributed as a scan advert.
    pub ble_addr: String,
    /// Best-known display label (Circle name or peer-advertised display name),
    /// truncated to at most 64 characters before it crosses the FFI.
    pub name: String,
    /// One of `connected`, `reachable-via-relay`, `seen-unidentified`,
    /// `paired-offline`, `unreachable`.
    pub state: String,
    /// Transport currently carrying this row, when connected (`ble`, `aware`,
    /// `udp`, `tcp`); empty when not connected.
    pub transport: String,
    /// Other transports this peer is also reachable over, in the fixed order
    /// `ble`, `aware`, `udp`, `tcp`. Empty until Phase 2 populates it.
    pub also_reachable_via: Vec<String>,
    /// Milliseconds-since-epoch this row was last heard from; `0` when never
    /// heard from (renders as an em-dash, never "0s").
    pub last_seen_ms: u64,
    /// Milliseconds-since-epoch the FMP session with this peer authenticated;
    /// `0` when there is no session (renders as an em-dash, never "0s").
    ///
    /// The age derived from this is the session's, not the current FMP index's:
    /// `receiver_idx` rotates on every rekey (default 120s) while the session
    /// itself survives, so a long age means the link has held, not that a
    /// handshake is stale.
    pub authenticated_at_ms: u64,
    /// The display name this peer broadcasts for itself in its BLE scan
    /// response; empty when it has advertised none or was not found over BLE.
    ///
    /// **Unauthenticated** — a plaintext broadcast anyone in range can forge.
    /// It exists so a device you have never paired with can still show the name
    /// its owner chose, and it must always rank *below* any name learned from
    /// signed pair traffic, never above.
    pub advertised_name: String,
    /// Smoothed round-trip time over this peer's link, milliseconds, as MMP
    /// measured it. `None` when there is no measurement — an unmeasured link
    /// renders as an em-dash, never as a confident `0ms`.
    pub srtt_ms: Option<f64>,
    /// Signal strength from the most recent scan advert attributed to this
    /// row, dBm; `None` when no advert has been attributed.
    pub rssi: Option<i32>,
    /// Advertised listener PSM from the most recent scan advert attributed to
    /// this row; `0` when none.
    pub psm: u16,
    /// One of `""`, `incoming-waiting`, `outbound-waiting` or `paired`.
    pub pair_state: String,
    /// Whether this npub is a member of the user's Circle.
    pub in_circle: bool,
    /// BLE role this device chose for the most recent recorded attempt against
    /// this peer: `central`, `peripheral`, or empty when nothing has been
    /// recorded. Never a guess — an empty string means "no attempt recorded",
    /// not "central by default".
    pub role: String,
    /// Milliseconds between discovery and resolution for the most recent
    /// recorded attempt; `0` when none was recorded or none was measured.
    pub discovery_ms: u64,
    /// Count of sends to this peer that failed at the link. Excludes packets
    /// rejected for exceeding the MTU — that is a caller bug, not a property of
    /// this peer's link.
    pub send_drops: u64,
    /// Recorded connect attempts against this peer, newest first, capped at 20.
    /// Empty when nothing has been recorded.
    pub attempts: Vec<PeerAttemptView>,
}

/// One recorded BLE connect attempt as rendered for the Dev tab (DIAG-01/03).
///
/// Every field is an observed fact from the fips transport's attempt log — a
/// peer with no recorded history renders as having none, never as having
/// succeeded or failed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerAttemptView {
    /// Wall-clock milliseconds since the Unix epoch at which the attempt
    /// resolved.
    pub at_ms: u64,
    /// `central` or `peripheral` — which side this device took.
    pub role: String,
    /// Milliseconds from discovery to resolution; `0` when not measured.
    pub discovery_ms: u64,
    /// One of `connected`, `connect-timeout`, `connect-error`,
    /// `pubkey-exchange-failed`, `lost-tiebreaker`, `pool-rejected`.
    pub outcome: String,
}

/// Outcome of a peer speedtest: upload + download throughput measured by PUTting
/// a fresh payload to the peer's mesh Blossom and GETting it back. `generation`
/// bumps on each completion so the UI can tell a fresh result from a stale one.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedtestView {
    /// A run is in flight (the UI shows a spinner / disables the button).
    pub running: bool,
    /// The peer the latest run targeted (npub), for the result label.
    pub peer_npub: String,
    /// Payload size moved each way, in bytes.
    pub bytes: u64,
    /// Upload (this device → peer) throughput, megabits per second.
    pub up_mbps: f64,
    /// Download (peer → this device) throughput, megabits per second.
    pub down_mbps: f64,
    /// Non-empty if the last run failed (e.g. peer unreachable / not paired).
    pub error: String,
    /// Bumped on each completed run (success or failure).
    pub generation: u64,
}

/// The device identity, in the derived forms the UI shows.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    pub own_npub: String,
    pub own_pubkey_hex: String,
    pub node_addr_hex: String,
    pub fips_addr: String, // <npub>.fips
    /// This node's mesh ULA (`fd00:: = fd + node_addr[0..15]`) — the address the
    /// Android VpnService assigns to the app-owned TUN.
    pub fips_ipv6: String,
    /// FIPS's effective IPv6 MTU (`transport_mtu - 77`) — the MTU the VpnService
    /// sets on the TUN so the kernel never hands FIPS oversized packets.
    pub fips_mtu: u16,
}

impl IdentityView {
    pub fn from_identity(id: &fips::Identity) -> Self {
        let npub = id.npub();
        let fips_ipv6 = fips::FipsAddress::from_node_addr(id.node_addr())
            .to_ipv6()
            .to_string();
        Self {
            own_pubkey_hex: hex::encode(id.pubkey().serialize()),
            node_addr_hex: id.node_addr().to_string(),
            fips_addr: format!("{npub}.fips"),
            fips_ipv6,
            fips_mtu: 0, // set by the runtime from node.effective_ipv6_mtu()
            own_npub: npub,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub running: bool,
    pub status_text: String,
}

/// BLE adapter/transport status — the control/observation plane the developer
/// UI renders. The byte plane (the radio) is separate (Android, P1 M4).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleStatus {
    /// Master switch (the `SetBleEnabled` action).
    pub enabled: bool,
    /// The node is both peripheral and central — symmetric per-peer PSM
    /// discovery, not fixed-central. Informational for the UI.
    pub role: String,
    /// Whether the scan loop is currently running, sourced from the radio's own
    /// scan-callback lifecycle (never derived from other flags).
    pub scanning: bool,
    /// False when `scanning` could not actually be observed (bridge absent, radio
    /// never started, or a non-Android build) — the UI must render unknown rather
    /// than asserting a value.
    pub scanning_known: bool,
    /// Whether the advertiser is currently running, sourced from the advertise
    /// callback's own install/clear lifecycle.
    pub advertising: bool,
    /// False when `advertising` could not actually be observed — the UI must
    /// render unknown rather than asserting a value.
    pub advertising_known: bool,
    /// Adapter label (a fixed tag on Android; "—" until the backend reports).
    pub adapter_name: String,
}

/// Wi-Fi Aware bulk-lane status — the control/observation plane. The radio
/// (attach/publish/subscribe/NDP) lives in the Android foreground service;
/// the byte plane is the ordinary UDP transport over the NDP interface. See
/// `docs/design/wifi-aware-interop.md`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiAwareStatus {
    /// Master switch (the `SetWifiAwareEnabled` action).
    pub enabled: bool,
    /// The UDP port the Aware radio advertises in its
    /// service-specific info (0 while the lane is off).
    pub port: u16,
    /// Whether the Aware lane is actively discovering right now — the Aware
    /// analogue of a BLE scan, sourced from the publish/subscribe session
    /// lifecycle (`publishSession != null || subscribeSession != null`), never
    /// derived from other flags.
    pub scanning: bool,
    /// False when `scanning` could not actually be observed (Kotlin has never
    /// pushed a value) — the UI must render unknown rather than asserting a
    /// value.
    pub scanning_known: bool,
}

/// One peer seen or connected over BLE. Keyed by `node_addr` from the in-band
/// `[0x00][pubkey:32]` exchange (never the rotating MAC); `npub` resolves once
/// the Noise handshake completes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlePeer {
    pub node_addr_hex: String,
    /// Resolved once the pubkey/Noise handshake completes; empty before then.
    pub npub: String,
    pub connected: bool,
    /// Learned from the peer's advert (the `BleAddr → PSM` map).
    pub psm: u16,
    pub rssi: Option<i32>,
}

/// One discovered scan advert — the radio-level view (per BLE address), where
/// PSM and RSSI are actually known (the mesh `BlePeer` is keyed by node_addr).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleAdvert {
    /// `BleAddr` string (`adapter/AA:BB:..`); the MAC rotates with privacy.
    pub addr: String,
    /// Advertised listener PSM (0 if absent).
    pub psm: u16,
    /// Signal strength, dBm (negative).
    pub rssi: i32,
}
