//! The Myco content layer: the embedded relay + Blossom stores wired to the
//! `nsite-deck` gateway engine, plus the Library and per-site sync status the FFI
//! surfaces. This is the in-process glue (`myco-core` is the only crate that names
//! a concrete relay/Blossom). The localhost `:4869` / `:24242` sockets and the
//! `:80` external door are **not** bound in P2 — the in-app WebView reaches the
//! gateway in-process via `gateway_get` (the `gatewayGet` JNI). Peer sync over
//! those sockets is P3.
//!
//! Sync is **spawn-not-block**: `open_site` runs on the Tokio runtime and writes
//! status into `sites`; the reducer never blocks on it (Kotlin polls `siteStatus`
//! via `Tick`). See `docs/design/nsite-layer.md` and the FFI contract.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::future::join_all;
use std::sync::atomic::{AtomicBool, Ordering};

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};
use nsite_deck::gateway::{self, Readiness};
use nsite_deck::seams::{BlobStore, ManifestFilter, PeerSource, RelayBackend};
use nsite_deck::{sync, GatewayResponse, SiteAddr, SyncOutcome};
use serde::{Deserialize, Serialize};

use myco_blossom::FsBlobStore;
use myco_relay::server::{Inbound, Origin};
use myco_relay::RelayStore;

/// Per-site sync/readiness, mirroring the FFI `SiteStatus` shape.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStatusView {
    /// The `<host>` label the WebView loads (`<host>.nsite`).
    pub host: String,
    pub author_npub: String,
    pub d_tag: Option<String>,
    pub title: String,
    /// `"syncing" | "ready" | "unreachable" | "incomplete"`.
    pub state: String,
    pub files_pulled: u64,
    pub files_total: u64,
    pub message: String,
    /// A staged newer version has finished downloading but isn't active yet
    /// (deferred — meaningful once open-instance gating lands; P-U3). In P-U1 an
    /// update auto-applies, so this is only briefly true.
    pub update_available: bool,
    /// Download progress of a staging update (0/0 when none). See
    /// `docs/design/nsite-updates.md` §3.3.
    pub update_pulled: u64,
    pub update_total: u64,
}

/// Status of the most recent "check for updates" run, so the UI can give the user
/// feedback (checking → result). `generation` bumps each time a check **finishes**,
/// letting the UI fire a one-shot toast. See `docs/design/nsite-updates.md` §3.3.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckView {
    pub checking: bool,
    pub message: String,
    pub generation: u64,
}

/// A Library entry (a pinned/opened site). Persisted to `library.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub author_npub: String,
    pub d_tag: Option<String>,
    pub title: String,
    pub url_host: String,
    pub pinned: bool,
    pub added_at: u64,
}

/// A **Circle** contact: a paired peer whose device we can pull nsites from over
/// the mesh — your circle doubles as the set of relays we fetch from. Added when
/// you scan someone's share QR. Persisted to `circle.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircleContact {
    /// The contact's device npub (their mesh/pairing identity).
    pub npub: String,
    /// A human label for the contact (from the share QR; a placeholder for now).
    pub name: String,
    pub added_at: u64,
}

/// An nsite **discovered** on a Circle peer's mesh relay ("nsites around me").
/// `holder_*` is the paired peer whose relay we found it on — opening it pulls
/// from them. Ephemeral (rebuilt each discovery run; not persisted).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredNsite {
    /// The `<host>` label to open.
    pub host: String,
    pub author_npub: String,
    pub d_tag: Option<String>,
    pub title: String,
    /// Unix seconds of the manifest version we saw (its `created_at`), so the UI can
    /// show "latest version: <datetime>" — handy to tell apart same-named sites from
    /// different authors/versions.
    pub updated_at: u64,
    /// The Circle peer who has it (the relay we found it on) — the pull holder.
    pub holder_npub: String,
    pub holder_name: String,
}

/// Cache/store counts for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheView {
    pub relay_events: u64,
    pub blob_count: u64,
    pub used_bytes: u64,
}

impl CacheView {
    /// The zeroed view, used when the content layer failed to open.
    pub fn empty() -> Self {
        Self {
            relay_events: 0,
            blob_count: 0,
            used_bytes: 0,
        }
    }
}

/// An incoming pairing request awaiting the user's accept/decline (surfaced to the
/// UI as a pop-up). The requester scanned our QR; `secret` is the one-time value
/// from that QR, echoed back to prove they actually saw it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequestView {
    pub npub: String,
    pub name: String,
    pub secret: String,
}

/// Mutual-pairing handshake events, delivered point-to-point over a peer's mesh
/// relay (NOT gossiped). Both are signed by the **device** key (the pairing
/// identity) and self-destruct (NIP-40). See `docs/design/event-gossip.md`.
pub const KIND_PAIR_REQUEST: u16 = 9101;
pub const KIND_PAIR_ACCEPT: u16 = 9102;
/// Sent when a peer forgets you, so both sides drop the pairing symmetrically.
pub const KIND_PAIR_REMOVE: u16 = 9103;
const PAIR_TTL_SECS: u64 = 120;

/// Retry budget for delivering a pair request/accept to a peer's relay. A
/// just-paired BLE session can take tens of seconds to stabilise, so we re-dial
/// (re-signing each time) until it acks. ~15 × 4s ≈ a 1-minute window, well under
/// the [`PAIR_TTL_SECS`] expiration of any single (re-signed) event.
const PAIR_DIAL_ATTEMPTS: usize = 15;
const PAIR_DIAL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(4);

/// Hop budget for manifest (update) propagation over the mesh — mirrors the chat
/// push plane's default. See `docs/design/nsite-updates.md` §4.
const MANIFEST_EVENT_TTL: u8 = 3;

/// The mesh access gate backing the relay + Blossom servers: content (reads, chat,
/// manifests, blobs) is restricted to **paired** (Circle) peers, but *anyone* may
/// publish the pairing handshake so a first-time pair request can bootstrap. Holds
/// the [`Content`] so the live Circle is consulted per request (`docs/design`).
pub struct CircleGate {
    content: Arc<Content>,
}

impl CircleGate {
    pub fn new(content: Arc<Content>) -> Self {
        Self { content }
    }
}

impl myco_relay::server::PeerGate for CircleGate {
    fn may_read(&self, ip: IpAddr) -> bool {
        self.content.is_paired_ip(ip)
    }

    fn may_publish(&self, ip: IpAddr, kind: u16) -> bool {
        // Pairing handshake is allowed from anyone (it's how a peer becomes paired);
        // all other kinds require an established Circle membership.
        kind == KIND_PAIR_REQUEST
            || kind == KIND_PAIR_ACCEPT
            || kind == KIND_PAIR_REMOVE
            || self.content.is_paired_ip(ip)
    }
}

/// The content layer. Cheap to `Arc`-clone; the gateway path clones one out of the
/// `AppRuntime` mutex and serves without holding it.
pub struct Content {
    relay: Arc<RelayStore>,
    blobs: Arc<FsBlobStore>,
    /// The pull source for not-yet-present sites. `None` in P2 M2 (local only);
    /// set to the IP online-fallback source in M3, the FIPS source in P3.
    source: Mutex<Option<Arc<dyn PeerSource>>>,
    /// "Mesh-only": when true, `open_site` never uses the IP online fallback —
    /// it pulls only over the mesh (holder + connected Circle peers). Lets you
    /// verify the mesh path even when this device has internet (e.g. a hotspot).
    offline_only: AtomicBool,
    library: Mutex<Vec<LibraryItem>>,
    library_path: PathBuf,
    /// The Circle: paired peers we pull from over the mesh. Persisted.
    circle: Mutex<Vec<CircleContact>>,
    circle_path: PathBuf,
    /// npubs of currently-connected mesh peers, refreshed by the runtime each poll.
    /// `open_site` pulls from connected Circle members (bounded to who's reachable,
    /// so it never blocks on an offline contact's connect timeout).
    connected_peers: Mutex<Vec<String>>,
    /// nsites discovered on Circle peers' relays ("nsites around me"). Rebuilt by
    /// each `SearchNsites` run; ephemeral (not persisted).
    discovered: Mutex<Vec<DiscoveredNsite>>,
    /// host_label -> current sync status (drives the FFI `siteStatus`).
    sites: Mutex<HashMap<String, SiteStatusView>>,
    /// The device's Nostr keypair (the pairing identity), used to sign pair
    /// request/accept events. Set once at startup from the persisted nsec.
    device_keys: Mutex<Option<Keys>>,
    /// User-chosen device label (memorable name). Set by the app on launch and on
    /// rename; stamped on outgoing pair events so peers show the chosen name.
    /// Falls back to a name derived from the npub when unset.
    device_name_override: Mutex<Option<String>>,
    /// Incoming pair requests awaiting the user's accept/decline (UI pop-up).
    pending_pairs: Mutex<Vec<PairRequestView>>,
    /// Persistent WS connections to peers' relays, so chat fan-out doesn't pay a
    /// fresh connect per message (slow over BLE).
    peer_relays: crate::peer_relay::PeerRelayPool,
    /// host_label -> a newer version being staged (downloaded) before activation.
    /// See `docs/design/nsite-updates.md` §2. P-U1: staged outside the relay store;
    /// activation stores the manifest (making it the served version).
    pending_updates: Mutex<HashMap<String, PendingUpdate>>,
    /// Status of the latest update check, for UI feedback (checking → result).
    update_check: Mutex<UpdateCheckView>,
    /// The **active version** the gateway serves per slot — decoupled from the
    /// relay's newest, so a newer (received/checked) manifest can sit in the relay
    /// store (NIP-01-faithful, propagated to peers) while we keep serving the fully
    /// downloaded version until its replacement is staged. See
    /// `docs/design/nsite-updates.md` §1. Persisted to `active.json`.
    active_manifests: Mutex<HashMap<String, Event>>,
    active_path: PathBuf,
}

