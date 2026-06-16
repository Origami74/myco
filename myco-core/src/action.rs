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
    /// (author-signed manifest + its blobs). Spawns the sync; does NOT launch any
    /// UI (Kotlin opens the fullscreen NsiteActivity on `ready`). Progress is
    /// observed via `siteStatus` on `Tick`. `holder` is the sharer's device npub
    /// from a scanned share QR — the mesh peer to pull from first (the public IP
    /// fallback is tried after); `None` for a plain pasted link.
    OpenNsite {
        link: String,
        #[serde(default)]
        holder: Option<String>,
    },
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
    /// Content only — the device identity (and the Circle) are untouched.
    WipeStores,

    // --- circle (paired peers) ---
    /// Add a paired peer to the **Circle**: the contact list of devices we pull
    /// nsites from over the mesh. Dispatched when a share QR is scanned. `npub` is
    /// the peer's device identity; `name` a human label from the QR.
    AddToCircle { npub: String, name: String },
    /// Forget a peer (remove from the Circle).
    RemoveFromCircle { npub: String },
}
