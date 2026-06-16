use serde::Serialize;

/// One JSON snapshot per `state()` call. `rev` lets the UI skip no-op redraws;
/// `error` is empty when healthy. Mirrors `docs/reference/ffi-surface.md`,
/// narrowed to the P0 surface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub rev: u64,
    pub error: String,
    pub app_version: String,
    pub identity: IdentityView,
    pub node: NodeStatus,
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