/// A [`RelayBackend`] view the **gateway** reads: it returns the core-chosen
/// **active** manifest for a slot (a version whose blobs are all local), falling
/// back to the relay's newest when we haven't pinned one. Every other call passes
/// straight through to the relay. This is what keeps a working app serving while a
/// newer manifest is still downloading. See `docs/design/nsite-updates.md` §1.
struct ActiveBackend<'a> {
    relay: &'a RelayStore,
    active: &'a Mutex<HashMap<String, Event>>,
}

#[async_trait]
impl RelayBackend for ActiveBackend<'_> {
    async fn store_event(&self, event: Event) -> anyhow::Result<bool> {
        self.relay.store_event(event).await
    }
    async fn get_manifest(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let key = manifest_key(kind, author, d_tag);
        let pinned = self.active.lock().unwrap().get(&key).cloned();
        if pinned.is_some() {
            return Ok(pinned);
        }
        self.relay.get_manifest(kind, author, d_tag).await
    }
    async fn query(&self, filter: &ManifestFilter) -> anyhow::Result<Vec<Event>> {
        self.relay.query(filter).await
    }
    async fn wipe(&self) -> anyhow::Result<()> {
        self.relay.wipe().await
    }
}

fn load_active(path: &Path) -> HashMap<String, Event> {
    let mut map = HashMap::new();
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(events) = serde_json::from_slice::<Vec<Event>>(&bytes) {
            for ev in events {
                let key = manifest_key(ev.kind.as_u16(), &ev.pubkey, event_d_tag(&ev).as_deref());
                map.insert(key, ev);
            }
        }
    }
    map
}

fn save_active(path: &Path, events: &[Event]) {
    if let Ok(json) = serde_json::to_vec(events) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}

/// A newer manifest version being downloaded in the background. Until its blobs
/// are all local it is **not** stored in the relay, so the gateway keeps serving
/// the active version (`docs/design/nsite-updates.md` §2/§5).
struct PendingUpdate {
    manifest: Event,
    total: u32,
    pulled: u32,
    /// All blobs local (download finished) — ready to activate.
    ready: bool,
}

/// The replaceable-slot key `(kind, author, d-tag)` as a string, for the staging
/// and active-version maps.
fn manifest_key(kind: u16, author: &PublicKey, d_tag: Option<&str>) -> String {
    format!("{kind}:{}:{}", author.to_hex(), d_tag.unwrap_or(""))
}

