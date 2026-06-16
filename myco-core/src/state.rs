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

    // --- content layer (P2) ---
    /// Per-site sync/readiness, keyed by `<host>` label. Kotlin polls this after
    /// `OpenNsite` to know when to launch the fullscreen NsiteActivity.
    pub sites: Vec<crate::content::SiteStatusView>,
    /// Pinned/opened sites.
    pub library: Vec<crate::content::LibraryItem>,
    /// Local relay/Blossom counts (for the developer screen + cache view).
    pub cache: crate::content::CacheView,
}

/// The device identity, in the derived forms the UI shows.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    pub own_npub: String,
    pub own_pubkey_hex: String,
    pub node_addr_hex: String,
    pub fips_addr: String, // <npub>.fips
}

impl IdentityView {
    pub fn from_identity(id: &fips::Identity) -> Self {
        let npub = id.npub();
        Self {
            own_pubkey_hex: hex::encode(id.pubkey().serialize()),
            node_addr_hex: id.node_addr().to_string(),
            fips_addr: format!("{npub}.fips"),
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
