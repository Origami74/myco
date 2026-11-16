//! The Myco content layer: the embedded relay + Blossom stores wired to the
//! `nsite-deck` gateway engine, plus the Library and per-site sync status the FFI
//! surfaces. This is the in-process glue (`myco-core` is the only crate that names
//! a concrete relay/Blossom). The in-app WebView reaches the gateway in-process
//! via `gateway_get` (the `gatewayGet` JNI); the relay is served on loopback
//! only (`ws://127.0.0.1:4870`) and the blob store on no socket at all. Other
//! phones reach this one only through neighborhood links (`crate::neighbor`).
//!
//! Sync is **spawn-not-block**: `open_site` runs on the Tokio runtime and writes
//! status into `sites`; the reducer never blocks on it (Kotlin polls `siteStatus`
//! via `Tick`). See `docs/design/nsite/nsite-layer.md` and the FFI contract.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::future::join_all;
use std::sync::atomic::{AtomicBool, Ordering};

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::{Event, Filter, Keys, Kind, PublicKey};
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
    /// A staged newer version has finished downloading but isn't active yet
    /// (deferred — meaningful once open-instance gating lands; P-U3). In P-U1 an
    /// update auto-applies, so this is only briefly true.
    pub update_available: bool,
    /// Download progress of a staging update (0/0 when none). See
    /// `docs/design/nsite/nsite-updates.md` §3.3.
    pub update_pulled: u64,
    pub update_total: u64,
}

/// Status of the most recent "check for updates" run, so the UI can give the user
/// feedback (checking → result). `generation` bumps each time a check **finishes**,
/// letting the UI fire a one-shot toast. See `docs/design/nsite/nsite-updates.md` §3.3.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckView {
    pub checking: bool,
    pub message: String,
    pub generation: u64,
}

/// How the nsite half of an update check ended.
enum NsiteCheck {
    /// No nsites installed; nothing was asked.
    Nothing,
    /// Checked, with the message to show.
    Done(String),
}

/// A running update check's hold on the throttle, from
/// [`Content::begin_update_check`]. [`Content::check_updates_with`] takes it,
/// so a check cannot run without one, and it releases the gate when the check
/// ends — or, through `Drop`, if the check never runs or ends early (its task
/// dropped before or during a poll, or a panic inside it), so the gate can
/// never stay shut.
pub struct InFlightCheck {
    content: Arc<Content>,
    done: bool,
}

impl InFlightCheck {
    /// Release the gate and post the result.
    fn complete(mut self, message: &str) {
        self.done = true;
        self.content.release_update_check(message, false);
    }
}

impl Drop for InFlightCheck {
    fn drop(&mut self) {
        if !self.done {
            self.content
                .release_update_check("Update check was interrupted", true);
        }
    }
}

/// The napplet half of an update-check toast.
fn napplet_update_message(updated: usize, checked: usize) -> String {
    match (updated, checked) {
        (_, 0) => "no napplets to check".to_string(),
        (0, _) => "napplets are up to date".to_string(),
        (n, _) => format!("{n} napplet(s) updated"),
    }
}

/// What kind of app a Library entry is.
///
/// Defaults to [`LibraryKind::Nsite`] so every entry written before napplets
/// existed reads back as what it is, with no migration pass over
/// `library.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    /// A static site Myco serves through the gateway (NIP-5A, 15128/35128).
    #[default]
    Nsite,
    /// A program Myco hosts through the capability seam (NIP-5D, 5129/15129/35129).
    Napplet,
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
    #[serde(default)]
    pub kind: LibraryKind,
    /// Capability domains the user approved — at install review, or later on
    /// the app's sheet. Napplets only. An inbound intent cannot add to it.
    #[serde(default)]
    pub granted: Vec<String>,
    /// Capability domains the user switched **off** on the app's sheet.
    ///
    /// Kept apart from "not granted" because the two mean different things at
    /// launch: a declared domain this build newly implements is granted on open
    /// (what the user agreed to was "what it declares"), but a domain the user
    /// has said no to must stay off however plainly the napplet declares it.
    /// Without this set the sheet's switch flipped itself back on at the next
    /// launch.
    #[serde(default)]
    pub denied: Vec<String>,
    /// The pointer this was added by — the `naddr` when there was one.
    ///
    /// Kept because an `naddr` carries the author's own relay hints, and those
    /// are frequently the only relays that hold the napplet: of Myco's default
    /// relays exactly one carried the napplet this was first tested against.
    /// Reloading from a reconstructed `<npub>:<dtag>` would throw the hints
    /// away and search blind.
    #[serde(default)]
    pub pointer: String,
    /// The `requires` list the review sheet showed when this napplet was
    /// installed — what the user actually saw and agreed to. Napplets only.
    ///
    /// Bounds what a launch may widen `granted` to: a domain this build newly
    /// implements is granted at open only if it was on this list. A later
    /// manifest declaring more than was reviewed goes back through the review
    /// sheet rather than being granted on the strength of an update check the
    /// user never saw. Kotlin ignores the key.
    #[serde(default)]
    pub reviewed: Vec<String>,
    /// Put here by Myco's first-run seed as one of its preinstalled napplets,
    /// not installed by the user. Napplets only.
    ///
    /// Such an entry counts that default's expected permissions as reviewed
    /// (`DEFAULT_NAPPLETS` in `runtime.rs`), so an update from the same author
    /// that stays within them is granted without a sheet. Cleared by nothing
    /// but removal: a napplet removed and added again is the user's own.
    #[serde(default)]
    pub preinstalled: bool,
    /// The roles the served version declared with `archetype` tags — the
    /// installed-napplet catalog NAP-INTENT resolves against. Napplets only.
    ///
    /// `None` means never recorded: an entry written before NAP-INTENT, or one
    /// whose manifest was not here yet. The catalog fills it lazily from the
    /// pinned manifest (`intent::backfill_archetypes`); `Some(empty)` is a
    /// napplet that declares no role. Rewritten whenever the served version
    /// moves, from the verified manifest. Kotlin ignores the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archetypes: Option<Vec<LibraryArchetype>>,
}

/// One `["archetype", <slug>, <convention>]` a napplet declared, as the
/// Library keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryArchetype {
    pub slug: String,
    pub convention: String,
    /// The parameter names the `i` tag advertised. Absent from entries
    /// written before NAP-INTENT exposed them; those read as none until the
    /// served version next moves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
}

impl LibraryArchetype {
    /// The archetypes a verified manifest declares.
    pub fn of_manifest(manifest: &myco_napplet_runtime::NappletManifest) -> Vec<Self> {
        manifest
            .archetypes
            .iter()
            .map(|a| Self {
                slug: a.slug.clone(),
                convention: a.convention.clone(),
                params: a.params.clone(),
            })
            .collect()
    }
}

/// A napplet's grants as the Library records them: what the user allowed, what
/// the user switched off, and what the review sheet showed them. A domain in
/// neither `granted` nor `denied` was never decided — which is what lets a
/// launch grant a declared domain this build newly implements, provided it was
/// on the `reviewed` list, without overriding a decision the user did make.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NappletGrants {
    pub granted: Vec<String>,
    pub denied: Vec<String>,
    /// The declared `requires` the user reviewed at install. See
    /// [`LibraryItem::reviewed`].
    pub reviewed: Vec<String>,
}

/// Whether an installed napplet can open, for its tile.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NappletStatusView {
    /// The shell host the Library entry carries as `url_host`.
    pub host: String,
    /// `ready` when the served manifest and its index blob are both here,
    /// `missing` when either is not.
    pub state: String,
    pub message: String,
    /// The served manifest's icon hash, if it declares one: what a tile keys
    /// its icon on, so an update that changes the icon redraws it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl NappletGrants {
    /// Allow or withdraw one domain, keeping the two sets disjoint.
    pub fn set(&mut self, domain: &str, allowed: bool) {
        self.granted.retain(|d| d != domain);
        self.denied.retain(|d| d != domain);
        if allowed {
            self.granted.push(domain.to_string());
        } else {
            self.denied.push(domain.to_string());
        }
    }
}

/// Cache/store counts for the UI.
///
/// These always describe the **embedded** store and blob directory, which is
/// what occupies space on this device. Configuring a custom relay or Blossom
/// does not change these numbers — it means they stop describing what is
/// actually serving, because a remote store's size is not something NIP-01 or
/// BUD-01 can report. The `external_*` flags let the screen note that the
/// built-in store is no longer in use rather than quietly showing a figure for
/// the wrong thing. See `reference/thinning-custom-relay.md` (D4).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheView {
    pub relay_events: u64,
    pub blob_count: u64,
    pub used_bytes: u64,
    /// The shell cache (`myco-cache`): what passed through and was not kept.
    pub event_cache: myco_cache::CacheStats,
    pub blob_cache: myco_cache::CacheStats,
    /// A custom relay is configured, so the embedded event store is not serving.
    pub external_relay: bool,
    /// A custom Blossom is configured, so the embedded blob store is not
    /// serving. Separate from the relay flag because one can be swapped without
    /// the other.
    pub external_blobs: bool,
}

impl CacheView {
    /// The zeroed view, used when the content layer failed to open.
    pub fn empty() -> Self {
        Self {
            relay_events: 0,
            blob_count: 0,
            used_bytes: 0,
            event_cache: myco_cache::CacheStats::default(),
            blob_cache: myco_cache::CacheStats::default(),
            external_relay: false,
            external_blobs: false,
        }
    }
}

/// At most this many authors' NIP-65 write relays, in total, join the default
/// relays in an nsite update check — one combined REQ each, so the bound is
/// on sockets, however many authors the Library holds.
const UPDATE_CHECK_AUTHOR_RELAYS: usize = 12;

/// How long napplet-driven internet lanes are skipped after every public relay
/// failed in one round. Short: a phone walking back into Wi-Fi should not wait
/// long to notice.
pub(crate) const INTERNET_DOWN_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// The shell cache's budgets, in bytes (Settings → Storage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    pub event_bytes: u64,
    pub blob_bytes: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
        // 32-bit builds run on budget TV boxes: the event map is capped at
        // 256 MiB there (half of it usable), and a few GB of flash is typical.
        if cfg!(not(target_pointer_width = "64")) {
            return Self {
                event_bytes: 128 * 1024 * 1024,
                blob_bytes: 512 * 1024 * 1024,
            };
        }
        Self {
            event_bytes: myco_cache::DEFAULT_EVENT_CACHE_BYTES,
            blob_bytes: myco_cache::DEFAULT_BLOB_CACHE_BYTES,
        }
    }
}

/// Open a cache, and try hard not to let it be the reason the content layer
/// does not open: a cache that will not open is deleted and reopened once at
/// the default budget. It only ever holds what can be fetched again, so any
/// open error — a stale map size, a corrupt file — is worth starting afresh
/// for. A second failure is returned.
fn open_cache<T>(
    dir: &Path,
    limit: u64,
    default_limit: u64,
    open: impl Fn(&Path, u64) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match open(dir, limit) {
        Ok(cache) => Ok(cache),
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "cache would not open; starting it afresh");
            let _ = std::fs::remove_dir_all(dir);
            open(dir, default_limit)
        }
    }
}

/// At most this many events are cached from one [`Content::remember`] call.
const REMEMBER_PER_BATCH: usize = 256;

/// At most this many [`Content::remember`] writes run at once; further calls
/// are dropped.
const REMEMBER_IN_FLIGHT: usize = 2;

/// How often an open window's loading page may start a new search for a
/// site that is not here yet. The page itself reloads every second.
const LOADING_RETRY_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// The content layer. Cheap to `Arc`-clone; the gateway path clones one out of the
/// `AppRuntime` mutex and serves without holding it.
pub struct Content {
    /// The event store, through the seam — so it can be the embedded relay or
    /// any other NIP-01 relay (`reference/thinning-custom-relay.md`, D3) —
    /// read together with the event cache, and written as **kept**
    /// (`tiered.rs`).
    relay: Arc<dyn RelayBackend>,
    /// The same two stores, written as **cache**: pulled backlog, query
    /// answers nobody asked to keep.
    cache_relay: Arc<dyn RelayBackend>,
    /// The shell's event cache, apart from the relay (`myco-cache`).
    event_cache: Arc<myco_cache::EventCache>,
    /// The custom relay, when one is configured — kept so its reachability can
    /// be reported. A backend that has gone away otherwise looks like an app
    /// with no content, every site missing and no explanation.
    relay_remote: Option<Arc<crate::remote_backend::RemoteBackend>>,
    /// The embedded store, when that is what `relay` points at.
    ///
    /// Held separately because two things it answers are not NIP-01 and cannot
    /// be asked of an arbitrary relay: the usage counts the Storage screen
    /// shows, and the selective retain the cache wipe needs. `None` once a
    /// custom relay is configured, which is exactly what the screen reports.
    relay_store: Option<Arc<RelayStore>>,
    /// The blob store, through the seam — embedded or someone else's Blossom —
    /// read together with the blob cache, written as kept.
    blobs: Arc<dyn BlobStore>,
    /// The same, written as cache: blobs a napplet fetched from elsewhere.
    cache_blobs: Arc<dyn BlobStore>,
    /// The shell's blob cache, apart from the local Blossom.
    blob_cache: Arc<myco_cache::BlobCache>,
    /// The custom Blossom, when one is configured, so its reachability can be
    /// reported the same way the relay's is.
    blobs_remote: Option<Arc<crate::remote_blobs::RemoteBlobStore>>,
    /// The embedded blob store, when that is what `blobs` points at. Holds the
    /// usage counts and the selective retain, neither of which is BUD-01.
    blobs_local: Option<Arc<FsBlobStore>>,
    /// The pull source for not-yet-present sites. `None` in P2 M2 (local only);
    /// set to the IP online-fallback source in M3, the FIPS source in P3.
    source: Mutex<Option<Arc<dyn PeerSource>>>,
    /// "Mesh-only": when true, nothing is fetched over the internet — apps and
    /// their files come only from this device and its neighborhoods. Lets you
    /// verify the mesh path even when this device has internet (e.g. a hotspot).
    offline_only: AtomicBool,
    /// When the internet last looked down from here, as a moment until which
    /// napplet-driven internet lanes are skipped. See [`Content::internet_looks_down`].
    internet_down_until: Mutex<Option<std::time::Instant>>,
    library: Mutex<Vec<LibraryItem>>,
    library_path: PathBuf,
    /// Woken on every Library write, for NAP-INTENT's `intent.changed`: the
    /// catalog is the Library, so a write may change what an archetype
    /// resolves to. One waiter; a write while nobody waits leaves a permit.
    library_changed: Arc<tokio::sync::Notify>,
    /// Releases as this phone keeps a new one ([`Content::release_updates`]):
    /// announcement, file events, APK sha256.
    release_updates: tokio::sync::broadcast::Sender<(Event, Vec<Event>, String)>,
    /// Per update extension, where its files (and manifests) are looked for
    /// in the neighborhoods that get it ([`Content::set_extension_source`]).
    extension_sources: Mutex<HashMap<&'static str, Arc<dyn PeerSource>>>,
    /// Nsite updates as they are activated ([`Content::site_updates`]).
    site_updates: tokio::sync::broadcast::Sender<Event>,
    /// Napplet updates as they are pinned ([`Content::napplet_updates`]).
    napplet_updates: tokio::sync::broadcast::Sender<Event>,
    /// host_label -> current sync status (drives the FFI `siteStatus`).
    sites: Mutex<HashMap<String, SiteStatusView>>,
    /// Sites the user removed, by host label. Nothing brings one back but
    /// the user asking for it again ([`Content::unforget_site`]): not an
    /// open window's loading page reloading, not a sync already in flight
    /// when the tile was removed.
    forgotten: Mutex<std::collections::HashSet<String>>,
    /// When the loading page last started a sync per host, so a site nobody
    /// has is asked about every [`LOADING_RETRY_EVERY`] rather than on every
    /// one-second reload.
    loading_retries: Mutex<HashMap<String, std::time::Instant>>,
    /// The device keypair. Behind an `Arc` because a remote blob store needs it
    /// to sign BUD-01 upload authorizations, and it is set after construction.
    device_keys: Arc<Mutex<Option<Keys>>>,
    /// host_label -> a newer version being staged (downloaded) before activation.
    /// See `docs/design/nsite/nsite-updates.md` §2. P-U1: staged outside the relay store;
    /// activation stores the manifest (making it the served version).
    pending_updates: Mutex<HashMap<String, PendingUpdate>>,
    /// Status of the latest update check, for UI feedback (checking → result).
    update_check: Mutex<UpdateCheckView>,
    /// The throttle every update-check trigger goes through (`update_gate.rs`).
    update_gate: Mutex<crate::update_gate::UpdateGate>,
    /// The **active version** the gateway serves per slot — decoupled from the
    /// relay's newest, so a newer (received/checked) manifest can sit in the relay
    /// store (NIP-01-faithful) while we keep serving the fully
    /// downloaded version until its replacement is staged. See
    /// `docs/design/nsite/nsite-updates.md` §1. Persisted to `active.json`.
    active_manifests: Arc<Mutex<HashMap<String, Event>>>,
    active_path: PathBuf,
    /// Whether each installed napplet can open right now, keyed by shell host.
    /// Rebuilt at startup, after a cache wipe, and whenever a version is
    /// pinned — the napplet counterpart of `sites` for nsites.
    napplet_status: Mutex<HashMap<String, NappletStatusView>>,
    /// Verified napplet icon bytes by sha256, read once: the Apps grid, the
    /// sheet and the Recents card ask for the same few icons over and over,
    /// and the store behind them may be a custom Blossom server. Content
    /// addressed, so an entry is never stale; one per installed icon.
    icon_memo: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    /// Keeps profiles, relay lists and manifests seen from outside in the
    /// embedded store. `None` with a custom relay: browsing is not written to
    /// someone else's relay, where "Clear local database" could not clear it.
    keep_seen: Option<crate::keep_seen::KeepSeen>,
    /// Bounds [`Content::remember`]'s writes in flight.
    remembering: Arc<tokio::sync::Semaphore>,
}