/// The `d` tag value of an event, if any.
fn event_d_tag(ev: &Event) -> Option<String> {
    ev.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d"))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

impl Content {
    /// Open the content layer under `data_dir` (relay + blossom subdirs).
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let relay = Arc::new(RelayStore::open(data_dir.join("relay"))?);
        let blobs = Arc::new(FsBlobStore::open(data_dir.join("blossom"))?);
        let library_path = data_dir.join("library.json");
        let library = load_library(&library_path);
        let circle_path = data_dir.join("circle.json");
        let circle = load_circle(&circle_path);
        let active_path = data_dir.join("active.json");
        let active_manifests = load_active(&active_path);
        Ok(Self {
            relay,
            blobs,
            source: Mutex::new(None),
            offline_only: AtomicBool::new(false),
            library: Mutex::new(library),
            library_path,
            circle: Mutex::new(circle),
            circle_path,
            connected_peers: Mutex::new(Vec::new()),
            discovered: Mutex::new(Vec::new()),
            sites: Mutex::new(HashMap::new()),
            device_keys: Mutex::new(None),
            device_name_override: Mutex::new(None),
            pending_pairs: Mutex::new(Vec::new()),
            peer_relays: crate::peer_relay::PeerRelayPool::new(),
            pending_updates: Mutex::new(HashMap::new()),
            update_check: Mutex::new(UpdateCheckView::default()),
            active_manifests: Mutex::new(active_manifests),
            active_path,
        })
    }

    /// Install the pull source (IP fallback in M3; FIPS in P3).
    pub fn set_source(&self, source: Arc<dyn PeerSource>) {
        *self.source.lock().unwrap() = Some(source);
    }

    /// Toggle "mesh-only": when on, the IP online fallback is never used.
    pub fn set_offline_only(&self, v: bool) {
        self.offline_only.store(v, Ordering::Relaxed);
    }

    pub fn is_offline_only(&self) -> bool {
        self.offline_only.load(Ordering::Relaxed)
    }

    /// The relay store (shared), for the mesh WS server.
    pub fn relay(&self) -> Arc<RelayStore> {
        self.relay.clone()
    }

    /// The blob store (shared), for the mesh Blossom server.
    pub fn blobs(&self) -> Arc<FsBlobStore> {
        self.blobs.clone()
    }

    // --- active version (what the gateway serves; docs/design/nsite-updates.md §1) ---

    /// The backend the gateway reads: serves the active (fully-downloaded) version,
    /// not necessarily the relay's newest.
    fn active_backend(&self) -> ActiveBackend<'_> {
        ActiveBackend { relay: self.relay.as_ref(), active: &self.active_manifests }
    }

    /// Pin `manifest` as the active version for its slot (atomic swap the gateway
    /// will serve) and persist. Called only once a version's blobs are all local.
    fn set_active(&self, manifest: &Event) {
        let key = manifest_key(
            manifest.kind.as_u16(),
            &manifest.pubkey,
            event_d_tag(manifest).as_deref(),
        );
        let snapshot = {
            let mut m = self.active_manifests.lock().unwrap();
            m.insert(key, manifest.clone());
            m.values().cloned().collect::<Vec<_>>()
        };
        save_active(&self.active_path, &snapshot);
    }

    // --- gateway (the in-app WebView serve path) ---

    /// Serve one `<host>.nsite/<path>` request direct from the local stores.
    pub async fn gateway_get(
        &self,
        host: &str,
        path: &str,
        range: Option<&str>,
    ) -> GatewayResponse {
        gateway::serve(&self.active_backend(), self.blobs.as_ref(), host, path, range).await
    }

    /// Serve and frame the response for the `gatewayGet` JNI: a 4-byte big-endian
    /// header length, then a JSON header (`status`, `contentType`, `headers`),
    /// then the raw body bytes. Kotlin slices the body after parsing the header.
    pub async fn gateway_get_framed(
        self: Arc<Self>,
        host: &str,
        path: &str,
        range: Option<&str>,
    ) -> Vec<u8> {
        let mut resp = self.gateway_get(host, path, range).await;
        // A 503 means the site isn't fully present yet. Replace the generic
        // loading body with the real sync status, and (re)trigger a sync if none
        // is in flight — so the loading page self-heals for a freshly scanned or
        // home-screen-launched site that hasn't been pulled yet.
        if resp.status == 503 {
            if let Some(addr) = nsite_deck::resolve_host(host) {
                let host_label = addr.host_label();
                let status = self.sites.lock().unwrap().get(&host_label).cloned();
                let syncing = status.as_ref().map(|s| s.state.as_str()) == Some("syncing");
                if !syncing {
                    // A WebView load doesn't know the holder; the IP fallback (and
                    // any earlier mesh attempt's cached result) covers the retry.
                    tokio::spawn(Arc::clone(&self).open_site(addr, None));
                }
                resp = GatewayResponse {
                    status: 503,
                    content_type: "text/html; charset=utf-8".to_string(),
                    body: loading_html(status.as_ref()).into_bytes(),
                    headers: Vec::new(),
                };
            }
        }
        frame_response(&resp)
    }

    // --- site entry ---

    /// Ensure a site is present, syncing if needed, updating its `siteStatus`.
    /// Source order (`docs/design/nsite-layer.md` §5): local → the **holder**'s
    /// relay/Blossom over the mesh (whoever shared it) → the public IP fallback.
    /// `holder` is the sharer's device npub from a share QR (`None` for a pasted
    /// link). Safe to call repeatedly; meant to be `spawn`ed, never awaited under
    /// the reducer lock.
    pub async fn open_site(self: Arc<Self>, addr: SiteAddr, holder: Option<String>) {
        self.set_status(&addr, "syncing", 0, 0, "Loading…");

        // Already complete locally? Serve direct, no fetch. If the manifest is
        // local but some blobs are missing, hold onto it: we'll fetch only the
        // missing blobs and skip the redundant manifest round-trip a full sync does.
        let known: Option<nsite_deck::Manifest> =
            match gateway::readiness(&self.active_backend(), self.blobs.as_ref(), &addr).await {
                Ok(Readiness::Ready(m)) => {
                    let n = m.paths.len() as u64;
                    self.set_active(&m.event);
                    self.set_status_titled(&addr, m.title.as_deref(), "ready", n, n, "Ready");
                    // Opening a present site "installs" it (pins to Library) so it
                    // persists and re-lists after an app restart.
                    self.add_to_library(&addr, m.title.as_deref(), now_secs());
                    return;
                }
                Ok(Readiness::Incomplete { manifest, .. }) => Some(manifest),
                Ok(Readiness::ManifestMissing) => None,
                Err(e) => {
                    self.set_status(&addr, "incomplete", 0, 0, &format!("error: {e}"));
                    return;
                }
            };

        // Ordered sources: the mesh holder first (pull from whoever shared it),
        // then any currently-connected Circle member (your paired peers double as
        // relays), then the public IP online fallback.
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        let mut tried: HashSet<String> = HashSet::new();
        if let Some(npub) = holder.as_deref() {
            if tried.insert(npub.to_string()) {
                match crate::ip_source::mesh_source_for(npub) {
                    Ok(mesh) => sources.push(Arc::new(mesh)),
                    Err(e) => tracing::warn!(error = %e, "skipping mesh source"),
                }
            }
        }
        for npub in self.connected_circle_npubs() {
            if tried.insert(npub.clone()) {
                match crate::ip_source::mesh_source_for(&npub) {
                    Ok(mesh) => sources.push(Arc::new(mesh)),
                    Err(e) => tracing::warn!(error = %e, npub, "skipping circle mesh source"),
                }
            }
        }
        // The IP online fallback — unless mesh-only is enforced.
        if !self.is_offline_only() {
            if let Some(ip) = self.source.lock().unwrap().clone() {
                sources.push(ip);
            }
        }
        if sources.is_empty() {
            self.set_status(&addr, "unreachable", 0, 0, "Can't reach anyone who has this app yet.");
            return;
        }
        tracing::info!(
            host = %addr.host_label(),
            holder = ?holder,
            sources = sources.len(),
            staged = known.is_some(),
            "open_site: syncing"
        );

        // Live progress so the UI shows "X/Y files" instead of sitting at 0/0.
        let progress = |present: usize, total: usize| {
            self.set_status(&addr, "syncing", present as u64, total as u64, "Downloading…");
        };

        // Try each in order; the first that goes Ready wins. Keep the best
        // non-ready outcome (incomplete > unreachable) to report if none succeed.
        let mut best = SyncOutcome::Unreachable;
        for source in &sources {
            // Manifest already local → fetch only its (missing) blobs, no manifest
            // refetch. Otherwise do a full sync (manifest + blobs).
            let outcome = match &known {
                Some(manifest) => {
                    sync::stage_blobs(self.blobs.as_ref(), source.as_ref(), manifest, &progress).await
                }
                None => {
                    sync::sync_site(
                        self.relay.as_ref(),
                        self.blobs.as_ref(),
                        source.as_ref(),
                        &addr,
                        &progress,
                    )
                    .await
                }
            };
            match outcome {
                Ok(SyncOutcome::Ready) => {
                    match &known {
                        // The manifest was already local — it's complete now. Ensure
                        // it's stored (idempotent) and pin it as the active version;
                        // title/count come straight from the manifest we held.
                        Some(m) => {
                            let _ = self.relay.store_event(m.event.clone()).await;
                            self.set_active(&m.event);
                            let n = m.paths.len() as u64;
                            self.set_status_titled(&addr, m.title.as_deref(), "ready", n, n, "Ready");
                            self.add_to_library(&addr, m.title.as_deref(), now_secs());
                        }
                        // Full sync stored the just-fetched manifest (the relay's
                        // newest) — pull it back to make it the active version.
                        None => {
                            let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
                            if let Ok(Some(ev)) = self
                                .relay
                                .get_manifest(kind, &addr.author, addr.d_tag.as_deref())
                                .await
                            {
                                self.set_active(&ev);
                            }
                            let title = self.lookup_title(&addr).await;
                            let n = self.manifest_file_count(&addr).await;
                            self.set_status_titled(&addr, title.as_deref(), "ready", n, n, "Ready");
                            self.add_to_library(&addr, title.as_deref(), now_secs());
                        }
                    }
                    tracing::info!(host = %addr.host_label(), "open_site: ready");
                    return;
                }
                Ok(outcome @ SyncOutcome::Incomplete { .. }) => best = outcome,
                Ok(SyncOutcome::Unreachable) => {}
                Err(e) => tracing::warn!(error = %e, "sync source errored"),
            }
        }
        tracing::info!(host = %addr.host_label(), outcome = ?best, "open_site: not ready (will retry)");
        match best {
            SyncOutcome::Incomplete { present, total } => self.set_status(
                &addr,
                "incomplete",
                present as u64,
                total as u64,
                "This app didn't download completely. Try again.",
            ),
            _ => self.set_status(
                &addr,
                "unreachable",
                0,
                0,
                "Can't reach anyone who has this app yet.",
            ),
        }
    }

    /// Import an externally-authored site from a bundle dir: `manifest.json` (the
    /// signed event) + a `blobs/` subdir of sha256-named files. The dev side-load.
    pub async fn import_dir(&self, dir: &Path) -> anyhow::Result<SyncOutcome> {
        let manifest_json = std::fs::read_to_string(dir.join("manifest.json"))?;
        let event: nostr::Event = serde_json::from_str(&manifest_json)?;
        let blobs_dir = dir.join("blobs");
        let mut blobs = Vec::new();
        if blobs_dir.is_dir() {
            for entry in std::fs::read_dir(&blobs_dir)?.filter_map(Result::ok) {
                if entry.path().is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let bytes = std::fs::read(entry.path())?;
                    blobs.push((name, bytes));
                }
            }
        }
        let outcome =
            sync::import_site(self.relay.as_ref(), self.blobs.as_ref(), event.clone(), &blobs)
                .await?;
        // Surface the imported site as `ready` (and pin it) so the UI can open it
        // with one tap and it persists across restarts.
        if outcome == SyncOutcome::Ready {
            if let Ok(manifest) = nsite_deck::Manifest::from_event(event) {
                let addr = SiteAddr {
                    author: manifest.author,
                    d_tag: manifest.d_tag.clone(),
                };
                let n = manifest.paths.len() as u64;
                self.set_active(&manifest.event);
                self.set_status_titled(&addr, manifest.title.as_deref(), "ready", n, n, "Ready");
                self.add_to_library(&addr, manifest.title.as_deref(), now_secs());
            }
        }
        Ok(outcome)
    }

    /// Import from in-memory artifacts (the IP-sync mirror path / tests).
    pub async fn import_event_blobs(
        &self,
        event: nostr::Event,
        blobs: &[(String, Vec<u8>)],
    ) -> anyhow::Result<SyncOutcome> {
        sync::import_site(self.relay.as_ref(), self.blobs.as_ref(), event, blobs).await
    }

    // --- library ---

    pub fn add_to_library(&self, addr: &SiteAddr, title: Option<&str>, added_at: u64) {
        let mut lib = self.library.lock().unwrap();
        let npub = addr.author.to_bech32().unwrap_or_default();
        if let Some(item) = lib
            .iter_mut()
            .find(|i| i.author_npub == npub && i.d_tag == addr.d_tag)
        {
            item.pinned = true;
            if let Some(t) = title {
                item.title = t.to_string();
            }
        } else {
            lib.push(LibraryItem {
                author_npub: npub,
                d_tag: addr.d_tag.clone(),
                title: title.unwrap_or("").to_string(),
                url_host: addr.host_label(),
                pinned: true,
                added_at,
            });
        }
        let snapshot = lib.clone();
        drop(lib);
        save_library(&self.library_path, &snapshot);
    }

    pub fn remove_from_library(&self, addr: &SiteAddr) {
        let npub = addr.author.to_bech32().unwrap_or_default();
        let mut lib = self.library.lock().unwrap();
        lib.retain(|i| !(i.author_npub == npub && i.d_tag == addr.d_tag));
        let snapshot = lib.clone();
        drop(lib);
        save_library(&self.library_path, &snapshot);
    }

    /// Forget a single nsite: drop it from the Library *and* its live status entry
    /// so it vanishes from the Apps grid immediately and does not re-list on the
    /// next launch. Cached blobs/events are left for the global eviction pass (P5);
    /// this is the per-app "remove" the user reaches via the app's long-press sheet.
    pub fn forget_site(&self, addr: &SiteAddr) {
        self.remove_from_library(addr);
        self.sites.lock().unwrap().remove(&addr.host_label());
        // Drop the active-version pin too (next open re-evaluates from the relay).
        let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
        let key = manifest_key(kind, &addr.author, addr.d_tag.as_deref());
        let snapshot = {
            let mut m = self.active_manifests.lock().unwrap();
            m.remove(&key);
            m.values().cloned().collect::<Vec<_>>()
        };
        save_active(&self.active_path, &snapshot);
    }

    /// Rebuild the per-site `siteStatus` from the persisted Library by checking
    /// each pinned site's readiness against the local stores. Run once at startup
    /// so "installed" sites re-list (as `ready`) after the app restarts — the
    /// relay + Blossom persist, but the in-memory status map does not.
    pub async fn refresh_library_status(self: Arc<Self>) {
        for item in self.library_snapshot() {
            let Some(addr) = library_addr(&item) else { continue };
            match gateway::readiness(&self.active_backend(), self.blobs.as_ref(), &addr).await {
                Ok(Readiness::Ready(m)) => {
                    // Bootstrap/refresh the active pointer to the served version, so
                    // a later received candidate can't divert the gateway to a
                    // not-yet-downloaded manifest.
                    self.set_active(&m.event);
                    let n = m.paths.len() as u64;
                    let title = m.title.as_deref().filter(|t| !t.is_empty());
                    self.set_status_titled(
                        &addr,
                        title.or(Some(item.title.as_str())),
                        "ready",
                        n,
                        n,
                        "Ready",
                    );
                }
                Ok(Readiness::Incomplete { present, total, .. }) => self.set_status_titled(
                    &addr,
                    Some(item.title.as_str()),
                    "incomplete",
                    present as u64,
                    total as u64,
                    "Needs re-download",
                ),
                Ok(Readiness::ManifestMissing) => self.set_status_titled(
                    &addr,
                    Some(item.title.as_str()),
                    "unreachable",
                    0,
                    0,
                    "Not downloaded yet",
                ),
                Err(_) => {}
            }
        }
    }

    // --- circle (paired peers we pull from) ---

    /// Add (or rename) a paired peer in the Circle. Idempotent by npub.
    pub fn add_to_circle(&self, npub: &str, name: &str) {
        if npub.is_empty() {
            return;
        }
        let mut circle = self.circle.lock().unwrap();
        if let Some(c) = circle.iter_mut().find(|c| c.npub == npub) {
            if !name.is_empty() {
                c.name = name.to_string();
            }
        } else {
            circle.push(CircleContact {
                npub: npub.to_string(),
                name: name.to_string(),
                added_at: now_secs(),
            });
        }
        let snapshot = circle.clone();
        drop(circle);
        save_circle(&self.circle_path, &snapshot);
    }

    /// Forget a peer (remove from the Circle).
    pub fn remove_from_circle(&self, npub: &str) {
        let mut circle = self.circle.lock().unwrap();
        circle.retain(|c| c.npub != npub);
        let snapshot = circle.clone();
        drop(circle);
        save_circle(&self.circle_path, &snapshot);
    }

    pub fn circle_snapshot(&self) -> Vec<CircleContact> {
        self.circle.lock().unwrap().clone()
    }

    /// Record which mesh peers are connected right now (called by the runtime from
    /// the node's peer snapshot), so `open_site` only tries reachable Circle members.
    pub fn set_connected_peers(&self, npubs: Vec<String>) {
        *self.connected_peers.lock().unwrap() = npubs;
    }

    /// Circle members that are connected mesh peers right now — the pull sources
    /// to try (besides an explicit holder) without risking an offline timeout, and
    /// the fan-out targets for outbound chat events (`docs/design/event-gossip.md`).
    pub fn connected_circle_npubs(&self) -> Vec<String> {
        let connected = self.connected_peers.lock().unwrap();
        self.circle
            .lock()
            .unwrap()
            .iter()
            .filter(|c| connected.iter().any(|n| n == &c.npub))
            .map(|c| c.npub.clone())
            .collect()
    }

    /// Whether `ip` is the mesh ULA of a **current** Circle member. The relay +
    /// Blossom access gates call this per request, so adding/removing a peer at
    /// runtime takes effect immediately (no cached set). A peer's ULA is
    /// `fd…+node_addr[0..15]` — `PeerIdentity::from_npub(npub).address()` — which is
    /// exactly the source address the mesh sockets see.
    pub fn is_paired_ip(&self, ip: IpAddr) -> bool {
        let IpAddr::V6(v6) = ip else { return false };
        self.circle.lock().unwrap().iter().any(|c| {
            fips::PeerIdentity::from_npub(&c.npub)
                .map(|p| p.address().to_ipv6() == v6)
                .unwrap_or(false)
        })
    }

    /// Library sites worth (re)trying right now: not yet `ready`, and not already
    /// `syncing` (an attempt is in flight). Skipping the in-flight ones is what lets
    /// a caller poll this every tick without piling on duplicate syncs — it re-tries
    /// roughly once per attempt-duration. Used to pull from a holder that just became
    /// reachable (a sharer who paired) or any newly-connected Circle peer.
    pub fn retriable_library_addrs(&self) -> Vec<SiteAddr> {
        // Snapshot the library first (releasing its lock) before taking `sites`, so
        // the two mutexes are never held nested.
        let lib = self.library_snapshot();
        let sites = self.sites.lock().unwrap();
        lib.iter()
            .filter_map(library_addr)
            .filter(|addr| {
                !matches!(
                    sites.get(&addr.host_label()).map(|s| s.state.as_str()),
                    Some("ready") | Some("syncing")
                )
            })
            .collect()
    }

    /// Connected Circle members as `(npub, name)` — discovery targets.
    fn connected_circle_contacts(&self) -> Vec<(String, String)> {
        let connected = self.connected_peers.lock().unwrap();
        self.circle
            .lock()
            .unwrap()
            .iter()
            .filter(|c| connected.iter().any(|n| n == &c.npub))
            .map(|c| (c.npub.clone(), c.name.clone()))
            .collect()
    }

    // --- pairing (mutual handshake over the mesh) ---

    /// Set the device keypair (the pairing identity) from the persisted nsec.
    pub fn set_device_keys(&self, nsec: &str) {
        match Keys::parse(nsec) {
            Ok(keys) => *self.device_keys.lock().unwrap() = Some(keys),
            Err(e) => tracing::warn!(error = %e, "pairing: bad device nsec"),
        }
    }

    pub fn pending_pairs_snapshot(&self) -> Vec<PairRequestView> {
        self.pending_pairs.lock().unwrap().clone()
    }

    /// Override the device label shown to peers (the app's memorable name). Empty
    /// clears the override (falls back to the npub-derived name).
    pub fn set_device_name(&self, name: &str) {
        let trimmed = name.trim();
        *self.device_name_override.lock().unwrap() =
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
    }

    /// Our own device label, sent to the peer so their pop-up / Circle entry has a
    /// name. Prefers the user-chosen override, else a name derived from the npub.
    fn device_name(&self) -> String {
        if let Some(name) = self.device_name_override.lock().unwrap().clone() {
            if !name.trim().is_empty() {
                return name;
            }
        }
        let npub = self
            .device_keys
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|k| k.public_key().to_bech32().ok())
            .unwrap_or_default();
        short_name(&npub)
    }

    /// Route an incoming pair event (the gossiper hands us the pair kinds; they are
    /// point-to-point and never gossiped). A **request** surfaces a pop-up; an
    /// **accept** means a peer accepted *our* request → add them to the Circle.
    pub fn handle_pair_event(&self, event: &Event) {
        let Ok(from) = event.pubkey.to_bech32() else { return };
        let name = tag_value(event, "n").unwrap_or_else(|| short_name(&from));
        match event.kind.as_u16() {
            KIND_PAIR_REQUEST => {
                tracing::info!(from = %from, "pair: request received (awaiting accept)");
                let secret = tag_value(event, "secret").unwrap_or_default();
                let mut pending = self.pending_pairs.lock().unwrap();
                if !pending.iter().any(|p| p.npub == from) {
                    pending.push(PairRequestView { npub: from, name, secret });
                }
            }
            KIND_PAIR_ACCEPT => {
                tracing::info!(from = %from, "pair: our request accepted — added to circle");
                self.add_to_circle(&from, &name);
                self.pending_pairs.lock().unwrap().retain(|p| p.npub != from);
            }
            KIND_PAIR_REMOVE => {
                tracing::info!(from = %from, "pair: peer unpaired — removing from circle");
                self.remove_from_circle(&from);
                self.pending_pairs.lock().unwrap().retain(|p| p.npub != from);
            }
            _ => {}
        }
    }

    /// Scanned a peer's QR: send a signed pair request to their mesh relay. We do
    /// not add them yet — only a mutual accept pairs both sides.
    pub async fn send_pair_request(&self, target_npub: &str, secret: &str) {
        self.dial_pair_event(target_npub, KIND_PAIR_REQUEST, secret).await;
    }

    /// Accept an incoming request: add the requester to our Circle and send them a
    /// signed accept so they add us too.
    pub async fn accept_pair_request(&self, npub: &str, name: &str) {
        self.add_to_circle(npub, name);
        self.pending_pairs.lock().unwrap().retain(|p| p.npub != npub);
        self.dial_pair_event(npub, KIND_PAIR_ACCEPT, "").await;
    }

    /// Decline an incoming request (drop it; no signal back).
    pub fn decline_pair_request(&self, npub: &str) {
        self.pending_pairs.lock().unwrap().retain(|p| p.npub != npub);
    }

    /// Tell a peer we've forgotten them, so they drop us from their Circle too.
    /// Best-effort: only lands if they're reachable (the local removal already
    /// happened synchronously in `remove_from_circle`).
    pub async fn send_unpair(&self, npub: &str) {
        self.dial_pair_event(npub, KIND_PAIR_REMOVE, "").await;
    }

    /// Build + sign a pair event and publish it to the target's mesh relay,
    /// **retrying** until the relay acks or we give up. A freshly-paired BLE session
    /// is flaky (handshake collisions, "connection not ready"), so a single
    /// fire-and-forget dial often misses — leaving the Circles asymmetric, which the
    /// access gate then turns into a hard "can't see their apps" failure. Each
    /// attempt rebuilds (re-signs) the event so its NIP-40 expiration stays fresh
    /// across the retry window.
    async fn dial_pair_event(&self, target_npub: &str, kind: u16, secret: &str) {
        let Some(keys) = self.device_keys.lock().unwrap().clone() else {
            tracing::warn!("pairing: device keys not set");
            return;
        };
        let name = self.device_name();
        let peer = match fips::PeerIdentity::from_npub(target_npub) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target_npub, error = %e, "pairing: bad target npub");
                return;
            }
        };
        let url = format!("ws://[{}]:4869", peer.address().to_ipv6());
        for attempt in 0..PAIR_DIAL_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(PAIR_DIAL_RETRY_DELAY).await;
            }
            let Some(event) = build_pair_event(&keys, kind, target_npub, &name, secret) else {
                tracing::warn!(target_npub, "pairing: could not build event");
                return;
            };
            if crate::ip_source::publish_event(&url, &event, 0, std::time::Duration::from_secs(10))
                .await
            {
                tracing::info!(target = %target_npub, kind, attempt, "pair: dial delivered");
                return;
            }
            tracing::debug!(target = %target_npub, kind, attempt, "pair: dial not delivered, retrying");
        }
        tracing::warn!(
            target = %target_npub,
            kind,
            "pair: gave up delivering after retries (session never came up)"
        );
    }

    // --- backlog pull (the read counterpart of fan-out) ---

    /// Pull recent chat (kind 9) from a peer's relay into the local store, so a
    /// freshly-opened nsite sees history it missed while the push flood is the live
    /// path. Best-effort, hard-bounded; stored events surface on the nsite's next
    /// REQ. (The efficient negentropy version is future work — needs NIP-77 on the
    /// relay.) `npub` is a connected Circle peer.
    pub async fn pull_recent_chat(&self, npub: &str) {
        let Ok(peer) = fips::PeerIdentity::from_npub(npub) else { return };
        let url = format!("ws://[{}]:4869", peer.address().to_ipv6());
        // kind 9 is the v1 chat kind (myco-bitchat); generalize when more apps land.
        let filter = serde_json::json!({ "kinds": [9], "limit": 100 });
        let events = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            crate::ip_source::query_relay(&url, filter),
        )
        .await
        {
            Ok(Ok(evs)) => evs,
            _ => return,
        };
        let mut stored = 0u32;
        for ev in events {
            if ev.verify().is_ok() && self.relay.store_event(ev).await.unwrap_or(false) {
                stored += 1;
            }
        }
        if stored > 0 {
            tracing::debug!(npub, stored, "pulled chat backlog from peer");
        }
    }

    // --- mesh fan-out ---

    /// Queue a pre-built relay frame (`["EVENT", {…}]`) to a peer's relay over a
    /// persistent pooled connection (no per-message connect). `npub` is the target
    /// Circle peer. Non-blocking.
    pub fn gossip_to_peer(&self, npub: &str, frame: String) {
        let Ok(peer) = fips::PeerIdentity::from_npub(npub) else { return };
        let url = format!("ws://[{}]:4869", peer.address().to_ipv6());
        self.peer_relays.send(npub, &url, frame);
    }

    /// Pull plane (req-ttl): forward a REQ's filters to connected Circle peers and
    /// aggregate their matching events. `req_ttl` is the remaining forward budget
    /// *after* this hop — it is stamped back into each filter so the next relay
    /// decrements from here (without it the peer would re-read the original value).
    /// `exclude` is the requester's mesh address (split-horizon). Per-peer queries
    /// run in parallel, each hard-bounded so a dead relay can't stall discovery.
    pub async fn pull_from_peers(
        &self,
        filters: Vec<serde_json::Value>,
        req_ttl: u8,
        exclude: Option<std::net::IpAddr>,
    ) -> Vec<Event> {
        // Re-stamp req-ttl (or strip it at the last hop) on each filter object.
        let filters: Vec<serde_json::Value> = filters
            .into_iter()
            .map(|mut f| {
                if let Some(obj) = f.as_object_mut() {
                    if req_ttl > 0 {
                        obj.insert("req-ttl".to_string(), serde_json::json!(req_ttl));
                    } else {
                        obj.remove("req-ttl");
                    }
                }
                f
            })
            .collect();

        let queries = self.connected_circle_npubs().into_iter().filter_map(|npub| {
            let peer = fips::PeerIdentity::from_npub(&npub).ok()?;
            let ip = std::net::IpAddr::V6(peer.address().to_ipv6());
            if exclude == Some(ip) {
                return None;
            }
            let url = format!("ws://[{}]:4869", ip);
            let filters = filters.clone();
            Some(async move {
                let mut out = Vec::new();
                for f in &filters {
                    if let Ok(Ok(evs)) = tokio::time::timeout(
                        std::time::Duration::from_secs(8),
                        crate::ip_source::query_relay(&url, f.clone()),
                    )
                    .await
                    {
                        out.extend(evs);
                    }
                }
                out
            })
        });

        join_all(queries)
            .await
            .into_iter()
            .flatten()
            .filter(|e: &Event| e.verify().is_ok())
            .collect()
    }

    // --- discovery ("nsites around me") ---

    /// Discover nsites on connected Circle peers' relays: query each reachable
    /// member's mesh relay (`ws://[fd00::peer]:4869`) for manifest events in
    /// parallel, then rebuild the discovered list. Spawn-not-block; the UI polls
    /// `discovered`. Opening a result pulls from that peer (its npub is the holder).
    pub async fn discover_from_circle(self: Arc<Self>) {
        let members = self.connected_circle_contacts();
        let queries = members.into_iter().map(|(npub, name)| async move {
            let Ok(peer) = fips::PeerIdentity::from_npub(&npub) else {
                return Vec::new();
            };
            let relay_url = format!("ws://[{}]:4869", peer.address().to_ipv6());
            let events = crate::ip_source::discover_manifests(
                &relay_url,
                std::time::Duration::from_secs(15),
                200,
            )
            .await;
            events
                .into_iter()
                .filter_map(|ev| nsite_deck::Manifest::from_event(ev).ok())
                .map(|m| {
                    let addr = SiteAddr { author: m.author, d_tag: m.d_tag.clone() };
                    DiscoveredNsite {
                        host: addr.host_label(),
                        author_npub: m.author.to_bech32().unwrap_or_default(),
                        d_tag: m.d_tag,
                        title: m.title.unwrap_or_default(),
                        updated_at: m.event.created_at.as_secs(),
                        holder_npub: npub.clone(),
                        holder_name: name.clone(),
                    }
                })
                .collect::<Vec<_>>()
        });

        let results = join_all(queries).await;
        // Dedup by (host, holder): the same site may appear once per holder.
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut found = Vec::new();
        for batch in results {
            for d in batch {
                if seen.insert((d.host.clone(), d.holder_npub.clone())) {
                    found.push(d);
                }
            }
        }
        *self.discovered.lock().unwrap() = found;
    }

    pub fn discovered_snapshot(&self) -> Vec<DiscoveredNsite> {
        self.discovered.lock().unwrap().clone()
    }

    // --- nsite updates (docs/design/nsite-updates.md) ---

    /// P-U1 manual update check (online). Polls online relays for newer manifests
    /// of every Library site in **one combined REQ per relay** (deduplicated, read
    /// until EOSE), and for each newer-than-active candidate stages its blobs and
    /// activates when complete. Spawn-not-block; the UI polls `siteStatus`.
    pub async fn check_updates(self: Arc<Self>) {
        self.set_update_check(true, "Checking for updates…");
        // Tracked sites + the union of their authors (one filter covers all).
        let addrs: Vec<SiteAddr> = self
            .library_snapshot()
            .iter()
            .filter_map(library_addr)
            .collect();
        if addrs.is_empty() {
            self.finish_update_check("No apps to check");
            return;
        }
        let authors: Vec<String> = {
            let mut s: HashSet<String> = HashSet::new();
            for a in &addrs {
                s.insert(a.author.to_hex());
            }
            s.into_iter().collect()
        };

        // Query set, one combined REQ per relay read until EOSE
        // (docs/design/nsite-updates.md §3.2):
        //  - connected peers' mesh relays, carrying `req-ttl: 1` so the check reaches
        //    2 hops just like discovery (their peers' manifests come back too);
        //  - online relays, unless mesh-only is on.
        let mesh_filter = serde_json::json!({
            "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
            "authors": authors,
            "req-ttl": 1,
        });
        let online_filter = serde_json::json!({
            "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
            "authors": authors,
        });
        let mut targets: Vec<(String, serde_json::Value)> = Vec::new();
        for npub in self.connected_circle_npubs() {
            if let Ok(peer) = fips::PeerIdentity::from_npub(&npub) {
                targets.push((
                    format!("ws://[{}]:4869", peer.address().to_ipv6()),
                    mesh_filter.clone(),
                ));
            }
        }
        if !self.is_offline_only() {
            for url in crate::ip_source::default_relays() {
                targets.push((url, online_filter.clone()));
            }
        }
        if targets.is_empty() {
            self.finish_update_check("No peers or relays to check");
            return;
        }
        let mesh_count = self.connected_circle_npubs().len();
        tracing::info!(
            apps = addrs.len(),
            mesh_peers = mesh_count,
            targets = targets.len(),
            offline_only = self.is_offline_only(),
            "update check: querying"
        );
        let queries = targets.into_iter().map(|(url, f)| async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                crate::ip_source::query_relay(&url, f),
            )
            .await
            {
                Ok(Ok(evs)) => evs,
                _ => Vec::new(),
            }
        });

        // Newest verified manifest per slot across all relays.
        let mut newest: HashMap<String, Event> = HashMap::new();
        let mut received = 0usize;
        for batch in join_all(queries).await {
            received += batch.len();
            for ev in batch {
                let kind = ev.kind.as_u16();
                if kind != nsite_deck::KIND_ROOT && kind != nsite_deck::KIND_NAMED {
                    continue;
                }
                if ev.verify().is_err() {
                    continue;
                }
                let key = manifest_key(kind, &ev.pubkey, event_d_tag(&ev).as_deref());
                match newest.get(&key) {
                    Some(prev) if prev.created_at >= ev.created_at => {}
                    _ => {
                        newest.insert(key, ev);
                    }
                }
            }
        }

        // Collect candidates strictly newer than what we currently serve.
        let mut candidates: Vec<(SiteAddr, Event)> = Vec::new();
        for addr in addrs {
            let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
            let key = manifest_key(kind, &addr.author, addr.d_tag.as_deref());
            let Some(cand) = newest.get(&key) else { continue };
            // Compare against the version we actually serve (the active pointer),
            // not merely the relay's newest.
            let active_ts = self
                .active_backend()
                .get_manifest(kind, &addr.author, addr.d_tag.as_deref())
                .await
                .ok()
                .flatten()
                .map(|e| e.created_at.as_secs())
                .unwrap_or(0);
            if cand.created_at.as_secs() > active_ts {
                candidates.push((addr, cand.clone()));
            }
        }
        tracing::info!(
            received,
            slots = newest.len(),
            candidates = candidates.len(),
            "update check: results"
        );
        if candidates.is_empty() {
            self.finish_update_check("All apps are up to date");
            return;
        }

        // Download + activate each, concurrently. Reflect progress, then report.
        self.set_update_check(true, &format!("Updating {} app(s)…", candidates.len()));
        let n = candidates.len();
        let results = join_all(
            candidates
                .into_iter()
                .map(|(addr, cand)| Arc::clone(&self).stage_update(addr, cand)),
        )
        .await;
        let applied = results.iter().filter(|b| **b).count();
        let msg = if applied == n {
            format!("{applied} app(s) updated")
        } else if applied == 0 {
            "Update found, but the download failed".to_string()
        } else {
            format!("{applied} of {n} updated; some downloads failed")
        };
        self.finish_update_check(&msg);
    }

    fn set_update_check(&self, checking: bool, message: &str) {
        let mut uc = self.update_check.lock().unwrap();
        uc.checking = checking;
        uc.message = message.to_string();
    }

    /// Mark the check complete and bump `generation` so the UI fires a one-shot
    /// result toast.
    fn finish_update_check(&self, message: &str) {
        let mut uc = self.update_check.lock().unwrap();
        uc.checking = false;
        uc.message = message.to_string();
        uc.generation += 1;
    }

    pub fn update_check_snapshot(&self) -> UpdateCheckView {
        self.update_check.lock().unwrap().clone()
    }

    /// Online update path: if `candidate` is newer than what we serve, download its
    /// blobs from online sources, activate, and propagate to peers (we now hold the
    /// blobs). Returns whether it activated.
    async fn stage_update(self: Arc<Self>, addr: SiteAddr, candidate: Event) -> bool {
        let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
        let active_ts = self
            .active_backend()
            .get_manifest(kind, &addr.author, addr.d_tag.as_deref())
            .await
            .ok()
            .flatten()
            .map(|e| e.created_at.as_secs())
            .unwrap_or(0);
        if candidate.created_at.as_secs() <= active_ts {
            return false;
        }
        // Pull blobs from connected mesh peers first (closer/faster, and the only
        // option under mesh-only), then the online fallback unless mesh-only.
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        for npub in self.connected_circle_npubs() {
            if let Ok(m) = crate::ip_source::mesh_source_for(&npub) {
                sources.push(Arc::new(m));
            }
        }
        if !self.is_offline_only() {
            sources.push(Arc::new(crate::ip_source::IpPeerSource::new(
                crate::ip_source::default_relays(),
                crate::ip_source::default_blossom_servers(),
            )));
        }
        // Activation stores the manifest in the relay (so peers REQ-ing us see it)
        // and then propagates it over the mesh.
        let activated = Arc::clone(&self)
            .download_and_activate(addr, candidate.clone(), sources, true)
            .await;
        if activated {
            self.forward_manifest(&candidate, MANIFEST_EVENT_TTL.saturating_sub(1), None);
        }
        activated
    }

    /// Download `candidate`'s blobs from `sources` (in order) into Blossom, then
    /// **activate** it — pin it as the active version the gateway serves (atomic
    /// swap). `store_in_relay` also stores the manifest so peers REQ-ing us see it
    /// (the online path; the push path already has it). The active version keeps
    /// serving until the download completes. Returns whether it activated.
    async fn download_and_activate(
        self: Arc<Self>,
        addr: SiteAddr,
        candidate: Event,
        sources: Vec<Arc<dyn PeerSource>>,
        store_in_relay: bool,
    ) -> bool {
        let host = addr.host_label();
        let Ok(manifest) = nsite_deck::Manifest::from_event(candidate.clone()) else {
            return false;
        };
        let total = manifest.blob_hashes().collect::<HashSet<_>>().len() as u32;
        {
            let mut pend = self.pending_updates.lock().unwrap();
            if let Some(p) = pend.get(&host) {
                // Already staging this version or newer — leave it.
                if p.manifest.created_at >= candidate.created_at {
                    return false;
                }
            }
            pend.insert(
                host.clone(),
                PendingUpdate { manifest: candidate.clone(), total, pulled: 0, ready: false },
            );
        }
        let progress = |pulled: usize, _total: usize| {
            if let Some(p) = self.pending_updates.lock().unwrap().get_mut(&host) {
                p.pulled = pulled as u32;
            }
        };
        // Try sources in order; the first that completes the download wins.
        let mut done = false;
        for source in &sources {
            if matches!(
                nsite_deck::sync::stage_blobs(self.blobs.as_ref(), source.as_ref(), &manifest, &progress).await,
                Ok(SyncOutcome::Ready)
            ) {
                done = true;
                break;
            }
        }
        if done {
            if let Some(p) = self.pending_updates.lock().unwrap().get_mut(&host) {
                p.ready = true;
                p.pulled = p.total;
            }
            if store_in_relay {
                let _ = self.relay.store_event(candidate.clone()).await;
            }
            self.set_active(&candidate);
            let n = manifest.paths.len() as u64;
            self.set_status_titled(&addr, manifest.title.as_deref(), "ready", n, n, "Updated");
            self.pending_updates.lock().unwrap().remove(&host);
            true
        } else {
            self.pending_updates.lock().unwrap().remove(&host);
            false
        }
    }

    /// A manifest landed in our relay over the mesh (a peer's push, forwarded by
    /// the gossiper). Propagate it like any event (`docs/design/nsite-updates.md`
    /// §4); if it's one of our installed sites, download its blobs from the sender
    /// and activate. Forwarding never waits on the download for sites we don't run.
    pub async fn on_manifest_event(self: Arc<Self>, event: Event, inbound: Inbound) {
        let d = event_d_tag(&event);
        let addr = SiteAddr { author: event.pubkey, d_tag: d };

        // Forward budget (mirrors chat): originate at the default for a local
        // publish, else the ttl that rode in. Clamp so a peer can't over-extend us.
        let effective = match inbound.origin {
            Origin::Local => MANIFEST_EVENT_TTL,
            Origin::Mesh => inbound.event_ttl.unwrap_or(0),
        }
        .min(MANIFEST_EVENT_TTL);
        let out_ttl = effective.saturating_sub(1);

        if !self.is_in_library(&addr) {
            // Not our app: pure relay — pass it on at once (we won't fetch/serve it).
            if effective > 0 {
                self.forward_manifest(&event, out_ttl, inbound.sender);
            }
            return;
        }

        // Our app: best-effort download from the sender (its mesh Blossom) first,
        // then the online fallback unless mesh-only. Activate when complete.
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        if let Some(IpAddr::V6(ip)) = inbound.sender {
            sources.push(Arc::new(
                crate::ip_source::IpPeerSource::new(
                    vec![format!("ws://[{ip}]:4869")],
                    vec![format!("http://[{ip}]:24242")],
                )
                .ignoring_manifest_servers(),
            ));
        }
        if !self.is_offline_only() {
            sources.push(Arc::new(crate::ip_source::IpPeerSource::new(
                crate::ip_source::default_relays(),
                crate::ip_source::default_blossom_servers(),
            )));
        }
        // Manifest is already in our relay (NIP-01), so don't re-store.
        let _ = Arc::clone(&self)
            .download_and_activate(addr, event.clone(), sources, false)
            .await;
        // Forward regardless of download outcome so the wave never stalls (§4).
        if effective > 0 {
            self.forward_manifest(&event, out_ttl, inbound.sender);
        }
    }

    /// Fan a manifest to connected Circle peers over the push plane (carrying a
    /// decremented `event-ttl`), split-horizon. `exclude` is the peer it came from.
    fn forward_manifest(&self, manifest: &Event, out_ttl: u8, exclude: Option<IpAddr>) {
        let mut ev_json = match serde_json::to_value(manifest) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "manifest gossip: serialize failed");
                return;
            }
        };
        if let Some(obj) = ev_json.as_object_mut() {
            obj.insert("event-ttl".to_string(), serde_json::json!(out_ttl));
        }
        let frame = serde_json::json!(["EVENT", ev_json]).to_string();
        for npub in self.connected_circle_npubs() {
            let ip = match fips::PeerIdentity::from_npub(&npub) {
                Ok(p) => IpAddr::V6(p.address().to_ipv6()),
                Err(_) => continue,
            };
            if exclude == Some(ip) {
                continue;
            }
            self.gossip_to_peer(&npub, frame.clone());
        }
    }

    /// Whether a site is in our Library (we "run" it, so we're interested in its
    /// updates — download before forwarding).
    fn is_in_library(&self, addr: &SiteAddr) -> bool {
        let npub = addr.author.to_bech32().unwrap_or_default();
        self.library
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.author_npub == npub && i.d_tag == addr.d_tag)
    }

    // --- wipe ---

    /// Clear the local relay + Blossom + Library + status (the `WipeStores` dev
    /// action). Content-only; identity is untouched.
    pub async fn wipe(&self) -> anyhow::Result<()> {
        self.relay.wipe().await?;
        self.blobs.wipe().await?;
        self.library.lock().unwrap().clear();
        self.sites.lock().unwrap().clear();
        self.discovered.lock().unwrap().clear();
        self.pending_updates.lock().unwrap().clear();
        self.active_manifests.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.library_path);
        let _ = std::fs::remove_file(&self.active_path);
        Ok(())
    }

    // --- snapshots for state() ---

    pub fn sites_snapshot(&self) -> Vec<SiteStatusView> {
        let pend = self.pending_updates.lock().unwrap();
        self.sites
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|mut s| {
                if let Some(p) = pend.get(&s.host) {
                    s.update_available = p.ready;
                    s.update_pulled = p.pulled as u64;
                    s.update_total = p.total as u64;
                }
                s
            })
            .collect()
    }

    pub fn library_snapshot(&self) -> Vec<LibraryItem> {
        self.library.lock().unwrap().clone()
    }

    pub fn cache_view(&self) -> CacheView {
        CacheView {
            relay_events: self.relay.count() as u64,
            blob_count: self.blobs.count() as u64,
            used_bytes: self.blobs.total_bytes(),
        }
    }

    // --- internal helpers ---

    fn set_status(&self, addr: &SiteAddr, state: &str, pulled: u64, total: u64, msg: &str) {
        self.set_status_titled(addr, None, state, pulled, total, msg);
    }

    fn set_status_titled(
        &self,
        addr: &SiteAddr,
        title: Option<&str>,
        state: &str,
        pulled: u64,
        total: u64,
        msg: &str,
    ) {
        let host = addr.host_label();
        let mut sites = self.sites.lock().unwrap();
        let entry = sites.entry(host.clone()).or_insert_with(|| SiteStatusView {
            host: host.clone(),
            author_npub: addr.author.to_bech32().unwrap_or_default(),
            d_tag: addr.d_tag.clone(),
            title: String::new(),
            state: String::new(),
            files_pulled: 0,
            files_total: 0,
            message: String::new(),
            update_available: false,
            update_pulled: 0,
            update_total: 0,
        });
        if let Some(t) = title {
            if !t.is_empty() {
                entry.title = t.to_string();
            }
        }
        entry.state = state.to_string();
        entry.files_pulled = pulled;
        entry.files_total = total;
        entry.message = msg.to_string();
    }

    async fn lookup_title(&self, addr: &SiteAddr) -> Option<String> {
        let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
        let event = self
            .relay
            .get_manifest(kind, &addr.author, addr.d_tag.as_deref())
            .await
            .ok()??;
        nsite_deck::Manifest::from_event(event).ok()?.title
    }

    async fn manifest_file_count(&self, addr: &SiteAddr) -> u64 {
        let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
        match self
            .relay
            .get_manifest(kind, &addr.author, addr.d_tag.as_deref())
            .await
        {
            Ok(Some(event)) => nsite_deck::Manifest::from_event(event)
                .map(|m| m.paths.len() as u64)
                .unwrap_or(0),
            _ => 0,
        }
    }
}

