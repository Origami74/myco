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
    /// Incoming pair requests awaiting accept/decline (the UI shows a pop-up).
    pub pending_pair_requests: Vec<crate::content::PairRequestView>,
    /// nsites discovered on Circle peers' relays (`SearchNsites` — "around me").
    pub discovered: Vec<crate::content::DiscoveredNsite>,
    /// "Mesh-only": the IP online fallback is disabled (pull only over the mesh).
    pub offline_only: bool,
    /// Status of the latest nsite update check (feedback for "Check for updates").
    pub update_check: crate::content::UpdateCheckView,
    /// Result of the dev-menu peer speedtest (a Blossom upload+download round-trip).
    pub speedtest: SpeedtestView,
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
    /// Whether the scan loop is currently running.
    pub scanning: bool,
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