/// A [`RelayBackend`] view the **gateway** reads: it returns the core-chosen
/// **active** manifest for a slot (a version whose blobs are all local), falling
/// back to the relay's newest when we haven't pinned one. Every other call passes
/// straight through to the relay. This is what keeps a working app serving while a
/// newer manifest is still downloading. See `docs/design/nsite/nsite-updates.md` §1.
///
/// The loopback relay reads through the same view ([`Content::pinned_relay`]),
/// so a client asking for an installed app's manifest gets the version this
/// phone has the files for, not a newer one kept in the store without them.
#[derive(Clone)]
struct ActiveBackend {
    relay: Arc<dyn RelayBackend>,
    active: Arc<Mutex<HashMap<String, Event>>>,
}

#[async_trait]
impl RelayBackend for ActiveBackend {
    async fn publish(&self, event: Event) -> anyhow::Result<()> {
        self.relay.publish(event).await
    }
    /// Serve the **pinned** version of any site that has one, rather than the
    /// newest the store holds.
    ///
    /// The substitution happens here, on the way out, because the seam no longer
    /// has a slot-shaped read to override — everything goes through `query` now.
    /// A pinned event shares its slot with the one it replaces (same kind,
    /// author, and `d` tag), but not its `created_at`, id or other tags, so a
    /// client's `since`, `ids` or tag filter can match the newer version
    /// and not the pin. The pin is served only when it matches the request
    /// itself; otherwise the slot answers nothing, as a relay holding only the
    /// pin would.
    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        let mut out = self.relay.query(filters).await?;
        let active = self.active.lock().unwrap();
        if active.is_empty() {
            return Ok(out);
        }
        for event in out.iter_mut() {
            let key = manifest_key(
                event.kind.as_u16(),
                &event.pubkey,
                event_d_tag(event).as_deref(),
            );
            if let Some(pinned) = active.get(&key) {
                *event = pinned.clone();
            }
        }
        drop(active);
        out.retain(|e| {
            filters
                .iter()
                .any(|f| f.match_event(e, nostr::filter::MatchEventOptions::new()))
        });
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }
}

/// The site's manifest from `source`, verified and parsed; `None` when the
/// source has none. The author and slot are checked here too, so a source
/// cannot answer for a different site.
async fn fetch_verified_manifest(
    source: &dyn PeerSource,
    addr: &SiteAddr,
) -> anyhow::Result<Option<nsite_deck::Manifest>> {
    let Some(event) = source
        .fetch_manifest(&addr.author, addr.d_tag.as_deref())
        .await?
    else {
        return Ok(None);
    };
    event
        .verify()
        .map_err(|e| anyhow::anyhow!("fetched manifest verification failed: {e}"))?;
    anyhow::ensure!(
        event.pubkey == addr.author
            && event.kind.as_u16() == nsite_deck::kind_for(addr.d_tag.as_deref())
            && event_d_tag(&event).as_deref() == addr.d_tag.as_deref(),
        "the source answered with a different site's manifest"
    );
    Ok(Some(nsite_deck::Manifest::from_event(event)?))
}