/// A status-aware loading page for a not-yet-ready site (meta-refresh re-checks
/// the gateway every second, by which time the re-triggered sync has progressed).
/// The chrome-less "getting this app" status screen (ui-07-getting-app.svg): the
/// app's favicon inside a determinate progress ring, its title, and an X/Y file
/// count. Self-refreshes each second; the favicon (fetched first) appears early.
fn loading_html(status: Option<&SiteStatusView>) -> String {
    const CIRC: f64 = 427.3; // 2π·68, the ring circumference
    // Poll the favicon every 300ms (cycling the common paths) so the icon fades in
    // the instant its blob lands — the sync fetches it first, ahead of the 1s reload.
    const ICON_JS: &str = "<script>(function(){var i=document.getElementById('ic'),\
s=['/favicon.ico','/favicon.png','/apple-touch-icon.png'],n=0,d=false;\
i.onload=function(){if(i.naturalWidth>0){d=true;i.style.opacity=1}};\
i.onerror=function(){if(d)return;n=(n+1)%s.length;setTimeout(function(){i.src=s[n]},300)};})();</script>";
    let (title, state, present, total) = match status {
        Some(s) => (
            if s.title.is_empty() { "This app".to_string() } else { s.title.clone() },
            s.state.as_str(),
            s.files_pulled,
            s.files_total,
        ),
        None => ("This app".to_string(), "syncing", 0, 0),
    };
    // Ring fill + accent color per state.
    let frac: f64 = match state {
        "unreachable" => 0.0,
        _ if total > 0 => (present as f64 / total as f64).clamp(0.0, 1.0),
        _ => 0.06, // a small "starting" sliver when the total isn't known yet
    };
    let dash = frac * CIRC;
    let (line, color) = match state {
        "unreachable" => (
            "Can't reach anyone with this app yet — Myco keeps trying.".to_string(),
            "#64748b",
        ),
        "incomplete" => ("Didn't finish downloading — retrying…".to_string(), "#d97706"),
        "syncing" if total > 0 => (format!("Downloading · {present} of {total} files"), "#059669"),
        _ => ("Getting this app…".to_string(), "#059669"),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"refresh\" content=\"1\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{title_esc}</title>\
<style>html,body{{height:100%;margin:0}}\
body{{display:flex;flex-direction:column;align-items:center;justify-content:center;\
font-family:-apple-system,system-ui,'Segoe UI',Roboto,sans-serif;background:#fff;color:#0f172a}}\
.ring{{position:relative;width:148px;height:148px}}\
.ring svg{{transform:rotate(-90deg)}}\
.icon{{position:absolute;inset:0;margin:auto;width:76px;height:76px;border-radius:20px;object-fit:cover;background:#f1f5f9}}\
.title{{margin-top:26px;font-size:1.5rem;font-weight:800}}\
.status{{margin-top:8px;font-size:.95rem;font-weight:600;color:{color}}}\
.hint{{margin-top:40px;font-size:.85rem;color:#94a3b8}}</style></head>\
<body><div class=\"ring\">\
<svg width=\"148\" height=\"148\" viewBox=\"0 0 148 148\">\
<circle cx=\"74\" cy=\"74\" r=\"68\" fill=\"none\" stroke=\"#e2e8f0\" stroke-width=\"7\"/>\
<circle cx=\"74\" cy=\"74\" r=\"68\" fill=\"none\" stroke=\"{color}\" stroke-width=\"7\" stroke-linecap=\"round\" stroke-dasharray=\"{dash:.1} {circ:.1}\"/>\
</svg>\
<img class=\"icon\" id=\"ic\" src=\"/favicon.ico\" style=\"opacity:0;transition:opacity .3s\">\
</div>\
<div class=\"title\">{title_esc}</div>\
<div class=\"status\">{line_esc}</div>\
<div class=\"hint\">Opens in place the moment it's ready.</div>{script}</body></html>",
        title_esc = html_escape_min(&title),
        line_esc = html_escape_min(&line),
        color = color,
        dash = dash,
        circ = CIRC,
        script = ICON_JS,
    )
}

