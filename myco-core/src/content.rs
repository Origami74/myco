//! The Myco content layer: the embedded relay + Blossom stores wired to the
//! `nsite-deck` gateway engine, plus the Library and per-site sync status the FFI
//! surfaces. This is the in-process glue (`myco-core` is the only crate that names
//! a concrete relay/Blossom). The localhost `:4870` / `:24243` sockets and the
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
use nostr::{Event, EventBuilder, Filter, Keys, Kind, PublicKey, Tag};
use nsite_deck::gateway::{self, Readiness};
use nsite_deck::seams::{BlobStore, PeerSource, RelayBackend};
use nsite_deck::{sync, GatewayResponse, SiteAddr, SyncOutcome};
use serde::{Deserialize, Serialize};

use crate::mesh_relay::{Inbound, Origin};
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
    /// What this peer may do to us. Not exposed in the UI yet — every peer gets
    /// the defaults — but stored per peer so turning a knob later is a UI change
    /// rather than a storage migration.
    #[serde(default)]
    pub perms: PeerPerms,
}

/// Per-peer permissions: what a **paired** peer is allowed to do against this
/// node. Pairing itself is not covered here — that is the auth plane's job, and
/// it happens before any of these apply.
///
/// Read every flag as a grant *we* make to *them*. "Multihop" is ambiguous on
/// its own, so it means specifically whether their traffic travels further
/// through us — not anything we send them. Both multihop flags are expressed as
/// per-peer ttl clamps rather than a separate check, so they reuse the machinery
/// the push and pull planes already have. See
/// `reference/thinning-custom-relay.md` (D10).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerPerms {
    /// May open a `REQ` and receive our stored events.
    #[serde(default = "yes")]
    pub relay_read: bool,
    /// Their `REQ` may be forwarded to our other peers (a hop-budget clamp).
    #[serde(default = "yes")]
    pub relay_read_multihop: bool,
    /// May publish events to us.
    #[serde(default = "yes")]
    pub relay_write: bool,
    /// Events from them may be forwarded onward by us (a hop-budget clamp).
    #[serde(default = "yes")]
    pub relay_write_multihop: bool,
    /// May `GET` / `HEAD` blobs from us.
    #[serde(default = "yes")]
    pub blossom_read: bool,
    /// May `PUT /upload` to us. **Off by default** — this is the one that costs
    /// us disk, and nothing in normal operation needs it: propagation is
    /// pull-based, so peers fetch blobs from the holder rather than pushing them.
    /// The dev-menu speedtest is the only caller, and it reports the refusal.
    #[serde(default = "no")]
    pub blossom_write: bool,
}

fn yes() -> bool {
    true
}
fn no() -> bool {
    false
}

impl Default for PeerPerms {
    fn default() -> Self {
        Self {
            relay_read: yes(),
            relay_read_multihop: yes(),
            relay_write: yes(),
            relay_write_multihop: yes(),
            blossom_read: yes(),
            blossom_write: no(),
        }
    }
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

/// One entry per site, keeping the freshest copy seen.
///
/// The same nsite legitimately turns up on several Circle peers' relays — the
/// query runs per holder — but to someone browsing "around you" that is one
/// app, not one per person who happens to have it. Ties on `updated_at` keep
/// the first seen, so the result is stable when nobody has a newer version.
///
/// Keeping the newest also picks the right holder to pull from: whoever
/// answered with the most recent manifest has the version we would want.
fn dedup_by_host(found: Vec<DiscoveredNsite>) -> Vec<DiscoveredNsite> {
    let mut best: Vec<DiscoveredNsite> = Vec::new();
    for d in found {
        match best.iter_mut().find(|b| b.host == d.host) {
            Some(existing) => {
                if d.updated_at > existing.updated_at {
                    *existing = d;
                }
            }
            None => best.push(d),
        }
    }
    best
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
            external_relay: false,
            external_blobs: false,
        }
    }
}

/// An incoming pairing request awaiting the user's accept/decline (surfaced to the
/// UI as a pop-up). The requester scanned our QR; `secret` is the one-time value
/// from that QR, echoed back to prove they actually saw it.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequestView {
    pub npub: String,
    pub name: String,
    pub secret: String,
}

/// An invite we sent that hasn't been accepted yet.
///
/// A pair request is delivered over the mesh, so it can fail simply because
/// there is no route to that peer *yet* — a bump between two phones that have
/// not met on the mesh is the normal case. Recording it means we can say
/// "waiting" instead of silently dropping it, and refuse to send a second one
/// for the same peer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundPairView {
    pub npub: String,
    pub name: String,
    /// Unix seconds when we first tried, so the UI can age it.
    pub since: u64,
}

/// Mutual-pairing handshake events, POSTed point-to-point to a peer's **auth
/// service** at `:4871` (never gossiped, and never stored — the relay refuses
/// these kinds from every source). Signed by the **device** key, which is the
/// pairing identity, and carrying a NIP-40 expiry the auth service checks on
/// receipt. See `docs/design/identity-pairing.md`.
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

/// Time budget stamped on a pull this node originates. Relative, and only bounds
/// how long a node downstream holds query state — late results are not an error,
/// they simply arrive to whoever is still listening
/// (`reference/thinning-custom-relay.md`, D8).
const PULL_BUDGET_MS: u32 = 10_000;