/// The newer of two manifests for one slot.
fn newer_manifest(
    a: Option<nsite_deck::Manifest>,
    b: Option<nsite_deck::Manifest>,
) -> Option<nsite_deck::Manifest> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.event.created_at > a.event.created_at {
            b
        } else {
            a
        }),
        (a, b) => a.or(b),
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
/// the active version (`docs/design/nsite/nsite-updates.md` §2/§5).
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
        Self::open_with_relay(data_dir, None)
    }

    /// Open with events stored on a **custom relay** instead of the embedded one.
    ///
    /// The embedded store is still opened and still occupies disk — it simply
    /// stops serving, which is what the Storage screen reports. Nothing else in
    /// the content layer changes: it reads and writes through the seam either
    /// way (`reference/thinning-custom-relay.md`, D3).
    pub fn open_with_relay(
        data_dir: &Path,
        custom: Option<Arc<crate::remote_backend::RemoteBackend>>,
    ) -> anyhow::Result<Self> {
        Self::open_with_backends(data_dir, custom, None)
    }

    /// Open with either store — or both — pointed at something we do not own.
    ///
    /// The embedded relay and blob directory are still opened and still occupy
    /// disk; they simply stop serving, which is what the Storage screen reports.
    /// The two are independent: swapping the relay leaves blobs local, and vice
    /// versa (`reference/thinning-custom-relay.md`, D3 and D9).
    pub fn open_with_backends(
        data_dir: &Path,
        custom: Option<Arc<crate::remote_backend::RemoteBackend>>,
        custom_blobs: Option<Arc<crate::remote_blobs::RemoteBlobStore>>,
    ) -> anyhow::Result<Self> {
        Self::open_with_caches(data_dir, custom, custom_blobs, CacheLimits::default())
    }

    /// [`Content::open_with_backends`] with the cache budgets from settings.
    ///
    /// The caches live on this device whatever the backends are: under
    /// `<data_dir>/cache/events` and `<data_dir>/cache/blobs`.
    pub fn open_with_caches(
        data_dir: &Path,
        custom: Option<Arc<crate::remote_backend::RemoteBackend>>,
        custom_blobs: Option<Arc<crate::remote_blobs::RemoteBlobStore>>,
        limits: CacheLimits,
    ) -> anyhow::Result<Self> {
        // A remote blob store signs its uploads with the device key, and the key
        // is loaded after construction — so share one holder rather than keeping
        // two copies that could fall out of step.
        let device_keys: Arc<Mutex<Option<Keys>>> = custom_blobs
            .as_ref()
            .map(|b| b.keys())
            .unwrap_or_else(|| Arc::new(Mutex::new(None)));
        let embedded = Arc::new(RelayStore::open(data_dir.join("relay"))?);
        let using_custom = custom.is_some();
        let kept_relay: Arc<dyn RelayBackend> = match &custom {
            Some(remote) => remote.clone(),
            None => embedded.clone(),
        };
        let event_cache = Arc::new(open_cache(
            &data_dir.join("cache").join("events"),
            limits.event_bytes,
            myco_cache::DEFAULT_EVENT_CACHE_BYTES,
            |dir, limit| myco_cache::EventCache::open(dir, limit),
        )?);
        let relay: Arc<dyn RelayBackend> = Arc::new(crate::tiered::TieredRelay::new(
            kept_relay.clone(),
            event_cache.clone(),
            crate::tiered::Tier::Kept,
        ));
        let cache_relay: Arc<dyn RelayBackend> = Arc::new(crate::tiered::TieredRelay::new(
            kept_relay,
            event_cache.clone(),
            crate::tiered::Tier::Cache {
                keep_kinds: !using_custom,
            },
        ));
        // Kept only while it is the thing serving: the usage counts and the
        // selective retain it backs describe our store, not someone else's.
        let relay_store = (!using_custom).then_some(embedded);
        // Through the kept view, so a kept event leaves the cache.
        let keep_seen = relay_store
            .is_some()
            .then(|| crate::keep_seen::KeepSeen::new(relay.clone()));

        let embedded_blobs = Arc::new(FsBlobStore::open(data_dir.join("blossom"))?);
        let kept_blobs: Arc<dyn BlobStore> = match &custom_blobs {
            Some(remote) => remote.clone(),
            None => embedded_blobs.clone(),
        };
        let blob_cache = Arc::new(open_cache(
            &data_dir.join("cache").join("blobs"),
            limits.blob_bytes,
            myco_cache::DEFAULT_BLOB_CACHE_BYTES,
            |dir, limit| myco_cache::BlobCache::open(dir, limit),
        )?);
        let blobs: Arc<dyn BlobStore> = Arc::new(crate::tiered::TieredBlobs::new(
            kept_blobs.clone(),
            blob_cache.clone(),
            crate::tiered::Tier::Kept,
        ));
        let cache_blobs: Arc<dyn BlobStore> = Arc::new(crate::tiered::TieredBlobs::new(
            kept_blobs,
            blob_cache.clone(),
            crate::tiered::Tier::Cache {
                keep_kinds: !using_custom,
            },
        ));
        let blobs_local = custom_blobs.is_none().then_some(embedded_blobs);
        let library_path = data_dir.join("library.json");
        let library = load_library(&library_path);
        // The Circle and its pairing handshake are gone; neighborhoods
        // replaced them. Their files go with them. So does file transfer
        // between phones, with its records, outbox and received files.
        for old in [
            "circle.json",
            "outbound_pairs.json",
            "file_transfers.json",
            "completed_transfers.json",
        ] {
            let _ = std::fs::remove_file(data_dir.join(old));
        }
        for old in ["file-outbox", "received"] {
            let _ = std::fs::remove_dir_all(data_dir.join(old));
        }
        let active_path = data_dir.join("active.json");
        let active_manifests = load_active(&active_path);
        Ok(Self {
            relay,
            cache_relay,
            event_cache,
            relay_remote: custom,
            relay_store,
            blobs_remote: custom_blobs,
            blobs_local,
            blobs,
            cache_blobs,
            blob_cache,
            source: Mutex::new(None),
            offline_only: AtomicBool::new(false),
            internet_down_until: Mutex::new(None),
            library: Mutex::new(library),
            library_path,
            library_changed: Arc::new(tokio::sync::Notify::new()),
            napplet_updates: tokio::sync::broadcast::channel(32).0,
            site_updates: tokio::sync::broadcast::channel(32).0,
            release_updates: tokio::sync::broadcast::channel(4).0,
            extension_sources: Mutex::new(HashMap::new()),
            sites: Mutex::new(HashMap::new()),
            forgotten: Mutex::new(std::collections::HashSet::new()),
            loading_retries: Mutex::new(HashMap::new()),
            device_keys: device_keys.clone(),
            pending_updates: Mutex::new(HashMap::new()),
            update_check: Mutex::new(UpdateCheckView::default()),
            update_gate: Mutex::new(crate::update_gate::UpdateGate::default()),
            active_manifests: Arc::new(Mutex::new(active_manifests)),
            active_path,
            napplet_status: Mutex::new(HashMap::new()),
            icon_memo: Mutex::new(HashMap::new()),
            keep_seen,
            remembering: Arc::new(tokio::sync::Semaphore::new(REMEMBER_IN_FLIGHT)),
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

    /// Whether a napplet's internet lane should be skipped right now: the
    /// user said mesh-only, or every public relay timed out a moment ago.
    ///
    /// The second is a breaker, not a setting. A phone with no route out
    /// still has DNS and TCP timeouts to pay, per relay, per call — and a
    /// napplet that fires several calls pays them several times over while
    /// its local results wait behind them. One full round of failures buys
    /// [`INTERNET_DOWN_FOR`] of skipping; the next call after that tries again.
    ///
    /// A tripped breaker also clears early the moment anything on the
    /// internet is heard from (`relay_health::internet_heard_since`) — a
    /// subscription's stream connecting, a lookup answered.
    pub fn internet_looks_down(&self) -> bool {
        if self.is_offline_only() {
            return true;
        }
        let mut until = self.internet_down_until.lock().unwrap();
        let Some(at) = *until else {
            return false;
        };
        if std::time::Instant::now() >= at {
            return false;
        }
        let tripped = at - INTERNET_DOWN_FOR;
        if crate::relay_health::internet_heard_since(tripped) {
            *until = None;
            return false;
        }
        true
    }

    /// Record how a round of internet lanes that began at `started` went.
    ///
    /// Trip the breaker only if every lane failed **and** nothing on the
    /// internet was heard from since the round began: one relay answering
    /// 502 while twenty-five others complete their handshakes is that relay's
    /// problem (the skip list's), not the internet's. Any success, or
    /// anything heard, resets it.
    pub fn note_internet_round(
        &self,
        any_succeeded: bool,
        any_tried: bool,
        started: std::time::Instant,
    ) {
        if !any_tried {
            return;
        }
        let heard = crate::relay_health::internet_heard_since(started);
        let mut until = self.internet_down_until.lock().unwrap();
        *until = if any_succeeded || heard {
            None
        } else {
            Some(std::time::Instant::now() + INTERNET_DOWN_FOR)
        };
    }

    /// Keep the profiles, relay lists and manifests among `events` — seen
    /// from outside and already verified — in the embedded store, behind the
    /// caller (`keep_seen.rs`). Nothing with a custom relay configured.
    ///
    /// Returns the write task, for tests to wait on.
    pub fn keep_seen<'a>(
        &self,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        self.keep_seen.as_ref()?.offer(events)
    }

    /// Remember `events` — seen from outside and already verified — in the
    /// shell cache: the kinds `keep_seen` keeps go to the local store (see
    /// [`Content::keep_seen`]), everything else to the cache. For answers a
    /// caller took rather than stored (a one-shot query, a pass-through pull).
    ///
    /// Spawned; the caller answers without waiting. Bounded like the
    /// keep-seen tap beside it: at most [`REMEMBER_PER_BATCH`] events per call
    /// and [`REMEMBER_IN_FLIGHT`] writes at once — a call beyond that is
    /// dropped, not queued, since a cache that misses an answer only costs a
    /// refetch. Returns the write task, for tests to wait on.
    pub fn remember<'a>(
        &self,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let events: Vec<&Event> = events.into_iter().collect();
        self.keep_seen(events.iter().copied());
        let keeps_kinds = self.keep_seen.is_some();
        let passing: Vec<Event> = events
            .into_iter()
            .filter(|e| !(keeps_kinds && crate::keep_seen::is_kept(e)))
            .filter(|e| !crate::tiered::is_private(e))
            .take(REMEMBER_PER_BATCH)
            .cloned()
            .collect();
        if passing.is_empty() {
            return None;
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let Ok(permit) = self.remembering.clone().try_acquire_owned() else {
            tracing::debug!(
                dropped = passing.len(),
                "event cache: busy; not caching this answer"
            );
            return None;
        };
        let cache = self.event_cache.clone();
        Some(runtime.spawn(async move {
            let _permit = permit;
            cache.insert(&passing).await;
        }))
    }

    /// Save both caches' indexes now, if they changed — on the way down, so
    /// the next launch rarely has to rebuild one from the store.
    pub async fn save_cache_snapshots(&self) {
        self.event_cache.save_snapshot().await;
        self.blob_cache.save_snapshot().await;
    }

    /// The event stores written as **cache** (see `tiered.rs`): what a pull
    /// brought in lands here unless it is one of the kinds kept as they pass.
    pub fn cache_relay(&self) -> Arc<dyn RelayBackend> {
        self.cache_relay.clone()
    }

    /// The blob stores written as cache: blobs a napplet fetched.
    pub fn cache_blobs(&self) -> Arc<dyn BlobStore> {
        self.cache_blobs.clone()
    }

    pub fn event_cache(&self) -> Arc<myco_cache::EventCache> {
        self.event_cache.clone()
    }

    pub fn blob_cache(&self) -> Arc<myco_cache::BlobCache> {
        self.blob_cache.clone()
    }

    /// Apply new cache budgets, evicting down to them now.
    pub async fn set_cache_limits(&self, limits: CacheLimits) {
        self.event_cache.set_limit(limits.event_bytes).await;
        self.blob_cache.set_limit(limits.blob_bytes);
    }

    /// Drop everything in both caches ("Clear cache"). The local relay and
    /// Blossom — what is kept — are untouched.
    pub async fn clear_cache(&self) -> anyhow::Result<()> {
        self.event_cache.clear().await?;
        self.blob_cache.clear().await
    }

    /// Once per launch, in the background: load each cache's index snapshot
    /// and repair it against its store (after a crash or an Android kill
    /// between snapshots).
    pub async fn start_caches(&self) {
        self.event_cache.startup().await;
        self.blob_cache.startup().await;
    }

    /// The caches' periodic upkeep, kept cheap for the battery: delete what
    /// expired — in the local relay too — evict to budget, and snapshot the
    /// indexes only if they changed. No store is walked: every kept write
    /// goes through the kept view, which drops the cached copy as it lands,
    /// and reads merge by id anyway.
    pub async fn upkeep_caches(&self) {
        if let Some(store) = &self.relay_store {
            store.sweep_expired().await;
        }
        self.event_cache.sweep_expired().await;
        self.event_cache.evict().await;
        self.blob_cache.evict();
        self.event_cache.save_snapshot().await;
        self.blob_cache.save_snapshot().await;
    }

    /// The store as the loopback relay serves it: an installed app's manifest slot reads
    /// as its **pinned** version, the one this phone has the files for, even
    /// when a newer one sits in the store. Everything else passes through.
    pub fn pinned_relay(&self) -> Arc<dyn RelayBackend> {
        Arc::new(self.active_backend())
    }

    /// The event store (shared), read together with the event cache.
    pub fn relay(&self) -> Arc<dyn RelayBackend> {
        self.relay.clone()
    }

    /// What to tell the user about the configured relay: its URL, and why it is
    /// unreachable if it is. Empty when the built-in store is in use.
    pub fn relay_health(&self) -> crate::remote_backend::BackendHealth {
        self.relay_remote
            .as_ref()
            .map(|r| r.health())
            .unwrap_or_default()
    }

    /// The embedded store, if that is what we are using. `None` once a custom
    /// relay is configured — the caller decides what an absent one means, since
    /// nothing here can be asked of an arbitrary relay.
    pub fn relay_store(&self) -> Option<Arc<RelayStore>> {
        self.relay_store.clone()
    }

    /// The blob store (shared), through the seam.
    pub fn blobs(&self) -> Arc<dyn BlobStore> {
        self.blobs.clone()
    }

    /// The embedded blob store, if that is what we are using: the usage counts,
    /// the selective retain and the release APK need the concrete one.
    pub fn blobs_local(&self) -> Option<Arc<FsBlobStore>> {
        self.blobs_local.clone()
    }

    /// What to tell the user about a configured Blossom: its URL, and why it is
    /// unreachable if it is.
    pub fn blobs_health(&self) -> crate::remote_backend::BackendHealth {
        self.blobs_remote
            .as_ref()
            .map(|b| b.health())
            .unwrap_or_default()
    }

    // --- active version (what the gateway serves; docs/design/nsite/nsite-updates.md §1) ---

    /// The backend the gateway reads: serves the active (fully-downloaded) version,
    /// not necessarily the relay's newest.
    fn active_backend(&self) -> ActiveBackend {
        ActiveBackend {
            relay: self.relay.clone(),
            active: self.active_manifests.clone(),
        }
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
        gateway::serve(
            &self.active_backend(),
            self.blobs.as_ref(),
            host,
            path,
            range,
        )
        .await
    }

    /// Serve and frame the response for the `gatewayGet` JNI: a 4-byte big-endian
    /// header length, then a JSON header (`status`, `contentType`, `headers`),
    /// then the raw body bytes. Kotlin slices the body after parsing the header.
    ///
    /// `allow_sync` decides what a 503 means. A **WebView load** passes `true`:
    /// the user asked for this site, so a missing one should start pulling and
    /// the loading page self-heals. A **passive probe** — a favicon fetch behind
    /// a grid of tiles the user has not chosen — passes `false`, because
    /// starting a sync there downloads and pins every site merely rendered on
    /// screen. See `gateway_get_framed_no_sync`.
    pub async fn gateway_get_framed(
        self: Arc<Self>,
        host: &str,
        path: &str,
        range: Option<&str>,
    ) -> Vec<u8> {
        self.gateway_get_framed_opts(host, path, range, true).await
    }

    /// [`Self::gateway_get_framed`] for passive probes: serves whatever is
    /// already local and never triggers a sync, so rendering a tile can never
    /// download or pin a site the user did not open.
    pub async fn gateway_get_framed_no_sync(
        self: Arc<Self>,
        host: &str,
        path: &str,
        range: Option<&str>,
    ) -> Vec<u8> {
        self.gateway_get_framed_opts(host, path, range, false).await
    }

    async fn gateway_get_framed_opts(
        self: Arc<Self>,
        host: &str,
        path: &str,
        range: Option<&str>,
        allow_sync: bool,
    ) -> Vec<u8> {
        let mut resp = self.gateway_get(host, path, range).await;
        // A 503 means the site isn't fully present yet. Replace the generic
        // loading body with the real sync status, and (re)trigger a sync if none
        // is in flight — so the loading page self-heals for a freshly scanned or
        // home-screen-launched site that hasn't been pulled yet.
        if resp.status == 503 && allow_sync {
            if let Some(addr) = nsite_deck::resolve_host(host) {
                let host_label = addr.host_label();
                let status = self.sites.lock().unwrap().get(&host_label).cloned();
                let syncing = status.as_ref().map(|s| s.state.as_str()) == Some("syncing");
                // The page reloads every second; a site nobody has would
                // otherwise be searched for every second, forever.
                let start = !syncing && !self.is_forgotten(&addr) && {
                    // Stamped only when a search actually starts, so a sync
                    // that just ended is retried a full interval after it
                    // began, not after the last reload that found it running.
                    let mut last = self.loading_retries.lock().unwrap();
                    let due = last
                        .get(&host_label)
                        .is_none_or(|t| t.elapsed() >= LOADING_RETRY_EVERY);
                    if due {
                        last.insert(host_label.clone(), std::time::Instant::now());
                    }
                    due
                };
                if start {
                    tokio::spawn(Arc::clone(&self).open_site(addr));
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
    /// Source order (`docs/design/nsite/nsite-layer.md` §5): local → the
    /// neighborhoods where this device gets `nsite-updates` → the public IP
    /// fallback. Safe to call repeatedly; meant to be `spawn`ed, never awaited
    /// under the reducer lock.
    pub async fn open_site(self: Arc<Self>, addr: SiteAddr) {
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

        // Ordered sources: the neighborhoods that carry nsites to us, then the
        // public IP online fallback.
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        if let Some(neighborhoods) =
            self.extension_source(crate::neighbor::extensions::NSITE_UPDATES)
        {
            sources.push(neighborhoods);
        }
        // The IP online fallback — unless mesh-only is enforced.
        if !self.is_offline_only() {
            if let Some(ip) = self.source.lock().unwrap().clone() {
                sources.push(ip);
            }
        }
        if sources.is_empty() {
            self.set_status(
                &addr,
                "unreachable",
                0,
                0,
                "Can't reach anyone who has this app yet.",
            );
            return;
        }
        tracing::info!(
            host = %addr.host_label(),
            sources = sources.len(),
            staged = known.is_some(),
            "open_site: syncing"
        );

        // Live progress so the UI shows "X/Y files" instead of sitting at 0/0.
        let progress = |present: usize, total: usize| {
            self.set_status(
                &addr,
                "syncing",
                present as u64,
                total as u64,
                "Downloading…",
            );
        };

        // An installed site's local manifest is the version it runs: fetch only
        // its missing files. A first open asks each source for the manifest
        // first, so a copy kept from browsing — possibly stale — is only the
        // fallback for a source that has the files but not the manifest.
        let installed = self.is_in_library(&addr);

        // Try each in order; the first that goes Ready wins. Keep the best
        // non-ready outcome (incomplete > unreachable) to report if none succeed.
        let mut best = SyncOutcome::Unreachable;
        for source in &sources {
            let target = match (&known, installed) {
                (Some(local), true) => Some(local.clone()),
                _ => match fetch_verified_manifest(source.as_ref(), &addr).await {
                    Ok(fresh) => newer_manifest(fresh, known.clone()),
                    Err(e) => {
                        tracing::warn!(error = %e, "sync source errored");
                        known.clone()
                    }
                },
            };
            let Some(target) = target else {
                continue;
            };
            let outcome =
                sync::stage_blobs(self.blobs.as_ref(), source.as_ref(), &target, &progress).await;
            match outcome {
                Ok(SyncOutcome::Ready) => {
                    // Every file is here: store the manifest (idempotent), and pin
                    // exactly the version whose files were just fetched — not
                    // whatever the store's newest is by now.
                    let _ = self.relay.publish(target.event.clone()).await;
                    self.set_active(&target.event);
                    let n = target.paths.len() as u64;
                    self.set_status_titled(&addr, target.title.as_deref(), "ready", n, n, "Ready");
                    self.add_to_library(&addr, target.title.as_deref(), now_secs());
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
        let outcome = sync::import_site(
            self.relay.as_ref(),
            self.blobs.as_ref(),
            event.clone(),
            &blobs,
        )
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

    // --- library ---

    pub fn add_to_library(&self, addr: &SiteAddr, title: Option<&str>, added_at: u64) {
        // A sync that finishes after the user removed the site does not pin
        // it back.
        if self.is_forgotten(addr) {
            return;
        }
        let mut lib = self.library.lock().unwrap();
        let npub = addr.author.to_bech32().unwrap_or_default();
        if let Some(item) = lib.iter_mut().find(|i| {
            i.kind == LibraryKind::Nsite && i.author_npub == npub && i.d_tag == addr.d_tag
        }) {
            item.pinned = true;
            if let Some(t) = title {
                item.title = t.to_string();
            }
        } else {
            // Matched on kind as well as `(author, d)`: an author may publish
            // an nsite and a napplet under the same `d` tag, and they are two
            // Library entries, not one entry that changes kind.
            lib.push(LibraryItem {
                author_npub: npub,
                d_tag: addr.d_tag.clone(),
                title: title.unwrap_or("").to_string(),
                url_host: addr.host_label(),
                pinned: true,
                added_at,
                kind: LibraryKind::Nsite,
                granted: Vec::new(),
                denied: Vec::new(),
                pointer: String::new(),
                reviewed: Vec::new(),
                preinstalled: false,
                archetypes: None,
            });
        }
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
    }

    /// Write the Library to disk and wake whoever watches it.
    fn persist_library(&self, snapshot: &[LibraryItem]) {
        save_library(&self.library_path, snapshot);
        self.library_changed.notify_one();
    }

    /// What is woken on every Library write. See [`Content::library_changed`]'s
    /// field.
    pub fn library_changed(&self) -> Arc<tokio::sync::Notify> {
        self.library_changed.clone()
    }

    /// Record the archetypes an installed napplet's served version declares.
    /// Returns whether anything changed; a napplet that is not installed is
    /// left alone.
    pub fn record_napplet_archetypes(
        &self,
        author_npub: &str,
        d_tag: Option<&str>,
        archetypes: Vec<LibraryArchetype>,
    ) -> bool {
        let mut lib = self.library.lock().unwrap();
        let Some(item) = lib.iter_mut().find(|i| {
            i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag
        }) else {
            return false;
        };
        if item.archetypes.as_ref() == Some(&archetypes) {
            return false;
        }
        item.archetypes = Some(archetypes);
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
        true
    }

    /// Add or update a napplet's Library entry, recording what install review
    /// granted it.
    ///
    /// Re-adding an already-installed napplet **replaces** its grants rather
    /// than merging: the review screen shows the whole set the user is agreeing
    /// to, so what they saw is what is stored. Merging would let a second
    /// install quietly accumulate capabilities across two screens neither of
    /// which showed the total. `requires` is the declared list that screen
    /// showed; it is recorded as [`LibraryItem::reviewed`].
    #[allow(clippy::too_many_arguments)]
    pub fn add_napplet_to_library(
        &self,
        author_npub: &str,
        d_tag: Option<&str>,
        title: Option<&str>,
        shell_host: &str,
        granted: Vec<String>,
        requires: Vec<String>,
        pointer: &str,
        added_at: u64,
    ) {
        let mut lib = self.library.lock().unwrap();
        if let Some(item) = lib.iter_mut().find(|i| {
            i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag
        }) {
            item.pinned = true;
            item.granted = granted;
            // A fresh review is a fresh decision: what was switched off before
            // is on the table again, and the screen showed the whole set —
            // which is also the new reviewed list.
            item.denied = Vec::new();
            item.reviewed = requires;
            item.url_host = shell_host.to_string();
            if !pointer.is_empty() {
                item.pointer = pointer.to_string();
            }
            if let Some(t) = title {
                item.title = t.to_string();
            }
        } else {
            lib.push(LibraryItem {
                author_npub: author_npub.to_string(),
                d_tag: d_tag.map(str::to_string),
                title: title.unwrap_or("").to_string(),
                url_host: shell_host.to_string(),
                pinned: true,
                added_at,
                kind: LibraryKind::Napplet,
                granted,
                denied: Vec::new(),
                pointer: pointer.to_string(),
                reviewed: requires,
                preinstalled: false,
                archetypes: None,
            });
        }
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
    }

    /// Replace a napplet's recorded grants — both decision sets. Used when an
    /// open widened them to a reviewed domain this build newly implements, and
    /// when a switch on the sheet moves a domain between the two. The reviewed
    /// list is left alone: only install review rewrites it.
    pub fn set_napplet_grants(
        &self,
        author_npub: &str,
        d_tag: Option<&str>,
        grants: NappletGrants,
    ) {
        let mut lib = self.library.lock().unwrap();
        let Some(item) = lib.iter_mut().find(|i| {
            i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag
        }) else {
            return;
        };
        item.granted = grants.granted;
        item.denied = grants.denied;
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
    }

    /// What a napplet was granted and what it was refused, or `None` for one
    /// that is not installed.
    ///
    /// An uninstalled napplet getting `None` is the safe answer, not an
    /// oversight: it still opens, and gets nothing but the mandatory handshake
    /// — and, unlike an installed one, nothing it declares is granted at open.
    pub fn napplet_grants(&self, author_npub: &str, d_tag: Option<&str>) -> Option<NappletGrants> {
        self.library
            .lock()
            .unwrap()
            .iter()
            .find(|i| {
                i.kind == LibraryKind::Napplet
                    && i.author_npub == author_npub
                    && i.d_tag.as_deref() == d_tag
            })
            .map(|i| NappletGrants {
                granted: i.granted.clone(),
                denied: i.denied.clone(),
                reviewed: i.reviewed.clone(),
            })
    }

    /// Whether the napplet at `(author_npub, d_tag)` is one of Myco's
    /// preinstalled defaults, still the entry the first-run seed put there.
    /// See [`LibraryItem::preinstalled`].
    pub fn napplet_is_preinstalled(&self, author_npub: &str, d_tag: Option<&str>) -> bool {
        self.library.lock().unwrap().iter().any(|i| {
            i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag
                && i.preinstalled
        })
    }

    /// Mark an installed napplet as a preinstalled default and count
    /// `expected` as reviewed on it — added to its reviewed list, never
    /// replacing it. Grants are not touched: a domain is granted at open only
    /// once a served version declares it, and one the user switched off stays
    /// off. Does nothing for a napplet that is not installed.
    pub fn mark_napplet_preinstalled(
        &self,
        author_npub: &str,
        d_tag: Option<&str>,
        expected: &[String],
    ) {
        let mut lib = self.library.lock().unwrap();
        let Some(item) = lib.iter_mut().find(|i| {
            i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag
        }) else {
            return;
        };
        item.preinstalled = true;
        for domain in expected {
            if !item.reviewed.contains(domain) {
                item.reviewed.push(domain.clone());
            }
        }
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
    }

    /// Record the answer to an update's permission review: the grants and
    /// refusals as they now stand, and the declared list the sheet showed as
    /// the new reviewed list. Unlike [`Content::add_napplet_to_library`] this
    /// keeps `denied` as the caller passes it — the sheet asked about what the
    /// update adds, not about what the user already switched off. Does
    /// nothing for a napplet that is no longer installed.
    pub fn record_napplet_update_review(
        &self,
        author_npub: &str,
        d_tag: Option<&str>,
        grants: NappletGrants,
    ) -> bool {
        let mut lib = self.library.lock().unwrap();
        let Some(item) = lib.iter_mut().find(|i| {
            i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag
        }) else {
            return false;
        };
        item.granted = grants.granted;
        item.denied = grants.denied;
        item.reviewed = grants.reviewed;
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
        true
    }

    /// Unpin a napplet and drop its grants.
    ///
    /// The grants go with the entry: a napplet re-added later must go through
    /// review again rather than inheriting what a previous install agreed to.
    pub fn forget_napplet(&self, author_npub: &str, d_tag: Option<&str>) {
        let mut lib = self.library.lock().unwrap();
        lib.retain(|i| {
            !(i.kind == LibraryKind::Napplet
                && i.author_npub == author_npub
                && i.d_tag.as_deref() == d_tag)
        });
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
    }

    /// Drop an **nsite** from the Library. Kind-aware: an author may publish
    /// an nsite and a napplet under one `d` tag, and forgetting the site must
    /// leave the napplet — and its grants and pointer — where they are.
    /// `forget_napplet` is the napplet's remover.
    pub fn remove_from_library(&self, addr: &SiteAddr) {
        let npub = addr.author.to_bech32().unwrap_or_default();
        let mut lib = self.library.lock().unwrap();
        lib.retain(|i| {
            !(i.kind == LibraryKind::Nsite && i.author_npub == npub && i.d_tag == addr.d_tag)
        });
        let snapshot = lib.clone();
        drop(lib);
        self.persist_library(&snapshot);
    }

    /// Forget a single nsite: drop it from the Library *and* its live status entry
    /// so it vanishes from the Apps grid immediately and does not re-list on the
    /// next launch. Cached blobs/events are left for the global eviction pass (P5);
    /// this is the per-app "remove" the user reaches via the app's long-press sheet.
    pub fn forget_site(&self, addr: &SiteAddr) {
        {
            // Under the `sites` lock, so an in-flight status update either
            // lands before this (and is removed here) or sees the removal.
            let mut sites = self.sites.lock().unwrap();
            self.forgotten.lock().unwrap().insert(addr.host_label());
            sites.remove(&addr.host_label());
        }
        self.loading_retries
            .lock()
            .unwrap()
            .remove(&addr.host_label());
        self.remove_from_library(addr);
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

    /// The user asked for a removed site again (pasted, scanned, or opened
    /// from a link or share): it may come back.
    pub fn unforget_site(&self, addr: &SiteAddr) {
        self.forgotten.lock().unwrap().remove(&addr.host_label());
    }

    fn is_forgotten(&self, addr: &SiteAddr) -> bool {
        self.forgotten.lock().unwrap().contains(&addr.host_label())
    }

    /// Rebuild the per-site `siteStatus` from the persisted Library by checking
    /// each pinned site's readiness against the local stores. Run once at startup
    /// so "installed" sites re-list (as `ready`) after the app restarts — the
    /// relay + Blossom persist, but the in-memory status map does not.
    pub async fn refresh_library_status(self: Arc<Self>) {
        for item in self.library_snapshot() {
            let Some(addr) = library_addr(&item) else {
                continue;
            };
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
        self.refresh_napplet_status().await;
    }

    /// Recompute [`NappletStatusView`] for every installed napplet: ready when
    /// the served manifest and its index blob are both local, missing when not.
    pub async fn refresh_napplet_status(&self) {
        let napplets: Vec<LibraryItem> = self
            .library_snapshot()
            .into_iter()
            .filter(|i| i.kind == LibraryKind::Napplet)
            .collect();
        let mut fresh = HashMap::new();
        for item in napplets {
            let mut icon_hash = None;
            let ready = match self.napplet_keep_set(&item).await {
                Some((event, index, icon)) => {
                    icon_hash = icon;
                    let here = self.blobs.has(&index).await;
                    // A napplet installed before pinning existed has no pin,
                    // so it serves the store's newest — and a newer manifest
                    // kept or pushed without its bytes would take its place.
                    // Pin the version whose bytes are here, as the nsite pass
                    // above does. Only when there is no pin: one that exists
                    // is already the version to serve.
                    let key = manifest_key(
                        event.kind.as_u16(),
                        &event.pubkey,
                        event_d_tag(&event).as_deref(),
                    );
                    let pinned = self.active_manifests.lock().unwrap().contains_key(&key);
                    if here && !pinned {
                        self.set_active_if_newer(&event);
                    }
                    here
                }
                None => false,
            };
            let (state, message) = if ready {
                ("ready", "Ready")
            } else {
                ("missing", "Not on this phone — hold to reload")
            };
            fresh.insert(
                item.url_host.clone(),
                NappletStatusView {
                    host: item.url_host.clone(),
                    state: state.to_string(),
                    message: message.to_string(),
                    icon: icon_hash,
                },
            );
        }
        *self.napplet_status.lock().unwrap() = fresh;
    }

    pub fn napplet_status_snapshot(&self) -> Vec<NappletStatusView> {
        let mut out: Vec<NappletStatusView> = self
            .napplet_status
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        out.sort_by(|a, b| a.host.cmp(&b.host));
        out
    }

    /// Library sites worth (re)trying right now: not yet `ready`, and not already
    /// `syncing` (an attempt is in flight). Skipping the in-flight ones is what lets
    /// a caller poll this every tick without piling on duplicate syncs — it re-tries
    /// roughly once per attempt-duration, so a neighbor who comes into reach, or
    /// the internet coming back, finishes the download.
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

    // --- device identity ---

    /// Set the device keypair from the persisted nsec.
    pub fn set_device_keys(&self, nsec: &str) {
        match Keys::parse(nsec) {
            Ok(keys) => *self.device_keys.lock().unwrap() = Some(keys),
            Err(e) => tracing::warn!(error = %e, "content: bad device nsec"),
        }
    }

    // --- nsite updates (docs/design/nsite/nsite-updates.md) ---

    /// Ask the update-check throttle whether a check from `trigger` may start
    /// now (`update_gate.rs`). `Some` marks one in flight; hand the token to
    /// [`Self::check_updates_with`], or drop it to release the gate.
    pub fn begin_update_check(
        self: &Arc<Self>,
        trigger: crate::update_gate::CheckTrigger,
    ) -> Option<InFlightCheck> {
        let decision = self.update_gate.lock().unwrap().begin(trigger, now_secs());
        tracing::info!(?trigger, ?decision, "update check requested");
        (decision == crate::update_gate::Decision::Run).then(|| InFlightCheck {
            content: self.clone(),
            done: false,
        })
    }

    /// P-U1 update check. Polls relays for newer manifests of every Library
    /// site in **one combined REQ per relay** (deduplicated, read until EOSE),
    /// and for each newer-than-active candidate stages its blobs and activates
    /// when complete. Spawn-not-block; the UI polls `siteStatus`.
    ///
    /// Checks napplets too, through `napplets` — one "Checking…" and one
    /// result for both. `napplets` resolves to `Some((updated, checked))`, or
    /// `None` when there are none to check; the napplet path lives in
    /// `napplet.rs` because it needs the host.
    ///
    /// `in_flight` is the token [`Self::begin_update_check`] handed out; it
    /// releases the gate when this ends, also if the task is dropped part-way.
    pub async fn check_updates_with<F>(self: Arc<Self>, in_flight: InFlightCheck, napplets: F)
    where
        F: std::future::Future<Output = Option<(usize, usize)>>,
    {
        self.set_update_check(true, "Checking for updates…");
        let (nsites, napplets) = tokio::join!(self.clone().check_nsite_updates(), napplets);
        let msg = match (nsites, napplets) {
            (NsiteCheck::Nothing, Some((updated, checked))) => {
                napplet_update_message(updated, checked)
            }
            (NsiteCheck::Nothing, None) => "No apps to check".to_string(),
            (NsiteCheck::Done(msg), None) => msg,
            (NsiteCheck::Done(msg), Some((updated, checked))) => {
                format!("{msg}; {}", napplet_update_message(updated, checked))
            }
        };
        in_flight.complete(&msg);
    }

    /// The nsite half of an update check. Progress lands in `update_check`
    /// as it goes; the final message is returned rather than posted, so the
    /// caller can join it with the napplet half.
    async fn check_nsite_updates(self: Arc<Self>) -> NsiteCheck {
        // Tracked sites + the union of their authors (one filter covers all).
        let addrs: Vec<SiteAddr> = self
            .library_snapshot()
            .iter()
            .filter_map(library_addr)
            .collect();
        if addrs.is_empty() {
            return NsiteCheck::Nothing;
        }
        let authors: Vec<String> = {
            let mut s: HashSet<String> = HashSet::new();
            for a in &addrs {
                s.insert(a.author.to_hex());
            }
            s.into_iter().collect()
        };

        // Query set: the online relays, one combined REQ each read until EOSE,
        // unless mesh-only is on (docs/design/nsite/nsite-updates.md §3.2).
        // Neighbors are not asked: they push updates through `nsite-updates`.
        let online_filter = serde_json::json!({
            "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
            "authors": authors,
        });
        // Online: the authors' NIP-65 write relays, then the defaults. Only
        // lists stored here decide the relay set, so the check never waits on
        // a list fetch. Every author's list rides along in the same REQ to the
        // defaults (refreshing stored lists, finding missing ones), and the
        // indexers are asked for the missing ones beside the queries — both
        // stored for the next check.
        let defaults = crate::ip_source::default_relays();
        let outbox = crate::ip_source::AuthorOutbox::new(self.relay());
        let mut pubkeys: Vec<PublicKey> = Vec::new();
        for a in &addrs {
            if !pubkeys.contains(&a.author) {
                pubkeys.push(a.author);
            }
        }
        let (online, missing_lists) = if self.is_offline_only() {
            (Vec::new(), Vec::new())
        } else {
            let (author_relays, missing) = outbox
                .stored_write_relays(&pubkeys, UPDATE_CHECK_AUTHOR_RELAYS)
                .await;
            (
                crate::ip_source::lookup_relays(&[], &author_relays, &defaults),
                missing,
            )
        };
        let list_filter = serde_json::json!({
            "kinds": [Kind::RelayList.as_u16()],
            "authors": authors,
        });
        // The authors' Blossom server lists ride along on every online ask,
        // so an update's files are looked for where the author keeps them
        // now, not where they did at install.
        let server_list_filter = serde_json::json!({
            "kinds": [crate::ip_source::SERVER_LIST_KIND],
            "authors": authors,
        });
        if online.is_empty() {
            return NsiteCheck::Done("No relays to check".to_string());
        }
        tracing::info!(
            apps = addrs.len(),
            targets = online.len(),
            offline_only = self.is_offline_only(),
            "update check: querying"
        );

        // One-shot: we don't hold long-lived sockets — or leak presence — to
        // public relays.
        let online_q = online.into_iter().map(|url| {
            // A relay named only by a relay list is resolved and refused if
            // it points at a private address; a default also carries the
            // authors' lists.
            let from_list = !defaults
                .iter()
                .any(|d| crate::ip_source::same_relay(d, &url));
            let filters = if from_list {
                vec![online_filter.clone(), server_list_filter.clone()]
            } else {
                vec![
                    online_filter.clone(),
                    list_filter.clone(),
                    server_list_filter.clone(),
                ]
            };
            let outbox = &outbox;
            async move {
                let dial = async {
                    if from_list && !outbox.may_dial(&url).await {
                        return Ok(Vec::new());
                    }
                    crate::ip_source::query_relay_filters(&url, filters).await
                };
                match crate::relay_health::timeout(&url, std::time::Duration::from_secs(15), dial)
                    .await
                {
                    Ok(Ok(evs)) => evs,
                    _ => Vec::new(),
                }
            }
        });
        // Bounded well inside the queries' own timeout, so it never
        // lengthens the check.
        let fetch_missing = outbox.fetch_lists(&missing_lists, outbox.indexers());
        let (online_res, ()) = futures_util::future::join(join_all(online_q), fetch_missing).await;
        for ev in online_res.iter().flatten() {
            if !pubkeys.contains(&ev.pubkey) {
                continue;
            }
            if ev.kind == Kind::RelayList {
                outbox.remember(ev.clone()).await;
            } else if ev.kind.as_u16() == crate::ip_source::SERVER_LIST_KIND {
                outbox.remember_server_list(ev.clone()).await;
            }
        }

        // Newest verified manifest per slot across all relays.
        let mut newest: HashMap<String, Event> = HashMap::new();
        let mut received = 0usize;
        for batch in online_res {
            received += batch.len();
            for ev in batch {
                let kind = ev.kind.as_u16();
                if kind != nsite_deck::KIND_ROOT && kind != nsite_deck::KIND_NAMED {
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
            let Some(cand) = newest.get(&key) else {
                continue;
            };
            // Compare against the version we actually serve (the active pointer),
            // not merely the relay's newest.
            let active_ts = nsite_deck::seams::newest_in_slot(
                &self.active_backend(),
                kind,
                &addr.author,
                addr.d_tag.as_deref(),
            )
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
            return NsiteCheck::Done("All apps are up to date".to_string());
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
        NsiteCheck::Done(msg)
    }

    fn set_update_check(&self, checking: bool, message: &str) {
        let mut uc = self.update_check.lock().unwrap();
        uc.checking = checking;
        uc.message = message.to_string();
    }

    /// End the running check: post `message` and release the gate. The
    /// result is written **under** the gate lock, so a check that starts the
    /// moment the gate opens cannot have its "Checking…" overwritten by this
    /// one's result. Lock order is gate, then `update_check`; nothing takes
    /// them the other way round.
    ///
    /// The toast (`generation`) fires when the check reports — a manual one,
    /// or one a manual press joined; an automatic check stays silent. An
    /// `interrupted` check posts its message only if someone is waiting on it.
    ///
    /// Poison-tolerant: this runs from `Drop`, possibly while unwinding, where
    /// a second panic would abort the process.
    fn release_update_check(&self, message: &str, interrupted: bool) {
        let mut gate = self.update_gate.lock().unwrap_or_else(|e| e.into_inner());
        let report = gate.finish();
        let mut uc = self.update_check.lock().unwrap_or_else(|e| e.into_inner());
        uc.checking = false;
        if report || !interrupted {
            uc.message = message.to_string();
        }
        if report {
            uc.generation += 1;
        }
    }

    pub fn update_check_snapshot(&self) -> UpdateCheckView {
        self.update_check.lock().unwrap().clone()
    }

    /// Online update path: if `candidate` is newer than what we serve, download its
    /// blobs, activate, and share it through `nsite-updates` (we now hold the
    /// blobs). Returns whether it activated.
    async fn stage_update(self: Arc<Self>, addr: SiteAddr, candidate: Event) -> bool {
        let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
        let active_ts = nsite_deck::seams::newest_in_slot(
            &self.active_backend(),
            kind,
            &addr.author,
            addr.d_tag.as_deref(),
        )
        .await
        .ok()
        .flatten()
        .map(|e| e.created_at.as_secs())
        .unwrap_or(0);
        if candidate.created_at.as_secs() <= active_ts {
            return false;
        }
        // The neighborhoods first (closer, and the only option under
        // mesh-only), then the online fallback unless mesh-only. Activation
        // stores the manifest and shares it through `nsite-updates`.
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        if let Some(neighborhoods) =
            self.extension_source(crate::neighbor::extensions::NSITE_UPDATES)
        {
            sources.push(neighborhoods);
        }
        sources.extend(self.fallback_sources(None));
        Arc::clone(&self)
            .download_and_activate(addr, candidate, sources, true)
            .await
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
                PendingUpdate {
                    manifest: candidate.clone(),
                    total,
                    pulled: 0,
                    ready: false,
                },
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
                nsite_deck::sync::stage_blobs(
                    self.blobs.as_ref(),
                    source.as_ref(),
                    &manifest,
                    &progress
                )
                .await,
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
                let _ = self.relay.publish(candidate.clone()).await;
            }
            self.set_active(&candidate);
            // An update this device now holds the files of: the
            // `nsite-updates` extension shares it with neighbors.
            let _ = self.site_updates.send(candidate.clone());
            let n = manifest.paths.len() as u64;
            self.set_status_titled(&addr, manifest.title.as_deref(), "ready", n, n, "Updated");
            self.pending_updates.lock().unwrap().remove(&host);
            true
        } else {
            self.pending_updates.lock().unwrap().remove(&host);
            false
        }
    }

    /// A napplet update a neighbor shared (`napplet-updates`), with the
    /// sources handed in so it can be exercised without a mesh.
    ///
    /// - Not a valid, signed NIP-5D manifest, or older than the version this
    ///   relay already keeps for the slot: dropped.
    /// - Not installed here: nothing fetched.
    /// - Installed, but not newer than the pinned version: a replay or a
    ///   downgrade, dropped.
    /// - Installed and newer: the bytes are fetched from `sources` in order,
    ///   verified, stored and pinned (the pin never moves back, see
    ///   [`Content::set_active_if_newer`]) — the next launch opens it, an open
    ///   window keeps its session. Once pinned, this device shares it onward
    ///   itself.
    ///
    /// Nothing here touches grants. The Library entry keeps what the user
    /// reviewed; a version declaring more comes back through the review sheet
    /// at its next open (`NappletHost::open_with`).
    pub(crate) async fn handle_napplet_manifest(
        &self,
        event: Event,
        sources: &[Arc<dyn PeerSource>],
    ) -> NappletPush {
        if event.verify().is_err() {
            return NappletPush::Dropped;
        }
        let manifest = match myco_napplet_runtime::NappletManifest::from_event(event.clone()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(event = %event.id, error = %e, "not a valid napplet manifest");
                return NappletPush::Dropped;
            }
        };
        let kind = event.kind.as_u16();
        let author = event.pubkey;
        let d_tag = manifest.d_tag.as_deref();

        // The relay kept something newer for this slot and refused this one:
        // a stale version.
        if let Ok(Some(newest)) =
            nsite_deck::seams::newest_in_slot(self.relay.as_ref(), kind, &author, d_tag).await
        {
            if newest.created_at > event.created_at {
                return NappletPush::Dropped;
            }
        }

        // Installed means the same author and `d` tag in the Library: the
        // signature already ties the manifest to `author`, so a napplet by
        // anyone else under the same name is a different napplet.
        let npub = author.to_bech32().unwrap_or_default();
        if self.napplet_grants(&npub, d_tag).is_none() {
            return NappletPush::NotInstalled;
        }

        if let Some(pinned) = self.pinned_manifest(kind, &author, d_tag) {
            if pinned.created_at >= event.created_at {
                return NappletPush::Dropped;
            }
        }

        let mut updated = false;
        for source in sources {
            match crate::napplet::ingest_event_into(
                self.relay.as_ref(),
                self.blobs.as_ref(),
                self,
                event.clone(),
                source.as_ref(),
            )
            .await
            {
                Ok(_) => {
                    updated = true;
                    break;
                }
                Err(e) => tracing::debug!(
                    napplet = %d_tag.unwrap_or("<root>"),
                    error = %e,
                    "napplet update: this source could not supply it"
                ),
            }
        }
        if updated {
            self.refresh_napplet_status().await;
            NappletPush::Updated
        } else {
            NappletPush::NotDownloaded
        }
    }

    /// The version pinned as served for a slot, if any — not the relay's
    /// newest.
    pub(crate) fn pinned_manifest(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> Option<Event> {
        self.active_manifests
            .lock()
            .unwrap()
            .get(&manifest_key(kind, author, d_tag))
            .cloned()
    }

    /// [`Content::set_active`] that never moves a slot backwards: a version
    /// older than the pinned one is refused; the same version, or another
    /// with the same `created_at`, is accepted. Returns whether it pinned.
    ///
    /// Every napplet pin goes through here ([`crate::napplet::ManifestStore`]).
    /// Two versions downloading at once — an update a neighbor shared and
    /// the update check, say — must not leave the older one served because it
    /// finished last, and a window opening must not re-pin what it resolved
    /// over a newer version pinned meanwhile. Equal is allowed because the
    /// legitimate re-pins are all of the same version: `open` re-pinning
    /// what it just resolved, "Download again" of an installed napplet
    /// whose bytes went missing, a seed re-fetching its default. A first
    /// install, or anything after a wipe, has no pin to compare with. Nsite
    /// activation calls `set_active` directly and is unaffected.
    fn set_active_if_newer(&self, manifest: &Event) -> bool {
        let key = manifest_key(
            manifest.kind.as_u16(),
            &manifest.pubkey,
            event_d_tag(manifest).as_deref(),
        );
        let (snapshot, updated) = {
            let mut m = self.active_manifests.lock().unwrap();
            let previous = m.get(&key).map(|cur| cur.created_at);
            if previous.is_some_and(|cur| cur > manifest.created_at) {
                return false;
            }
            m.insert(key, manifest.clone());
            let updated = previous.is_some_and(|cur| cur < manifest.created_at);
            (m.values().cloned().collect::<Vec<_>>(), updated)
        };
        save_active(&self.active_path, &snapshot);
        // A newer version replaced an older one: an update, which the
        // `napplet-updates` extension shares with neighbors.
        if updated {
            let _ = self.napplet_updates.send(manifest.clone());
        }
        true
    }

    /// A release this phone now keeps, for the `myco-release` extension.
    pub(crate) fn release_kept(&self, announcement: Event, files: Vec<Event>, sha256: String) {
        let _ = self.release_updates.send((announcement, files, sha256));
    }

    /// Releases as this phone keeps a new one.
    pub fn release_updates(&self) -> tokio::sync::broadcast::Receiver<(Event, Vec<Event>, String)> {
        self.release_updates.subscribe()
    }

    /// Where an update extension's apps are looked for in the neighborhoods
    /// (`crate::neighbor::updates::ExtensionSource`): opening a site or
    /// installing a napplet that is not here yet, and `release_apk` for the
    /// release APK. Set once per extension by the runtime.
    pub fn set_extension_source(&self, extension: &'static str, source: Arc<dyn PeerSource>) {
        self.extension_sources
            .lock()
            .unwrap()
            .insert(extension, source);
    }

    pub(crate) fn extension_source(&self, extension: &str) -> Option<Arc<dyn PeerSource>> {
        self.extension_sources
            .lock()
            .unwrap()
            .get(extension)
            .cloned()
    }

    /// Nsite manifests as an update is activated — this device now holds
    /// their files.
    pub fn site_updates(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.site_updates.subscribe()
    }

    /// Apply an nsite update a neighbor shared: if the site is in the
    /// Library and the version is newer than the active one, fetch its
    /// files from `first` (then the public servers, unless offline-only)
    /// and activate it. Returns whether it activated.
    pub async fn apply_site_update(
        self: Arc<Self>,
        event: Event,
        first: Arc<dyn PeerSource>,
    ) -> bool {
        if event.verify().is_err() {
            return false;
        }
        let addr = SiteAddr {
            author: event.pubkey,
            d_tag: event_d_tag(&event),
        };
        if !self.is_in_library(&addr) {
            return false;
        }
        let key = manifest_key(event.kind.as_u16(), &event.pubkey, addr.d_tag.as_deref());
        if self
            .active_manifests
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|cur| cur.created_at >= event.created_at)
        {
            return false;
        }
        let mut sources = vec![first];
        sources.extend(self.fallback_sources(None));
        self.download_and_activate(addr, event, sources, true).await
    }

    /// The public servers, unless offline-only: where a version's files are
    /// looked for when the neighborhoods can't supply them. The same for
    /// nsites and napplets.
    ///
    /// `max_blob_bytes` caps each blob as it streams in, before the hash check
    /// has anything to say: whoever signed the manifest also chose what it
    /// references. Napplets pass NAP-RESOURCE's cap; nsites have no agreed
    /// per-blob limit yet and pass `None`.
    pub(crate) fn fallback_sources(
        &self,
        max_blob_bytes: Option<usize>,
    ) -> Vec<Arc<dyn PeerSource>> {
        if self.is_offline_only() {
            return Vec::new();
        }
        let source = crate::ip_source::IpPeerSource::new(
            crate::ip_source::default_relays(),
            crate::ip_source::default_blossom_servers(),
        )
        .with_author_servers(Arc::new(crate::ip_source::AuthorOutbox::new(self.relay())));
        let source = match max_blob_bytes {
            Some(max) => source.with_max_blob_bytes(max),
            None => source,
        };
        vec![Arc::new(source)]
    }

    /// Napplet manifests as they are pinned over an older version — updates
    /// this device now holds the files of.
    pub fn napplet_updates(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.napplet_updates.subscribe()
    }

    /// Whether an **nsite** is in our Library (we "run" it, so we're interested
    /// in its updates — download before forwarding). Kind-aware: a napplet
    /// entry under the same `(author, d)` is not the nsite, and must not make
    /// the nsite's manifest look installed — that staged every blob of an
    /// uninstalled site and put its tile on the grid.
    fn is_in_library(&self, addr: &SiteAddr) -> bool {
        let npub = addr.author.to_bech32().unwrap_or_default();
        self.library
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.kind == LibraryKind::Nsite && i.author_npub == npub && i.d_tag == addr.d_tag)
    }

    // --- wipe ---

    /// Clear the local relay + Blossom + Library + status (the `WipeStores` dev
    /// action). Content-only; identity is untouched.
    pub async fn wipe(&self) -> anyhow::Result<()> {
        // Only ours to clear. A custom relay's contents belong to whoever runs
        // it, and NIP-01 has no "delete everything" to ask for anyway.
        if let Some(store) = &self.relay_store {
            nsite_deck::seams::AdminBackend::wipe(store.as_ref()).await?;
        }
        self.blobs.wipe().await?;
        self.library.lock().unwrap().clear();
        self.sites.lock().unwrap().clear();
        self.pending_updates.lock().unwrap().clear();
        self.active_manifests.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.library_path);
        let _ = std::fs::remove_file(&self.active_path);
        Ok(())
    }

    /// The shell cache is not cleared here: it can hold the whole blob
    /// budget, and deleting that must not hold the caller. The runtime spawns
    /// [`Content::clear_cache`] beside this (and beside [`Content::wipe`]).
    ///
    /// Clear cached relay events + Blossom blobs **except** those backing pinned
    /// nsites (Settings → Storage → "Clear local database"). The served manifest version of
    /// each pinned site and every blob it references survive, so installed apps keep
    /// working offline; everything else — unpinned opened sites and staged
    /// updates — is dropped. Identity and neighborhoods are untouched.
    ///
    /// `keep_author` is the user key's pubkey, when there is one: its kind 0,
    /// kind 10002 and kind 10050 survive too. They were published once, at first napplet
    /// use, and are never republished — `user.nsec` still exists after a wipe,
    /// so nothing regenerates them — and without them the user's own outbox
    /// plan falls back and every napplet sees a bare pubkey.
    pub async fn wipe_cache(&self, keep_author: Option<PublicKey>) -> anyhow::Result<()> {
        // Pinned Library entries are the apps we must keep working.
        let pinned: Vec<LibraryItem> = self
            .library
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.pinned)
            .cloned()
            .collect();

        // Build the keep-sets: the served manifest event id of each pinned site,
        // plus every blob hash that manifest references.
        let mut keep_events: HashSet<[u8; 32]> = HashSet::new();
        let mut keep_blobs: HashSet<String> = HashSet::new();
        let mut keep_active: HashSet<String> = HashSet::new();
        let backend = self.active_backend();
        for item in &pinned {
            // A napplet is one manifest and one blob. Both stay, or the tile
            // stays and the app behind it is gone — which is what happened the
            // first time "Clear local database" met an installed napplet.
            if item.kind == LibraryKind::Napplet {
                if let Some((event, index_hash, icon)) = self.napplet_keep_set(item).await {
                    keep_events.insert(event.id.to_bytes());
                    keep_blobs.insert(index_hash);
                    keep_blobs.extend(icon);
                    keep_active.insert(manifest_key(
                        event.kind.as_u16(),
                        &event.pubkey,
                        item.d_tag.as_deref(),
                    ));
                }
                continue;
            }
            let Some(addr) = library_addr(item) else {
                continue;
            };
            let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
            if let Ok(Some(ev)) = nsite_deck::seams::newest_in_slot(
                &backend,
                kind,
                &addr.author,
                addr.d_tag.as_deref(),
            )
            .await
            {
                keep_events.insert(ev.id.to_bytes());
                if let Ok(m) = nsite_deck::Manifest::from_event(ev) {
                    keep_blobs.extend(m.paths.into_values());
                }
                keep_active.insert(manifest_key(kind, &addr.author, addr.d_tag.as_deref()));
            }
        }

        // The release APK this phone carries for its neighborhoods, and the signed
        // events that vouch for it.
        let (release_events, release_apk) = crate::release_apk::kept_release(self).await;
        keep_events.extend(release_events.iter().map(|id| id.to_bytes()));
        keep_blobs.extend(release_apk);

        if let Some(store) = &self.relay_store {
            // The user's own profile and relay lists, by the pubkey alone: the
            // store keeps one of each per author, so this is at most three
            // events. Read from the embedded store itself — it is the only
            // thing being retained.
            if let Some(pk) = keep_author {
                let own = Filter::new().author(pk).kinds([
                    Kind::Metadata,
                    Kind::RelayList,
                    Kind::InboxRelays,
                ]);
                match store.query(&[own]).await {
                    Ok(events) => keep_events.extend(events.iter().map(|e| e.id.to_bytes())),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "wipe_cache: could not read the user's own profile and relay list; they will go"
                    ),
                }
            }
            store.retain_events(&keep_events).await;
            // A pinned version may no longer be in the store at all: a newer
            // manifest for the slot — pushed by a peer, or kept when a napplet
            // browsed it — replaced it there, and the retain just dropped that
            // newer one as cache. The gateway substitutes a pin only for an
            // event the slot still returns, so put each pin back; an older one
            // than the store holds is a no-op.
            let pins: Vec<Event> = self
                .active_manifests
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| keep_active.contains(*k))
                .map(|(_, e)| e.clone())
                .collect();
            for pin in pins {
                if let Err(e) = store.publish(pin).await {
                    tracing::warn!(error = %e, "wipe_cache: could not restore a pinned manifest");
                }
            }
        }
        if let Some(store) = &self.blobs_local {
            store.retain_blobs(&keep_blobs);
        }

        // Drop unpinned Library entries and the live status of anything unpinned.
        let pinned_hosts: HashSet<String> = pinned.iter().map(|i| i.url_host.clone()).collect();
        {
            let mut lib = self.library.lock().unwrap();
            lib.retain(|i| i.pinned);
            let snapshot = lib.clone();
            drop(lib);
            self.persist_library(&snapshot);
        }
        self.sites
            .lock()
            .unwrap()
            .retain(|host, _| pinned_hosts.contains(host));
        self.pending_updates.lock().unwrap().clear();
        let active_snapshot = {
            let mut m = self.active_manifests.lock().unwrap();
            m.retain(|k, _| keep_active.contains(k));
            m.values().cloned().collect::<Vec<_>>()
        };
        save_active(&self.active_path, &active_snapshot);
        Ok(())
    }

    /// The manifest event, artifact hash and icon hash (when it has one) an
    /// installed napplet is served from, for the keep-sets: the active
    /// manifest when one is pinned, else the newest in the slot.
    async fn napplet_keep_set(
        &self,
        item: &LibraryItem,
    ) -> Option<(Event, String, Option<String>)> {
        use nostr::nips::nip19::FromBech32;
        let author = PublicKey::from_bech32(&item.author_npub).ok()?;
        let (event, manifest) = self.served_napplet(&author, item.d_tag.as_deref()).await?;
        let icon = manifest.icon.map(|i| i.sha256);
        Some((event, manifest.artifact, icon))
    }

    /// An installed napplet's verified icon bytes and declared MIME type, from
    /// this phone's stores only — `None` for no icon, an icon not here, or
    /// bytes that are not what the manifest signed. The caller shows generic
    /// artwork then; an icon never stands between a person and the napplet.
    pub async fn napplet_icon(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> Option<(Arc<Vec<u8>>, String)> {
        let (_, manifest) = self.served_napplet(author, d_tag).await?;
        let icon = manifest.icon?;
        let sha256 = icon.sha256.to_ascii_lowercase();
        if let Some(bytes) = self.icon_memo.lock().unwrap().get(&sha256).cloned() {
            return Some((bytes, icon.mime));
        }
        let bytes = self.cache_blobs.get(&sha256).await.ok()??;
        if !crate::napplet::verified_icon(&icon, &bytes) {
            return None;
        }
        let bytes = Arc::new(bytes);
        self.icon_memo.lock().unwrap().insert(sha256, bytes.clone());
        Some((bytes, icon.mime))
    }

    /// A napplet icon by hash, from either store tier — for an install sheet
    /// previewing a napplet not installed yet. Only bytes that hash to
    /// `sha256` and start like `mime`; `None` otherwise.
    pub async fn icon_blob(&self, sha256: &str, mime: &str) -> Option<Vec<u8>> {
        let icon = myco_napplet_runtime::ManifestIcon {
            sha256: sha256.to_ascii_lowercase(),
            mime: mime.to_string(),
        };
        let bytes = self.cache_blobs.get(&icon.sha256).await.ok()??;
        crate::napplet::verified_icon(&icon, &bytes).then_some(bytes)
    }

    /// The manifest an installed napplet is served from, parsed: its pin,
    /// held in memory, when it has one — an installed napplet always does —
    /// else the newest the slot returns.
    async fn served_napplet(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> Option<(Event, myco_napplet_runtime::NappletManifest)> {
        let kind = match d_tag {
            Some(_) => myco_napplet_runtime::KIND_NAMED,
            None => myco_napplet_runtime::KIND_ROOT,
        };
        let event = match self.pinned_manifest(kind, author, d_tag) {
            Some(pinned) => pinned,
            None => nsite_deck::seams::newest_in_slot(&self.active_backend(), kind, author, d_tag)
                .await
                .ok()??,
        };
        let manifest = myco_napplet_runtime::NappletManifest::from_event(event.clone()).ok()?;
        Some((event, manifest))
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
        // Always the embedded store's own figures — that is what takes up space
        // here. Nothing external is configurable yet, so neither flag is set;
        // they follow the configured backend once that lands.
        //
        // Called from `AppRuntime::state()` under the reducer lock, every
        // second: the blob figures must not touch the disk. Until the store's
        // first background walk lands they read as 0.
        let blobs = self
            .blobs_local
            .as_ref()
            .and_then(|b| b.try_stats())
            .unwrap_or_default();
        CacheView {
            // The embedded store's own count, or nothing to count when a custom
            // relay has taken over — which the flag tells the screen to say.
            relay_events: self.relay_store.as_ref().map_or(0, |s| s.count() as u64),
            blob_count: blobs.count as u64,
            used_bytes: blobs.bytes,
            event_cache: self.event_cache.stats(),
            blob_cache: self.blob_cache.stats(),
            external_relay: self.relay_store.is_none(),
            external_blobs: self.blobs_local.is_none(),
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
        // Removed: no tile, whatever an in-flight sync has to report. Checked
        // under the `sites` lock, which `forget_site` also takes, so a removal
        // cannot land between the check and the insert.
        if self.is_forgotten(addr) {
            return;
        }
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
            if s.title.is_empty() {
                "This app".to_string()
            } else {
                s.title.clone()
            },
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
        "incomplete" => (
            "Didn't finish downloading — retrying…".to_string(),
            "#d97706",
        ),
        "syncing" if total > 0 => (
            format!("Downloading · {present} of {total} files"),
            "#059669",
        ),
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
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

/// Resolve a Library entry back to an **nsite** address.
///
/// Returns `None` for a napplet. A napplet shares the Library with nsites but
/// nothing else: it has no 15128/35128 manifest, so handing one to the nsite
/// sync engine starts a sync that can never finish and leaves a tile stuck
/// syncing forever beside the napplet's own.
///
/// Every path from the Library into nsite machinery goes through here, which is
/// why the check lives here rather than at each caller — a new caller gets the
/// exclusion for free instead of having to remember it.
///
/// Also returns `None` when the npub fails to parse, which a hand-edited file
/// can produce.
fn library_addr(item: &LibraryItem) -> Option<SiteAddr> {
    if item.kind != LibraryKind::Nsite {
        return None;
    }
    let author = PublicKey::from_bech32(&item.author_npub).ok()?;
    Some(SiteAddr {
        author,
        d_tag: item.d_tag.clone(),
    })
}

/// Seconds since the Unix epoch (Library `added_at`).
pub(crate) fn now_secs() -> u64 {
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

/// What became of a napplet update a neighbor shared. See
/// [`Content::handle_napplet_manifest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NappletPush {
    /// Invalid, stale, or not newer than the installed version.
    Dropped,
    /// Not installed here: nothing fetched.
    NotInstalled,
    /// Installed: the new version's bytes are here and pinned.
    Updated,
    /// Installed, but no source supplied the bytes.
    NotDownloaded,
}

#[async_trait]
impl crate::napplet::ManifestStore for Content {
    async fn current(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        // The active view substitutes the pinned version for the relay's
        // newest — the same gate the nsite gateway reads through.
        nsite_deck::seams::newest_in_slot(&self.active_backend(), kind, author, d_tag).await
    }

    /// A napplet's pin never moves back: every napplet pin — open, install,
    /// "Download again", the update check, an update a neighbor shared — goes
    /// through [`Content::set_active_if_newer`]. See there for why.
    ///
    /// A napplet pin that moved also rewrites the installed entry's
    /// archetypes from the verified manifest: the catalog NAP-INTENT resolves
    /// against follows the version that opens.
    fn pin(&self, manifest: &Event) {
        if !self.set_active_if_newer(manifest) {
            return;
        }
        if !myco_napplet_runtime::is_napplet_kind(manifest.kind.as_u16()) {
            return;
        }
        if let Ok(parsed) = myco_napplet_runtime::NappletManifest::from_event(manifest.clone()) {
            let npub = parsed.author.to_bech32().unwrap_or_default();
            self.record_napplet_archetypes(
                &npub,
                parsed.d_tag.as_deref(),
                LibraryArchetype::of_manifest(&parsed),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip19::ToBech32;
    use nostr::EventBuilder;
    use nsite_deck::testing::build_test_site;

    /// `cache_view` reads the blob figures without touching the disk, so they
    /// read 0 until the store's first walk lands; wait for it here.
    fn settle_blob_stats(content: &Content) {
        if let Some(blobs) = &content.blobs_local {
            blobs.stats();
        }
    }

    /// Removing a site that nobody has — stuck on "can't reach anyone" —
    /// makes its tile go away and keeps it away: a sync still in flight, or
    /// an open window reloading its loading page, must not bring it back.
    /// Asking for it again does.
    #[tokio::test]
    async fn a_removed_site_stays_removed_until_asked_for_again() {
        let dir = std::env::temp_dir().join(format!(
            "myco-forget-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let content = Arc::new(Content::open(&dir).unwrap());
        let npub = nostr::ToBech32::to_bech32(&nostr::Keys::generate().public_key()).unwrap();
        let addr = nsite_deck::parse_link(&npub).unwrap();

        content.clone().open_site(addr.clone()).await;
        let listed = |c: &Content| c.sites.lock().unwrap().contains_key(&addr.host_label());
        assert!(listed(&content), "an unreachable site has no tile");

        content.forget_site(&addr);
        assert!(!listed(&content));
        // What an in-flight sync or a reloading window would do.
        content.clone().open_site(addr.clone()).await;
        content.add_to_library(&addr, None, now_secs());
        assert!(!listed(&content), "the removed tile came back");
        assert!(
            content.library_snapshot().is_empty(),
            "the removed site was pinned again"
        );

        content.unforget_site(&addr);
        content.clone().open_site(addr.clone()).await;
        assert!(listed(&content), "asking again did not bring it back");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn update_test_content(tag: &str) -> (Arc<Content>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "myco-update-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (Arc::new(Content::open(&dir).unwrap()), dir)
    }

    /// Automatic update checks share the manual one's gate: they are throttled,
    /// they release the gate when done, and only a manual check fires the
    /// result toast (`generation`). Hermetic — nothing installed, so nothing
    /// is asked of any relay.
    #[tokio::test]
    async fn automatic_update_checks_are_throttled_and_silent() {
        use crate::update_gate::CheckTrigger::{Auto, Manual};
        let (content, dir) = update_test_content("gate");
        let generation = |c: &Content| c.update_check_snapshot().generation;

        let auto = content.begin_update_check(Auto).expect("first auto check");
        content
            .clone()
            .check_updates_with(auto, async { None })
            .await;
        assert_eq!(generation(&content), 0, "an automatic check toasted");

        assert!(content.begin_update_check(Auto).is_none(), "not throttled");
        let manual = content
            .begin_update_check(Manual)
            .expect("manual throttled");
        assert!(
            content.begin_update_check(Manual).is_none(),
            "a second check overlapped the running one"
        );
        content
            .clone()
            .check_updates_with(manual, async { None })
            .await;
        assert_eq!(generation(&content), 1);
        assert!(!content.update_check_snapshot().checking);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A manual press while an automatic check runs joins it and gets its result.
    #[tokio::test]
    async fn a_manual_press_joins_a_running_check() {
        use crate::update_gate::CheckTrigger::{Auto, Manual};
        let (content, dir) = update_test_content("join");
        let auto = content.begin_update_check(Auto).unwrap();
        assert!(content.begin_update_check(Manual).is_none());
        content
            .clone()
            .check_updates_with(auto, async { None })
            .await;
        let uc = content.update_check_snapshot();
        assert_eq!(uc.generation, 1);
        assert_eq!(uc.message, "No apps to check");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A token dropped before its check ever runs (the spawned task dropped
    /// before its first poll) releases the gate, silently.
    #[tokio::test]
    async fn an_unused_update_token_releases_the_gate() {
        use crate::update_gate::CheckTrigger::{Auto, Manual};
        let (content, dir) = update_test_content("unused");
        drop(content.begin_update_check(Auto).unwrap());
        assert_eq!(content.update_check_snapshot().generation, 0);
        let manual = content.begin_update_check(Manual);
        assert!(manual.is_some(), "gate stayed shut");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A check dropped part-way (its task cancelled) still releases the gate,
    /// and a user who pressed "Check for updates" still gets a result.
    #[tokio::test]
    async fn a_dropped_update_check_releases_the_gate_and_reports() {
        use crate::update_gate::CheckTrigger::{Auto, Manual};
        let (content, dir) = update_test_content("drop");
        let never = || std::future::pending::<Option<(usize, usize)>>();
        let ms10 = std::time::Duration::from_millis(10);

        // Automatic and cancelled: silent.
        let auto = content.begin_update_check(Auto).unwrap();
        let check = content.clone().check_updates_with(auto, never());
        let _ = tokio::time::timeout(ms10, check).await;
        let uc = content.update_check_snapshot();
        assert!(!uc.checking);
        assert_eq!(uc.generation, 0);

        // Manual and cancelled: the presser hears about it.
        let manual = content
            .begin_update_check(Manual)
            .expect("gate stayed shut");
        let check = content.clone().check_updates_with(manual, never());
        let _ = tokio::time::timeout(ms10, check).await;
        let uc = content.update_check_snapshot();
        assert!(!uc.checking);
        assert_eq!(uc.generation, 1);
        assert_eq!(uc.message, "Update check was interrupted");
        assert!(content.begin_update_check(Manual).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whenever the gate reads as open, the previous check's result is already
    /// posted, so a check that starts in that moment cannot have its
    /// "Checking…" overwritten by the old result. Races the release on another
    /// thread against an observer that holds the gate lock while it looks.
    #[tokio::test]
    async fn the_result_is_posted_before_the_gate_opens() {
        use crate::update_gate::{CheckTrigger::Manual, Decision};
        let (content, dir) = update_test_content("order");
        for round in 0..200 {
            let token = content.begin_update_check(Manual).unwrap();
            content.set_update_check(true, "Checking for updates…");
            let expected = format!("done {round}");
            let msg = expected.clone();
            let releaser = std::thread::spawn(move || token.complete(&msg));
            loop {
                let gate = content.update_gate.lock().unwrap();
                if gate.decide(Manual, now_secs()) == Decision::Run {
                    let uc = content.update_check.lock().unwrap();
                    assert_eq!(uc.message, expected, "gate opened before the result");
                    assert!(!uc.checking);
                    break;
                }
                drop(gate);
                std::thread::yield_now();
            }
            releaser.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With a custom relay the content layer publishes there, not to its own store.
    #[tokio::test]
    async fn content_runs_on_a_relay_it_does_not_own() {
        // A relay that is emphatically not ours: its own store, its own socket.
        let theirs = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(crate::mesh_relay::serve_on(theirs.clone(), listener));

        let dir = tmp("custom-relay");
        let _ = std::fs::remove_dir_all(&dir);
        let backend = Arc::new(crate::remote_backend::RemoteBackend::new(format!(
            "ws://{addr}"
        )));
        let content = Content::open_with_relay(&dir, Some(backend)).unwrap();

        // Publishing through the content layer lands on their relay, not ours.
        let site = build_test_site(&[("/index.html", b"hi")], None, Some("Remote"));
        content
            .relay()
            .publish(site.manifest.clone())
            .await
            .unwrap();
        assert_eq!(theirs.count(), 1, "the event went to the custom relay");

        // And reads come back through the seam.
        let found = nsite_deck::seams::newest_in_slot(
            content.relay().as_ref(),
            nsite_deck::KIND_ROOT,
            &site.author,
            None,
        )
        .await
        .unwrap();
        assert_eq!(found.map(|e| e.id), Some(site.manifest.id));

        // The Storage screen is told the built-in store is no longer serving.
        let cache = content.cache_view();
        assert!(cache.external_relay, "usage must report the swap");
        assert_eq!(cache.relay_events, 0, "our store holds nothing now");
        assert!(content.relay_store().is_none());

        // Wiping is ours only: their relay keeps its events.
        content.wipe().await.unwrap();
        assert_eq!(
            theirs.count(),
            1,
            "a custom relay's contents are not ours to clear"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An nsite actually renders with its manifest on a relay we do not own.
    ///
    /// The seam test above proves publish and slot-read work. This proves the
    /// thing a user would notice: manifest on the remote relay, blobs local,
    /// and the gateway serving the page. That split is the normal shape when
    /// only the relay is swapped, so it is worth pinning rather than assuming.
    #[tokio::test]
    async fn the_gateway_serves_a_site_whose_manifest_lives_on_a_custom_relay() {
        let theirs = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(crate::mesh_relay::serve_on(theirs.clone(), listener));

        let dir = tmp("custom-relay-gateway");
        let _ = std::fs::remove_dir_all(&dir);
        let backend = Arc::new(crate::remote_backend::RemoteBackend::new(format!(
            "ws://{addr}"
        )));
        let content = Content::open_with_relay(&dir, Some(backend)).unwrap();

        // Import the usual way: blobs to the local store, manifest to the relay
        // — which now happens to be someone else's.
        let site = build_test_site(&[("/index.html", b"<h1>remote</h1>")], None, None);
        nsite_deck::import_site(
            content.relay().as_ref(),
            content.blobs().as_ref(),
            site.manifest.clone(),
            &site.blobs,
        )
        .await
        .expect("import");
        assert_eq!(theirs.count(), 1, "the manifest went to the custom relay");

        let host = format!("{}.nsite", site.author.to_bech32().unwrap());
        let resp = nsite_deck::serve(
            &content.active_backend(),
            content.blobs().as_ref(),
            &host,
            "/",
            None,
        )
        .await;

        assert_eq!(resp.status, 200, "the page must render");
        assert_eq!(resp.body, b"<h1>remote</h1>");

        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(super) fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-content-test-{}-{}", std::process::id(), tag))
    }

    /// Write a generated site to a bundle dir (`manifest.json` + `blobs/`).
    pub(super) fn write_bundle(dir: &Path, site: &nsite_deck::testing::TestSite) {
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
            &[
                ("/index.html", b"<h1>hi</h1>"),
                ("/app.js", b"console.log(1)"),
            ],
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
        settle_blob_stats(&content);
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
    async fn wipe_cache_keeps_pinned_drops_the_rest() {
        let dir = tmp("wipe-cache");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        // Two distinct sites; both land in the Library as pinned on import.
        let keep = build_test_site(&[("/index.html", b"<h1>keep</h1>")], None, Some("Keep"));
        let drop = build_test_site(&[("/index.html", b"<h1>drop</h1>")], None, Some("Drop"));
        let keep_host = format!("{}.nsite", keep.author.to_bech32().unwrap());
        let drop_host = format!("{}.nsite", drop.author.to_bech32().unwrap());

        for (tag, site) in [("keep", &keep), ("drop", &drop)] {
            let bundle = dir.join(tag);
            write_bundle(&bundle, site);
            assert_eq!(
                content.import_dir(&bundle).await.unwrap(),
                SyncOutcome::Ready
            );
        }
        assert_eq!(content.cache_view().relay_events, 2);
        settle_blob_stats(&content);
        assert_eq!(content.cache_view().blob_count, 2);

        // Unpin the second site (no longer a kept app), then drop the cache.
        content.remove_from_library(&SiteAddr {
            author: drop.author,
            d_tag: None,
        });
        content.wipe_cache(None).await.unwrap();

        // The pinned site still serves from local stores; the unpinned one is gone.
        assert_eq!(content.cache_view().relay_events, 1);
        assert_eq!(content.cache_view().blob_count, 1);
        assert_eq!(content.gateway_get(&keep_host, "/", None).await.status, 200);
        assert_eq!(content.gateway_get(&drop_host, "/", None).await.status, 503);
        assert_eq!(content.library_snapshot().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Clear local database" keeps the user's own profile and relay lists. They are
    /// published once, at first napplet use, and `user.nsec` outlives the
    /// wipe, so nothing would ever publish them again: without this the
    /// user's outbox plan degrades to fallback and napplets see a bare
    /// pubkey. The same kinds by anyone else are cache, and go.
    #[tokio::test]
    async fn wipe_cache_keeps_the_users_profile_and_relay_list() {
        let dir = tmp("wipe-own-profile");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        let user = Keys::generate();
        let other = Keys::generate();
        let mut own = Vec::new();
        let mut theirs = Vec::new();
        for (keys, out) in [(&user, &mut own), (&other, &mut theirs)] {
            let profile = EventBuilder::new(Kind::Metadata, r#"{"name":"x"}"#)
                .sign_with_keys(keys)
                .unwrap();
            let relays = crate::outbox::own_relay_list(keys).unwrap();
            let dm_relays = crate::outbox::own_dm_relay_list(keys).unwrap();
            for event in [profile, relays, dm_relays] {
                content.relay().publish(event.clone()).await.unwrap();
                out.push(event.id);
            }
        }
        assert_eq!(content.cache_view().relay_events, 6);

        content.wipe_cache(Some(user.public_key())).await.unwrap();

        let left = content.relay().query(&[Filter::new()]).await.unwrap();
        let left: Vec<nostr::EventId> = left.into_iter().map(|e| e.id).collect();
        for id in &own {
            assert!(left.contains(id), "the user's own event was wiped");
        }
        for id in &theirs {
            assert!(!left.contains(id), "another author's profile survived");
        }
        assert_eq!(left.len(), 3);
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
            assert_eq!(
                content.library_snapshot().len(),
                1,
                "import should pin to Library"
            );
        }

        // Restart: a fresh Content over the same dir. The status map starts empty;
        // refresh_library_status re-lists the pinned site as ready.
        let content = Arc::new(Content::open(&dir).unwrap());
        assert_eq!(
            content.library_snapshot().len(),
            1,
            "Library persists on disk"
        );
        assert!(
            content.sites_snapshot().is_empty(),
            "status map is empty before refresh"
        );

        content.clone().refresh_library_status().await;
        let sites = content.sites_snapshot();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].state, "ready");
        assert_eq!(sites[0].host, host);
        assert_eq!(sites[0].title, "Persisted");

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
        content.clone().open_site(addr).await;

        let sites = content.sites_snapshot();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].state, "unreachable");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
pub(crate) mod library_kind_tests {
    use super::tests::{tmp, write_bundle};
    use super::*;
    use nostr::nips::nip19::ToBech32;
    use nostr::EventBuilder;
    use nsite_deck::testing::build_test_site;

    fn entry(kind: LibraryKind, d_tag: &str) -> LibraryItem {
        LibraryItem {
            author_npub: "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr"
                .to_string(),
            d_tag: Some(d_tag.to_string()),
            title: String::new(),
            url_host: String::new(),
            pinned: true,
            added_at: 0,
            kind,
            granted: Vec::new(),
            denied: Vec::new(),
            pointer: String::new(),
            reviewed: Vec::new(),
            preinstalled: false,
            archetypes: None,
        }
    }

    /// A napplet must never reach the nsite sync engine. It has no 15128/35128
    /// manifest, so a sync started for one never finishes and leaves a tile
    /// stuck syncing beside the napplet's own — which is exactly what happened
    /// the first time a napplet was installed on a device.
    #[test]
    fn a_napplet_is_not_an_nsite_address() {
        assert!(library_addr(&entry(LibraryKind::Napplet, "dingdong")).is_none());
        assert!(library_addr(&entry(LibraryKind::Nsite, "bitchat")).is_some());
    }

    /// An author may publish an nsite and a napplet under the same `d` tag.
    /// They are two Library entries; adding one must not turn the other into
    /// it — which stopped the nsite syncing and left the napplet's tile
    /// pointing at an nsite host.
    #[tokio::test]
    async fn an_nsite_and_a_napplet_with_one_d_tag_are_two_entries() {
        let dir = tmp("library-kinds");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let author = nostr::Keys::generate().public_key();
        let npub = author.to_bech32().unwrap();
        let addr = SiteAddr {
            author,
            d_tag: Some("bitchat".into()),
        };

        content.add_to_library(&addr, Some("Bitchat site"), 1);
        content.add_napplet_to_library(
            &npub,
            Some("bitchat"),
            Some("Bitchat app"),
            "bitchat.napplet.localhost",
            vec!["relay".into()],
            vec!["relay".into()],
            "naddr1x",
            2,
        );
        let lib = content.library_snapshot();
        assert_eq!(lib.len(), 2, "one entry swallowed the other");
        let site = lib.iter().find(|i| i.kind == LibraryKind::Nsite).unwrap();
        let app = lib.iter().find(|i| i.kind == LibraryKind::Napplet).unwrap();
        assert!(library_addr(site).is_some(), "the nsite stopped being one");
        assert_eq!(app.granted, vec!["relay".to_string()]);
        assert!(site.granted.is_empty());

        // And the other way round.
        content.add_to_library(&addr, Some("Bitchat site again"), 3);
        assert_eq!(content.library_snapshot().len(), 2);
        assert_eq!(
            content
                .napplet_grants(&npub, Some("bitchat"))
                .unwrap()
                .granted,
            vec!["relay".to_string()],
            "re-adding the nsite touched the napplet's grants"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Forgetting the nsite half of a shared `d` tag leaves the napplet half:
    /// its entry, its grants and its pointer. `remove_from_library` used to
    /// match on `(author, d)` alone and took both.
    #[tokio::test]
    async fn forgetting_the_nsite_keeps_its_napplet_twin() {
        let dir = tmp("library-forget-twin");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let author = nostr::Keys::generate().public_key();
        let npub = author.to_bech32().unwrap();
        let addr = SiteAddr {
            author,
            d_tag: Some("bitchat".into()),
        };

        content.add_to_library(&addr, Some("Bitchat site"), 1);
        content.add_napplet_to_library(
            &npub,
            Some("bitchat"),
            Some("Bitchat app"),
            "bitchat.napplet.localhost",
            vec!["relay".into()],
            vec!["relay".into()],
            "naddr1x",
            2,
        );
        assert_eq!(content.library_snapshot().len(), 2);

        content.forget_site(&addr);

        let lib = content.library_snapshot();
        assert_eq!(lib.len(), 1, "forgetting the nsite took the napplet too");
        let app = &lib[0];
        assert_eq!(app.kind, LibraryKind::Napplet);
        assert_eq!(app.granted, vec!["relay".to_string()]);
        assert_eq!(app.pointer, "naddr1x");
        assert!(
            content.napplet_grants(&npub, Some("bitchat")).is_some(),
            "the napplet's grants went with the nsite"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A napplet-only Library does not make the same-slot nsite "installed":
    /// `is_in_library` is what decides whether a manifest a neighbor shares
    /// gets every blob staged and a tile on the grid.
    #[tokio::test]
    async fn a_napplet_entry_does_not_make_the_nsite_twin_installed() {
        let dir = tmp("library-napplet-only");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let author = nostr::Keys::generate().public_key();
        let npub = author.to_bech32().unwrap();

        content.add_napplet_to_library(
            &npub,
            Some("bitchat"),
            Some("Bitchat app"),
            "bitchat.napplet.localhost",
            vec!["relay".into()],
            vec!["relay".into()],
            "naddr1x",
            2,
        );
        let addr = SiteAddr {
            author,
            d_tag: Some("bitchat".into()),
        };
        assert!(
            !content.is_in_library(&addr),
            "a napplet entry passed for the nsite twin"
        );

        // And the nsite itself still counts once it is added.
        content.add_to_library(&addr, Some("Bitchat site"), 3);
        assert!(content.is_in_library(&addr));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A manifest kept because a lookup saw it — an nsite's or a napplet's —
    /// installs nothing: no Library entry, no site status, no napplet
    /// status, even after the Library is re-read.
    #[tokio::test]
    async fn a_kept_manifest_installs_nothing() {
        use myco_napplet_runtime::testing::NappletBuilder;
        let dir = tmp("keep-seen-not-installed");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let site = build_test_site(&[("/index.html", b"<h1>seen</h1>")], Some("blog"), None);
        let napplet = NappletBuilder::new().d_tag(Some("game")).build();

        content
            .keep_seen([&site.manifest, &napplet.manifest])
            .unwrap()
            .await
            .unwrap();
        assert_eq!(content.cache_view().relay_events, 2, "both were kept");

        Arc::clone(&content).refresh_library_status().await;
        assert!(content.library_snapshot().is_empty());
        assert!(content.sites_snapshot().is_empty());
        assert!(content.napplet_status_snapshot().is_empty());
        assert!(!content.is_in_library(&SiteAddr {
            author: site.author,
            d_tag: Some("blog".into()),
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A newer manifest kept for an installed site does not change what the
    /// site serves — the pinned version does — and "Clear local database" drops the
    /// newer one and leaves the pinned version serving.
    #[tokio::test]
    async fn a_kept_newer_manifest_leaves_the_installed_version_serving() {
        let dir = tmp("keep-seen-pinned");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let keys = nostr::Keys::generate();
        let v1 = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"<h1>v1</h1>")],
            None,
            Some("Site"),
        );
        let bundle = dir.join("v1");
        write_bundle(&bundle, &v1);
        assert_eq!(
            content.import_dir(&bundle).await.unwrap(),
            SyncOutcome::Ready
        );
        let host = format!("{}.nsite", keys.public_key().to_bech32().unwrap());

        // v2 names bytes this phone does not have.
        let v2_site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"<h1>v2</h1>")],
            None,
            Some("Site"),
        );
        let v2 = EventBuilder::new(v2_site.manifest.kind, "")
            .tags(v2_site.manifest.tags.clone())
            .custom_created_at(nostr::Timestamp::from(
                v1.manifest.created_at.as_secs() + 60,
            ))
            .sign_with_keys(&keys)
            .unwrap();
        content.keep_seen([&v2]).unwrap().await.unwrap();
        let newest = nsite_deck::seams::newest_in_slot(
            content.relay().as_ref(),
            nsite_deck::KIND_ROOT,
            &keys.public_key(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(newest.map(|e| e.id), Some(v2.id), "v2 was not kept");

        let served = content.gateway_get(&host, "/", None).await;
        assert_eq!(served.status, 200);
        assert_eq!(served.body, b"<h1>v1</h1>");

        // A client asking this phone's relay gets v1 too — the version whose
        // files this phone has.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(crate::mesh_relay::serve_on(
            content.pinned_relay(),
            listener,
        ));
        let answered = crate::ip_source::query_relay(
            &url,
            serde_json::json!({
                "kinds": [nsite_deck::KIND_ROOT],
                "authors": [keys.public_key().to_hex()],
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            answered.iter().map(|e| e.id).collect::<Vec<_>>(),
            [v1.manifest.id],
            "the relay offered a version without its files"
        );
        // A filter only the newer version matches gets nothing, not a stale v1.
        let answered = crate::ip_source::query_relay(
            &url,
            serde_json::json!({
                "kinds": [nsite_deck::KIND_ROOT],
                "authors": [keys.public_key().to_hex()],
                "since": v2.created_at.as_secs(),
            }),
        )
        .await
        .unwrap();
        assert!(
            answered.is_empty(),
            "the pin was served to a filter it does not match"
        );

        content.wipe_cache(None).await.unwrap();
        let served = content.gateway_get(&host, "/", None).await;
        assert_eq!(served.status, 200, "the wipe took the installed site down");
        assert_eq!(served.body, b"<h1>v1</h1>");
        assert_eq!(content.library_snapshot().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A napplet installed before pinning existed has no pin. The startup
    /// pass pins the version whose bytes are here, so a newer manifest kept
    /// afterwards — bytes not here — does not take its place: it still
    /// opens v1.
    #[tokio::test]
    async fn an_unpinned_installed_napplet_still_opens_v1_after_v2_is_kept() {
        use crate::napplet::ManifestStore;
        use myco_napplet_runtime::testing::NappletBuilder;
        let dir = tmp("keep-seen-legacy-napplet");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let keys = nostr::Keys::generate();
        let v1 = NappletBuilder::new()
            .keys(keys.clone())
            .d_tag(Some("game"))
            .files(&[("/index.html", b"<h1>v1</h1>")])
            .created_at(1_000)
            .build();
        for (_, bytes) in &v1.blobs {
            content.blobs().put(bytes).await.unwrap();
        }
        // Installed the old way: manifest and bytes, no pin.
        content.relay().publish(v1.manifest.clone()).await.unwrap();
        content.add_napplet_to_library(
            &keys.public_key().to_bech32().unwrap(),
            Some("game"),
            Some("Game"),
            "game.napplet.localhost",
            vec![],
            vec![],
            "naddr1game",
            1,
        );
        assert!(content.active_manifests.lock().unwrap().is_empty());

        Arc::clone(&content).refresh_library_status().await;

        let v2 = NappletBuilder::new()
            .keys(keys.clone())
            .d_tag(Some("game"))
            .files(&[("/index.html", b"<h1>v2</h1>")])
            .created_at(2_000)
            .build();
        content.keep_seen([&v2.manifest]).unwrap().await.unwrap();
        let newest = nsite_deck::seams::newest_in_slot(
            content.relay().as_ref(),
            myco_napplet_runtime::KIND_NAMED,
            &keys.public_key(),
            Some("game"),
        )
        .await
        .unwrap();
        assert_eq!(newest.map(|e| e.id), Some(v2.manifest.id));

        let current = content
            .current(
                myco_napplet_runtime::KIND_NAMED,
                &keys.public_key(),
                Some("game"),
            )
            .await
            .unwrap()
            .expect("the napplet lost its manifest");
        assert_eq!(current.id, v1.manifest.id, "v2 took v1's place");
        let opened = myco_napplet_runtime::resolve(current, content.blobs().as_ref())
            .await
            .expect("v1 does not open");
        assert!(opened.index_html.contains("v1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Profiles and manifests seen are kept only in the embedded store. With
    /// a custom relay there is no tap: browsing is not written to someone
    /// else's relay, where "Clear local database" could not reach it.
    #[tokio::test]
    async fn a_custom_relay_turns_keeping_off() {
        let dir = tmp("keep-seen-custom-relay");
        let _ = std::fs::remove_dir_all(&dir);
        let backend = Arc::new(crate::remote_backend::RemoteBackend::new(
            "ws://127.0.0.1:9".to_string(),
        ));
        let content = Content::open_with_relay(&dir, Some(backend)).unwrap();
        let profile = EventBuilder::metadata(&nostr::Metadata::new().name("x"))
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        assert!(content.keep_seen([&profile]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keeping an event moves it out of the cache; "Clear cache" leaves what
    /// is kept alone; "Clear local database"'s retain leaves the cache to the clear
    /// the runtime spawns beside it.
    #[tokio::test]
    async fn keeping_dedups_and_both_clears_behave() {
        let dir = tmp("cache-clears");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let keys = nostr::Keys::generate();
        let both = EventBuilder::text_note("both")
            .sign_with_keys(&keys)
            .unwrap();
        let passing = EventBuilder::text_note("passing")
            .sign_with_keys(&keys)
            .unwrap();
        content.cache_relay().publish(both.clone()).await.unwrap();
        content
            .cache_relay()
            .publish(passing.clone())
            .await
            .unwrap();
        content.relay().publish(both.clone()).await.unwrap();
        assert!(!content.event_cache().contains(&both.id.to_bytes()));
        assert!(content.event_cache().contains(&passing.id.to_bytes()));
        content.upkeep_caches().await;
        assert_eq!(
            content.relay().query(&[Filter::new()]).await.unwrap().len(),
            2
        );

        content.clear_cache().await.unwrap();
        let left = content.relay().query(&[Filter::new()]).await.unwrap();
        assert_eq!(left.iter().map(|e| e.id).collect::<Vec<_>>(), [both.id]);
        let blob = content.cache_blobs().put(b"fetched").await.unwrap();
        assert!(
            content.blobs().has(&blob).await,
            "reads miss the blob cache"
        );
        assert!(
            !content.blobs_local().unwrap().has(&blob).await,
            "a fetch was kept"
        );

        content
            .cache_relay()
            .publish(passing.clone())
            .await
            .unwrap();
        // "Clear local database" is the retain plus a cache clear the runtime spawns
        // beside it; the retain alone leaves the cache for that.
        content.wipe_cache(None).await.unwrap();
        assert!(content.event_cache().contains(&passing.id.to_bytes()));
        content.clear_cache().await.unwrap();
        assert_eq!(content.event_cache().stats().count, 0);
        assert_eq!(content.blob_cache().stats().count, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Storage screen reads these keys (`AppCoreClient.kt`, `cacheTier`)
    /// and treats a missing one as 0 — so a rename here would compile, pass,
    /// and show an empty cache. Pin them.
    #[test]
    fn the_cache_view_keeps_the_keys_the_app_reads() {
        let mut view = CacheView::empty();
        view.event_cache = myco_cache::CacheStats {
            count: 1,
            bytes: 2,
            limit: 3,
        };
        view.blob_cache.limit = 4;
        let v = serde_json::to_value(&view).unwrap();
        assert_eq!(v["eventCache"]["count"], 1);
        assert_eq!(v["eventCache"]["bytes"], 2);
        assert_eq!(v["eventCache"]["limit"], 3);
        assert_eq!(v["blobCache"]["limit"], 4);
        for key in [
            "relayEvents",
            "blobCount",
            "usedBytes",
            "externalRelay",
            "externalBlobs",
        ] {
            assert!(v.get(key).is_some(), "{key} is gone");
        }
    }

    /// A PeerSource over fixed events and blobs.
    struct FixedSource {
        manifest: Option<Event>,
        blobs: Vec<(String, Vec<u8>)>,
    }

    #[async_trait]
    impl PeerSource for FixedSource {
        async fn fetch_manifest(
            &self,
            _author: &PublicKey,
            _d_tag: Option<&str>,
        ) -> anyhow::Result<Option<Event>> {
            Ok(self.manifest.clone())
        }

        async fn fetch_blob(
            &self,
            sha256_hex: &str,
            _servers: &[String],
        ) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(self
                .blobs
                .iter()
                .find(|(h, _)| h == sha256_hex)
                .map(|(_, b)| b.clone()))
        }
    }

    /// Opening a site for the first time asks the source for its manifest,
    /// rather than staging a stale one kept from browsing, and pins exactly
    /// the version whose files it fetched.
    #[tokio::test]
    async fn a_first_open_fetches_past_a_stale_kept_manifest() {
        let dir = tmp("keep-seen-first-open");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let keys = nostr::Keys::generate();
        let old = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"<h1>old</h1>")],
            None,
            Some("Site"),
        );
        let new_site = nsite_deck::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"<h1>new</h1>")],
            None,
            Some("Site"),
        );
        let new = EventBuilder::new(new_site.manifest.kind, "")
            .tags(new_site.manifest.tags.clone())
            .custom_created_at(nostr::Timestamp::from(
                old.manifest.created_at.as_secs() + 60,
            ))
            .sign_with_keys(&keys)
            .unwrap();
        content.keep_seen([&old.manifest]).unwrap().await.unwrap();
        // The source has the new version and both versions' files.
        content.set_source(Arc::new(FixedSource {
            manifest: Some(new.clone()),
            blobs: old.blobs.iter().chain(&new_site.blobs).cloned().collect(),
        }));

        let addr = SiteAddr {
            author: keys.public_key(),
            d_tag: None,
        };
        Arc::clone(&content).open_site(addr).await;

        let host = format!("{}.nsite", keys.public_key().to_bech32().unwrap());
        let served = content.gateway_get(&host, "/", None).await;
        assert_eq!(served.status, 200);
        assert_eq!(
            served.body, b"<h1>new</h1>",
            "the stale kept copy was installed"
        );
        let pinned: Vec<_> = content
            .active_manifests
            .lock()
            .unwrap()
            .values()
            .map(|e| e.id)
            .collect();
        assert_eq!(pinned, [new.id]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Clear local database" keeps installed apps working. A napplet is one manifest
    /// and one blob; both survive, or the tile survives and the app does not.
    #[tokio::test]
    async fn wipe_cache_keeps_an_installed_napplet() {
        use myco_napplet_runtime::testing::NappletBuilder;
        let dir = tmp("wipe-napplet");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        let napplet = NappletBuilder::new().d_tag(Some("ding")).build();
        for (_, bytes) in &napplet.blobs {
            content.blobs().put(bytes).await.unwrap();
        }
        content
            .relay()
            .publish(napplet.manifest.clone())
            .await
            .unwrap();
        // Something else to prove the wipe still wipes.
        content.blobs().put(b"stray bytes").await.unwrap();
        // The first count runs in the background; wait for it.
        if let Some(blobs) = &content.blobs_local {
            blobs.stats();
        }
        assert_eq!(content.cache_view().blob_count, 2);

        let npub = napplet.author.to_bech32().unwrap();
        content.add_napplet_to_library(
            &npub,
            Some("ding"),
            Some("Ding"),
            "ding.napplet.localhost",
            vec![],
            vec![],
            "naddr1ding",
            1,
        );
        content.wipe_cache(None).await.unwrap();

        assert_eq!(
            content.cache_view().relay_events,
            1,
            "the napplet manifest was wiped"
        );
        assert_eq!(
            content.cache_view().blob_count,
            1,
            "the index blob was wiped"
        );
        let kept = nsite_deck::seams::newest_in_slot(
            content.relay().as_ref(),
            myco_napplet_runtime::KIND_NAMED,
            &napplet.author,
            Some("ding"),
        )
        .await
        .unwrap();
        assert_eq!(kept.map(|e| e.id), Some(napplet.manifest.id));
        assert_eq!(content.library_snapshot().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default is the safe one for every entry written before napplets
    /// existed: they are nsites, and they keep syncing.
    #[test]
    fn an_entry_with_no_kind_recorded_is_an_nsite() {
        let stored = r#"{
            "authorNpub": "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr",
            "dTag": "bitchat", "title": "Bitchat", "urlHost": "x",
            "pinned": true, "addedAt": 0
        }"#;
        let item: LibraryItem = serde_json::from_str(stored).unwrap();
        assert_eq!(item.kind, LibraryKind::Nsite);
        assert!(library_addr(&item).is_some());
    }
}