fn html_escape_min(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The framing the `gatewayGet` JNI returns: `[u32 BE header-len][header JSON][body]`.
fn frame_response(resp: &GatewayResponse) -> Vec<u8> {
    let header = serde_json::json!({
        "status": resp.status,
        "contentType": resp.content_type,
        "headers": resp.headers,
    });
    let header_bytes = serde_json::to_vec(&header).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + header_bytes.len() + resp.body.len());
    out.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&resp.body);
    out
}

/// Resolve a Library entry back to a site address (its npub may fail to parse if
/// the file was hand-edited; such entries are skipped).
fn library_addr(item: &LibraryItem) -> Option<SiteAddr> {
    let author = PublicKey::from_bech32(&item.author_npub).ok()?;
    Some(SiteAddr {
        author,
        d_tag: item.d_tag.clone(),
    })
}

/// Seconds since the Unix epoch (Library `added_at`).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// First value of the first tag named `name` (e.g. `["n", "Alice"]` → "Alice").
fn tag_value(event: &Event, name: &str) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some(name))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

/// A short device label from an npub (`Myco-xxxxxx`), the placeholder until a
/// memorable name lands.
fn short_name(npub: &str) -> String {
    format!(
        "Myco-{}",
        npub.trim_start_matches("npub1").chars().take(6).collect::<String>()
    )
}

