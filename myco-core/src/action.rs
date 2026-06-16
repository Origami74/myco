use serde::Deserialize;

/// Actions Kotlin dispatches into the reducer. Serialized internally-tagged:
/// `{"type": "snake_case", ...camelCaseFields}` — matching the FFI contract in
/// `docs/reference/ffi-surface.md`. P1 adds the node lifecycle and the BLE
/// master switch; site, peer, and settings actions arrive in later phases.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum NativeAppAction {
    /// Pure read; does not bump `rev`.
    GetState,
    /// Advance time-based work; bumps `rev`. (== `refresh()`.)
    Tick,
    /// Start the embedded FIPS node (spawns its transport loops).
    StartNode,
    /// Stop the embedded FIPS node.
    StopNode,
    /// Master switch for the BLE L2CAP transport. On Android this gates whether
    /// the node brings up its BLE backend; the actual radio lives in the
    /// foreground service (P1 M4).
    SetBleEnabled { enabled: bool },

    // --- site entry / Library (P2) ---
    /// Resolve a pasted nsite link / `<host>` and drive its sync to readiness
    /// (author-signed manifest + its blobs) from the pull source. Spawns the
    /// sync; does NOT launch any UI (Kotlin opens the fullscreen NsiteActivity on
    /// `ready`). Progress is observed via `siteStatus` on `Tick`.
    OpenNsite { link: String },
    /// DEV-ONLY side-load: import an already-signed manifest + blobs from a bundle
    /// directory (`<dir>/manifest.json` + `<dir>/blobs/<sha256>`). The app never
    /// authors or signs — it only stores externally-created artifacts.
    ImportNsite { dir: String },
    /// Pin a site to the Library (exempt from eviction; eviction itself is P5).
    AddToLibrary { link: String },
    /// Unpin a site from the Library.
    RemoveFromLibrary { link: String },
    /// Query reachable relays for kind 15128/35128 ("nsites around me"). Stub in
    /// P2 (returns the empty set); real discovery lands with peer sync (P3).
    SearchNsites { query: Option<String> },
    /// Clear the local relay + Blossom + Library + site status (dev/test reset).
    /// Content only — the device identity is untouched.
    WipeStores,
}