/// Longest a single forwarded hop will wait on a peer, used when no budget rode
/// in (an older peer, or a pull that never carried one). A budget that did
/// arrive only ever shortens this.
const PULL_HOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The mesh access gate backing the relay + Blossom servers: content (reads, chat,
/// manifests, blobs) is restricted to **paired** (Circle) peers, and what a paired
/// peer may do is its own [`PeerPerms`] record.
///
/// There are **no exceptions**. Pairing used to need one — an unpaired peer had
/// to be able to publish the handshake kinds to bootstrap — but that now happens
/// on the auth plane (`crate::auth_service`), so the content ports can simply
/// require membership (`reference/thinning-custom-relay.md`, D6).
///
/// Holds the [`Content`] so the live Circle is consulted per request: adding a
/// peer, removing one, or changing a permission takes effect immediately.
pub struct CircleGate {
    content: Arc<Content>,
}

impl CircleGate {
    pub fn new(content: Arc<Content>) -> Self {
        Self { content }
    }
}

impl crate::mesh_relay::PeerGate for CircleGate {
    fn may_read(&self, ip: IpAddr) -> bool {
        self.content.perms_for_ip(ip).is_some_and(|p| p.relay_read)
    }

    fn may_connect(&self, ip: IpAddr) -> bool {
        // Membership alone, checked before the WebSocket upgrade. A peer with a
        // narrower permission set still gets a socket; what it may do with it is
        // decided per message below.
        self.content.perms_for_ip(ip).is_some()
    }

    fn may_publish(&self, ip: IpAddr, kind: u16) -> bool {
        // Pairing kinds are auth-plane control traffic, not content. They are
        // refused here from every source, paired or not, so nothing writes them
        // into a store that may not even be ours (D6).
        if kind == KIND_PAIR_REQUEST || kind == KIND_PAIR_ACCEPT || kind == KIND_PAIR_REMOVE {
            return false;
        }
        self.content.perms_for_ip(ip).is_some_and(|p| p.relay_write)
    }

    fn max_req_ttl(&self, ip: IpAddr) -> u8 {
        match self.content.perms_for_ip(ip) {
            Some(p) if p.relay_read_multihop => crate::mesh_relay::MAX_REQ_TTL,
            _ => 0,
        }
    }
}

/// The content layer. Cheap to `Arc`-clone; the gateway path clones one out of the
/// `AppRuntime` mutex and serves without holding it.
pub struct Content {
    /// The event store, through the seam — so it can be the embedded relay or
    /// any other NIP-01 relay (`reference/thinning-custom-relay.md`, D3).
    relay: Arc<dyn RelayBackend>,
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
    /// The blob store, through the seam — embedded or someone else's Blossom.
    blobs: Arc<dyn BlobStore>,
    /// The custom Blossom, when one is configured, so its reachability can be
    /// reported the same way the relay's is.
    blobs_remote: Option<Arc<crate::remote_blobs::RemoteBlobStore>>,
    /// The embedded blob store, when that is what `blobs` points at. Holds the
    /// usage counts and the selective retain, neither of which is BUD-01.
    blobs_local: Option<Arc<FsBlobStore>>,
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
    /// The device keypair. Behind an `Arc` because a remote blob store needs it
    /// to sign BUD-01 upload authorizations, and it is set after construction.
    device_keys: Arc<Mutex<Option<Keys>>>,
    /// User-chosen device label (memorable name). Set by the app on launch and on
    /// rename; stamped on outgoing pair events so peers show the chosen name.
    /// Falls back to a name derived from the npub when unset.
    device_name_override: Mutex<Option<String>>,
    /// Incoming pair requests awaiting the user's accept/decline (UI pop-up).
    pending_pairs: Mutex<Vec<PairRequestView>>,
    /// Invites we sent that are still unanswered (see [`OutboundPairView`]).
    outbound_pairs: Mutex<Vec<OutboundPairView>>,
    /// Persistent WS connections to peers' relays, so chat fan-out and manifest
    /// fetches don't pay a fresh connect per message (slow over BLE). `Arc` so a
    /// mesh `PeerSource` can borrow the same pool for its manifest REQs.
    peer_relays: Arc<crate::peer_relay::PeerRelayPool>,
    /// Raw filters of the subscriptions in-app clients currently have open on our
    /// loopback relay, keyed by the relay's per-connection sub key. Fed by the relay
    /// via the gossiper hooks; the core stores them **verbatim** (it never interprets
    /// kinds). On a Circle peer reappearing, these are replayed to it to pull the
    /// backlog the client missed — see [`Content::resync_from_peer`].
    active_local_subs: Mutex<HashMap<String, Vec<serde_json::Value>>>,
    /// The set of Circle peers the pool last reported as connected — diffed each
    /// keepwarm tick to spot the absent→present (reappeared) edge.
    prev_pool_connected: Mutex<HashSet<String>>,
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
    relay: &'a dyn RelayBackend,
    active: &'a Mutex<HashMap<String, Event>>,
}

#[async_trait]
impl RelayBackend for ActiveBackend<'_> {
    async fn publish(&self, event: Event) -> anyhow::Result<()> {
        self.relay.publish(event).await
    }
    /// Serve the **pinned** version of any site that has one, rather than the
    /// newest the store holds.
    ///
    /// The substitution happens here, on the way out, because the seam no longer
    /// has a slot-shaped read to override — everything goes through `query` now.
    /// A pinned event shares its slot with the one it replaces (same kind,
    /// author, and `d` tag), so anything that matched the newer one matches it.
    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        let mut out = self.relay.query(filters).await?;
        let active = self.active.lock().unwrap().clone();
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
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
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
        // A remote blob store signs its uploads with the device key, and the key
        // is loaded after construction — so share one holder rather than keeping
        // two copies that could fall out of step.
        let device_keys: Arc<Mutex<Option<Keys>>> = custom_blobs
            .as_ref()
            .map(|b| b.keys())
            .unwrap_or_else(|| Arc::new(Mutex::new(None)));
        let embedded = Arc::new(RelayStore::open(data_dir.join("relay"))?);
        let using_custom = custom.is_some();
        let relay: Arc<dyn RelayBackend> = match &custom {
            Some(remote) => remote.clone(),
            None => embedded.clone(),
        };
        // Kept only while it is the thing serving: the usage counts and the
        // selective retain it backs describe our store, not someone else's.
        let relay_store = (!using_custom).then_some(embedded);

        let embedded_blobs = Arc::new(FsBlobStore::open(data_dir.join("blossom"))?);
        let blobs: Arc<dyn BlobStore> = match &custom_blobs {
            Some(remote) => remote.clone(),
            None => embedded_blobs.clone(),
        };
        let blobs_local = custom_blobs.is_none().then_some(embedded_blobs);
        let library_path = data_dir.join("library.json");
        let library = load_library(&library_path);
        let circle_path = data_dir.join("circle.json");
        let circle = load_circle(&circle_path);
        let active_path = data_dir.join("active.json");
        let active_manifests = load_active(&active_path);
        Ok(Self {
            relay,
            relay_remote: custom,
            relay_store,
            blobs_remote: custom_blobs,
            blobs_local,
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
            device_keys: device_keys.clone(),
            device_name_override: Mutex::new(None),
            pending_pairs: Mutex::new(Vec::new()),
            outbound_pairs: Mutex::new(Vec::new()),
            peer_relays: Arc::new(crate::peer_relay::PeerRelayPool::new()),
            active_local_subs: Mutex::new(HashMap::new()),
            prev_pool_connected: Mutex::new(HashSet::new()),
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

    /// The event store (shared), for the mesh WS proxy in front of it.
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

    /// The embedded blob store, if that is what we are using. The mesh Blossom
    /// server needs the concrete one: it serves our own blobs to peers, and a
    /// custom server is reached by its own URL rather than proxied through us.
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

    // --- active version (what the gateway serves; docs/design/nsite-updates.md §1) ---

    /// The backend the gateway reads: serves the active (fully-downloaded) version,
    /// not necessarily the relay's newest.
    fn active_backend(&self) -> ActiveBackend<'_> {
        ActiveBackend {
            relay: self.relay.as_ref(),
            active: &self.active_manifests,
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
                match crate::ip_source::mesh_source_for(self.peer_relays.clone(), npub) {
                    Ok(mesh) => sources.push(Arc::new(mesh)),
                    Err(e) => tracing::warn!(error = %e, "skipping mesh source"),
                }
            }
        }
        for npub in self.circle_npubs() {
            if tried.insert(npub.clone()) {
                match crate::ip_source::mesh_source_for(self.peer_relays.clone(), &npub) {
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
            holder = ?holder,
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

        // Try each in order; the first that goes Ready wins. Keep the best
        // non-ready outcome (incomplete > unreachable) to report if none succeed.
        let mut best = SyncOutcome::Unreachable;
        for source in &sources {
            // Manifest already local → fetch only its (missing) blobs, no manifest
            // refetch. Otherwise do a full sync (manifest + blobs).
            let outcome = match &known {
                Some(manifest) => {
                    sync::stage_blobs(self.blobs.as_ref(), source.as_ref(), manifest, &progress)
                        .await
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
                            let _ = self.relay.publish(m.event.clone()).await;
                            self.set_active(&m.event);
                            let n = m.paths.len() as u64;
                            self.set_status_titled(
                                &addr,
                                m.title.as_deref(),
                                "ready",
                                n,
                                n,
                                "Ready",
                            );
                            self.add_to_library(&addr, m.title.as_deref(), now_secs());
                        }
                        // Full sync stored the just-fetched manifest (the relay's
                        // newest) — pull it back to make it the active version.
                        None => {
                            let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
                            if let Ok(Some(ev)) = nsite_deck::seams::newest_in_slot(
                                self.relay.as_ref(),
                                kind,
                                &addr.author,
                                addr.d_tag.as_deref(),
                            )
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
    }

    // --- circle (paired peers we pull from) ---

    /// Add (or rename) a paired peer in the Circle. Idempotent by npub.
    pub fn add_to_circle(&self, npub: &str, name: &str) {
        if npub.is_empty() {
            return;
        }
        // Whether they accepted ours or we accepted theirs, any invite we were
        // holding for this peer is answered.
        self.outbound_pairs
            .lock()
            .unwrap()
            .retain(|p| p.npub != npub);
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
                perms: PeerPerms::default(),
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

    /// Record which mesh peers are *directly* connected right now (called by the
    /// runtime from the node's peer snapshot). This is only an edge detector for
    /// dial backoff — nothing gates on it, because whether a Circle member is a
    /// direct neighbour or twenty hops away is FIPS's business, not ours.
    pub fn set_connected_peers(&self, npubs: Vec<String>) {
        let mut cur = self.connected_peers.lock().unwrap();
        // A peer newly present in the mesh view is a reconnect edge: forget its
        // dial backoff so the next keepwarm tick dials it immediately.
        for npub in &npubs {
            if !cur.contains(npub) {
                self.peer_relays.reset_backoff(npub);
            }
        }
        *cur = npubs;
    }

    /// Every Circle member's npub. Hop count is deliberately not a factor: FIPS
    /// routes to a mesh address whether the peer is adjacent or many hops away,
    /// so a routed `ws://<npub>.fips:4870` dial reaches any of them. A member
    /// who is genuinely offline costs one bounded dial (the callers time out)
    /// and is then held off by the per-peer backoff in [`crate::peer_relay`].
    /// See `docs/design/event-gossip.md`.
    pub fn circle_npubs(&self) -> Vec<String> {
        self.circle
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.npub.clone())
            .collect()
    }

    /// The permissions granted to the peer at `ip`, or `None` if `ip` is not a
    /// current Circle member. One lookup answers both "are they paired" and "what
    /// may they do", so the access checks never consult two sources that could
    /// disagree. See `reference/thinning-custom-relay.md` (D10).
    ///
    /// Consulted per request, so adding a peer, removing one, or changing a
    /// permission takes effect immediately — there is no cached set. A peer's ULA
    /// is `fd…+node_addr[0..15]` (`PeerIdentity::from_npub(npub).address()`),
    /// which is exactly the source address the mesh sockets see.
    pub fn perms_for_ip(&self, ip: IpAddr) -> Option<PeerPerms> {
        let IpAddr::V6(v6) = ip else { return None };
        self.circle
            .lock()
            .unwrap()
            .iter()
            .find(|c| {
                fips::PeerIdentity::from_npub(&c.npub)
                    .map(|p| p.address().to_ipv6() == v6)
                    .unwrap_or(false)
            })
            .map(|c| c.perms.clone())
    }

    /// Whether events arriving from the peer at `ip` may be forwarded onward by
    /// us. Without the grant their events are still stored and shown locally —
    /// they simply stop here (`reference/thinning-custom-relay.md`, D10).
    pub fn may_forward_from(&self, ip: IpAddr) -> bool {
        self.perms_for_ip(ip)
            .is_some_and(|p| p.relay_write_multihop)
    }

    /// Whether the peer at `ip` may upload blobs to our Blossom. Off by default:
    /// propagation is pull-based, so nothing in normal operation pushes blobs to
    /// a peer, and an upload costs us disk.
    pub fn may_upload_blobs(&self, ip: IpAddr) -> bool {
        self.perms_for_ip(ip).is_some_and(|p| p.blossom_write)
    }

    /// Whether the peer at `ip` may read blobs from our Blossom.
    pub fn may_read_blobs(&self, ip: IpAddr) -> bool {
        self.perms_for_ip(ip).is_some_and(|p| p.blossom_read)
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

    /// Circle members we hold a live mesh relay connection to right now. The
    /// keepwarm tick keeps one open per member, so this reflects who is
    /// actually reachable — at any hop count — rather than who happens to be
    /// an adjacent node.
    pub fn reachable_npubs(&self) -> Vec<String> {
        let live = self.peer_relays.connected_npubs();
        self.circle
            .lock()
            .unwrap()
            .iter()
            .filter(|c| live.contains(&c.npub))
            .map(|c| c.npub.clone())
            .collect()
    }

    /// Circle members as `(npub, name)` — discovery targets. Like
    /// [`circle_npubs`](Self::circle_npubs), every member regardless of hop count.
    fn circle_contacts(&self) -> Vec<(String, String)> {
        self.circle
            .lock()
            .unwrap()
            .iter()
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
        *self.device_name_override.lock().unwrap() = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
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
    pub fn handle_pair_event(self: &Arc<Self>, event: &Event) {
        let Ok(from) = event.pubkey.to_bech32();
        let name = tag_value(event, "n").unwrap_or_else(|| short_name(&from));
        match event.kind.as_u16() {
            KIND_PAIR_REQUEST => {
                tracing::info!(from = %from, "pair: request received (awaiting accept)");
                let secret = tag_value(event, "secret").unwrap_or_default();
                let mut pending = self.pending_pairs.lock().unwrap();
                if !pending.iter().any(|p| p.npub == from) {
                    pending.push(PairRequestView {
                        npub: from,
                        name,
                        secret,
                    });
                }
            }
            KIND_PAIR_ACCEPT => {
                tracing::info!(from = %from, "pair: our request accepted — added to circle");
                self.add_to_circle(&from, &name);
                self.pending_pairs
                    .lock()
                    .unwrap()
                    .retain(|p| p.npub != from);
                // They are a reachable source *now*, so retry anything still
                // waiting on a holder rather than idling until the next
                // connected-peer poll edge.
                for addr in self.retriable_library_addrs() {
                    let content = self.clone();
                    let holder = from.clone();
                    tokio::spawn(async move { content.open_site(addr, Some(holder)).await });
                }
            }
            KIND_PAIR_REMOVE => {
                tracing::info!(from = %from, "pair: peer unpaired — removing from circle");
                self.remove_from_circle(&from);
                self.pending_pairs
                    .lock()
                    .unwrap()
                    .retain(|p| p.npub != from);
            }
            _ => {}
        }
    }

    /// Scanned a peer's QR: send a signed pair request to their mesh relay. We do
    /// not add them yet — only a mutual accept pairs both sides.
    /// Invite a peer to pair, at most once.
    ///
    /// Two things are deliberately *not* done again. Someone already in the
    /// Circle needs no invite — sharing an app with them used to send one every
    /// time, which they then had to accept for a relationship they already had.
    /// And an invite still waiting for an answer is not re-sent: delivery rides
    /// the mesh, so a bump between two phones that have not met yet fails until
    /// a route exists, and bumping again should not queue a second request.
    ///
    /// The record outlives the send, so the invite reads as *waiting* rather
    /// than disappearing when it could not be delivered. It clears when they
    /// accept (either direction — see [`Self::add_to_circle`]).
    pub async fn send_pair_request(&self, target_npub: &str, name: &str, secret: &str) {
        if self.is_in_circle(target_npub) {
            tracing::debug!(target_npub, "pair: already in the Circle, no invite sent");
            return;
        }
        {
            let mut outbound = self.outbound_pairs.lock().unwrap();
            if outbound.iter().any(|p| p.npub == target_npub) {
                tracing::debug!(target_npub, "pair: invite already waiting, not re-sending");
                return;
            }
            outbound.push(OutboundPairView {
                npub: target_npub.to_string(),
                name: name.to_string(),
                since: now_secs(),
            });
        }
        self.dial_pair_event(target_npub, KIND_PAIR_REQUEST, secret)
            .await;
    }

    /// Invites we sent that are still unanswered.
    pub fn outbound_pairs_snapshot(&self) -> Vec<OutboundPairView> {
        self.outbound_pairs.lock().unwrap().clone()
    }

    /// Drop a waiting invite — the user withdrew it, or it is being retried.
    pub fn forget_outbound_pair(&self, npub: &str) {
        self.outbound_pairs
            .lock()
            .unwrap()
            .retain(|p| p.npub != npub);
    }

    fn is_in_circle(&self, npub: &str) -> bool {
        self.circle.lock().unwrap().iter().any(|c| c.npub == npub)
    }

    /// Accept an incoming request: add the requester to our Circle and send them a
    /// signed accept so they add us too.
    pub async fn accept_pair_request(&self, npub: &str, name: &str) {
        self.add_to_circle(npub, name);
        self.pending_pairs
            .lock()
            .unwrap()
            .retain(|p| p.npub != npub);
        self.dial_pair_event(npub, KIND_PAIR_ACCEPT, "").await;
    }

    /// Decline an incoming request (drop it; no signal back).
    pub fn decline_pair_request(&self, npub: &str) {
        self.pending_pairs
            .lock()
            .unwrap()
            .retain(|p| p.npub != npub);
    }

    /// Tell a peer we've forgotten them, so they drop us from their Circle too.
    /// Best-effort and **fire-once**: it only lands if they're reachable within the
    /// dial window (the local removal already happened synchronously in
    /// `remove_from_circle`). If they're offline it is not re-sent later, so their
    /// Circle keeps a stale entry for us until they forget us or we re-pair. A
    /// durable handshake (queue + ack) is a possible later improvement.
    pub async fn send_unpair(&self, npub: &str) {
        self.dial_pair_event(npub, KIND_PAIR_REMOVE, "").await;
    }

    /// Build + sign a pair event and POST it to the target's **auth service**,
    /// **retrying** until it acks or we give up. A freshly-paired BLE session is
    /// flaky (handshake collisions, "connection not ready"), so a single
    /// fire-and-forget dial often misses — leaving the Circles asymmetric, which the
    /// access gate then turns into a hard "can't see their apps" failure. Each
    /// attempt rebuilds (re-signs) the event so its NIP-40 expiration stays fresh
    /// across the retry window.
    ///
    /// This goes to `:4871`, not the relay: pairing creates the circle that gates
    /// the content ports, so it does not travel on them
    /// (`reference/thinning-custom-relay.md`, D6). Unlike a relay `OK`, the
    /// response distinguishes *delivered and waiting on them* from *never reached
    /// them*, so a pending request stops the retry loop instead of burning the
    /// whole window on a peer who already has it.
    async fn dial_pair_event(&self, target_npub: &str, kind: u16, secret: &str) {
        let Some(keys) = self.device_keys.lock().unwrap().clone() else {
            tracing::warn!("pairing: device keys not set");
            return;
        };
        let name = self.device_name();
        // Bind only to validate the npub; the dial is by name (see below).
        let _peer = match fips::PeerIdentity::from_npub(target_npub) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target_npub, error = %e, "pairing: bad target npub");
                return;
            }
        };
        let url = crate::ip_source::mesh_auth_url(target_npub);
        for attempt in 0..PAIR_DIAL_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(PAIR_DIAL_RETRY_DELAY).await;
            }
            let Some(event) = build_pair_event(&keys, kind, target_npub, &name, secret) else {
                tracing::warn!(target_npub, "pairing: could not build event");
                return;
            };
            match crate::ip_source::post_pair_event(
                &url,
                &event,
                std::time::Duration::from_secs(10),
            )
            .await
            {
                crate::ip_source::PairDelivery::Accepted(status) => {
                    tracing::info!(target = %target_npub, kind, attempt, status, "pair: delivered");
                    return;
                }
                // They answered and said no. Retrying cannot change that, and
                // hammering a peer that already refused is exactly what the
                // retry loop must not do.
                crate::ip_source::PairDelivery::Refused(status) => {
                    tracing::warn!(target = %target_npub, kind, status, "pair: refused by peer");
                    return;
                }
                crate::ip_source::PairDelivery::Unreachable => {
                    tracing::debug!(target = %target_npub, kind, attempt, "pair: not delivered, retrying");
                }
            }
        }
        tracing::warn!(
            target = %target_npub,
            kind,
            "pair: gave up delivering after retries (session never came up)"
        );
    }

    // --- local subscription registry (recreated against reappearing peers) ---

    /// The relay reports a local (in-app) client opening a `REQ`. We keep its raw
    /// filters — **without interpreting them** — so we can recreate the subscription
    /// against Circle peers as they (re)appear. Called from the gossiper hook.
    pub fn record_local_sub(&self, key: String, filters: Vec<serde_json::Value>) {
        self.active_local_subs.lock().unwrap().insert(key, filters);
    }

    /// The relay reports a local client's subscription closing (`CLOSE` or the
    /// connection dropping).
    pub fn drop_local_sub(&self, key: &str) {
        self.active_local_subs.lock().unwrap().remove(key);
    }

    // --- backlog resync (the read counterpart of fan-out) ---

    /// Recreate every open local subscription against `npub`'s relay: replay each
    /// client's filters to the peer and fold its matching events into our store, so
    /// a freshly-reachable Circle peer delivers the backlog our clients missed while
    /// it was away. myco is filter-agnostic here — it just re-runs whatever the
    /// in-app clients are subscribed to; it has no notion of chat vs anything else.
    /// Best-effort and hard-bounded. Runs on the pool's (re)connect edge — so it
    /// covers a Circle peer reachable only multi-hop, which the direct-neighbour
    /// snapshot never surfaced.
    pub async fn resync_from_peer(&self, npub: &str) {
        let subs = {
            let guard = self.active_local_subs.lock().unwrap();
            if guard.is_empty() {
                return;
            }
            guard.values().cloned().collect::<Vec<_>>()
        };
        let Ok(_peer) = fips::PeerIdentity::from_npub(npub) else {
            return;
        };
        let url = crate::ip_source::mesh_relay_url(npub);
        let mut stored = 0u32;
        for filters in subs {
            let events = self
                .peer_relays
                .request(npub, &url, filters, std::time::Duration::from_secs(15))
                .await;
            for ev in events {
                // Signatures were checked by the pool at ingress. Storing is
                // idempotent, so this counts events pulled, not new arrivals.
                if self.relay.publish(ev).await.is_ok() {
                    stored += 1;
                }
            }
        }
        if stored > 0 {
            tracing::debug!(npub, stored, "resynced backlog from reappeared peer");
        }
    }

    /// One keepwarm pass (driven by a runtime tick): ensure a live pooled connection
    /// to every Circle member — so a dropped connection is respawned promptly, not
    /// lazily on the next outbound frame — and, for each member the pool has just
    /// (re)connected (absent→present since last pass), spawn a backlog resync. This
    /// is what restores a Circle relay link **mutually and fast** after a mesh flap,
    /// regardless of where the peer sits in the mesh.
    pub fn keepwarm_tick(self: &Arc<Self>) {
        let circle: HashSet<String> = self.circle_npubs().into_iter().collect();
        for npub in &circle {
            if fips::PeerIdentity::from_npub(npub).is_ok() {
                // Teach the node this npub's address→pubkey mapping first. We
                // dial a raw `fd00::` literal below, which skips DNS — and
                // without the identity the node has no pubkey to open a session
                // with, so the dial fails as unroutable for anyone who isn't
                // already a direct neighbour. See `dns_intercept::warm_route`.
                crate::dns_intercept::warm_route(npub);
                let url = crate::ip_source::mesh_relay_url(npub);
                self.peer_relays.ensure(npub, &url);
            }
        }
        let now = self.peer_relays.connected_npubs();
        let mut prev = self.prev_pool_connected.lock().unwrap();
        // Newly-connected Circle members → recreate their subscriptions.
        for npub in now.difference(&prev) {
            if circle.contains(npub) {
                let me = self.clone();
                let npub = npub.clone();
                tokio::spawn(async move { me.resync_from_peer(&npub).await });
            }
        }
        *prev = now;
    }

    // --- mesh fan-out ---

    /// Queue a pre-built relay frame (`["EVENT", {…}]`) to a peer's relay over a
    /// persistent pooled connection (no per-message connect). `npub` is the target
    /// Circle peer. Non-blocking.
    pub fn gossip_to_peer(&self, npub: &str, frame: String) {
        let Ok(_peer) = fips::PeerIdentity::from_npub(npub) else {
            return;
        };
        let url = crate::ip_source::mesh_relay_url(npub);
        self.peer_relays.send(npub, &url, frame);
    }

    /// Pull plane: forward a REQ's filters to connected Circle peers and
    /// aggregate their matching events. `meta` is the incoming envelope, already
    /// decremented, so the hop budget and query id carry onward while the filters
    /// stay canonical NIP-01. `exclude` is the requester's mesh address
    /// (split-horizon). Per-peer queries run in parallel, each bounded by the
    /// budget that arrived so a dead relay can't stall discovery.
    pub async fn pull_from_peers(
        &self,
        filters: Vec<serde_json::Value>,
        meta: crate::mesh_wire::MeshMeta,
        exclude: Option<std::net::IpAddr>,
    ) -> Vec<Event> {
        // The filters go out untouched — hops, query id, and budget ride the
        // envelope. `meta` is the *incoming* one, already decremented, so the
        // query id survives the hop and every node downstream serves it once.

        // All filters ride in one REQ per peer over the shared connection (the relay
        // any-matches across them), so a pull is a single round-trip, not one socket
        // per filter.
        let pool = &self.peer_relays;
        let queries = self.circle_npubs().into_iter().filter_map(|npub| {
            let peer = fips::PeerIdentity::from_npub(&npub).ok()?;
            let ip = std::net::IpAddr::V6(peer.address().to_ipv6());
            if exclude == Some(ip) {
                return None;
            }
            let url = crate::ip_source::mesh_relay_url(&npub);
            let filters = filters.clone();
            let meta = meta.clone();
            // Wait only as long as the budget that arrived allows, not a fresh
            // full-length timer. Otherwise this hop's window sits *inside* the
            // one above it, and a peer further out returns after the requester
            // has already given up (D8).
            let timeout = meta.hop_timeout(PULL_HOP_TIMEOUT);
            Some(async move {
                pool.request_with(&npub, &url, filters, Some(meta), timeout)
                    .await
            })
        });

        join_all(queries).await.into_iter().flatten().collect()
    }

    // --- discovery ("nsites around me") ---

    /// Discover nsites on connected Circle peers' relays: query each reachable
    /// member's mesh relay (`ws://<npub>.fips:4870`) for manifest events in
    /// parallel, then rebuild the discovered list. Spawn-not-block; the UI polls
    /// `discovered`. Opening a result pulls from that peer (its npub is the holder).
    pub async fn discover_from_circle(self: Arc<Self>) {
        let members = self.circle_contacts();
        let pool = &self.peer_relays;
        let queries = members.into_iter().map(move |(npub, name)| async move {
            let Ok(_peer) = fips::PeerIdentity::from_npub(&npub) else {
                return Vec::new();
            };
            let relay_url = crate::ip_source::mesh_relay_url(&npub);
            // Manifest kinds only. One more hop reaches our peers' peers (2 hops
            // in total), carried in the envelope so the filter stays canonical.
            let filter = serde_json::json!({
                "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
                "limit": 200,
            });
            let events = pool
                .request_with(
                    &npub,
                    &relay_url,
                    vec![filter],
                    Some(crate::mesh_wire::MeshMeta::pull(
                        1,
                        crate::mesh_wire::new_query_id(),
                        PULL_BUDGET_MS,
                    )),
                    std::time::Duration::from_secs(15),
                )
                .await;
            events
                .into_iter()
                .filter_map(|ev| nsite_deck::Manifest::from_event(ev).ok())
                .map(|m| {
                    let addr = SiteAddr {
                        author: m.author,
                        d_tag: m.d_tag.clone(),
                    };
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
        *self.discovered.lock().unwrap() = dedup_by_host(results.into_iter().flatten().collect());
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
        //  - connected peers' mesh relays, carrying one more hop so the check reaches
        //    2 hops just like discovery (their peers' manifests come back too),
        //    which rides the envelope rather than the filter;
        //  - online relays, unless mesh-only is on.
        let mesh_filter = serde_json::json!({
            "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
            "authors": authors,
        });
        let online_filter = serde_json::json!({
            "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
            "authors": authors,
        });
        let mesh_peers: Vec<(String, String)> = self
            .circle_npubs()
            .into_iter()
            .filter_map(|npub| {
                fips::PeerIdentity::from_npub(&npub).ok()?; // validate
                let url = crate::ip_source::mesh_relay_url(&npub);
                Some((npub, url))
            })
            .collect();
        let online: Vec<String> = if self.is_offline_only() {
            Vec::new()
        } else {
            crate::ip_source::default_relays()
        };
        if mesh_peers.is_empty() && online.is_empty() {
            self.finish_update_check("No peers or relays to check");
            return;
        }
        let mesh_count = mesh_peers.len();
        tracing::info!(
            apps = addrs.len(),
            mesh_peers = mesh_count,
            targets = mesh_count + online.len(),
            offline_only = self.is_offline_only(),
            "update check: querying"
        );

        // Mesh peers pull over the shared persistent connection; public relays stay
        // one-shot (we don't hold long-lived sockets — or leak presence — to them).
        let pool = &self.peer_relays;
        let mesh_q = mesh_peers.into_iter().map(|(npub, url)| {
            let f = mesh_filter.clone();
            async move {
                let meta = crate::mesh_wire::MeshMeta::pull(
                    1,
                    crate::mesh_wire::new_query_id(),
                    PULL_BUDGET_MS,
                );
                pool.request_with(
                    &npub,
                    &url,
                    vec![f],
                    Some(meta),
                    std::time::Duration::from_secs(15),
                )
                .await
            }
        });
        let online_q = online.into_iter().map(|url| {
            let f = online_filter.clone();
            async move {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    crate::ip_source::query_relay(&url, f),
                )
                .await
                {
                    Ok(Ok(evs)) => evs,
                    _ => Vec::new(),
                }
            }
        });
        let (mesh_res, online_res) =
            futures_util::future::join(join_all(mesh_q), join_all(online_q)).await;

        // Newest verified manifest per slot across all relays.
        let mut newest: HashMap<String, Event> = HashMap::new();
        let mut received = 0usize;
        for batch in mesh_res.into_iter().chain(online_res) {
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
        // Pull blobs from connected mesh peers first (closer/faster, and the only
        // option under mesh-only), then the online fallback unless mesh-only.
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        for npub in self.circle_npubs() {
            if let Ok(m) = crate::ip_source::mesh_source_for(self.peer_relays.clone(), &npub) {
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
        let addr = SiteAddr {
            author: event.pubkey,
            d_tag: d,
        };

        // Forward budget (mirrors chat): originate at the default for a local
        // publish, else the ttl that rode in. Clamp so a peer can't over-extend us.
        // The same per-peer clamp the chat push plane applies: a peer we have not
        // granted multihop writes still gets its manifest stored and served here,
        // it simply travels no further through us. Manifests were missing this
        // check, so that grant was enforced on one plane but not the other (D10).
        let peer_cap = match inbound.sender {
            Some(ip) if !self.may_forward_from(ip) => 0,
            _ => MANIFEST_EVENT_TTL,
        };
        let effective = match inbound.origin {
            Origin::Local => MANIFEST_EVENT_TTL,
            Origin::Mesh => inbound.event_ttl.unwrap_or(0),
        }
        .min(MANIFEST_EVENT_TTL)
        .min(peer_cap);
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
            // The one place a bare mesh address is right: this is whoever just
            // sent us the event, known only as a transport address — an address
            // does not reduce back to an npub. It is safe here precisely because
            // they just reached us, so the node already holds their identity;
            // everywhere else, peers are addressed as `<npub>.fips` so that
            // resolving the name registers that identity (see
            // `ip_source::mesh_relay_url`).
            sources.push(Arc::new(
                crate::ip_source::IpPeerSource::new(
                    vec![format!("ws://[{ip}]:4870")],
                    vec![format!("http://[{ip}]:24243")],
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
    /// decremented hop budget), split-horizon. `exclude` is the peer it came from.
    fn forward_manifest(&self, manifest: &Event, out_ttl: u8, exclude: Option<IpAddr>) {
        let ev_json = match serde_json::to_value(manifest) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "manifest gossip: serialize failed");
                return;
            }
        };
        let frame = crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::push(out_ttl),
            serde_json::json!(["EVENT", ev_json]),
        );
        for npub in self.circle_npubs() {
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
        // Only ours to clear. A custom relay's contents belong to whoever runs
        // it, and NIP-01 has no "delete everything" to ask for anyway.
        if let Some(store) = &self.relay_store {
            nsite_deck::seams::AdminBackend::wipe(store.as_ref()).await?;
        }
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

    /// Clear cached relay events + Blossom blobs **except** those backing pinned
    /// nsites (Settings → Storage → "Delete cache"). The served manifest version of
    /// each pinned site and every blob it references survive, so installed apps keep
    /// working offline; everything else — unpinned opened sites, discovered
    /// listings, staged updates — is dropped. Identity and Circle are untouched.
    pub async fn wipe_cache(&self) -> anyhow::Result<()> {
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

        if let Some(store) = &self.relay_store {
            store.retain_events(&keep_events);
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
            save_library(&self.library_path, &snapshot);
        }
        self.sites
            .lock()
            .unwrap()
            .retain(|host, _| pinned_hosts.contains(host));
        self.discovered.lock().unwrap().clear();
        self.pending_updates.lock().unwrap().clear();
        let active_snapshot = {
            let mut m = self.active_manifests.lock().unwrap();
            m.retain(|k, _| keep_active.contains(k));
            m.values().cloned().collect::<Vec<_>>()
        };
        save_active(&self.active_path, &active_snapshot);
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
        // Always the embedded store's own figures — that is what takes up space
        // here. Nothing external is configurable yet, so neither flag is set;
        // they follow the configured backend once that lands.
        CacheView {
            // The embedded store's own count, or nothing to count when a custom
            // relay has taken over — which the flag tells the screen to say.
            relay_events: self.relay_store.as_ref().map_or(0, |s| s.count() as u64),
            blob_count: self.blobs_local.as_ref().map_or(0, |b| b.count() as u64),
            used_bytes: self.blobs_local.as_ref().map_or(0, |b| b.total_bytes()),
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
        let event = nsite_deck::seams::newest_in_slot(
            self.relay.as_ref(),
            kind,
            &addr.author,
            addr.d_tag.as_deref(),
        )
        .await
        .ok()??;
        nsite_deck::Manifest::from_event(event).ok()?.title
    }

    async fn manifest_file_count(&self, addr: &SiteAddr) -> u64 {
        let kind = nsite_deck::kind_for(addr.d_tag.as_deref());
        match nsite_deck::seams::newest_in_slot(
            self.relay.as_ref(),
            kind,
            &addr.author,
            addr.d_tag.as_deref(),
        )
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
pub(crate) fn now_secs() -> u64 {
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
        npub.trim_start_matches("npub1")
            .chars()
            .take(6)
            .collect::<String>()
    )
}

/// Build + sign a pair-request/accept event (device key), addressed to
/// `target_npub` via a `p` tag, carrying our `n` name, the one-time `secret`
/// (request only), and a short NIP-40 expiration.
pub(crate) fn build_pair_event(
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
    EventBuilder::new(Kind::from(kind), "")
        .tags(tags)
        .sign_with_keys(keys)
        .ok()
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

    /// A `circle.json` written before per-peer permissions existed must load with
    /// the defaults — and crucially with `blossom.write` **off**. A missing field
    /// must never read as a grant, so serde's default has to be `false` rather
    /// than `bool::default()` by accident (`reference/thinning-custom-relay.md`,
    /// D10).
    #[test]
    fn a_pre_permissions_circle_loads_with_upload_denied() {
        let legacy = r#"[{"npub":"npub1abc","name":"Old Phone","addedAt":1}]"#;
        let loaded: Vec<CircleContact> = serde_json::from_str(legacy).unwrap();

        assert_eq!(loaded.len(), 1);
        let p = &loaded[0].perms;
        assert!(!p.blossom_write, "upload must not be granted by omission");
        assert!(p.blossom_read, "reads stay on for an existing peer");
        assert!(p.relay_read && p.relay_write);
        assert!(p.relay_read_multihop && p.relay_write_multihop);
    }

    /// The content layer works with the store swapped for a relay we do not own.
    ///
    /// This is what every phase before it was for: the same import, gateway read
    /// and library behaviour, with events living on a relay Myco only reaches
    /// over NIP-01. It also pins what the Storage screen is told — the usage
    /// counts stop describing what serves, and say so.
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

    /// The content ports have no exceptions left.
    ///
    /// Pairing kinds used to be the one thing an unpaired peer could publish to
    /// the relay, and the event landed in the store as a side effect. Both are
    /// now refused: the handshake belongs to the auth plane, so a stranger has no
    /// write path into a store that may not even be ours
    /// (`reference/thinning-custom-relay.md`, D6).
    #[test]
    fn the_relay_gate_refuses_pairing_kinds_from_everyone() {
        use crate::mesh_relay::PeerGate;

        let dir = tmp("gate-no-exceptions");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        // A paired peer, so this is not simply "unpaired is refused".
        let peer = Keys::generate();
        let peer_npub = peer.public_key().to_bech32().unwrap();
        content.add_to_circle(&peer_npub, "Peer");
        let ip = IpAddr::V6(
            fips::PeerIdentity::from_npub(&peer_npub)
                .unwrap()
                .address()
                .to_ipv6(),
        );

        let gate = CircleGate::new(content.clone());
        assert!(gate.may_publish(ip, 9), "ordinary content still flows");
        for kind in [KIND_PAIR_REQUEST, KIND_PAIR_ACCEPT, KIND_PAIR_REMOVE] {
            assert!(
                !gate.may_publish(ip, kind),
                "kind {kind} is auth-plane traffic and must not reach the relay"
            );
        }

        // And an unpaired stranger gets nothing at all.
        let stranger = Keys::generate().public_key().to_bech32().unwrap();
        let stranger_ip = IpAddr::V6(
            fips::PeerIdentity::from_npub(&stranger)
                .unwrap()
                .address()
                .to_ipv6(),
        );
        assert!(!gate.may_read(stranger_ip));
        assert!(!gate.may_publish(stranger_ip, KIND_PAIR_REQUEST));
        assert!(!gate.may_publish(stranger_ip, 9));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A peer denied multihop writes must not have its **manifests** relayed
    /// either.
    ///
    /// The chat push plane consulted the grant; the manifest push plane did not,
    /// so the same permission was enforced on one plane and ignored on the other.
    /// Both clamps read the same record, so testing the record is what pins the
    /// invariant (`reference/thinning-custom-relay.md`, D10).
    #[test]
    fn revoking_multihop_writes_covers_both_push_planes() {
        let dir = tmp("multihop-clamp");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        let peer = Keys::generate();
        let npub = peer.public_key().to_bech32().unwrap();
        content.add_to_circle(&npub, "Peer");
        let ip = IpAddr::V6(
            fips::PeerIdentity::from_npub(&npub)
                .unwrap()
                .address()
                .to_ipv6(),
        );

        assert!(
            content.may_forward_from(ip),
            "multihop writes are granted by default"
        );

        // Revoke it the way the UI eventually will.
        {
            let mut circle = content.circle.lock().unwrap();
            circle[0].perms.relay_write_multihop = false;
        }
        assert!(
            !content.may_forward_from(ip),
            "a revoked peer's events stop here, on either plane"
        );
        // An unknown peer is not forwarded for either.
        let stranger = Keys::generate().public_key().to_bech32().unwrap();
        let stranger_ip = IpAddr::V6(
            fips::PeerIdentity::from_npub(&stranger)
                .unwrap()
                .address()
                .to_ipv6(),
        );
        assert!(!content.may_forward_from(stranger_ip));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The defaults are the whole permission model until the UI exposes them, so
    /// pin them rather than trusting the struct to stay as written.
    #[test]
    fn default_permissions_are_open_except_uploads() {
        let p = PeerPerms::default();
        assert!(p.relay_read);
        assert!(p.relay_read_multihop);
        assert!(p.relay_write);
        assert!(p.relay_write_multihop);
        assert!(p.blossom_read);
        assert!(!p.blossom_write);
    }

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
        assert_eq!(content.cache_view().blob_count, 2);

        // Unpin the second site (no longer a kept app), then drop the cache.
        content.remove_from_library(&SiteAddr {
            author: drop.author,
            d_tag: None,
        });
        content.wipe_cache().await.unwrap();

        // The pinned site still serves from local stores; the unpinned one is gone.
        assert_eq!(content.cache_view().relay_events, 1);
        assert_eq!(content.cache_view().blob_count, 1);
        assert_eq!(content.gateway_get(&keep_host, "/", None).await.status, 200);
        assert_eq!(content.gateway_get(&drop_host, "/", None).await.status, 503);
        assert_eq!(content.library_snapshot().len(), 1);

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

            // Every Circle member is a target regardless of hop count: only Bob is
            // a direct neighbour, but Alice is still reachable multi-hop and must
            // remain a pull/discovery/chat target. FIPS decides how to get there.
            content.set_connected_peers(vec!["npub1bob".to_string()]);
            let all = content.circle_npubs();
            assert_eq!(all.len(), 2);
            assert!(all.contains(&"npub1alice".to_string()));
            assert!(all.contains(&"npub1bob".to_string()));

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

    #[test]
    fn device_name_uses_override_then_falls_back() {
        let dir = tmp("devname");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();

        // The npub-derived fallback before any override is set.
        let fallback = content.device_name();
        content.set_device_name("green sammy");
        assert_eq!(content.device_name(), "green sammy", "override wins");
        // Clearing the override (blank) restores the fallback.
        content.set_device_name("   ");
        assert_eq!(content.device_name(), fallback, "blank clears the override");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_invite_is_sent_once_and_never_to_an_existing_member() {
        let dir = tmp("invite-once");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let peer = "npub1mqelkzqp4659fws35h2wvr7z9caka5ml8qddj3ssnwaulwpxdd9sdc3esw";

        // No route exists in a unit test, so the dial fails — the point is that
        // the invite is *remembered* rather than lost with it.
        content.send_pair_request(peer, "them", "s1").await;
        assert_eq!(
            content.outbound_pairs_snapshot().len(),
            1,
            "invite is recorded"
        );

        // Bumping again must not queue a second one.
        content.send_pair_request(peer, "them", "s2").await;
        assert_eq!(
            content.outbound_pairs_snapshot().len(),
            1,
            "a waiting invite is not re-sent"
        );

        // Accepting clears it, and they are then in the Circle...
        content.add_to_circle(peer, "them");
        assert!(
            content.outbound_pairs_snapshot().is_empty(),
            "accepting clears it"
        );

        // ...so sharing an app with them sends nothing.
        content.send_pair_request(peer, "them", "s3").await;
        assert!(
            content.outbound_pairs_snapshot().is_empty(),
            "no invite to someone already in the Circle"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_keeps_one_entry_per_site_preferring_the_newest() {
        let mk = |host: &str, holder: &str, updated: u64| DiscoveredNsite {
            host: host.to_string(),
            author_npub: "npub1author".to_string(),
            d_tag: None,
            title: "T".to_string(),
            updated_at: updated,
            holder_npub: holder.to_string(),
            holder_name: holder.to_string(),
        };
        // Two peers carry the same site at different versions, plus an unrelated one.
        let out = dedup_by_host(vec![
            mk("site-a", "npub1alice", 100),
            mk("site-b", "npub1alice", 50),
            mk("site-a", "npub1bob", 300),
        ]);

        assert_eq!(out.len(), 2, "one entry per site, not one per holder");
        let a = out.iter().find(|d| d.host == "site-a").unwrap();
        assert_eq!(a.updated_at, 300, "keeps the freshest copy");
        assert_eq!(
            a.holder_npub, "npub1bob",
            "and therefore the holder worth pulling from"
        );
    }

    #[test]
    fn pair_remove_event_drops_peer_from_circle() {
        use nostr::Keys;
        let dir = tmp("unpair");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();

        let peer = Keys::generate();
        let peer_npub = peer.public_key().to_bech32().unwrap();
        let content = Arc::new(content);
        content.add_to_circle(&peer_npub, "Peer");
        assert_eq!(content.circle_snapshot().len(), 1);

        // The peer signs a PAIR_REMOVE; handling it drops them from our Circle.
        let event = build_pair_event(&peer, KIND_PAIR_REMOVE, &peer_npub, "Peer", "")
            .expect("build pair-remove event");
        content.handle_pair_event(&event);
        assert!(
            content.circle_snapshot().is_empty(),
            "peer removed on unpair"
        );

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
