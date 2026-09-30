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
//! via `Tick`). See `docs/design/nsite/nsite-layer.md` and the FFI contract.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::future::join_all;
use std::sync::atomic::{AtomicBool, Ordering};

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::{Event, EventBuilder, Filter, Keys, Kind, PublicKey, Tag};
use nsite_deck::gateway::{self, Readiness};
use nsite_deck::seams::{BlobStore, PeerSource, RelayBackend};
use nsite_deck::{sync, GatewayResponse, SiteAddr, SyncOutcome};
use serde::{Deserialize, Serialize};

use crate::file_transfer::{self, FileMessage, FileTransferRecord, FileTransferView};
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundPairView {
    pub npub: String,
    pub name: String,
    /// Unix seconds when we first tried, so the UI can age it.
    pub since: u64,
}

/// How long an invite stays valid as proof that we asked to pair.
///
/// An accept is only honoured while the matching invite is outstanding, so this
/// bounds how long a captured accept could be replayed back at us — and stops
/// invites nobody ever answered accumulating as standing authorisations.
const INVITE_VALID_SECS: u64 = 7 * 24 * 60 * 60;

/// Mutual-pairing handshake events, POSTed point-to-point to a peer's **auth
/// service** at `:4873` (never gossiped, and never stored — the relay refuses
/// these kinds from every source). Signed by the **device** key, which is the
/// pairing identity, and carrying a NIP-40 expiry the auth service checks on
/// receipt. See `docs/design/core/identity-pairing.md`.
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

/// Time budget stamped on a pull this node originates. Relative, and only bounds
/// how long a node downstream holds query state — late results are not an error,
/// they simply arrive to whoever is still listening
/// (`reference/thinning-custom-relay.md`, D8).
pub(crate) const PULL_BUDGET_MS: u32 = 10_000;

/// At most this many authors' NIP-65 write relays, in total, join the default
/// relays in an nsite update check — one combined REQ each, so the bound is
/// on sockets, however many authors the Library holds.
const UPDATE_CHECK_AUTHOR_RELAYS: usize = 12;

/// Longest a single forwarded hop will wait on a peer, used when no budget rode
/// in (an older peer, or a pull that never carried one). A budget that did
/// arrive only ever shortens this.
const PULL_HOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// How many events passing through a pull are stored together.
const PULL_REMEMBER_BATCH: usize = 32;

/// How long napplet-driven internet lanes are skipped after every public relay
/// failed in one round. Short: a phone walking back into Wi-Fi should not wait
/// long to notice.
pub(crate) const INTERNET_DOWN_FOR: std::time::Duration = std::time::Duration::from_secs(30);

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
// Constructed by the Android runtime's mesh relay wiring; the host build has
// no mesh socket to gate.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct CircleGate {
    content: Arc<Content>,
}

impl CircleGate {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
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

    fn may_forward(&self, ip: IpAddr) -> bool {
        self.content.may_forward_from(ip)
    }

    fn max_req_ttl(&self, ip: IpAddr) -> u8 {
        match self.content.perms_for_ip(ip) {
            Some(p) if p.relay_read_multihop => crate::mesh_relay::MAX_REQ_TTL,
            _ => 0,
        }
    }
}