/// Build + sign a pair-request/accept event (device key), addressed to
/// `target_npub` via a `p` tag, carrying our `n` name, the one-time `secret`
/// (request only), and a short NIP-40 expiration.
fn build_pair_event(
    keys: &Keys,
    kind: u16,
    target_npub: &str,
    our_name: &str,
    secret: &str,
) -> Option<Event> {
    let target = PublicKey::from_bech32(target_npub).ok()?;
    let exp = (now_secs() + PAIR_TTL_SECS).to_string();
    let mut tags = vec![
        Tag::parse(["p", &target.to_hex()]).ok()?,
        Tag::parse(["n", our_name]).ok()?,
        Tag::parse(["expiration", &exp]).ok()?,
    ];
    if !secret.is_empty() {
        tags.push(Tag::parse(["secret", secret]).ok()?);
    }
    EventBuilder::new(Kind::from(kind), "").tags(tags).sign_with_keys(keys).ok()
}

fn load_library(path: &Path) -> Vec<LibraryItem> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_library(path: &Path, items: &[LibraryItem]) {
    if let Ok(json) = serde_json::to_vec(items) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}

fn load_circle(path: &Path) -> Vec<CircleContact> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_circle(path: &Path, items: &[CircleContact]) {
    if let Ok(json) = serde_json::to_vec(items) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip19::ToBech32;
    use nsite_deck::testing::build_test_site;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-content-test-{}-{}", std::process::id(), tag))
    }

    /// Write a generated site to a bundle dir (`manifest.json` + `blobs/`).
    fn write_bundle(dir: &Path, site: &nsite_deck::testing::TestSite) {
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&site.manifest).unwrap(),
        )
        .unwrap();
        for (hash, bytes) in &site.blobs {
            std::fs::write(dir.join("blobs").join(hash), bytes).unwrap();
        }
    }

    #[tokio::test]
    async fn import_dir_then_serve_and_wipe() {
        let dir = tmp("e2e");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        let site = build_test_site(
            &[("/index.html", b"<h1>hi</h1>"), ("/app.js", b"console.log(1)")],
            None,
            Some("E2E"),
        );
        let host = format!("{}.nsite", site.author.to_bech32().unwrap());
        let bundle = dir.join("bundle");
        write_bundle(&bundle, &site);

        let outcome = content.import_dir(&bundle).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Ready);

        // Served direct from local stores.
        let resp = content.gateway_get(&host, "/", None).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"<h1>hi</h1>");
        let js = content.gateway_get(&host, "/app.js", None).await;
        assert_eq!(js.content_type, "text/javascript; charset=utf-8");

        assert_eq!(content.cache_view().relay_events, 1);
        assert_eq!(content.cache_view().blob_count, 2);

        // Framed response round-trips: header len → header JSON → body.
        let framed = content.clone().gateway_get_framed(&host, "/", None).await;
        let hlen = u32::from_be_bytes(framed[0..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&framed[4..4 + hlen]).unwrap();
        assert_eq!(header["status"], 200);
        assert_eq!(&framed[4 + hlen..], b"<h1>hi</h1>");

        // Wipe clears everything; the site no longer serves.
        content.wipe().await.unwrap();
        assert_eq!(content.cache_view().relay_events, 0);
        assert_eq!(content.cache_view().blob_count, 0);
        let after = content.gateway_get(&host, "/", None).await;
        assert_eq!(after.status, 503, "wiped site must not serve content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn imported_site_persists_and_relists_after_restart() {
        let dir = tmp("persist-lib");
        let _ = std::fs::remove_dir_all(&dir);

        let site = build_test_site(&[("/index.html", b"hi")], None, Some("Persisted"));
        let host = site.author.to_bech32().unwrap();
        let bundle = dir.join("bundle");
        write_bundle(&bundle, &site);

        // First run: import (auto-pins to Library).
        {
            let content = Content::open(&dir).unwrap();
            content.import_dir(&bundle).await.unwrap();
            assert_eq!(content.library_snapshot().len(), 1, "import should pin to Library");
        }

        // Restart: a fresh Content over the same dir. The status map starts empty;
        // refresh_library_status re-lists the pinned site as ready.
        let content = Arc::new(Content::open(&dir).unwrap());
        assert_eq!(content.library_snapshot().len(), 1, "Library persists on disk");
        assert!(content.sites_snapshot().is_empty(), "status map is empty before refresh");

        content.clone().refresh_library_status().await;
        let sites = content.sites_snapshot();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].state, "ready");
        assert_eq!(sites[0].host, host);
        assert_eq!(sites[0].title, "Persisted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn circle_add_remove_persists_and_filters_to_connected() {
        let dir = tmp("circle");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let content = Content::open(&dir).unwrap();
            content.add_to_circle("npub1alice", "Alice");
            content.add_to_circle("npub1bob", "Bob");
            content.add_to_circle("npub1alice", "Alice 2"); // idempotent by npub (rename)
            let snap = content.circle_snapshot();
            assert_eq!(snap.len(), 2, "two distinct contacts");
            assert_eq!(
                snap.iter().find(|c| c.npub == "npub1alice").unwrap().name,
                "Alice 2",
                "re-adding renames in place"
            );

            // Only connected members are offered as pull sources.
            content.set_connected_peers(vec!["npub1bob".to_string()]);
            assert_eq!(content.connected_circle_npubs(), vec!["npub1bob".to_string()]);

            content.remove_from_circle("npub1bob");
            assert_eq!(content.circle_snapshot().len(), 1);
        }
        // Persists across a reopen (restart).
        let content = Content::open(&dir).unwrap();
        let snap = content.circle_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].npub, "npub1alice");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_site_without_source_is_unreachable() {
        let dir = tmp("nosrc");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        // A site we don't have and no pull source installed → unreachable.
        let site = build_test_site(&[("/index.html", b"x")], None, None);
        let addr = nsite_deck::SiteAddr {
            author: site.author,
            d_tag: None,
        };
        content.clone().open_site(addr, None).await;

        let sites = content.sites_snapshot();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].state, "unreachable");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
