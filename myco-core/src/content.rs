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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures_util::future::join_all;
use std::sync::atomic::{AtomicBool, Ordering};

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::PublicKey;
use nsite_deck::gateway::{self, Readiness};
use nsite_deck::seams::{BlobStore, PeerSource, RelayBackend};
use nsite_deck::{sync, GatewayResponse, SiteAddr, SyncOutcome};
use serde::{Deserialize, Serialize};

use myco_blossom::FsBlobStore;
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

    // --- gateway (the in-app WebView serve path) ---

    /// Serve one `<host>.nsite/<path>` request direct from the local stores.
    pub async fn gateway_get(
        &self,
        host: &str,
        path: &str,
        range: Option<&str>,
    ) -> GatewayResponse {
        gateway::serve(self.relay.as_ref(), self.blobs.as_ref(), host, path, range).await
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

        // Already complete locally? Serve direct, no fetch.
        match gateway::readiness(self.relay.as_ref(), self.blobs.as_ref(), &addr).await {
            Ok(Readiness::Ready(m)) => {
                let n = m.paths.len() as u64;
                self.set_status_titled(&addr, m.title.as_deref(), "ready", n, n, "Ready");
                // Opening a present site "installs" it (pins to Library) so it
                // persists and re-lists after an app restart.
                self.add_to_library(&addr, m.title.as_deref(), now_secs());
                return;
            }
            Ok(_) => {}
            Err(e) => {
                self.set_status(&addr, "incomplete", 0, 0, &format!("error: {e}"));
                return;
            }
        }

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

        // Live progress so the UI shows "X/Y files" instead of sitting at 0/0.
        let progress = |present: usize, total: usize| {
            self.set_status(&addr, "syncing", present as u64, total as u64, "Downloading…");
        };

        // Try each in order; the first that goes Ready wins. Keep the best
        // non-ready outcome (incomplete > unreachable) to report if none succeed.
        let mut best = SyncOutcome::Unreachable;
        for source in &sources {
            match sync::sync_site(
                self.relay.as_ref(),
                self.blobs.as_ref(),
                source.as_ref(),
                &addr,
                &progress,
            )
            .await
            {
                Ok(SyncOutcome::Ready) => {
                    let title = self.lookup_title(&addr).await;
                    let n = self.manifest_file_count(&addr).await;
                    self.set_status_titled(&addr, title.as_deref(), "ready", n, n, "Ready");
                    self.add_to_library(&addr, title.as_deref(), now_secs());
                    return;
                }
                Ok(outcome @ SyncOutcome::Incomplete { .. }) => best = outcome,
                Ok(SyncOutcome::Unreachable) => {}
                Err(e) => tracing::warn!(error = %e, "sync source errored"),
            }
        }
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

    /// Rebuild the per-site `siteStatus` from the persisted Library by checking
    /// each pinned site's readiness against the local stores. Run once at startup
    /// so "installed" sites re-list (as `ready`) after the app restarts — the
    /// relay + Blossom persist, but the in-memory status map does not.
    pub async fn refresh_library_status(self: Arc<Self>) {
        for item in self.library_snapshot() {
            let Some(addr) = library_addr(&item) else { continue };
            match gateway::readiness(self.relay.as_ref(), self.blobs.as_ref(), &addr).await {
                Ok(Readiness::Ready(m)) => {
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

    // --- wipe ---

    /// Clear the local relay + Blossom + Library + status (the `WipeStores` dev
    /// action). Content-only; identity is untouched.
    pub async fn wipe(&self) -> anyhow::Result<()> {
        self.relay.wipe().await?;
        self.blobs.wipe().await?;
        self.library.lock().unwrap().clear();
        self.sites.lock().unwrap().clear();
        self.discovered.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.library_path);
        Ok(())
    }

    // --- snapshots for state() ---

    pub fn sites_snapshot(&self) -> Vec<SiteStatusView> {
        self.sites.lock().unwrap().values().cloned().collect()
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