/// The shell cache's budgets, in bytes (Settings → Storage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    pub event_bytes: u64,
    pub blob_bytes: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
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
    /// The same two stores, written as **cache**: pulled backlog, mesh
    /// pass-through, query answers nobody asked to keep.
    cache_relay: Arc<dyn RelayBackend>,
    /// The shell's event cache, apart from the relay (`myco-cache`).
    event_cache: Arc<myco_cache::EventCache>,
    /// The relay hub, once the runtime has built it: what a pull hands its
    /// events to so live subscriptions see them as they land. Weak — the hub
    /// holds this `Content` through its gossiper and gate.
    hub: Mutex<Option<std::sync::Weak<crate::mesh_relay::RelayHub>>>,
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
    /// "Mesh-only": when true, `open_site` never uses the IP online fallback —
    /// it pulls only over the mesh (holder + connected Circle peers). Lets you
    /// verify the mesh path even when this device has internet (e.g. a hotspot).
    offline_only: AtomicBool,
    /// When the internet last looked down from here, as a moment until which
    /// napplet-driven internet lanes are skipped. See [`Content::internet_looks_down`].
    internet_down_until: Mutex<Option<std::time::Instant>>,
    library: Mutex<Vec<LibraryItem>>,
    library_path: PathBuf,
    /// The Circle: paired peers we pull from over the mesh. Persisted.
    circle: Mutex<Vec<CircleContact>>,
    circle_path: PathBuf,
    /// npubs of currently-connected mesh peers, refreshed by the runtime each poll.
    /// `open_site` pulls from connected Circle members (bounded to who's reachable,
    /// so it never blocks on an offline contact's connect timeout).
    connected_peers: Mutex<Vec<String>>,
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
    /// Invites we have sent and not yet had answered. **Persisted**, because an
    /// accept is only honoured against one of these — an in-memory-only record
    /// would refuse a legitimate accept that arrives after a restart.
    outbound_pairs: Mutex<Vec<OutboundPairView>>,
    outbound_pairs_path: PathBuf,
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
    /// See `docs/design/nsite/nsite-updates.md` §2. P-U1: staged outside the relay store;
    /// activation stores the manifest (making it the served version).
    pending_updates: Mutex<HashMap<String, PendingUpdate>>,
    /// Status of the latest update check, for UI feedback (checking → result).
    update_check: Mutex<UpdateCheckView>,
    /// The throttle every update-check trigger goes through (`update_gate.rs`).
    update_gate: Mutex<crate::update_gate::UpdateGate>,
    /// Native paired-peer file transfers. Metadata is persisted; file keys and
    /// encrypted outbox paths remain inside the app-private data directory.
    file_transfers: Mutex<Vec<FileTransferRecord>>,
    /// Incoming transfers this device finished, newest last, as `(id, peer)`.
    /// The row itself is forgotten as soon as the shell publishes the file, but
    /// the sender may still be retrying its `ready` if our `complete` was lost —
    /// this is what lets a late `ready` be answered with a fresh `complete`
    /// rather than refused as unknown. The peer is kept alongside the id so an
    /// answer goes only to the peer the transfer was with; every other check on
    /// that path binds the peer, and this one must too. Persisted, so a restart
    /// in the window between publishing the file and the sender giving up does
    /// not cost them the whole offer TTL.
    completed_incoming: Mutex<VecDeque<(String, String)>>,
    completed_incoming_path: PathBuf,
    file_transfers_path: PathBuf,
    file_outbox_dir: PathBuf,
    received_dir: PathBuf,
    /// The **active version** the gateway serves per slot — decoupled from the
    /// relay's newest, so a newer (received/checked) manifest can sit in the relay
    /// store (NIP-01-faithful, propagated to peers) while we keep serving the fully
    /// downloaded version until its replacement is staged. See
    /// `docs/design/nsite/nsite-updates.md` §1. Persisted to `active.json`.
    active_manifests: Arc<Mutex<HashMap<String, Event>>>,
    active_path: PathBuf,
    /// Whether each installed napplet can open right now, keyed by shell host.
    /// Rebuilt at startup, after a cache wipe, and whenever a version is
    /// pinned — the napplet counterpart of `sites` for nsites.
    napplet_status: Mutex<HashMap<String, NappletStatusView>>,
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
/// The Circle-facing relay reads through the same view ([`Content::pinned_relay`]),
/// so a peer asking for an installed app's manifest gets the version this phone
/// has the files for, not a newer one kept in the store without them.
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
    /// Circle peer's `since`, `ids` or tag filter can match the newer version
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
        let outbound_pairs_path = data_dir.join("outbound_pairs.json");
        let outbound_pairs = load_outbound_pairs(&outbound_pairs_path);
        let circle_path = data_dir.join("circle.json");
        let circle = load_circle(&circle_path);
        let active_path = data_dir.join("active.json");
        let active_manifests = load_active(&active_path);
        let file_transfers_path = data_dir.join("file_transfers.json");
        let file_transfers = load_file_transfers(&file_transfers_path);
        let completed_incoming_path = data_dir.join("completed_transfers.json");
        let completed_incoming = load_completed_incoming(&completed_incoming_path);
        let file_outbox_dir = data_dir.join("file-outbox");
        let received_dir = data_dir.join("received");
        let _ = std::fs::create_dir_all(&file_outbox_dir);
        let _ = std::fs::create_dir_all(&received_dir);
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
            circle: Mutex::new(circle),
            circle_path,
            connected_peers: Mutex::new(Vec::new()),
            sites: Mutex::new(HashMap::new()),
            forgotten: Mutex::new(std::collections::HashSet::new()),
            loading_retries: Mutex::new(HashMap::new()),
            device_keys: device_keys.clone(),
            device_name_override: Mutex::new(None),
            pending_pairs: Mutex::new(Vec::new()),
            outbound_pairs: Mutex::new(outbound_pairs),
            outbound_pairs_path,
            peer_relays: Arc::new(crate::peer_relay::PeerRelayPool::new()),
            hub: Mutex::new(None),
            active_local_subs: Mutex::new(HashMap::new()),
            prev_pool_connected: Mutex::new(HashSet::new()),
            pending_updates: Mutex::new(HashMap::new()),
            update_check: Mutex::new(UpdateCheckView::default()),
            update_gate: Mutex::new(crate::update_gate::UpdateGate::default()),
            file_transfers: Mutex::new(file_transfers),
            completed_incoming: Mutex::new(completed_incoming),
            completed_incoming_path,
            file_transfers_path,
            file_outbox_dir,
            received_dir,
            active_manifests: Arc::new(Mutex::new(active_manifests)),
            active_path,
            napplet_status: Mutex::new(HashMap::new()),
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

    /// The shared per-peer relay pool, for building a mesh source against a
    /// specific holder.
    pub fn peer_relays(&self) -> Arc<crate::peer_relay::PeerRelayPool> {
        self.peer_relays.clone()
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

    /// The store as the Circle sees it: an installed app's manifest slot reads
    /// as its **pinned** version, the one this phone has the files for, even
    /// when a newer one sits in the store. Everything else passes through.
    pub fn pinned_relay(&self) -> Arc<dyn RelayBackend> {
        Arc::new(self.active_backend())
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
    /// Source order (`docs/design/nsite/nsite-layer.md` §5): local → the **holder**'s
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
            });
        }
        let snapshot = lib.clone();
        drop(lib);
        save_library(&self.library_path, &snapshot);
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
            });
        }
        let snapshot = lib.clone();
        drop(lib);
        save_library(&self.library_path, &snapshot);
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
        save_library(&self.library_path, &snapshot);
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
        save_library(&self.library_path, &snapshot);
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
        save_library(&self.library_path, &snapshot);
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
        save_library(&self.library_path, &snapshot);
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
        save_library(&self.library_path, &snapshot);
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
            let ready = match self.napplet_keep_set(&item).await {
                Some((event, index)) => {
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

    // --- circle (paired peers we pull from) ---

    /// Add (or rename) a paired peer in the Circle. Idempotent by npub.
    pub fn add_to_circle(&self, npub: &str, name: &str) {
        if npub.is_empty() {
            return;
        }
        // Whether they accepted ours or we accepted theirs, any invite we were
        // holding for this peer is answered — and spent, so it stops being an
        // authorisation for a second accept.
        self.drop_invite(npub);
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
    /// See `docs/design/core/event-gossip.md`.
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
    /// Returns whether the event was acted on. `false` means it was refused —
    /// the caller answers the sender accordingly.
    pub fn handle_pair_event(self: &Arc<Self>, event: &Event) -> bool {
        let Ok(from) = event.pubkey.to_bech32();
        let name = tag_value(event, "n").unwrap_or_else(|| short_name(&from));

        // Addressed to us? Every pair event names its target in a `p` tag, and
        // the signature covers it. Without this check an event addressed to
        // someone else can be captured and replayed at us, and it verifies
        // perfectly — the signature says who wrote it, never who it was for.
        if !self.is_addressed_to_us(event) {
            tracing::warn!(from = %from, "pair: refused, not addressed to this device");
            return false;
        }

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
                // An accept is only meaningful as the answer to an invite we
                // sent. Without this, anyone could sign one and add themselves
                // to the Circle unprompted — which grants relay read/write,
                // blob reads, and multihop forwarding. The signature proves who
                // sent it, not that we ever asked.
                if !self.has_outstanding_invite(&from) {
                    tracing::warn!(
                        from = %from,
                        "pair: refused an accept we never invited"
                    );
                    return false;
                }
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
            _ => return false,
        }
        true
    }

    /// Does this pair event name **us** in its `p` tag?
    ///
    /// `false` when the device key is not loaded yet: we cannot tell, and
    /// guessing in the permissive direction is what this check exists to stop.
    fn is_addressed_to_us(&self, event: &Event) -> bool {
        let Some(keys) = self.device_keys.lock().unwrap().clone() else {
            return false;
        };
        tag_value(event, "p").is_some_and(|target| target == keys.public_key().to_hex())
    }

    /// Is there an invite to `npub` still outstanding — and recent enough to
    /// still count? Invites are persisted, so this survives a restart between
    /// sending the request and the peer answering it.
    fn has_outstanding_invite(&self, npub: &str) -> bool {
        let now = now_secs();
        self.outbound_pairs
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.npub == npub && now.saturating_sub(p.since) <= INVITE_VALID_SECS)
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
            let snapshot = outbound.clone();
            drop(outbound);
            // Persisted before the dial: the accept can arrive after a restart,
            // and it is only honoured against a recorded invite.
            save_outbound_pairs(&self.outbound_pairs_path, &snapshot);
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
        self.drop_invite(npub);
    }

    /// Forget an invite, on disk as well as in memory. One place, so an invite
    /// cannot survive on disk as a standing authorisation after being answered.
    fn drop_invite(&self, npub: &str) {
        let snapshot = {
            let mut outbound = self.outbound_pairs.lock().unwrap();
            outbound.retain(|p| p.npub != npub);
            outbound.clone()
        };
        save_outbound_pairs(&self.outbound_pairs_path, &snapshot);
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
    /// This goes to `:4873`, not the relay: pairing creates the circle that gates
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
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        let hub = self.hub();
        // Every open subscription at once, each event taken as the peer sends
        // it — through the hub, so the client whose subscription this is sees
        // what it missed without asking again.
        let (tx, mut merged) = tokio::sync::mpsc::unbounded_channel();
        for filters in subs {
            let mut events = self.peer_relays.request_stream(npub, &url, filters, None);
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = events.recv().await {
                    if tx.send(ev).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let mut stored = 0u32;
        while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, merged.recv()).await {
            // Signatures were checked by the pool at ingress. Backlog, not a
            // keep: the cache, bar the kinds kept as seen.
            let accepted = match &hub {
                Some(hub) => hub.accept_pulled(ev).await.is_ok(),
                None => self.cache_relay.publish(ev).await.is_ok(),
            };
            if accepted {
                stored += 1;
            }
        }
        if stored > 0 {
            tracing::debug!(npub, stored, "resynced backlog from reappeared peer");
        }
    }

    /// Hand pulls the relay hub, so what they bring reaches live
    /// subscriptions. Called once, when the runtime builds the hub.
    pub fn set_hub(&self, hub: &Arc<crate::mesh_relay::RelayHub>) {
        *self.hub.lock().unwrap() = Some(Arc::downgrade(hub));
    }

    fn hub(&self) -> Option<Arc<crate::mesh_relay::RelayHub>> {
        self.hub.lock().unwrap().as_ref().and_then(|h| h.upgrade())
    }

    /// One keepwarm pass (driven by a runtime tick): ensure a live pooled connection
    /// to every Circle member — so a dropped connection is respawned promptly, not
    /// lazily on the next outbound frame — and, for each member the pool has just
    /// (re)connected (absent→present since last pass), spawn a backlog resync. This
    /// is what restores a Circle relay link **mutually and fast** after a mesh flap,
    /// regardless of where the peer sits in the mesh.
    pub fn keepwarm_tick(self: &Arc<Self>) {
        // Cheap, and the only clock the transfer state machine has.
        self.sweep_file_transfers();
        let resend = self.stalled_file_messages(file_transfer::now_secs());
        if !resend.is_empty() {
            let content = Arc::clone(self);
            tokio::spawn(async move {
                for (peer_npub, message) in resend {
                    let Ok(target) = PublicKey::from_bech32(&peer_npub) else {
                        continue;
                    };
                    let transfer_id = message.transfer_id().to_string();
                    match content
                        .send_file_message(&target, &peer_npub, message)
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(transfer = %transfer_id, "file share: re-sent pending message")
                        }
                        Err(e) => {
                            tracing::debug!(transfer = %transfer_id, error = %e, "file share: re-send failed")
                        }
                    }
                }
            });
        }
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

    // --- native paired-peer file transfer ---

    /// Snapshot transfer rows for the FFI/UI. Secrets and local source paths
    /// never cross this boundary.
    pub fn file_transfers_snapshot(&self) -> Vec<FileTransferView> {
        self.file_transfers
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.view.clone())
            .collect()
    }

    /// Encrypt a local file, retain the ciphertext in the app-private outbox,
    /// and send only a gift-wrapped metadata offer. The ciphertext is not put
    /// into Blossom until the recipient accepts.
    pub async fn start_file_share(
        self: Arc<Self>,
        path: String,
        name: String,
        mime: String,
        target_npub: String,
    ) -> anyhow::Result<()> {
        if !self.is_in_circle(&target_npub) {
            anyhow::bail!("file share target is not in the Circle");
        }
        let keys = self
            .device_keys
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
        let target = PublicKey::from_bech32(&target_npub)
            .map_err(|e| anyhow::anyhow!("invalid target npub: {e}"))?;
        let imported_from_android_share = Path::new(&path)
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .map(|name| name == "myco-share-outbox")
            .unwrap_or(false);
        let plain = tokio::fs::read(&path).await?;
        let filename = file_transfer::safe_filename(&name, "shared-file");
        let mime = if mime.trim().is_empty() {
            "application/octet-stream".to_string()
        } else {
            mime
        };
        if let Some(reason) = file_transfer::rejected_payload(&filename, &mime) {
            anyhow::bail!(reason);
        }
        let transfer_id = file_transfer::new_transfer_id();
        let (package, key) =
            file_transfer::encrypt_file(&plain, &transfer_id, &target_npub, &filename)?;
        let outbox_path = self.file_outbox_dir.join(format!("{transfer_id}.bin"));
        tokio::fs::write(&outbox_path, &package).await?;
        if imported_from_android_share {
            // Android's copy is only an input staging file. Once the native
            // encrypted outbox exists, retaining it would create a second
            // private history of every shared file.
            let _ = tokio::fs::remove_file(&path).await;
        }
        let now = file_transfer::now_secs();
        let expires_at = now + file_transfer::OFFER_TTL_SECS;
        let peer_name = self
            .circle_snapshot()
            .into_iter()
            .find(|p| p.npub == target_npub)
            .map(|p| p.name)
            .unwrap_or_else(|| target_npub.chars().take(16).collect());
        self.insert_file_transfer(FileTransferRecord {
            view: FileTransferView {
                id: transfer_id.clone(),
                direction: "outgoing".to_string(),
                peer_npub: target_npub.clone(),
                peer_name,
                name: filename.clone(),
                mime: mime.clone(),
                size: plain.len() as u64,
                status: "offered".to_string(),
                blob_hash: String::new(),
                received_path: String::new(),
                publish_pending: false,
                error: String::new(),
                updated_at: now,
            },
            source_path: Some(outbox_path.to_string_lossy().into_owned()),
            key_b64: Some(file_transfer::encode_key(&key)),
            ciphertext_size: package.len() as u64,
            expires_at,
            last_resend_at: 0,
        });
        let sender_npub = keys.public_key().to_bech32()?;
        let message = FileMessage::Offer {
            transfer_id: transfer_id.clone(),
            sender_npub,
            recipient_npub: target_npub.clone(),
            filename,
            mime,
            size: plain.len() as u64,
            issued_at: now,
            expires_at,
        };
        if let Err(e) = self.send_file_message(&target, &target_npub, message).await {
            // The row stays. `error` is the only channel this failure has to the
            // user, and deleting the row here is what made every failed send
            // look like a successful one.
            self.set_file_status(&transfer_id, "failed", &e.to_string());
            return Err(e);
        }
        tracing::info!(target = %target_npub, transfer = %transfer_id, "file share offer sent");
        Ok(())
    }

    /// Respond to an incoming offer. Accepting only sends the control reply;
    /// the sender uploads the encrypted blob afterward.
    pub async fn respond_file_transfer(
        self: Arc<Self>,
        transfer_id: String,
        accepted: bool,
    ) -> anyhow::Result<()> {
        let target_npub = {
            let records = self.file_transfers.lock().unwrap();
            let record = records
                .iter()
                .find(|r| r.view.id == transfer_id && r.view.direction == "incoming")
                .ok_or_else(|| anyhow::anyhow!("file offer not found"))?;
            if record.view.status != "waiting_user" {
                anyhow::bail!("file offer is no longer waiting for a response");
            }
            record.view.peer_npub.clone()
        };
        let keys = self
            .device_keys
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
        let target = PublicKey::from_bech32(&target_npub)?;
        let own_npub = keys.public_key().to_bech32()?;
        self.set_file_status(
            &transfer_id,
            if accepted { "accepted" } else { "denied" },
            "",
        );
        let message = FileMessage::Response {
            transfer_id: transfer_id.clone(),
            sender_npub: own_npub,
            recipient_npub: target_npub.clone(),
            accepted,
            reason: (!accepted).then(|| "declined by recipient".to_string()),
        };
        let result = self.send_file_message(&target, &target_npub, message).await;
        if !accepted && result.is_ok() {
            self.forget_file_transfer(&transfer_id);
        }
        result
    }

    /// Called by the mesh gossiper for every newly-arrived gift wrap. Invalid,
    /// expired, non-file, and non-Circle messages are ignored.
    pub async fn handle_file_event(self: &Arc<Self>, event: &Event) {
        if event.kind != Kind::GiftWrap {
            return;
        }
        let Some(keys) = self.device_keys.lock().unwrap().clone() else {
            return;
        };
        let Ok(unwrapped) = nostr::nips::nip59::extract_rumor(&keys, event).await else {
            return;
        };
        if unwrapped.rumor.kind != Kind::PrivateDirectMessage {
            return;
        }
        let Ok(message) = serde_json::from_str::<FileMessage>(&unwrapped.rumor.content) else {
            return;
        };
        // The id reaches a filesystem path and every lookup below keys on it, so
        // it is checked once, here, before any branch has a chance to use it.
        if !file_transfer::valid_transfer_id(message.transfer_id()) {
            tracing::warn!("file message with a malformed transfer id ignored");
            return;
        }
        let sender_npub = unwrapped.sender.to_bech32().unwrap_or_default();
        if !self.is_in_circle(&sender_npub) {
            tracing::warn!(sender = %sender_npub, "file message from non-Circle sender ignored");
            return;
        }
        let own_npub = match keys.public_key().to_bech32() {
            Ok(v) => v,
            Err(_) => return,
        };
        match message {
            FileMessage::Offer {
                transfer_id,
                recipient_npub,
                filename,
                mime,
                size,
                expires_at,
                ..
            } if recipient_npub == own_npub => {
                if expires_at <= file_transfer::now_secs()
                    || size > file_transfer::MAX_FILE_BYTES as u64
                {
                    tracing::warn!(transfer_id, "expired or oversized file offer ignored");
                    return;
                }
                let offered_name = file_transfer::safe_filename(&filename, "shared-file");
                if let Some(reason) = file_transfer::rejected_payload(&offered_name, &mime) {
                    tracing::warn!(transfer_id, %reason, "file offer refused by payload policy");
                    return;
                }
                if self
                    .file_transfers
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|r| r.view.id == transfer_id)
                {
                    return;
                }
                // Drop finished rows to make room first; refuse the offer only if
                // the list is genuinely full of live ones. Otherwise a peer that
                // spams offers grows the persisted list without bound.
                if self.file_transfers.lock().unwrap().len() >= file_transfer::MAX_TRACKED_TRANSFERS
                {
                    self.prune_finished_transfers();
                    if self.file_transfers.lock().unwrap().len()
                        >= file_transfer::MAX_TRACKED_TRANSFERS
                    {
                        tracing::warn!(transfer_id, "file offer refused, too many open transfers");
                        return;
                    }
                }
                let peer_name = self
                    .circle_snapshot()
                    .into_iter()
                    .find(|p| p.npub == sender_npub)
                    .map(|p| p.name)
                    .unwrap_or_else(|| "peer".to_string());
                self.insert_file_transfer(FileTransferRecord {
                    view: FileTransferView {
                        id: transfer_id,
                        direction: "incoming".to_string(),
                        peer_npub: sender_npub,
                        peer_name,
                        name: offered_name,
                        mime,
                        size,
                        status: "waiting_user".to_string(),
                        blob_hash: String::new(),
                        received_path: String::new(),
                        publish_pending: false,
                        error: String::new(),
                        updated_at: file_transfer::now_secs(),
                    },
                    source_path: None,
                    key_b64: None,
                    ciphertext_size: 0,
                    expires_at,
                    last_resend_at: 0,
                });
            }
            FileMessage::Response {
                transfer_id,
                recipient_npub,
                accepted,
                reason,
                ..
            } if recipient_npub == own_npub => {
                // A decline also travels this way when the *sender* cancels, so
                // the recipient's own pending row resolves instead of waiting
                // for its sweeper. An accept only ever makes sense outbound.
                if !accepted {
                    if self.has_file_transfer(&transfer_id, "outgoing", &sender_npub)
                        || self.has_file_transfer(&transfer_id, "incoming", &sender_npub)
                    {
                        self.set_file_status(
                            &transfer_id,
                            "denied",
                            reason.as_deref().unwrap_or("declined by recipient"),
                        );
                        self.clear_transfer_secrets(&transfer_id);
                    }
                    return;
                }
                // Only an offer still on the wire can be accepted. Without the
                // status check a replayed accept restarts a finished transfer.
                if !self.transfer_in_state(&transfer_id, "outgoing", &sender_npub, &["offered"]) {
                    return;
                }
                self.set_file_status(&transfer_id, "accepted", "");
                let content = Arc::clone(self);
                tokio::spawn(async move {
                    if let Err(e) = content.finish_outgoing_transfer(&transfer_id).await {
                        content.set_file_status(&transfer_id, "failed", &e.to_string());
                    }
                });
            }
            FileMessage::Ready {
                transfer_id,
                recipient_npub,
                filename,
                mime,
                size,
                blob_hash,
                ciphertext_size,
                key_wrap,
                ..
            } if recipient_npub == own_npub => {
                // The user's accept is what authorises the download. Matching on
                // the transfer alone let a sender follow its own offer straight
                // with a `ready`, and the file would be fetched, decrypted and
                // published to Downloads without anyone ever tapping Accept.
                if !self.transfer_in_state(&transfer_id, "incoming", &sender_npub, &["accepted"]) {
                    // A `ready` for a transfer we already finished means our
                    // `complete` never reached the sender: answer it again so
                    // their row stops retrying and clears.
                    if self.was_completed_incoming(&transfer_id, &sender_npub) {
                        let content = Arc::clone(self);
                        tokio::spawn(async move {
                            if let Err(e) = content.send_complete(&transfer_id, &sender_npub).await
                            {
                                tracing::debug!(transfer = %transfer_id, error = %e, "file share: repeat completion failed");
                            }
                        });
                        return;
                    }
                    // The sender retries `ready` until it hears `complete`, so
                    // a repeat while the download is already running is the
                    // expected case, not an out-of-order message.
                    if self.transfer_in_state(
                        &transfer_id,
                        "incoming",
                        &sender_npub,
                        &["downloading"],
                    ) {
                        tracing::debug!(transfer_id, "file ready repeated while downloading");
                        return;
                    }
                    tracing::warn!(
                        transfer_id,
                        "file ready arrived before the offer was accepted"
                    );
                    return;
                }
                if !file_transfer::valid_blob_hash(&blob_hash) {
                    self.set_file_status(
                        &transfer_id,
                        "failed",
                        "the sender sent a malformed blob id",
                    );
                    return;
                }
                // The offer is the contract. The user approved a specific name,
                // type and size; a `ready` that describes a different file is a
                // bait-and-switch, not a correction, so it fails the transfer
                // rather than quietly replacing what was agreed to.
                if let Some(mismatch) =
                    self.file_offer_mismatch(&transfer_id, &filename, &mime, size)
                {
                    tracing::warn!(transfer_id, %mismatch, "file ready contradicts the accepted offer");
                    self.set_file_status(&transfer_id, "failed", &mismatch);
                    return;
                }
                if ciphertext_size > file_transfer::MAX_PACKAGE_BYTES {
                    self.set_file_status(&transfer_id, "failed", "the sender's file is too large");
                    return;
                }
                let sender = match PublicKey::from_bech32(&sender_npub) {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let key = match file_transfer::unwrap_key(&keys, &sender, &key_wrap) {
                    Ok(v) => v,
                    Err(e) => {
                        self.set_file_status(
                            &transfer_id,
                            "failed",
                            &format!("key unwrap failed: {e}"),
                        );
                        return;
                    }
                };
                self.update_file_transfer(&transfer_id, |r| {
                    r.view.status = "downloading".to_string();
                    r.view.blob_hash = blob_hash.clone();
                    r.view.updated_at = file_transfer::now_secs();
                    r.key_b64 = Some(file_transfer::encode_key(&key));
                    r.ciphertext_size = ciphertext_size;
                });
                let content = Arc::clone(self);
                tokio::spawn(async move {
                    if let Err(e) = content
                        .finish_incoming_transfer(&transfer_id, &sender_npub)
                        .await
                    {
                        content.set_file_status(&transfer_id, "failed", &e.to_string());
                        content.clear_transfer_secrets(&transfer_id);
                    }
                });
            }
            FileMessage::Complete {
                transfer_id,
                recipient_npub,
                ..
            } if recipient_npub == own_npub
                && self.has_file_transfer(&transfer_id, "outgoing", &sender_npub) =>
            {
                // A completed send has nothing left to tell the user, so this
                // is the one terminal state that still clears itself.
                self.set_file_status(&transfer_id, "completed", "");
                self.forget_file_transfer(&transfer_id);
            }
            _ => {}
        }
    }

    async fn finish_outgoing_transfer(&self, transfer_id: &str) -> anyhow::Result<()> {
        let (target_npub, source_path, key_b64, filename, mime, size) = {
            let records = self.file_transfers.lock().unwrap();
            let r = records
                .iter()
                .find(|r| r.view.id == transfer_id && r.view.direction == "outgoing")
                .ok_or_else(|| anyhow::anyhow!("outgoing transfer disappeared"))?;
            (
                r.view.peer_npub.clone(),
                r.source_path
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("outbox path missing"))?,
                r.key_b64
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("file key missing"))?,
                r.view.name.clone(),
                r.view.mime.clone(),
                r.view.size,
            )
        };
        let package = tokio::fs::read(&source_path).await?;
        let blob_hash = self.blobs.put(&package).await?;
        let key = file_transfer::decode_key(&key_b64)?;
        let keys = self
            .device_keys
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
        let target = PublicKey::from_bech32(&target_npub)?;
        let key_wrap = file_transfer::wrap_key(&keys, &target, &key)?;
        let sender_npub = keys.public_key().to_bech32()?;
        let message = FileMessage::Ready {
            transfer_id: transfer_id.to_string(),
            sender_npub,
            recipient_npub: target_npub.clone(),
            filename,
            mime,
            size,
            blob_hash: blob_hash.clone(),
            ciphertext_size: package.len() as u64,
            key_wrap,
        };
        self.send_file_message(&target, &target_npub, message)
            .await?;
        self.update_file_transfer(transfer_id, |r| {
            r.view.status = "ready".to_string();
            r.view.blob_hash = blob_hash.clone();
            r.view.updated_at = file_transfer::now_secs();
        });
        Ok(())
    }

    async fn finish_incoming_transfer(
        &self,
        transfer_id: &str,
        sender_npub: &str,
    ) -> anyhow::Result<()> {
        let (filename, blob_hash, key_b64, declared_size, own_npub) = {
            let records = self.file_transfers.lock().unwrap();
            let r = records
                .iter()
                .find(|r| r.view.id == transfer_id && r.view.direction == "incoming")
                .ok_or_else(|| anyhow::anyhow!("incoming transfer disappeared"))?;
            let keys = self
                .device_keys
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
            (
                r.view.name.clone(),
                r.view.blob_hash.clone(),
                r.key_b64
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("file key missing"))?,
                r.ciphertext_size,
                keys.public_key().to_bech32()?,
            )
        };
        // A paired peer still only gets to send what it said it would send. The
        // ceiling is the size declared in `ready` (or the absolute package cap
        // when a pre-upgrade record carries none), enforced first against the
        // advertised length and then again while the body streams in, so an
        // over-long or length-lying response is dropped rather than buffered.
        let limit = if declared_size == 0 {
            file_transfer::MAX_PACKAGE_BYTES
        } else {
            declared_size.min(file_transfer::MAX_PACKAGE_BYTES)
        };
        let base = crate::ip_source::mesh_blossom_url(sender_npub);
        let url = format!("{base}/{blob_hash}");
        // Nothing about this download is bounded by total time any more, so the
        // row's own state is what ends it: the user cancelling, or the sweeper
        // failing an expired offer, must stop the fetch rather than have it
        // finish minutes later and publish a file that was called off.
        let still_wanted =
            || self.transfer_in_state(transfer_id, "incoming", sender_npub, &["downloading"]);
        // A mesh hop can be slow and can drop mid-body, so the fetch is bounded
        // by silence rather than by total time, and a cut connection is tried
        // again a few times before the transfer is failed.
        let mut attempt = 0;
        let package = loop {
            attempt += 1;
            // Re-warmed per attempt: a retry is usually a link that just
            // flapped, and the route it flapped away from is the stale one.
            crate::dns_intercept::warm_route(sender_npub);
            match Self::fetch_package(
                &url,
                limit,
                file_transfer::DOWNLOAD_IDLE_TIMEOUT,
                &still_wanted,
            )
            .await
            {
                Ok(package) => break package,
                Err(e) if attempt < file_transfer::DOWNLOAD_ATTEMPTS && is_transport_error(&e) => {
                    tracing::warn!(
                        transfer = %transfer_id,
                        attempt,
                        error = %e,
                        "file share: download interrupted, retrying"
                    );
                    tokio::time::sleep(file_transfer::DOWNLOAD_RETRY_DELAY).await;
                    if !still_wanted() {
                        anyhow::bail!("transfer is no longer being downloaded");
                    }
                }
                Err(e) => return Err(e),
            }
        };
        if !still_wanted() {
            anyhow::bail!("transfer is no longer being downloaded");
        }
        if file_transfer::sha256_hex(&package) != blob_hash {
            anyhow::bail!("downloaded encrypted blob hash mismatch");
        }
        let key = file_transfer::decode_key(&key_b64)?;
        let plain = file_transfer::decrypt_file(&package, &key, transfer_id, &own_npub, &filename)?;
        let destination = self
            .received_dir
            .join(format!("{transfer_id}-{}", filename));
        tokio::fs::write(&destination, &plain).await?;
        self.update_file_transfer(transfer_id, |r| {
            r.view.status = "completed".to_string();
            r.view.received_path = destination.to_string_lossy().into_owned();
            r.view.publish_pending = true;
            r.view.updated_at = file_transfer::now_secs();
            r.key_b64 = None;
        });
        self.remember_completed_incoming(transfer_id, sender_npub);
        if let Err(e) = self.send_complete(transfer_id, sender_npub).await {
            // The receiver's local copy is already complete. A failed sender
            // acknowledgement must not turn this back into a failed transfer
            // or prevent Android from publishing it to Downloads.
            tracing::warn!(transfer = %transfer_id, error = %e, "file share: completion acknowledgement failed");
        }
        Ok(())
    }

    /// One download of the encrypted package: bounded against `limit` before
    /// and while the body streams in, and failed if the peer goes quiet for
    /// [`file_transfer::DOWNLOAD_IDLE_TIMEOUT`] — not by total duration, which
    /// a large file over a slow Bluetooth hop legitimately exceeds.
    ///
    /// `still_wanted` is asked between chunks, and answering `false` ends the
    /// fetch. With no total bound, a peer trickling a byte before every idle
    /// timeout would otherwise hold this task and its buffer for as long as it
    /// liked, whatever the row said.
    async fn fetch_package(
        url: &str,
        limit: u64,
        idle: Duration,
        still_wanted: &(dyn Fn() -> bool + Sync),
    ) -> anyhow::Result<Vec<u8>> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let mut response = tokio::time::timeout(idle, client.get(url).send())
            .await
            .map_err(|_| anyhow::anyhow!("peer Blossom did not answer"))??;
        if !response.status().is_success() {
            anyhow::bail!("peer Blossom returned {}", response.status());
        }
        if let Some(advertised) = response.content_length() {
            if advertised > limit {
                anyhow::bail!("peer offered {advertised} bytes but declared {limit}");
            }
        }
        let mut package: Vec<u8> = Vec::with_capacity(limit.min(1024 * 1024) as usize);
        loop {
            let chunk = tokio::time::timeout(idle, response.chunk())
                .await
                .map_err(|_| {
                    anyhow::anyhow!("download stalled: no data for {}s", idle.as_secs())
                })??;
            let Some(chunk) = chunk else { break };
            if package.len() as u64 + chunk.len() as u64 > limit {
                anyhow::bail!("peer sent more than the {limit} bytes it declared");
            }
            package.extend_from_slice(&chunk);
            if !still_wanted() {
                anyhow::bail!("transfer is no longer being downloaded");
            }
        }
        Ok(package)
    }

    /// Tell the sender their file arrived. Sent once on completion and again
    /// for every retried `ready`, since the first may have been dropped.
    async fn send_complete(&self, transfer_id: &str, sender_npub: &str) -> anyhow::Result<()> {
        let own_npub = {
            let keys = self
                .device_keys
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
            keys.public_key().to_bech32()?
        };
        let target = PublicKey::from_bech32(sender_npub)?;
        let message = FileMessage::Complete {
            transfer_id: transfer_id.to_string(),
            sender_npub: own_npub,
            recipient_npub: sender_npub.to_string(),
        };
        self.send_file_message(&target, sender_npub, message).await
    }

    fn remember_completed_incoming(&self, transfer_id: &str, peer_npub: &str) {
        let snapshot = {
            let mut done = self.completed_incoming.lock().unwrap();
            done.retain(|(id, _)| id != transfer_id);
            done.push_back((transfer_id.to_string(), peer_npub.to_string()));
            while done.len() > file_transfer::MAX_TRACKED_TRANSFERS {
                done.pop_front();
            }
            done.clone()
        };
        save_completed_incoming(&self.completed_incoming_path, &snapshot);
    }

    /// Whether `transfer_id` is one we finished **with this peer**. Matching on
    /// the id alone would answer any Circle member who guessed it.
    pub(crate) fn was_completed_incoming(&self, transfer_id: &str, peer_npub: &str) -> bool {
        self.completed_incoming
            .lock()
            .unwrap()
            .iter()
            .any(|(id, peer)| id == transfer_id && peer == peer_npub)
    }

    async fn send_file_message(
        &self,
        target: &PublicKey,
        target_npub: &str,
        message: FileMessage,
    ) -> anyhow::Result<()> {
        let keys = self
            .device_keys
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
        let content = serde_json::to_string(&message)?;
        let event = EventBuilder::private_msg(&keys, *target, content, std::iter::empty()).await?;
        let event_json = serde_json::to_value(event)?;
        // TTL 0 means store and handle at the addressed Circle relay, without
        // flooding a private file-control message to every Circle member.
        let frame = crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::push(0),
            serde_json::json!(["EVENT", event_json]),
        );
        self.gossip_to_peer(target_npub, frame);
        Ok(())
    }

    fn insert_file_transfer(&self, record: FileTransferRecord) {
        let snapshot = {
            let mut records = self.file_transfers.lock().unwrap();
            records.retain(|r| r.view.id != record.view.id);
            records.push(record);
            records.clone()
        };
        save_file_transfers(&self.file_transfers_path, &snapshot);
    }

    fn update_file_transfer<F>(&self, transfer_id: &str, update: F)
    where
        F: FnOnce(&mut FileTransferRecord),
    {
        let snapshot = {
            let mut records = self.file_transfers.lock().unwrap();
            let Some(record) = records.iter_mut().find(|r| r.view.id == transfer_id) else {
                return;
            };
            update(record);
            // Every step forward resets the clock, so the deadline measures a
            // *stall* rather than the whole transfer. Without this the sweeper
            // would eventually kill a large transfer that is progressing
            // normally, just for taking longer than the original offer window.
            if matches!(
                record.view.status.as_str(),
                "offered" | "waiting_user" | "accepted" | "ready" | "downloading"
            ) {
                record.expires_at = file_transfer::now_secs() + file_transfer::OFFER_TTL_SECS;
            }
            records.clone()
        };
        save_file_transfers(&self.file_transfers_path, &snapshot);
    }

    fn set_file_status(&self, transfer_id: &str, status: &str, error: &str) {
        self.update_file_transfer(transfer_id, |r| {
            r.view.status = status.to_string();
            r.view.error = error.to_string();
            r.view.updated_at = file_transfer::now_secs();
        });
    }

    /// Remove a terminal transfer from the persisted transfer list and clean
    /// its app-private ciphertext/plaintext staging file. A completed receive
    /// calls this only after Android has copied the file into MediaStore, so a
    /// repeated send is always a fresh offer/blob rather than a replay.
    pub fn forget_file_transfer(&self, transfer_id: &str) {
        let (snapshot, cleanup_paths) = {
            let mut records = self.file_transfers.lock().unwrap();
            let Some(index) = records.iter().position(|r| r.view.id == transfer_id) else {
                return;
            };
            let record = &records[index];
            if !matches!(
                record.view.status.as_str(),
                "completed" | "denied" | "failed" | "cancelled"
            ) {
                return;
            }
            let record = records.remove(index);
            let mut paths = Vec::new();
            if let Some(path) = record.source_path.as_deref() {
                if let Some(path) = self.transfer_cleanup_path(path) {
                    paths.push(path);
                }
            }
            if !record.view.received_path.is_empty() {
                if let Some(path) = self.transfer_cleanup_path(&record.view.received_path) {
                    paths.push(path);
                }
            }
            (records.clone(), paths)
        };
        save_file_transfers(&self.file_transfers_path, &snapshot);
        for path in cleanup_paths {
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::debug!(path = %path.display(), %error, "file share: staging cleanup failed");
                }
            }
        }
    }

    fn transfer_cleanup_path(&self, path: &str) -> Option<PathBuf> {
        let path = PathBuf::from(path);
        (path.starts_with(&self.file_outbox_dir) || path.starts_with(&self.received_dir))
            .then_some(path)
    }

    /// Why a `ready` message disagrees with the offer the user accepted, or
    /// `None` when it describes the same file. Compared against the offer's
    /// already-sanitised name so a sender cannot slip a new one past the check
    /// by spelling it differently.
    fn file_offer_mismatch(
        &self,
        transfer_id: &str,
        filename: &str,
        mime: &str,
        size: u64,
    ) -> Option<String> {
        let records = self.file_transfers.lock().unwrap();
        let view = &records.iter().find(|r| r.view.id == transfer_id)?.view;
        let offered_name = file_transfer::safe_filename(filename, "shared-file");
        if offered_name != view.name {
            return Some(format!(
                "the sender changed the file name after you accepted \"{}\"",
                view.name
            ));
        }
        if mime != view.mime {
            return Some(format!(
                "the sender changed the file type after you accepted \"{}\"",
                view.name
            ));
        }
        if size != view.size {
            return Some(format!(
                "the sender changed the file size after you accepted \"{}\"",
                view.name
            ));
        }
        None
    }

    /// Drop the key and staging file of a transfer that will never finish, while
    /// keeping its row so the UI can still explain what happened.
    fn clear_transfer_secrets(&self, transfer_id: &str) {
        let stale = {
            let mut records = self.file_transfers.lock().unwrap();
            let Some(record) = records.iter_mut().find(|r| r.view.id == transfer_id) else {
                return;
            };
            record.key_b64 = None;
            let stale = record.source_path.take();
            let snapshot = records.clone();
            drop(records);
            save_file_transfers(&self.file_transfers_path, &snapshot);
            stale
        };
        if let Some(path) = stale.as_deref().and_then(|p| self.transfer_cleanup_path(p)) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Fail every transfer whose offer window has closed. Without this a control
    /// message that never arrives — the push plane is best-effort and drops
    /// frames while a peer is in dial backoff — leaves a transfer pending
    /// forever with no way for the user to clear it. Driven by the keepwarm
    /// tick, so an offer dies `OFFER_TTL_SECS` after it was made.
    pub fn sweep_file_transfers(&self) {
        let now = file_transfer::now_secs();
        let expired: Vec<String> = self
            .file_transfers
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                r.expires_at > 0
                    && r.expires_at <= now
                    && !matches!(
                        r.view.status.as_str(),
                        "completed" | "denied" | "failed" | "cancelled"
                    )
            })
            .map(|r| r.view.id.clone())
            .collect();
        for id in expired {
            tracing::info!(transfer = %id, "file share: offer timed out");
            self.set_file_status(&id, "failed", "timed out waiting for the other phone");
            self.clear_transfer_secrets(&id);
        }
    }

    /// The control messages a stalled transfer is still waiting to have heard.
    ///
    /// Every control message is a single push, and the push plane drops frames
    /// while a peer is in dial backoff — so an accept pressed during a BLE flap
    /// never arrives and the sender waits out the whole offer TTL. Each state
    /// that waits on the *other* side is re-sent from what the record already
    /// holds once it has sat for [`file_transfer::RESEND_AFTER_SECS`] since the
    /// last step or retry: an outgoing `offered` re-sends the offer, an incoming
    /// `accepted` the accept, an outgoing `ready` the ready. Safe to repeat —
    /// every receiving handler is gated on the state it advances from, so a
    /// duplicate of a message that already landed is ignored. Stamps the rows
    /// it returns; the caller sends.
    pub(crate) fn stalled_file_messages(&self, now: u64) -> Vec<(String, FileMessage)> {
        let keys = self.device_keys.lock().unwrap().clone();
        let Some(keys) = keys else {
            return Vec::new();
        };
        let Ok(own_npub) = keys.public_key().to_bech32();
        // Taken before the transfer lock, never under it: every other path
        // takes the Circle lock first.
        let circle: HashSet<String> = self.circle_npubs().into_iter().collect();
        let mut out = Vec::new();
        let snapshot = {
            let mut records = self.file_transfers.lock().unwrap();
            for r in records.iter_mut() {
                let since = r.view.updated_at.max(r.last_resend_at);
                if r.expires_at <= now
                    || now.saturating_sub(since) < file_transfer::RESEND_AFTER_SECS
                {
                    continue;
                }
                // Removing someone stops us talking to them. Their side drops a
                // message from a non-Circle sender anyway; retrying at a person
                // just removed is the part that would be wrong.
                if !circle.contains(&r.view.peer_npub) {
                    continue;
                }
                let state = (r.view.direction.as_str(), r.view.status.as_str());
                if !matches!(
                    state,
                    ("outgoing", "offered") | ("incoming", "accepted") | ("outgoing", "ready")
                ) {
                    continue;
                }
                // Stamped before the message is built, so a row we cannot
                // rebuild waits out the window like any other rather than
                // retrying the same failing work every tick.
                r.last_resend_at = now;
                let message = match state {
                    ("outgoing", "offered") => FileMessage::Offer {
                        transfer_id: r.view.id.clone(),
                        sender_npub: own_npub.clone(),
                        recipient_npub: r.view.peer_npub.clone(),
                        filename: r.view.name.clone(),
                        mime: r.view.mime.clone(),
                        size: r.view.size,
                        issued_at: r.expires_at.saturating_sub(file_transfer::OFFER_TTL_SECS),
                        expires_at: r.expires_at,
                    },
                    ("incoming", "accepted") => FileMessage::Response {
                        transfer_id: r.view.id.clone(),
                        sender_npub: own_npub.clone(),
                        recipient_npub: r.view.peer_npub.clone(),
                        accepted: true,
                        reason: None,
                    },
                    ("outgoing", "ready") => {
                        let (Some(key_b64), Ok(target)) = (
                            r.key_b64.as_deref(),
                            PublicKey::from_bech32(&r.view.peer_npub),
                        ) else {
                            continue;
                        };
                        let Ok(key) = file_transfer::decode_key(key_b64) else {
                            continue;
                        };
                        let Ok(key_wrap) = file_transfer::wrap_key(&keys, &target, &key) else {
                            continue;
                        };
                        FileMessage::Ready {
                            transfer_id: r.view.id.clone(),
                            sender_npub: own_npub.clone(),
                            recipient_npub: r.view.peer_npub.clone(),
                            filename: r.view.name.clone(),
                            mime: r.view.mime.clone(),
                            size: r.view.size,
                            blob_hash: r.view.blob_hash.clone(),
                            ciphertext_size: r.ciphertext_size,
                            key_wrap,
                        }
                    }
                    _ => continue,
                };
                out.push((r.view.peer_npub.clone(), message));
            }
            records.clone()
        };
        if !out.is_empty() {
            save_file_transfers(&self.file_transfers_path, &snapshot);
        }
        out
    }

    /// Cancel a transfer the user no longer wants and tell the other phone, so
    /// its own row resolves instead of sitting pending until the sweeper runs.
    pub async fn cancel_file_transfer(self: Arc<Self>, transfer_id: String) -> anyhow::Result<()> {
        let (peer_npub, direction) = {
            let records = self.file_transfers.lock().unwrap();
            let record = records
                .iter()
                .find(|r| r.view.id == transfer_id)
                .ok_or_else(|| anyhow::anyhow!("transfer not found"))?;
            if matches!(
                record.view.status.as_str(),
                "completed" | "denied" | "failed" | "cancelled"
            ) {
                anyhow::bail!("transfer has already finished");
            }
            (record.view.peer_npub.clone(), record.view.direction.clone())
        };
        self.set_file_status(&transfer_id, "cancelled", "cancelled on this phone");
        self.clear_transfer_secrets(&transfer_id);
        let own_npub = {
            let keys = self
                .device_keys
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("device identity is not ready"))?;
            keys.public_key().to_bech32()?
        };
        let target = PublicKey::from_bech32(&peer_npub)?;
        // A cancel is a decline in both directions: it is the same "this is not
        // happening" the responder already knows how to act on, so no new
        // message type is added to the wire for it.
        let message = FileMessage::Response {
            transfer_id: transfer_id.clone(),
            sender_npub: own_npub,
            recipient_npub: peer_npub.clone(),
            accepted: false,
            reason: Some(if direction == "outgoing" {
                "cancelled by the sender".to_string()
            } else {
                "cancelled by the recipient".to_string()
            }),
        };
        self.send_file_message(&target, &peer_npub, message).await
    }

    /// Like [`Self::has_file_transfer`], but also requires the transfer to be in
    /// one of `states`. Every message that advances a transfer uses this: the
    /// state machine's guarantees come from refusing out-of-order messages, not
    /// from trusting the sender to send them in order.
    fn transfer_in_state(
        &self,
        transfer_id: &str,
        direction: &str,
        peer_npub: &str,
        states: &[&str],
    ) -> bool {
        self.file_transfers.lock().unwrap().iter().any(|r| {
            r.view.id == transfer_id
                && r.view.direction == direction
                && r.view.peer_npub == peer_npub
                && states.contains(&r.view.status.as_str())
        })
    }

    /// Drop finished rows, oldest first, to make room for a new transfer.
    fn prune_finished_transfers(&self) {
        let finished: Vec<String> = {
            let mut records = self.file_transfers.lock().unwrap();
            records.sort_by_key(|r| r.view.updated_at);
            records
                .iter()
                .filter(|r| {
                    matches!(
                        r.view.status.as_str(),
                        "completed" | "denied" | "failed" | "cancelled"
                    )
                })
                .map(|r| r.view.id.clone())
                .collect()
        };
        for id in finished {
            self.forget_file_transfer(&id);
        }
    }

    fn has_file_transfer(&self, transfer_id: &str, direction: &str, peer_npub: &str) -> bool {
        self.file_transfers.lock().unwrap().iter().any(|r| {
            r.view.id == transfer_id
                && r.view.direction == direction
                && r.view.peer_npub == peer_npub
        })
    }

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

    /// Pull plane, collected: every event [`Self::pull_from_peers_stream`]
    /// brings, once every peer has finished or run out of budget. For a
    /// caller that needs the whole answer; one that can use events as they
    /// come takes the stream.
    pub async fn pull_from_peers(
        self: &Arc<Self>,
        filters: Vec<serde_json::Value>,
        meta: crate::mesh_wire::MeshMeta,
        exclude: Option<std::net::IpAddr>,
    ) -> Vec<Event> {
        let mut events = self.pull_from_peers_stream(filters, meta, exclude);
        let mut out = Vec::new();
        while let Some(event) = events.recv().await {
            out.push(event);
        }
        out
    }

    /// Pull plane: forward a REQ's filters to connected Circle peers and
    /// hand on each matching event the moment a peer sends it — every peer's
    /// answer merged into one stream, which ends when the last peer is done.
    /// `meta` is the incoming envelope, already decremented, so the hop
    /// budget and query id carry onward while the filters stay canonical
    /// NIP-01. `exclude` is the requester's mesh address (split-horizon).
    /// Each peer is bounded by the budget that arrived, so a dead relay can't
    /// stall discovery — and the nearest peer's answer is never held for the
    /// slowest's.
    pub fn pull_from_peers_stream(
        self: &Arc<Self>,
        filters: Vec<serde_json::Value>,
        meta: crate::mesh_wire::MeshMeta,
        exclude: Option<std::net::IpAddr>,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // The filters go out untouched — hops, query id, and budget ride the
        // envelope. `meta` is the *incoming* one, already decremented, so the
        // query id survives the hop and every node downstream serves it once.

        // All filters ride in one REQ per peer over the shared connection (the relay
        // any-matches across them), so a pull is a single round-trip, not one socket
        // per filter.
        for npub in self.circle_npubs() {
            let Ok(peer) = fips::PeerIdentity::from_npub(&npub) else {
                continue;
            };
            let ip = std::net::IpAddr::V6(peer.address().to_ipv6());
            if exclude == Some(ip) {
                continue;
            }
            let url = crate::ip_source::mesh_relay_url(&npub);
            // Wait only as long as the budget that arrived allows, not a fresh
            // full-length timer. Otherwise this hop's window sits *inside* the
            // one above it, and a peer further out returns after the requester
            // has already given up (D8).
            let deadline = tokio::time::Instant::now() + meta.hop_timeout(PULL_HOP_TIMEOUT);
            let mut events =
                self.peer_relays
                    .request_stream(&npub, &url, filters.clone(), Some(meta.clone()));
            let (this, tx) = (self.clone(), tx.clone());
            tokio::spawn(async move {
                // Passing through on their way to whoever asked: keep the
                // profiles, relay lists and manifests, and cache the rest, so
                // the next ask stops here — in small batches, not an LMDB
                // write per event. Verified by the pool at ingress.
                let mut passing = Vec::new();
                while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
                    passing.push(event.clone());
                    if passing.len() >= PULL_REMEMBER_BATCH {
                        this.remember(&passing);
                        passing.clear();
                    }
                    if tx.send(event).is_err() {
                        break;
                    }
                }
                this.remember(&passing);
            });
        }
        rx
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

        // Query set, one combined REQ per relay read until EOSE
        // (docs/design/nsite/nsite-updates.md §3.2):
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
        if mesh_peers.is_empty() && online.is_empty() {
            return NsiteCheck::Done("No peers or relays to check".to_string());
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
            // A relay named only by a relay list is resolved and refused if
            // it points at a private address; a default also carries the
            // authors' lists.
            let from_list = !defaults
                .iter()
                .any(|d| crate::ip_source::same_relay(d, &url));
            let filters = if from_list {
                vec![online_filter.clone()]
            } else {
                vec![online_filter.clone(), list_filter.clone()]
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
        let (mesh_res, online_res, ()) =
            futures_util::future::join3(join_all(mesh_q), join_all(online_q), fetch_missing).await;
        for ev in online_res.iter().flatten() {
            if ev.kind == Kind::RelayList && pubkeys.contains(&ev.pubkey) {
                outbox.remember(ev.clone()).await;
            }
        }

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
            self.forward_updated_manifest(&candidate);
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

    /// How many hops a pushed manifest — an nsite's or a napplet's — may still
    /// travel from here. 0 means stop here.
    ///
    /// Mirrors chat: a local publish originates at the default — or at the
    /// budget a napplet chose through NAP-MESH, where `Some(0)` means "store
    /// here, send nowhere" — and a mesh push at the ttl that rode in, clamped
    /// so a peer can't over-extend us. The same per-peer clamp the chat push
    /// plane applies: a peer we have not granted multihop writes still gets
    /// its manifest stored and served here, it simply travels no further
    /// through us. Manifests were missing this check, so that grant was
    /// enforced on one plane but not the other (D10).
    fn manifest_forward_budget(&self, inbound: &Inbound) -> u8 {
        let peer_cap = match inbound.sender {
            Some(ip) if !self.may_forward_from(ip) => 0,
            _ => crate::mesh_wire::EVENT_TTL,
        };
        match inbound.origin {
            Origin::Local => inbound.event_ttl.unwrap_or(crate::mesh_wire::EVENT_TTL),
            Origin::Mesh => inbound.event_ttl.unwrap_or(0),
        }
        .min(crate::mesh_wire::EVENT_TTL)
        .min(peer_cap)
    }

    /// A manifest landed in our relay over the mesh (a peer's push, forwarded by
    /// the gossiper). Propagate it like any event (`docs/design/nsite/nsite-updates.md`
    /// §4); if it's one of our installed sites, download its blobs from the sender
    /// and activate. Forwarding never waits on the download for sites we don't run.
    pub async fn on_manifest_event(self: Arc<Self>, event: Event, inbound: Inbound) {
        let d = event_d_tag(&event);
        let addr = SiteAddr {
            author: event.pubkey,
            d_tag: d,
        };

        let effective = self.manifest_forward_budget(&inbound);
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
        let sources = self.manifest_push_sources(inbound.sender, None);
        // Manifest is already in our relay (NIP-01), so don't re-store.
        let _ = Arc::clone(&self)
            .download_and_activate(addr, event.clone(), sources, false)
            .await;
        // Forward regardless of download outcome so the wave never stalls (§4).
        if effective > 0 {
            self.forward_manifest(&event, out_ttl, inbound.sender);
        }
    }

    /// Where to fetch the bytes of a manifest pushed to us, in order: the peer
    /// that sent it (its mesh Blossom), then the public Blossom servers unless
    /// offline-only. The same for nsites and napplets.
    ///
    /// `max_blob_bytes` caps each blob as it streams in, before the hash check
    /// has anything to say: whoever pushed the manifest also chose what it
    /// references. Napplets pass NAP-RESOURCE's cap; nsites have no agreed
    /// per-blob limit yet and pass `None`.
    fn manifest_push_sources(
        &self,
        sender: Option<IpAddr>,
        max_blob_bytes: Option<usize>,
    ) -> Vec<Arc<dyn PeerSource>> {
        let cap = |source: crate::ip_source::IpPeerSource| match max_blob_bytes {
            Some(max) => source.with_max_blob_bytes(max),
            None => source,
        };
        let mut sources: Vec<Arc<dyn PeerSource>> = Vec::new();
        if let Some(IpAddr::V6(ip)) = sender {
            // The one place a bare mesh address is right: this is whoever just
            // sent us the event, known only as a transport address — an address
            // does not reduce back to an npub. It is safe here precisely because
            // they just reached us, so the node already holds their identity;
            // everywhere else, peers are addressed as `<npub>.fips` so that
            // resolving the name registers that identity (see
            // `ip_source::mesh_relay_url`).
            sources.push(Arc::new(cap(crate::ip_source::IpPeerSource::new(
                vec![format!("ws://[{ip}]:4870")],
                vec![format!("http://[{ip}]:24243")],
            )
            .ignoring_manifest_servers())));
        }
        if !self.is_offline_only() {
            sources.push(Arc::new(cap(crate::ip_source::IpPeerSource::new(
                crate::ip_source::default_relays(),
                crate::ip_source::default_blossom_servers(),
            ))));
        }
        sources
    }

    /// Send a manifest this device just brought in by an update check to the
    /// Circle: we now hold its bytes, so we are a source for the next hop.
    /// Originates at the default budget, like any local publish.
    pub fn forward_updated_manifest(&self, manifest: &Event) {
        self.forward_manifest(
            manifest,
            crate::mesh_wire::EVENT_TTL.saturating_sub(1),
            None,
        );
    }

    /// A napplet manifest (`15129` / `35129`) landed in our relay over the push
    /// plane. The napplet counterpart of [`Content::on_manifest_event`]: the same
    /// interest-aware download-then-forward policy, hop budget, split-horizon
    /// and multihop clamp, with the napplet's own checks in front
    /// (`docs/design/napplet/napplet-runtime.md` §7.3).
    ///
    /// Snapshots (`5129`) never come here — they are immutable builds that no
    /// Library entry resolves by, so there is nothing to update, and they stay
    /// on the plain gossip path.
    pub async fn on_napplet_manifest_event(&self, event: Event, inbound: Inbound) {
        // A napplet is one `index.html`; nothing it may reference is bigger
        // than what a napplet may fetch by hash.
        let sources = self.manifest_push_sources(
            inbound.sender,
            Some(myco_napplet_runtime::nap::resource::MAX_BYTES),
        );
        let id = event.id;
        let outcome = self
            .handle_napplet_manifest(event, &inbound, &sources, |manifest, ttl| {
                self.forward_manifest(manifest, ttl, inbound.sender)
            })
            .await;
        tracing::info!(event = %id, ?outcome, "napplet manifest push");
    }

    /// The policy behind [`Content::on_napplet_manifest_event`], with the
    /// sources and the fan-out handed in so it can be exercised without a
    /// mesh. `forward` is called at most once, with the outbound hop budget,
    /// and only after any download has finished.
    ///
    /// - Not a valid, signed NIP-5D manifest, or older than the version this
    ///   relay already keeps for the slot: stopped here, not passed on.
    /// - Not installed here: passed on at once, nothing fetched — as an nsite
    ///   we don't run.
    /// - Installed, but not newer than the pinned version: a replay or a
    ///   downgrade, stopped here.
    /// - Installed and newer: the bytes are fetched from `sources` in order,
    ///   verified, stored and pinned (the pin never moves back, see
    ///   [`Content::set_active_if_newer`]) — the next launch opens it, an open
    ///   window keeps its session — and then it is passed on. A download that
    ///   fails still passes it on, so the wave never stalls on one phone.
    ///
    /// Nothing here touches grants. The Library entry keeps what the user
    /// reviewed; a version declaring more comes back through the review sheet
    /// at its next open (`NappletHost::open_with`).
    pub(crate) async fn handle_napplet_manifest(
        &self,
        event: Event,
        inbound: &Inbound,
        sources: &[Arc<dyn PeerSource>],
        forward: impl FnOnce(&Event, u8),
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
        // a stale version, not worth anyone's hop.
        if let Ok(Some(newest)) =
            nsite_deck::seams::newest_in_slot(self.relay.as_ref(), kind, &author, d_tag).await
        {
            if newest.created_at > event.created_at {
                return NappletPush::Dropped;
            }
        }

        let budget = self.manifest_forward_budget(inbound);

        // Installed means the same author and `d` tag in the Library: the
        // signature already ties the manifest to `author`, so a napplet by
        // anyone else under the same name is a different napplet.
        let npub = author.to_bech32().unwrap_or_default();
        if self.napplet_grants(&npub, d_tag).is_none() {
            if budget > 0 {
                forward(&event, budget - 1);
            }
            return NappletPush::Relayed;
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
        }
        if budget > 0 {
            forward(&event, budget - 1);
        }
        if updated {
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
    /// Two versions downloading at once — a push from the Circle and the
    /// update check, say — must not leave the older one served because it
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
        let snapshot = {
            let mut m = self.active_manifests.lock().unwrap();
            if m.get(&key)
                .is_some_and(|cur| cur.created_at > manifest.created_at)
            {
                return false;
            }
            m.insert(key, manifest.clone());
            m.values().cloned().collect::<Vec<_>>()
        };
        save_active(&self.active_path, &snapshot);
        true
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
        self.clear_file_transfers();
        Ok(())
    }

    /// Drop every transfer and the files staged for them.
    ///
    /// The `received/` directory holds **decrypted plaintext** between the
    /// native receive finishing and Android publishing it, and `file-outbox/`
    /// holds the encrypted copy of everything being sent. A privacy control that
    /// leaves either behind is not doing its job, so both wipes clear them.
    fn clear_file_transfers(&self) {
        self.file_transfers.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.file_transfers_path);
        for dir in [&self.file_outbox_dir, &self.received_dir] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// The shell cache is not cleared here: it can hold the whole blob
    /// budget, and deleting that must not hold the caller. The runtime spawns
    /// [`Content::clear_cache`] beside this (and beside [`Content::wipe`]).
    ///
    /// Clear cached relay events + Blossom blobs **except** those backing pinned
    /// nsites (Settings → Storage → "Clear local database"). The served manifest version of
    /// each pinned site and every blob it references survive, so installed apps keep
    /// working offline; everything else — unpinned opened sites and staged
    /// updates — is dropped. Identity and Circle are untouched.
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
                if let Some((event, index_hash)) = self.napplet_keep_set(item).await {
                    keep_events.insert(event.id.to_bytes());
                    keep_blobs.insert(index_hash);
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
        self.clear_file_transfers();

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
        self.pending_updates.lock().unwrap().clear();
        let active_snapshot = {
            let mut m = self.active_manifests.lock().unwrap();
            m.retain(|k, _| keep_active.contains(k));
            m.values().cloned().collect::<Vec<_>>()
        };
        save_active(&self.active_path, &active_snapshot);
        Ok(())
    }

    /// The manifest event and index-blob hash an installed napplet is served
    /// from, for the keep-sets: the active manifest when one is pinned, else
    /// the newest in the slot.
    async fn napplet_keep_set(&self, item: &LibraryItem) -> Option<(Event, String)> {
        use nostr::nips::nip19::FromBech32;
        let author = nostr::PublicKey::from_bech32(&item.author_npub).ok()?;
        let kind = match item.d_tag {
            Some(_) => myco_napplet_runtime::KIND_NAMED,
            None => myco_napplet_runtime::KIND_ROOT,
        };
        let event = nsite_deck::seams::newest_in_slot(
            &self.active_backend(),
            kind,
            &author,
            item.d_tag.as_deref(),
        )
        .await
        .ok()??;
        let manifest = myco_napplet_runtime::NappletManifest::from_event(event.clone()).ok()?;
        let index = manifest.index_entry()?.sha256.clone();
        Some((event, index))
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

fn load_outbound_pairs(path: &Path) -> Vec<OutboundPairView> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_outbound_pairs(path: &Path, items: &[OutboundPairView]) {
    if let Ok(json) = serde_json::to_vec(items) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}

/// Whether a download error is the connection's fault (worth another try)
/// rather than the peer's answer (a status, a size lie, a hash mismatch).
fn is_transport_error(error: &anyhow::Error) -> bool {
    match error.downcast_ref::<reqwest::Error>() {
        Some(e) => e.is_timeout() || e.is_connect() || e.is_body() || e.is_request(),
        None => {
            let text = error.to_string();
            text.starts_with("download stalled") || text.starts_with("peer Blossom did not answer")
        }
    }
}

fn load_file_transfers(path: &Path) -> Vec<FileTransferRecord> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_file_transfers(path: &Path, items: &[FileTransferRecord]) {
    if let Ok(json) = serde_json::to_vec(items) {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path));
    }
}

fn load_completed_incoming(path: &Path) -> VecDeque<(String, String)> {
    std::fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save_completed_incoming(path: &Path, items: &VecDeque<(String, String)>) {
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

/// What became of a napplet manifest pushed to this device. See
/// [`Content::handle_napplet_manifest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NappletPush {
    /// Invalid, stale, or not newer than the installed version: stopped here.
    Dropped,
    /// Not installed here: passed on, nothing fetched.
    Relayed,
    /// Installed: the new version's bytes are here and pinned, then passed on.
    Updated,
    /// Installed, but no source supplied the bytes: passed on regardless.
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
    /// "Download again", the update check, a push from the Circle — goes
    /// through [`Content::set_active_if_newer`]. See there for why.
    fn pin(&self, manifest: &Event) {
        self.set_active_if_newer(manifest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip19::ToBech32;
    use nsite_deck::testing::build_test_site;

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

        content.clone().open_site(addr.clone(), None).await;
        let listed = |c: &Content| c.sites.lock().unwrap().contains_key(&addr.host_label());
        assert!(listed(&content), "an unreachable site has no tile");

        content.forget_site(&addr);
        assert!(!listed(&content));
        // What an in-flight sync or a reloading window would do.
        content.clone().open_site(addr.clone(), None).await;
        content.add_to_library(&addr, None, now_secs());
        assert!(!listed(&content), "the removed tile came back");
        assert!(
            content.library_snapshot().is_empty(),
            "the removed site was pinned again"
        );

        content.unforget_site(&addr);
        content.clone().open_site(addr.clone(), None).await;
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

    /// The multihop-write clamp holds for napplet manifests too: a push from a
    /// peer we have not granted multihop writes is kept here and goes no
    /// further; the same push from a granted peer is passed on.
    #[tokio::test]
    async fn a_napplet_manifest_from_a_peer_without_multihop_goes_no_further() {
        let dir = tmp("napplet-multihop");
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
        let from_peer = Inbound {
            origin: Origin::Mesh,
            event_ttl: Some(3),
            sender: Some(ip),
        };
        let push = |at: u64| {
            let content = content.clone();
            let inbound = from_peer;
            async move {
                let napplet = myco_napplet_runtime::testing::NappletBuilder::new()
                    .created_at(at)
                    .build();
                content
                    .relay()
                    .publish(napplet.manifest.clone())
                    .await
                    .unwrap();
                let mut forwarded = Vec::new();
                let outcome = content
                    .handle_napplet_manifest(napplet.manifest, &inbound, &[], |_, ttl| {
                        forwarded.push(ttl)
                    })
                    .await;
                (outcome, forwarded)
            }
        };

        let (outcome, forwarded) = push(1_000).await;
        assert_eq!(outcome, NappletPush::Relayed);
        assert_eq!(
            forwarded,
            vec![2],
            "a granted peer's push was not passed on"
        );

        content.circle.lock().unwrap()[0].perms.relay_write_multihop = false;
        let (outcome, forwarded) = push(2_000).await;
        assert_eq!(outcome, NappletPush::Relayed);
        assert!(
            forwarded.is_empty(),
            "a revoked peer's napplet went further"
        );

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

    /// A record for driving the transfer state machine without a peer.
    fn incoming_record(id: &str, expires_at: u64) -> FileTransferRecord {
        FileTransferRecord {
            view: FileTransferView {
                id: id.to_string(),
                direction: "incoming".to_string(),
                peer_npub: "npub1peer".to_string(),
                peer_name: "Peer".to_string(),
                name: "photo.jpg".to_string(),
                mime: "image/jpeg".to_string(),
                size: 2048,
                status: "waiting_user".to_string(),
                blob_hash: String::new(),
                received_path: String::new(),
                publish_pending: false,
                error: String::new(),
                updated_at: 0,
            },
            source_path: None,
            key_b64: None,
            ciphertext_size: 0,
            expires_at,
            last_resend_at: 0,
        }
    }

    /// A `ready` message is only allowed to describe the file the user actually
    /// said yes to. Changing the name, type or size after the accept is a
    /// bait-and-switch, and each one has to be caught on its own — the AEAD
    /// cannot catch it, because the sender recomputes the AAD from whatever it
    /// sends.
    #[test]
    fn a_ready_message_cannot_change_what_the_user_accepted() {
        let dir = tmp("file-ready-mismatch");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        content.insert_file_transfer(incoming_record("t1", u64::MAX));

        assert!(
            content
                .file_offer_mismatch("t1", "photo.jpg", "image/jpeg", 2048)
                .is_none(),
            "the offered file itself must pass"
        );
        assert!(
            content
                .file_offer_mismatch("t1", "invoice.pdf", "image/jpeg", 2048)
                .is_some(),
            "a renamed file must be refused"
        );
        assert!(
            content
                .file_offer_mismatch("t1", "photo.jpg", "application/pdf", 2048)
                .is_some(),
            "a retyped file must be refused"
        );
        assert!(
            content
                .file_offer_mismatch("t1", "photo.jpg", "image/jpeg", 900 * 1024 * 1024)
                .is_some(),
            "a resized file must be refused"
        );
        // Spelling the same name with a path prefix is still the same name.
        assert!(
            content
                .file_offer_mismatch("t1", "../photo.jpg", "image/jpeg", 2048)
                .is_none(),
            "comparison happens after sanitising, not before"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A slow Bluetooth hop used to fail a large download by hitting a total
    /// timeout while bytes were still arriving ("error decoding response
    /// body"). The fetch is now bounded by silence: a body that keeps trickling
    /// in completes no matter how long it takes, and one that goes quiet fails
    /// with an error the retry loop recognises as the connection's fault.
    #[tokio::test]
    async fn a_download_is_failed_by_silence_not_by_total_time() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            // First request: 8 bytes, drip-fed slower than the old total cap
            // would scale to. Second: 8 bytes promised, 4 sent, then silence.
            // Third: another trickle, for the abandoned case. Each is served on
            // its own task, so the one that goes quiet does not hold up the
            // next connection.
            for stalls in [false, true, false] {
                let (mut sock, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n")
                        .await
                        .unwrap();
                    for i in 0..8u8 {
                        if stalls && i == 4 {
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            break;
                        }
                        sock.write_all(&[i]).await.unwrap();
                        sock.flush().await.unwrap();
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/blob");
        let idle = Duration::from_millis(600);

        let wanted = || true;
        let slow = Content::fetch_package(&url, 1024, idle, &wanted)
            .await
            .unwrap();
        assert_eq!(
            slow,
            (0..8u8).collect::<Vec<_>>(),
            "a trickle still completes"
        );

        let stalled = Content::fetch_package(&url, 1024, idle, &wanted)
            .await
            .unwrap_err();
        assert!(
            stalled.to_string().starts_with("download stalled"),
            "silence must fail the fetch: {stalled}"
        );
        assert!(is_transport_error(&stalled), "and be retried, not reported");
        assert!(
            !is_transport_error(&anyhow::anyhow!("downloaded encrypted blob hash mismatch")),
            "a wrong answer from the peer is not retried"
        );

        // Cancelled, or swept after the offer expired: the fetch stops instead
        // of finishing minutes later and publishing a file nobody wants.
        let abandoned = Content::fetch_package(&url, 1024, idle, &|| false)
            .await
            .unwrap_err();
        assert!(
            abandoned.to_string().starts_with("transfer is no longer"),
            "a row that left `downloading` ends the fetch: {abandoned}"
        );
        assert!(
            !is_transport_error(&abandoned),
            "and is not treated as a connection fault worth retrying"
        );
    }

    /// An accept pressed while the peer's relay link is in dial backoff is
    /// dropped on the floor, and the sender used to wait out the whole offer
    /// TTL for it. Each side re-sends the message its state is waiting to have
    /// heard — but only once the row has sat still for a while, never for a
    /// finished or expired row, and not again until the window has passed.
    #[test]
    fn a_stalled_transfer_resends_its_pending_control_message() {
        let dir = tmp("file-resend");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        content.set_device_keys(&Keys::generate().secret_key().to_bech32().unwrap());
        let peer = Keys::generate().public_key().to_bech32().unwrap();
        content.add_to_circle(&peer, "peer");
        let now = 1_000_000;
        let mut offered = incoming_record("offered", now + 60);
        offered.view.direction = "outgoing".to_string();
        offered.view.status = "offered".to_string();
        offered.view.peer_npub = peer.clone();
        offered.view.updated_at = now - 30;
        let mut accepted = incoming_record("accepted", now + 60);
        accepted.view.status = "accepted".to_string();
        accepted.view.peer_npub = peer.clone();
        accepted.view.updated_at = now - 30;
        let mut fresh = incoming_record("fresh", now + 60);
        fresh.view.status = "accepted".to_string();
        fresh.view.peer_npub = peer.clone();
        fresh.view.updated_at = now - 2;
        let mut waiting = incoming_record("waiting", now + 60);
        waiting.view.peer_npub = peer.clone();
        waiting.view.updated_at = now - 30;
        let mut expired = incoming_record("expired", now - 1);
        expired.view.status = "accepted".to_string();
        expired.view.peer_npub = peer.clone();
        expired.view.updated_at = now - 30;
        // Removed from the Circle mid-transfer: we stop talking to them.
        let removed_peer = Keys::generate().public_key().to_bech32().unwrap();
        let mut removed = incoming_record("removed", now + 60);
        removed.view.status = "accepted".to_string();
        removed.view.peer_npub = removed_peer;
        removed.view.updated_at = now - 30;
        for r in [offered, accepted, fresh, waiting, expired, removed] {
            content.insert_file_transfer(r);
        }

        let mut resent: Vec<(String, String)> = content
            .stalled_file_messages(now)
            .into_iter()
            .map(|(npub, m)| {
                let kind = match m {
                    FileMessage::Offer { .. } => "offer",
                    FileMessage::Response { accepted: true, .. } => "accept",
                    _ => "other",
                };
                (npub, format!("{}:{kind}", m.transfer_id()))
            })
            .collect();
        resent.sort();
        assert_eq!(
            resent,
            vec![
                (peer.clone(), "accepted:accept".to_string()),
                (peer, "offered:offer".to_string())
            ],
            "only the rows waiting on the other side, with a peer still in the Circle, \
             and only once they have stalled"
        );
        assert!(
            content.stalled_file_messages(now + 5).is_empty(),
            "a retry must not repeat until the window has passed again"
        );
        // By now the row that was fresh has stalled as well.
        assert_eq!(
            content
                .stalled_file_messages(now + file_transfer::RESEND_AFTER_SECS)
                .len(),
            3,
            "and repeats once it has"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The receiver forgets a finished row as soon as the shell publishes the
    /// file, but its `complete` may have been lost and the sender is then
    /// retrying `ready` against a transfer we no longer track. The id has to
    /// stay known so that retry is answered, and the memory must not grow
    /// without bound.
    #[test]
    fn a_finished_incoming_transfer_stays_known_after_its_row_is_forgotten() {
        let dir = tmp("file-completed-memory");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let peer = Keys::generate().public_key().to_bech32().unwrap();
        let other = Keys::generate().public_key().to_bech32().unwrap();
        assert!(!content.was_completed_incoming("t1", &peer));
        content.remember_completed_incoming("t1", &peer);
        content.forget_file_transfer("t1");
        assert!(
            content.was_completed_incoming("t1", &peer),
            "forgetting the row must not forget the id"
        );
        assert!(
            !content.was_completed_incoming("t1", &other),
            "another Circle member asking about the same id is not answered"
        );
        // A restart between publishing the file and the sender giving up must
        // not cost them the rest of the offer TTL.
        let reopened = Content::open(&dir).unwrap();
        assert!(
            reopened.was_completed_incoming("t1", &peer),
            "the memory has to survive a restart"
        );
        for i in 0..file_transfer::MAX_TRACKED_TRANSFERS {
            content.remember_completed_incoming(&format!("later-{i}"), &peer);
        }
        assert!(
            !content.was_completed_incoming("t1", &peer),
            "the oldest id is evicted at the cap"
        );
        assert!(content.was_completed_incoming("later-0", &peer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The control plane is best-effort — `PeerRelayPool::send` drops frames
    /// while a peer is in dial backoff — so a transfer can be left waiting on a
    /// message that will never arrive. The sweeper is the only thing that ends
    /// it, and the row has to survive as `failed` so the UI can say why.
    #[test]
    fn an_offer_that_is_never_answered_expires_instead_of_hanging() {
        let dir = tmp("file-sweep");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        content.insert_file_transfer(incoming_record("stale", 1));
        content.insert_file_transfer(incoming_record("fresh", u64::MAX));

        content.sweep_file_transfers();

        let rows = content.file_transfers_snapshot();
        let stale = rows.iter().find(|r| r.id == "stale").unwrap();
        assert_eq!(stale.status, "failed", "an expired offer must terminate");
        assert!(
            !stale.error.is_empty(),
            "the row has to survive carrying its reason — deleting it is what \
             made every failure invisible"
        );
        assert_eq!(
            rows.iter().find(|r| r.id == "fresh").unwrap().status,
            "waiting_user",
            "a live offer is untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `received/` holds decrypted plaintext until Android publishes it. Both
    /// wipes have to clear it — a "delete my data" control that leaves the
    /// plaintext of received files on disk is worse than not having one.
    #[tokio::test]
    async fn wiping_clears_staged_plaintext() {
        let dir = tmp("file-wipe-staging");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        let staged = dir.join("received").join("leftover.txt");
        std::fs::write(&staged, b"decrypted secrets").unwrap();
        let outbox = dir.join("file-outbox").join("abc.bin");
        std::fs::write(&outbox, b"ciphertext").unwrap();
        content.insert_file_transfer(incoming_record("t1", u64::MAX));

        content.wipe_cache(None).await.unwrap();

        assert!(
            !staged.exists(),
            "decrypted plaintext must not survive a wipe"
        );
        assert!(
            !outbox.exists(),
            "the encrypted outbox must not survive either"
        );
        assert!(content.file_transfers_snapshot().is_empty());
        assert!(
            !dir.join("file_transfers.json").exists(),
            "the transfer list itself is metadata about who sent you what",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The accept is the whole consent model. A sender must not be able to
    /// follow its own offer with a `ready` and have the file fetched, decrypted
    /// and published while the prompt is still on screen — so every message that
    /// advances a transfer is gated on the state it is allowed to advance from.
    #[test]
    fn a_ready_is_refused_until_the_user_has_accepted() {
        let dir = tmp("file-consent-gate");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        content.insert_file_transfer(incoming_record("t1", u64::MAX));

        assert!(
            !content.transfer_in_state("t1", "incoming", "npub1peer", &["accepted"]),
            "a freshly offered transfer is not yet accepted",
        );

        content.set_file_status("t1", "accepted", "");
        assert!(
            content.transfer_in_state("t1", "incoming", "npub1peer", &["accepted"]),
            "the accept is what opens the gate",
        );
        // ...and only for the peer that made the offer.
        assert!(
            !content.transfer_in_state("t1", "incoming", "npub1other", &["accepted"]),
            "a third Circle member cannot drive someone else's transfer",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The deadline has to measure a stall, not the whole transfer. A large
    /// file that is moving along normally must not be killed for outliving the
    /// window its offer was made in.
    #[test]
    fn progress_pushes_the_deadline_back() {
        let dir = tmp("file-stall-window");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();
        content.insert_file_transfer(incoming_record("moving", 1));

        content.set_file_status("moving", "downloading", "");
        content.sweep_file_transfers();

        assert_eq!(
            content
                .file_transfers_snapshot()
                .iter()
                .find(|r| r.id == "moving")
                .unwrap()
                .status,
            "downloading",
            "a transfer that just made progress must survive the sweep",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `forget` is the UI's dismiss button, so it has to accept every terminal
    /// state — including a cancel — and refuse anything still in flight.
    #[test]
    fn only_finished_transfers_can_be_dismissed() {
        let dir = tmp("file-forget");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();

        content.insert_file_transfer(incoming_record("live", u64::MAX));
        content.forget_file_transfer("live");
        assert_eq!(
            content.file_transfers_snapshot().len(),
            1,
            "an in-flight transfer must not be dismissable"
        );

        for status in ["completed", "denied", "failed", "cancelled"] {
            content.set_file_status("live", status, "");
            content.forget_file_transfer("live");
            assert!(
                content.file_transfers_snapshot().is_empty(),
                "{status} is terminal and must dismiss"
            );
            content.insert_file_transfer(incoming_record("live", u64::MAX));
        }
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
    fn pair_remove_event_drops_peer_from_circle() {
        use nostr::Keys;
        let dir = tmp("unpair");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Content::open(&dir).unwrap();

        // This device's own identity: a pair event is only honoured if it names
        // us, so the test has to have one.
        let us = Keys::generate();
        content.set_device_keys(&us.secret_key().to_bech32().unwrap());
        let us_npub = us.public_key().to_bech32().unwrap();

        let peer = Keys::generate();
        let peer_npub = peer.public_key().to_bech32().unwrap();
        let content = Arc::new(content);
        content.add_to_circle(&peer_npub, "Peer");
        assert_eq!(content.circle_snapshot().len(), 1);

        // The peer signs a PAIR_REMOVE addressed to us; handling it drops them.
        let event = build_pair_event(&peer, KIND_PAIR_REMOVE, &us_npub, "Peer", "")
            .expect("build pair-remove event");
        assert!(content.handle_pair_event(&event));
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

#[cfg(test)]
pub(crate) mod library_kind_tests {
    use super::tests::{tmp, write_bundle};
    use super::*;
    use nostr::nips::nip19::ToBech32;
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
    /// `is_in_library` is what decides whether a manifest arriving from a
    /// peer gets every blob staged and a tile on the grid.
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

        // A Circle peer asking over the wire gets v1 too — the version whose
        // files this phone can hand over.
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
            "the Circle relay offered a version without its files"
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

    /// A mesh peer's relay on loopback: answers every `REQ` — bare or in a
    /// `MESH` envelope, which the real proxy refuses from loopback — with
    /// `events` and `EOSE`. Returns its URL.
    pub(crate) async fn mesh_peer_holding(events: Vec<Event>) -> String {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let events = Arc::new(events);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let events = events.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    while let Some(Ok(Message::Text(txt))) = ws.next().await {
                        let Ok(mut frame) = serde_json::from_str::<serde_json::Value>(&txt) else {
                            continue;
                        };
                        if frame[0] == "MESH" {
                            frame = frame[2].clone();
                        }
                        if frame[0] != "REQ" {
                            continue;
                        }
                        let sub = frame[1].clone();
                        for ev in events.iter() {
                            let out = serde_json::json!(["EVENT", sub, ev]).to_string();
                            let _ = ws.send(Message::Text(out)).await;
                        }
                        let eose = serde_json::json!(["EOSE", sub]).to_string();
                        let _ = ws.send(Message::Text(eose)).await;
                    }
                });
            }
        });
        url
    }

    /// Events passing through on a multi-hop pull for a peer are remembered
    /// here — the profiles kept in the relay, the notes in the cache — through
    /// the real pull path over the peer pool.
    #[tokio::test]
    async fn a_multi_hop_pull_keeps_what_passes_through() {
        let dir = tmp("keep-seen-pull");
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        // A Circle member whose relay (a mock on loopback) holds a profile
        // and a note.
        let peer = nostr::Keys::generate();
        let peer_npub = peer.public_key().to_bech32().unwrap();
        let author = nostr::Keys::generate();
        let profile = EventBuilder::metadata(&nostr::Metadata::new().name("far"))
            .sign_with_keys(&author)
            .unwrap();
        let note = EventBuilder::text_note("passing")
            .sign_with_keys(&author)
            .unwrap();
        let url = mesh_peer_holding(vec![profile.clone(), note.clone()]).await;
        content.add_to_circle(&peer_npub, "peer");
        content.peer_relays().redirect(&peer_npub, &url);

        let pulled = content
            .pull_from_peers(
                vec![serde_json::json!({ "authors": [author.public_key().to_hex()] })],
                crate::mesh_wire::MeshMeta::pull(0, crate::mesh_wire::new_query_id(), 5_000),
                None,
            )
            .await;
        assert_eq!(pulled.len(), 2, "the pull did not reach the peer");

        let relay = content.relay_store().unwrap();
        let cache = content.event_cache();
        for _ in 0..100 {
            if relay.count() > 0 && cache.contains(&note.id.to_bytes()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let kept = relay.query(&[Filter::new()]).await.unwrap();
        assert_eq!(
            kept.iter().map(|e| e.id).collect::<Vec<_>>(),
            [profile.id],
            "only the profile is kept"
        );
        assert!(
            cache.contains(&note.id.to_bytes()),
            "the note was not cached"
        );
        assert!(
            !cache.contains(&profile.id.to_bytes()),
            "the profile is held twice"
        );
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
        Arc::clone(&content).open_site(addr, None).await;

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
