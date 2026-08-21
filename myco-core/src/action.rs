use serde::Deserialize;

/// Actions Kotlin dispatches into the reducer. Serialized internally-tagged:
/// `{"type": "snake_case", ...camelCaseFields}` — matching the FFI contract in
/// `docs/reference/ffi-surface.md`. P1 adds the node lifecycle and the BLE
/// master switch; site, peer, and settings actions arrive in later phases.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
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
    /// Master switch for the Wi-Fi Aware bulk lane. Gates whether the node
    /// carries a UDP transport instance (bound socket) for platform-pushed
    /// peers; the Aware radio itself lives in the Android foreground service.
    /// Flipping it while the node runs restarts the node so the transport set
    /// matches the switch. See docs/design/wifi-aware-interop.md.
    SetWifiAwareEnabled { enabled: bool },

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
    /// Forget a single nsite: remove it from the Library and the Apps grid.
    ForgetNsite { link: String },
    /// Check online relays for newer versions of installed nsites and stage/apply
    /// them (`docs/design/nsite-updates.md`). Spawn-not-block.
    CheckNsiteUpdates,
    /// Discover nsites on connected Circle peers' relays ("nsites around me"):
    /// query each reachable member's mesh relay for kind 15128/35128 manifests.
    /// Spawn-not-block; results land in `discovered`. `query` is an optional title
    /// filter (unused for now).
    SearchNsites {
        #[serde(default)]
        query: Option<String>,
    },
    /// Clear the local relay + Blossom + Library + site status (dev/test reset).
    /// Content only — the device identity (and the Circle) are untouched.
    WipeStores,
    /// Clear cached relay events + Blossom blobs **except** those backing pinned
    /// nsites (Settings → Storage → "Delete cache"). Pinned apps keep working
    /// offline; unpinned opened sites, discovered listings and staged updates go.
    WipeCache,

    // --- circle (paired peers) ---
    /// Add a paired peer to the **Circle**: the contact list of devices we pull
    /// nsites from over the mesh. Dispatched when a share QR is scanned. `npub` is
    /// the peer's device identity; `name` a human label from the QR.
    AddToCircle { npub: String, name: String },
    /// Forget a peer (remove from the Circle).
    RemoveFromCircle { npub: String },

    // --- mutual pairing ---
    /// Scanned a peer's pairing QR: send them a signed pair request over the mesh
    /// (to their relay). Pairs nobody yet — only a mutual accept adds both sides.
    /// `npub`/`name` are the scanned peer's; `secret` is the QR's one-time value.
    SendPairRequest {
        npub: String,
        name: String,
        secret: String,
    },
    /// Accept an incoming pair request: add the requester to the Circle and signal
    /// them (a pair-accept) so they add us back.
    AcceptPairRequest { npub: String, name: String },
    /// Dismiss an incoming pair request without pairing.
    DeclinePairRequest { npub: String },
    /// Withdraw an invite we sent that is still waiting, so it can be sent again.
    CancelPairInvite { npub: String },

    /// Toggle "mesh-only": when enabled, never use the public IP relay/Blossom
    /// fallback — pull only over the mesh. Lets you verify the mesh path even
    /// when this device has internet (e.g. it's acting as a hotspot).
    SetOfflineOnly { enabled: bool },

    /// Point the event store at a **custom relay**, or back at the built-in one
    /// with an empty `url`.
    ///
    /// Persisted and applied on the next launch: the backend is chosen when the
    /// content layer is constructed, so swapping it live would mean tearing down
    /// and rebuilding everything holding state on top of it. The UI says so
    /// rather than pretending the change is immediate.
    SetCustomRelay { url: String },

    /// Point the blob store at a **custom Blossom server**, or back at the
    /// built-in one with an empty `url`. Persisted and applied on the next
    /// launch, like [`NativeAppAction::SetCustomRelay`].
    SetCustomBlossom { url: String },

    /// Report how many concurrent Wi-Fi Aware data paths this chipset supports
    /// (`Characteristics.getNumberOfSupportedDataPaths()`), which is what the
    /// Aware UDP socket pool is sized to.
    ///
    /// Persisted, and applied at the next **node** start rather than now: the
    /// pool is bound when the node is built, and rebuilding it live would drop
    /// every established link — including BLE ones that have nothing to do with
    /// Aware. Kotlin pushes this whenever it can read it, which is not on every
    /// launch: the call needs API 33 (above our minSdk) and returns nothing
    /// while Wi-Fi is off. Reading last launch's answer off disk is what makes
    /// the number available at the one moment it is needed.
    SetAwareDataPaths { count: u8 },

    /// Set this device's human label (memorable name). Stamped on outgoing pair
    /// request/accept events so peers show the name the user chose. The Android
    /// app owns the value (persisted there) and re-applies it on launch.
    SetDeviceName { name: String },

    /// Dev-menu speedtest against a mesh peer: PUT a fresh payload to the peer's
    /// Blossom and GET it back, timing each leg. Spawn-not-block; the result lands
    /// in `speedtest`. `npub` is the target peer's device identity.
    SpeedtestPeer { npub: String },
}
