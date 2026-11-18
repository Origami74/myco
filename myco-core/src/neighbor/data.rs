//! The data plane: what `relay`, `outbox` and `resource` reach through an
//! address.
//!
//! Every neighborhood has its own view ([`Scoped`]), so nothing published
//! into one lands in the user's own relay or in another neighborhood. One
//! copy per event, with the neighborhoods it belongs to beside it.
//! Links carry it to peers: a publish is forwarded on every link carrying the
//! neighborhood, a subscription stays open on each of them (and on links
//! that start carrying it later) while the napplet listens, and a blob missing here is
//! asked of them (`GET /nh/{pk}/blob/{sha256}?n=`), verified and kept.
//!
//! ## What a peer is given
//!
//! A neighborhood is addressed as a whole, and what travels follows one rule:
//! a peer gets an event — pushed to it, following its open `REQ`, or in a
//! `REQ`'s backlog — only if this device published it, or it arrived here by
//! push with hops left and this neighborhood passes events on
//! ([`Provenance`]). What a subscription pulls is kept and shown here, never
//! passed on. And never an event of an extension this device does not share
//! there, nor one by an author muted there.
//!
//! ## A store per neighborhood
//!
//! Each neighborhood keeps its events in its own relay store
//! ([`EventStores`]), so NIP-01's replaceable and NIP-09's deletion rules
//! apply within one neighborhood and never across: a newer replaceable event
//! or a deletion kept in one leaves every other neighborhood's copy alone.
//! The same event kept in two neighborhoods is stored twice. Blobs are named
//! by their hash and stay in one shared directory; the scope table says which
//! neighborhood sees which.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use myco_napplet_runtime::nap::neighbor::{Feed, NeighborData, NeighborError, Offer};
use myco_napplet_runtime::NappletIdentity;
use nostr::{Event, Filter};
use nsite_deck::RelayBackend;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::host::{Neighbors, Target};
use super::id::NeighborhoodId;
use super::store::Record;

/// Whether peers may be given an event this device keeps in a
/// neighborhood, and how far it may still travel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Provenance {
    /// Published from this device: a napplet's publish, or an update it
    /// shares.
    pub mine: bool,
    /// Hops it may still be passed on with.
    pub hops: u8,
}

impl Provenance {
    /// Published here, to travel `hops`.
    pub fn mine(hops: u8) -> Self {
        Self { mine: true, hops }
    }

    /// Pushed by a peer with `ttl` hops left, into a neighborhood that
    /// passes events on (`pass_on`) or not.
    pub fn pushed(ttl: u8, pass_on: bool) -> Self {
        Self {
            mine: false,
            hops: if pass_on { ttl } else { 0 },
        }
    }

    /// Pulled through a subscription, or kept for this device alone: shown
    /// here, never passed on.
    pub fn pulled() -> Self {
        Self::default()
    }

    /// Whether a peer may be given it.
    pub fn passes_on(self) -> bool {
        self.mine || self.hops > 0
    }

    fn merge(self, other: Self) -> Self {
        Self {
            mine: self.mine || other.mine,
            hops: self.hops.max(other.hops),
        }
    }
}

/// An event as it is kept in a neighborhood: what the live feed carries.
#[derive(Debug, Clone)]
pub struct Kept {
    pub n: NeighborhoodId,
    pub event: Event,
    pub provenance: Provenance,
    /// The link it was pushed in on, if any: not pushed back there.
    pub from: Option<String>,
    /// Kept before, and now allowed further: news to links, not to this
    /// device's own subscribers.
    pub again: bool,
}

/// Whether [`Scoped::add`] kept an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// New to the neighborhood.
    New,
    /// Already there, and now allowed further than before.
    Upgraded,
    /// Already there, no further than before.
    Already,
    /// The store did not keep it (an older replaceable, a deleted or
    /// ephemeral event): not in the neighborhood either.
    Refused,
}

impl Added {
    /// Whether the event is in the neighborhood now.
    pub fn taken(self) -> bool {
        self != Added::Refused
    }
}

/// How much one neighborhood may keep here. Past it, its oldest events (or
/// blobs) leave its scope.
#[derive(Debug, Clone, Copy)]
pub struct Quota {
    pub events: usize,
    pub bytes: u64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            events: 10_000,
            bytes: 256 * 1024 * 1024,
        }
    }
}

/// Each neighborhood's own event store, opened on first use. One store per
/// neighborhood keeps NIP-01 replaceable and NIP-09 deletion rules inside it.
#[async_trait]
pub trait EventStores: Send + Sync {
    /// Neighborhood `n`'s store.
    fn store(&self, n: NeighborhoodId) -> anyhow::Result<Arc<dyn RelayBackend>>;
    /// Drop every event in `n`'s store whose id is not in `keep`.
    async fn retain(&self, n: NeighborhoodId, keep: HashSet<[u8; 32]>);
    /// Delete `n`'s store and everything in it.
    async fn remove(&self, n: NeighborhoodId);
}

/// [`EventStores`] on disk: one embedded relay store per neighborhood, in
/// `<dir>/<neighborhood id>/`.
pub struct LmdbStores {
    dir: PathBuf,
    open: Mutex<HashMap<NeighborhoodId, Arc<myco_relay::RelayStore>>>,
}

/// The LMDB map reserved per neighborhood: far above its quota, and small
/// enough that many neighborhoods fit a 32-bit process's address space.
#[cfg(target_pointer_width = "64")]
const STORE_MAP_SIZE: usize = 256 * 1024 * 1024;
#[cfg(not(target_pointer_width = "64"))]
const STORE_MAP_SIZE: usize = 32 * 1024 * 1024;

impl LmdbStores {
    /// The stores under `dir`. A single store left there by an earlier build,
    /// shared by every neighborhood, is deleted with the scope table that
    /// pointed into it.
    pub fn open(dir: PathBuf, scopes: &Path) -> Self {
        let shared = dir.join("lmdb");
        if shared.is_dir() {
            tracing::info!(
                "neighborhood store: one store per neighborhood now; the shared one is dropped"
            );
            let _ = std::fs::remove_dir_all(&shared);
            for leftover in ["events.json", "expiring.log"] {
                let _ = std::fs::remove_file(dir.join(leftover));
            }
            let _ = std::fs::remove_file(scopes);
        }
        Self {
            dir,
            open: Mutex::new(HashMap::new()),
        }
    }

    fn relay(&self, n: NeighborhoodId) -> anyhow::Result<Arc<myco_relay::RelayStore>> {
        let mut open = self.open.lock().unwrap();
        if let Some(store) = open.get(&n) {
            return Ok(store.clone());
        }
        let store = Arc::new(
            match myco_relay::RelayStore::open_with_map_size(
                self.dir.join(n.to_hex()),
                STORE_MAP_SIZE,
            ) {
                Ok(store) => store,
                Err(e) => {
                    tracing::warn!(error = %e, "neighborhood store unavailable; kept in memory");
                    myco_relay::RelayStore::in_memory()
                }
            },
        );
        open.insert(n, store.clone());
        Ok(store)
    }
}

#[async_trait]
impl EventStores for LmdbStores {
    fn store(&self, n: NeighborhoodId) -> anyhow::Result<Arc<dyn RelayBackend>> {
        Ok(self.relay(n)?)
    }

    async fn retain(&self, n: NeighborhoodId, keep: HashSet<[u8; 32]>) {
        match self.relay(n) {
            Ok(store) => store.retain_events(&keep).await,
            Err(e) => tracing::warn!(error = %e, "neighborhood store unavailable"),
        }
    }

    async fn remove(&self, n: NeighborhoodId) {
        let store = self.open.lock().unwrap().remove(&n);
        // Closed before its files go, once nothing else holds it.
        drop(store);
        let dir = self.dir.join(n.to_hex());
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await;
    }
}

/// [`EventStores`] in memory, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemStores(Mutex<HashMap<NeighborhoodId, Arc<nsite_deck::testing::MemRelay>>>);

#[cfg(test)]
impl MemStores {
    fn relay(&self, n: NeighborhoodId) -> Arc<nsite_deck::testing::MemRelay> {
        self.0.lock().unwrap().entry(n).or_default().clone()
    }

    /// How many neighborhoods have a store.
    pub fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

#[cfg(test)]
#[async_trait]
impl EventStores for MemStores {
    fn store(&self, n: NeighborhoodId) -> anyhow::Result<Arc<dyn RelayBackend>> {
        Ok(self.relay(n))
    }

    async fn retain(&self, n: NeighborhoodId, keep: HashSet<[u8; 32]>) {
        let old = self.relay(n);
        let kept = nsite_deck::testing::MemRelay::new();
        for e in old.query(&[Filter::new()]).await.unwrap_or_default() {
            if keep.contains(&e.id.to_bytes()) {
                let _ = kept.publish(e).await;
            }
        }
        self.0.lock().unwrap().insert(n, Arc::new(kept));
    }

    async fn remove(&self, n: NeighborhoodId) {
        self.0.lock().unwrap().remove(&n);
    }
}

/// Deletes blobs no neighborhood keeps any more. The Blossom seam has no
/// delete, so this is the runtime's, over the concrete store
/// ([`StorePrune`]); without one, a blob that leaves every scope stays
/// stored but is seen by nobody.
#[async_trait]
pub trait Prune: Send + Sync {
    /// Drop one blob.
    async fn remove_blob(&self, sha256: &str);
}

/// [`Prune`] over the Blossom directory.
pub struct StorePrune {
    pub blobs: Option<Arc<myco_blossom::FsBlobStore>>,
}

#[async_trait]
impl Prune for StorePrune {
    async fn remove_blob(&self, sha256: &str) {
        if let Some(blobs) = self.blobs.clone() {
            let sha = sha256.to_string();
            let _ = tokio::task::spawn_blocking(move || blobs.remove(&sha)).await;
        }
    }
}

/// One key (event id, or `blob:<sha256>`) as kept in one neighborhood.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Held {
    #[serde(default, rename = "m", skip_serializing_if = "is_false")]
    mine: bool,
    #[serde(default, rename = "h", skip_serializing_if = "is_zero")]
    hops: u8,
    /// When it was kept, in order: the oldest are evicted first.
    #[serde(default, rename = "t")]
    seq: u64,
    /// Its size, for a blob in the neighborhood's own store.
    #[serde(default, rename = "b", skip_serializing_if = "is_zero64")]
    bytes: u64,
}

impl Held {
    fn provenance(&self) -> Provenance {
        Provenance {
            mine: self.mine,
            hops: self.hops,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(n: &u8) -> bool {
    *n == 0
}
fn is_zero64(n: &u64) -> bool {
    *n == 0
}

fn is_blob(key: &str) -> bool {
    key.starts_with("blob:")
}

/// What one neighborhood keeps, for its quota.
#[derive(Debug, Clone, Copy, Default)]
struct Usage {
    events: usize,
    bytes: u64,
}

/// The scope table: every key, with the neighborhoods it is kept in.
/// `neighborhood-scopes.json`, format 2. Format 1 (key → neighborhoods)
/// reads as everything pulled: nothing in it is passed on.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Table {
    v: u8,
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    keys: BTreeMap<String, BTreeMap<NeighborhoodId, Held>>,
    #[serde(skip)]
    usage: HashMap<NeighborhoodId, Usage>,
}

impl Table {
    fn new() -> Self {
        Self {
            v: 2,
            ..Self::default()
        }
    }

    fn load(file: &Path) -> Self {
        let Some(value) = std::fs::read(file)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            return Self::new();
        };
        let mut table = if value.get("v").and_then(|v| v.as_u64()) == Some(2) {
            serde_json::from_value::<Table>(value).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "neighborhood scope table unreadable");
                Self::new()
            })
        } else {
            // Format 1: which neighborhoods, nothing about where from.
            let old: BTreeMap<String, BTreeSet<NeighborhoodId>> =
                serde_json::from_value(value).unwrap_or_default();
            let mut table = Self::new();
            for (key, ns) in old {
                for n in ns {
                    table.seq += 1;
                    let held = Held {
                        mine: false,
                        hops: 0,
                        seq: table.seq,
                        bytes: 0,
                    };
                    table.keys.entry(key.clone()).or_default().insert(n, held);
                }
            }
            table
        };
        table.v = 2;
        table.recount();
        table
    }

    fn recount(&mut self) {
        self.usage.clear();
        for (key, ns) in &self.keys {
            for (n, held) in ns {
                let u = self.usage.entry(*n).or_default();
                if is_blob(key) {
                    u.bytes += held.bytes;
                } else {
                    u.events += 1;
                }
            }
        }
    }

    fn get(&self, key: &str, n: NeighborhoodId) -> Option<Held> {
        self.keys.get(key).and_then(|ns| ns.get(&n)).copied()
    }

    /// Keep `key` in `n`, as new.
    fn insert(&mut self, key: &str, n: NeighborhoodId, provenance: Provenance, bytes: u64) {
        self.seq += 1;
        let held = Held {
            mine: provenance.mine,
            hops: provenance.hops,
            seq: self.seq,
            bytes,
        };
        let u = self.usage.entry(n).or_default();
        if is_blob(key) {
            u.bytes += bytes;
        } else {
            u.events += 1;
        }
        self.keys
            .entry(key.to_string())
            .or_default()
            .insert(n, held);
    }

    fn upgrade(&mut self, key: &str, n: NeighborhoodId, provenance: Provenance) {
        if let Some(held) = self.keys.get_mut(key).and_then(|ns| ns.get_mut(&n)) {
            held.mine = provenance.mine;
            held.hops = provenance.hops;
        }
    }

    /// Take `key` out of `n`. Returns whether no neighborhood keeps it now.
    fn remove(&mut self, key: &str, n: NeighborhoodId) -> bool {
        let Some(ns) = self.keys.get_mut(key) else {
            return false;
        };
        let Some(held) = ns.remove(&n) else {
            return false;
        };
        if let Some(u) = self.usage.get_mut(&n) {
            if is_blob(key) {
                u.bytes = u.bytes.saturating_sub(held.bytes);
            } else {
                u.events = u.events.saturating_sub(1);
            }
        }
        if ns.is_empty() {
            self.keys.remove(key);
            true
        } else {
            false
        }
    }

    /// Hold `n` to `quota`: past it, its oldest events (or blobs, for
    /// `blobs`) leave its scope until it is back under nine tenths. Returns
    /// the keys no neighborhood keeps any more.
    fn evict(&mut self, n: NeighborhoodId, quota: Quota, blobs: bool) -> Vec<String> {
        let usage = self.usage.get(&n).copied().unwrap_or_default();
        let over = if blobs {
            usage.bytes > quota.bytes
        } else {
            usage.events > quota.events
        };
        if !over {
            return Vec::new();
        }
        // Others' first, oldest first, then this device's own: a peer
        // pushing thousands of events must not evict what we published
        // (posts, shared update manifests) while its own are left.
        let mut oldest: Vec<(bool, u64, String)> = self
            .keys
            .iter()
            .filter(|(key, _)| is_blob(key) == blobs)
            .filter_map(|(key, ns)| {
                ns.get(&n)
                    .map(|h| (h.provenance().mine, h.seq, key.clone()))
            })
            .collect();
        oldest.sort();
        let mut orphans = Vec::new();
        for (_, _, key) in oldest {
            let usage = self.usage.get(&n).copied().unwrap_or_default();
            let under = if blobs {
                usage.bytes <= quota.bytes / 10 * 9
            } else {
                usage.events <= quota.events / 10 * 9
            };
            if under {
                break;
            }
            if self.remove(&key, n) {
                orphans.push(key);
            }
        }
        orphans
    }
}

/// Neighborhood events: their store, which neighborhood each belongs to and
/// how it got there, and a live feed of new ones.
pub struct Scoped {
    pub stores: Arc<dyn EventStores>,
    pub blobs: Arc<dyn nsite_deck::BlobStore>,
    /// The device's own kept blobs, for files an extension shares without
    /// copying them (see [`super::updates`]). Set once by the runtime.
    pub kept: std::sync::OnceLock<Arc<dyn nsite_deck::BlobStore>>,
    /// Deletes what no neighborhood keeps any more. Set once by the runtime.
    pub prune: std::sync::OnceLock<Arc<dyn Prune>>,
    file: PathBuf,
    table: Arc<Mutex<Table>>,
    quota: Mutex<Quota>,
    /// Held (shared) across a store write and its scope entry, and taken
    /// whole by a prune: a prune never drops an event being kept.
    gate: tokio::sync::RwLock<()>,
    /// The table changed since it was last written.
    dirty: AtomicBool,
    /// A write is scheduled.
    scheduled: AtomicBool,
    /// Neighborhoods some event left: their stores are due a prune.
    prune_events: Mutex<HashSet<NeighborhoodId>>,
    /// Blobs that left every scope, due to be deleted.
    orphan_blobs: Mutex<Vec<String>>,
    /// Neighborhoods left: nothing more is kept in them, so a keep that was
    /// in flight when they were left can't bring their store back. Cleared
    /// when one is joined again ([`Scoped::rejoin`]).
    left: Mutex<HashSet<NeighborhoodId>>,
    /// One table write at a time, so an older one never lands last.
    writing: Arc<Mutex<()>>,
    me: Weak<Scoped>,
    live: broadcast::Sender<Kept>,
}

/// Longest [`Neighbors::ask_links`] waits for the links' answers.
const ASK_LINKS_FOR: std::time::Duration = std::time::Duration::from_secs(10);

/// The scope table, under the data dir.
pub const SCOPES_FILE: &str = "neighborhood-scopes.json";

/// How long the scope table may lag what is kept: written at most this
/// often, off the async workers, and once more when the store closes.
const WRITE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

fn write_atomic(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, file)
}

impl Scoped {
    pub fn open(
        data_dir: &Path,
        stores: Arc<dyn EventStores>,
        blobs: Arc<dyn nsite_deck::BlobStore>,
    ) -> Arc<Self> {
        let file = data_dir.join(SCOPES_FILE);
        let table = Table::load(&file);
        Arc::new_cyclic(|me| Self {
            stores,
            blobs,
            kept: std::sync::OnceLock::new(),
            prune: std::sync::OnceLock::new(),
            file,
            table: Arc::new(Mutex::new(table)),
            quota: Mutex::new(Quota::default()),
            gate: tokio::sync::RwLock::new(()),
            dirty: AtomicBool::new(false),
            scheduled: AtomicBool::new(false),
            prune_events: Mutex::new(HashSet::new()),
            orphan_blobs: Mutex::new(Vec::new()),
            left: Mutex::new(HashSet::new()),
            writing: Arc::new(Mutex::new(())),
            me: me.clone(),
            live: broadcast::channel(256).0,
        })
    }

    /// Set how much each neighborhood may keep.
    #[cfg(test)]
    pub fn set_quota(&self, quota: Quota) {
        *self.quota.lock().unwrap() = quota;
    }

    /// The table changed: write it within [`WRITE_EVERY`].
    fn changed(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        if self.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let me = self.me.clone();
                handle.spawn(async move {
                    tokio::time::sleep(WRITE_EVERY).await;
                    if let Some(scoped) = me.upgrade() {
                        scoped.flush().await;
                    }
                });
            }
            Err(_) => {
                self.scheduled.store(false, Ordering::SeqCst);
                self.write_now();
            }
        }
    }

    /// Prune what left every scope, and write the table if it changed.
    pub async fn flush(&self) {
        self.scheduled.store(false, Ordering::SeqCst);
        self.prune_now().await;
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let (table, writing, file) = (self.table.clone(), self.writing.clone(), self.file.clone());
        let written = tokio::task::spawn_blocking(move || {
            let _one = writing.lock().unwrap();
            let bytes = serde_json::to_vec(&*table.lock().unwrap())?;
            write_atomic(&file, &bytes).map_err(anyhow::Error::from)
        })
        .await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::warn!("could not write the neighborhood scope table");
            self.dirty.store(true, Ordering::SeqCst);
        }
    }

    /// Write the table here and now.
    fn write_now(&self) {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let _one = self.writing.lock().unwrap();
        let written = serde_json::to_vec(&*self.table.lock().unwrap())
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&self.file, &bytes).map_err(anyhow::Error::from));
        if let Err(e) = written {
            tracing::warn!(error = %e, "could not write the neighborhood scope table");
        }
    }

    /// Delete from the stores what left a neighborhood: its events from its
    /// own store, and blobs no neighborhood keeps any more.
    async fn prune_now(&self) {
        let blobs = std::mem::take(&mut *self.orphan_blobs.lock().unwrap());
        let events = std::mem::take(&mut *self.prune_events.lock().unwrap());
        if blobs.is_empty() && events.is_empty() {
            return;
        }
        let _all = self.gate.write().await;
        let (keeps, blobs) = {
            let table = self.table.lock().unwrap();
            let keeps: Vec<(NeighborhoodId, HashSet<[u8; 32]>)> = events
                .into_iter()
                .map(|n| {
                    let keep = table
                        .keys
                        .iter()
                        .filter(|(k, ns)| !is_blob(k) && ns.contains_key(&n))
                        .filter_map(|(k, _)| {
                            let mut id = [0u8; 32];
                            hex::decode_to_slice(k, &mut id).ok().map(|()| id)
                        })
                        .collect();
                    (n, keep)
                })
                .collect();
            // Kept again since: not ours to delete.
            let blobs: Vec<String> = blobs
                .into_iter()
                .filter(|sha| !table.keys.contains_key(&format!("blob:{sha}")))
                .collect();
            (keeps, blobs)
        };
        for (n, keep) in keeps {
            self.stores.retain(n, keep).await;
        }
        if let Some(prune) = self.prune.get().cloned() {
            for sha in blobs {
                prune.remove_blob(&sha).await;
            }
        }
    }

    /// Note blobs that left every scope, for the next prune.
    fn orphaned(&self, keys: Vec<String>) {
        for key in keys {
            if let Some(sha) = key.strip_prefix("blob:") {
                self.orphan_blobs.lock().unwrap().push(sha.to_string());
            }
        }
    }

    /// Keep `event` in neighborhood `n`, as `provenance` allows it to travel.
    /// Once in a neighborhood, an event keeps the furthest it was ever
    /// allowed. `from` is the link that pushed it, if one did.
    pub(super) async fn add(
        &self,
        n: NeighborhoodId,
        event: Event,
        provenance: Provenance,
        from: Option<&str>,
    ) -> anyhow::Result<Added> {
        let _keeping = self.gate.read().await;
        if self.left.lock().unwrap().contains(&n) {
            return Ok(Added::Already);
        }
        let key = event.id.to_hex();
        let before = self.table.lock().unwrap().get(&key, n);
        if let Some(before) = before {
            let merged = before.provenance().merge(provenance);
            if merged == before.provenance() {
                return Ok(Added::Already);
            }
            self.table.lock().unwrap().upgrade(&key, n, merged);
            self.changed();
            let _ = self.live.send(Kept {
                n,
                event,
                provenance: merged,
                from: from.map(str::to_string),
                again: true,
            });
            return Ok(Added::Upgraded);
        }
        let store = self.stores.store(n)?;
        let before_slot = Self::slot_ids(&store, &event).await;
        store.publish(event.clone()).await?;
        // A relay takes a publish and may keep nothing: an older replaceable,
        // a deleted id, an ephemeral kind. Only what it holds is in scope.
        // Replaceable and deleted are judged in `n`'s own store alone.
        let held = !store
            .query(&[Filter::new().id(event.id).limit(1)])
            .await?
            .is_empty();
        if !held {
            return Ok(Added::Refused);
        }
        let quota = *self.quota.lock().unwrap();
        let (orphans, evicted) = {
            let mut table = self.table.lock().unwrap();
            table.insert(&key, n, provenance, 0);
            let before = table.usage.get(&n).map_or(0, |u| u.events);
            let orphans = table.evict(n, quota, false);
            let after = table.usage.get(&n).map_or(0, |u| u.events);
            (orphans, after < before)
        };
        // A newer replaceable put an older one out of `n`'s store: the
        // table lets it go too.
        if !before_slot.is_empty() {
            let now: HashSet<String> = Self::slot_ids(&store, &event).await.into_iter().collect();
            let mut table = self.table.lock().unwrap();
            for old in before_slot.iter().filter(|id| !now.contains(*id)) {
                table.remove(old, n);
            }
        }
        // A NIP-09 deletion took events out of `n`'s store: the table lets
        // them go too, or `has` would go on saying yes and the quota would
        // count them.
        if event.kind == nostr::Kind::EventDeletion {
            let named: Vec<nostr::EventId> = event.tags.event_ids().copied().collect();
            if !named.is_empty() {
                let still: HashSet<String> = store
                    .query(&[Filter::new().ids(named.iter().copied())])
                    .await?
                    .into_iter()
                    .map(|e| e.id.to_hex())
                    .collect();
                let mut table = self.table.lock().unwrap();
                for id in named.iter().map(|id| id.to_hex()) {
                    if !still.contains(&id) {
                        table.remove(&id, n);
                    }
                }
            }
        }
        if evicted {
            self.prune_events.lock().unwrap().insert(n);
        }
        self.orphaned(orphans);
        self.changed();
        let _ = self.live.send(Kept {
            n,
            event,
            provenance,
            from: from.map(str::to_string),
            again: false,
        });
        Ok(Added::New)
    }

    async fn add_blob(&self, id: NeighborhoodId, bytes: &[u8], sha256: &str) -> anyhow::Result<()> {
        let quota = *self.quota.lock().unwrap();
        if bytes.len() as u64 > quota.bytes {
            anyhow::bail!("larger than a neighborhood keeps");
        }
        let _keeping = self.gate.read().await;
        if self.left.lock().unwrap().contains(&id) {
            anyhow::bail!("not in that neighborhood any more");
        }
        self.blobs.put(bytes).await?;
        let key = format!("blob:{sha256}");
        let orphans = {
            let mut table = self.table.lock().unwrap();
            if table.get(&key, id).is_some() {
                return Ok(());
            }
            table.insert(&key, id, Provenance::mine(0), bytes.len() as u64);
            table.evict(id, quota, true)
        };
        self.orphaned(orphans);
        self.changed();
        Ok(())
    }

    fn scopes_blob(&self, id: NeighborhoodId, sha256: &str) -> bool {
        self.table
            .lock()
            .unwrap()
            .get(&format!("blob:{sha256}"), id)
            .is_some()
    }

    async fn blob(&self, id: NeighborhoodId, sha256: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.scopes_blob(id, sha256) {
            return Ok(None);
        }
        if let Some(bytes) = self.blobs.get(sha256).await? {
            return Ok(Some(bytes));
        }
        match self.kept.get() {
            Some(kept) => kept.get(sha256).await,
            None => Ok(None),
        }
    }

    /// Whether a blob in neighborhood `id`'s scope is here, without reading it.
    pub(super) async fn has_blob(&self, id: NeighborhoodId, sha256: &str) -> bool {
        if !self.scopes_blob(id, sha256) {
            return false;
        }
        if self.blobs.has(sha256).await {
            return true;
        }
        match self.kept.get() {
            Some(kept) => kept.has(sha256).await,
            None => false,
        }
    }

    /// Put files already kept on the device into neighborhood `id`'s scope,
    /// without copying them: they are read from [`Scoped::kept`].
    pub(super) fn share_kept(&self, id: NeighborhoodId, sha256s: &[String]) -> anyhow::Result<()> {
        let mut changed = false;
        {
            let mut table = self.table.lock().unwrap();
            for sha in sha256s {
                let key = format!("blob:{sha}");
                if table.get(&key, id).is_none() {
                    table.insert(&key, id, Provenance::mine(0), 0);
                    changed = true;
                }
            }
        }
        if changed {
            self.changed();
        }
        Ok(())
    }

    /// Keep an event this device publishes itself into `id`: given to peers
    /// asking there.
    pub(super) async fn add_shared(&self, id: NeighborhoodId, event: Event) -> anyhow::Result<()> {
        self.add(id, event, Provenance::mine(0), None)
            .await
            .map(|_| ())
    }

    /// Whether `event` is already kept in neighborhood `id`.
    pub fn has(&self, id: NeighborhoodId, event: &Event) -> bool {
        self.provenance(id, event).is_some()
    }

    /// How `event` came to be kept in `id`, if it is.
    pub fn provenance(&self, id: NeighborhoodId, event: &Event) -> Option<Provenance> {
        self.table
            .lock()
            .unwrap()
            .get(&event.id.to_hex(), id)
            .map(|h| h.provenance())
    }

    /// Everything kept in `id` matching `filters`, with how it got there.
    async fn backlog(
        &self,
        id: NeighborhoodId,
        filters: &[Filter],
    ) -> anyhow::Result<Vec<(Event, Provenance)>> {
        let events = self.stores.store(id)?.query(filters).await?;
        let table = self.table.lock().unwrap();
        Ok(events
            .into_iter()
            .filter_map(|e| {
                let p = table.get(&e.id.to_hex(), id)?.provenance();
                Some((e, p))
            })
            .collect())
    }

    /// Take neighborhood `id` out of every scope; what nothing else keeps is
    /// deleted. Its record stays (Nearby forgetting what it held).
    pub(super) async fn forget(&self, id: NeighborhoodId) {
        self.purge(id, false).await;
    }

    /// [`Scoped::forget`] for a neighborhood left: nothing is kept in it
    /// again until it is joined again.
    pub(super) async fn leave(&self, id: NeighborhoodId) {
        self.purge(id, true).await;
    }

    /// Kept in again: `id` was joined (again).
    pub(super) fn rejoin(&self, id: NeighborhoodId) {
        self.left.lock().unwrap().remove(&id);
    }

    async fn purge(&self, id: NeighborhoodId, leaving: bool) {
        // All under the write gate: a keep in flight either lands before
        // and is purged, or after and finds the neighborhood left.
        let orphans = {
            let _all = self.gate.write().await;
            if leaving {
                self.left.lock().unwrap().insert(id);
            }
            let orphans = {
                let mut table = self.table.lock().unwrap();
                let keys: Vec<String> = table
                    .keys
                    .iter()
                    .filter(|(_, ns)| ns.contains_key(&id))
                    .map(|(k, _)| k.clone())
                    .collect();
                let orphans = keys
                    .into_iter()
                    .filter(|k| table.remove(k, id))
                    .collect::<Vec<_>>();
                table.usage.remove(&id);
                orphans
            };
            self.prune_events.lock().unwrap().remove(&id);
            self.stores.remove(id).await;
            orphans
        };
        self.orphaned(orphans);
        self.changed();
        self.prune_now().await;
    }

    /// The ids `n`'s store holds in `event`'s replaceable or addressable
    /// slot, if it has one.
    async fn slot_ids(store: &Arc<dyn RelayBackend>, event: &Event) -> Vec<String> {
        let kind = event.kind;
        if !(kind.is_replaceable() || kind.is_addressable()) {
            return Vec::new();
        }
        let mut filter = Filter::new().author(event.pubkey).kind(kind);
        if kind.is_addressable() {
            filter = filter.identifier(event.tags.identifier().unwrap_or_default());
        }
        store
            .query(&[filter])
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.id.to_hex())
            .collect()
    }

    /// Every event as it is kept, in whichever neighborhood.
    pub(super) fn live(&self) -> broadcast::Receiver<Kept> {
        self.live.subscribe()
    }

    fn feed(&self, id: NeighborhoodId) -> myco_napplet_runtime::seams::PulledEvents {
        let rx = self.live.subscribe();
        Box::pin(futures_util::stream::unfold(rx, move |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(kept) if kept.n == id && !kept.again => return Some((kept.event, rx)),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }
}

impl Drop for Scoped {
    fn drop(&mut self) {
        self.write_now();
    }
}

/// A blob store that keeps nothing: what neighborhood uploads get when their
/// store could not be opened. Uploads then fail rather than land elsewhere.
pub struct NoBlobs;

#[async_trait]
impl nsite_deck::BlobStore for NoBlobs {
    async fn has(&self, _sha256_hex: &str) -> bool {
        false
    }
    async fn get(&self, _sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn put(&self, _bytes: &[u8]) -> anyhow::Result<String> {
        anyhow::bail!("no neighborhood blob store on this device")
    }
    async fn wipe(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Neighbors {
    /// Delete what no neighborhood keeps any more. Set once.
    pub fn set_prune(&self, prune: Arc<dyn Prune>) {
        let _ = self.scoped.prune.set(prune);
    }

    fn target(
        &self,
        napplet: &NappletIdentity,
        address: &str,
    ) -> Result<(Target, Record), NeighborError> {
        let target = self
            .resolve(napplet, address)
            .ok_or(NeighborError::NotFound)?;
        let record = self
            .records()
            .into_iter()
            .find(|r| r.id == target.neighborhood)
            .ok_or(NeighborError::NotFound)?;
        Ok((target, record))
    }

    pub(super) fn record(&self, n: NeighborhoodId) -> Option<Record> {
        self.records().into_iter().find(|r| r.id == n)
    }

    /// Whether `n` is a neighborhood this device carries over links.
    pub fn carries(&self, n: NeighborhoodId) -> bool {
        self.record(n).is_some()
    }

    /// A blob a linked peer asks for in `n`: only one kept in `n`'s scope,
    /// and only where this side offers blob reads.
    pub async fn link_blob(&self, n: NeighborhoodId, sha256: &str) -> Option<Vec<u8>> {
        let record = self.record(n)?;
        if !record.offer.blob_read {
            return None;
        }
        if let Some(bytes) = self.scoped.blob(n, sha256).await.ok().flatten() {
            return Some(bytes);
        }
        if record.policy.shares(super::extensions::NAPPLET_UPDATES) {
            if let Some(held) = self.held.get() {
                return held.file(sha256).await;
            }
        }
        None
    }

    /// Whether a linked peer asking in `n` could get a blob: what
    /// [`Neighbors::link_blob`] would serve, without reading it.
    pub async fn link_has(&self, n: NeighborhoodId, sha256: &str) -> bool {
        let Some(record) = self.record(n) else {
            return false;
        };
        if !record.offer.blob_read {
            return false;
        }
        if self.scoped.has_blob(n, sha256).await {
            return true;
        }
        record.policy.shares(super::extensions::NAPPLET_UPDATES)
            && match self.held.get() {
                Some(held) => held.has_file(sha256).await,
                None => false,
            }
    }

    /// Whether `n` (or only `peer` in it) could supply a blob, without
    /// transferring it: here first, then a `HEAD` to every link carrying it
    /// at once, each bounded. `Some(true)` as soon as one has it,
    /// `Some(false)` when all said no (or there is nobody to ask), `None`
    /// when some could not be asked.
    async fn has_in(&self, n: NeighborhoodId, sha256: &str, peer: Option<&str>) -> Option<bool> {
        if peer.is_none() && self.scoped.has_blob(n, sha256).await {
            return Some(true);
        }
        let Some(links) = &self.links else {
            return Some(false);
        };
        let rest = format!("blob/{sha256}?n={}", n.to_hex());
        let mut asks = futures_util::stream::FuturesUnordered::new();
        for link in links.carrying(n) {
            if peer.is_some_and(|p| p != link.peer) {
                continue;
            }
            let rest = rest.clone();
            asks.push(async move {
                let answer = tokio::time::timeout(
                    HAS_TIMEOUT,
                    super::service::call(&link, reqwest::Method::HEAD, &rest, None),
                )
                .await;
                match answer {
                    Ok(Ok(resp)) if resp.status().is_success() => Some(true),
                    Ok(Ok(resp)) if resp.status().is_client_error() => Some(false),
                    _ => None,
                }
            });
        }
        use futures_util::StreamExt as _;
        let mut unknown = false;
        while let Some(answer) = asks.next().await {
            match answer {
                Some(true) => return Some(true),
                Some(false) => {}
                None => unknown = true,
            }
        }
        if unknown {
            None
        } else {
            Some(false)
        }
    }

    /// Ask the links carrying `n` (or only `peer`'s) for a blob, one after
    /// another, until one has it. What comes back must hash to `sha256`; it
    /// is then kept in `n`'s scope here, so the next ask is local.
    pub(super) async fn blob_from_links(
        &self,
        n: NeighborhoodId,
        sha256: &str,
        peer: Option<&str>,
    ) -> Option<Vec<u8>> {
        let links = self.links.as_ref()?;
        let rest = format!("blob/{sha256}?n={}", n.to_hex());
        for link in links.carrying(n) {
            if peer.is_some_and(|p| p != link.peer) {
                continue;
            }
            let Ok(resp) = super::service::call(&link, reqwest::Method::GET, &rest, None).await
            else {
                continue;
            };
            links.mark_seen(&link.id);
            if !resp.status().is_success() {
                continue;
            }
            let Some(bytes) = read_capped(resp, MAX_LINK_BLOB).await else {
                tracing::debug!(link = %link.id, "peer sent too much, or broke off");
                continue;
            };
            if sha256_hex(&bytes) != sha256 {
                tracing::debug!(link = %link.id, "peer sent a blob that is not the one asked for");
                continue;
            }
            if let Err(e) = self.scoped.add_blob(n, &bytes, sha256).await {
                tracing::debug!(error = %e, "could not keep a fetched blob");
            }
            return Some(bytes);
        }
        None
    }

    /// What this side offers peers in `n`, while it carries `n`.
    pub fn offer(&self, n: NeighborhoodId) -> Option<Offer> {
        self.record(n).map(|r| r.offer)
    }

    /// Everything kept here in `n` matching `filters`, muted authors aside:
    /// this device's own view, pulled events included.
    pub(super) async fn kept(&self, n: NeighborhoodId, filters: &[Filter]) -> Vec<Event> {
        let Some(record) = self.record(n) else {
            return Vec::new();
        };
        let keep = unmuted(&record);
        self.scoped
            .backlog(n, filters)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(e, _)| e)
            .filter(|e| keep(e))
            .collect()
    }

    /// Leave neighborhood `n`'s content behind: out of its scope, and what
    /// no other neighborhood keeps deleted.
    pub(crate) async fn forget_content(&self, n: NeighborhoodId) {
        self.scoped.forget(n).await;
    }

    /// What a linked peer's `REQ` in `n` is answered with: only what may be
    /// passed on ([`Provenance::passes_on`]), shared here and not muted.
    pub async fn link_backlog(&self, n: NeighborhoodId, filters: &[Filter]) -> Vec<Event> {
        let Some(record) = self.record(n) else {
            return Vec::new();
        };
        let keep = unmuted(&record);
        let mut events: Vec<Event> = self
            .scoped
            .backlog(n, filters)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|(e, p)| p.passes_on() && keep(e) && shared(&record, e))
            .map(|(e, _)| e)
            .collect();
        // Napplets held here are answered too, without having been pushed:
        // exposed, not shared, where this device sends napplet updates.
        if record.policy.shares(super::extensions::NAPPLET_UPDATES) {
            if let Some(held) = self.held.get() {
                for event in held.manifests(filters).await {
                    if keep(&event) && !events.iter().any(|e| e.id == event.id) {
                        events.push(event);
                    }
                }
            }
        }
        // A REQ without a limit must not dump the whole store: the newest
        // [`MAX_LINK_BACKLOG`] go.
        if events.len() > MAX_LINK_BACKLOG {
            events.sort_by_key(|e| std::cmp::Reverse(e.created_at));
            events.truncate(MAX_LINK_BACKLOG);
        }
        events
    }

    /// New events kept in any neighborhood, for the link relay's open
    /// subscriptions.
    pub(super) fn live_events(&self) -> broadcast::Receiver<Kept> {
        self.scoped.live()
    }

    /// Whether an event just kept may follow a linked peer's open `REQ`:
    /// it is in `n`, may be passed on, did not come from that link, `link_id`
    /// still carries `n`, its extension is shared and its author not muted
    /// here. Following a `REQ` is a push like any other, which the peer
    /// keeps as pulled.
    pub(super) fn link_may_see(&self, link_id: &str, n: NeighborhoodId, kept: &Kept) -> bool {
        if kept.n != n || !kept.provenance.passes_on() || kept.from.as_deref() == Some(link_id) {
            return false;
        }
        let carried = self
            .links
            .as_ref()
            .is_some_and(|l| l.carrying(n).iter().any(|l| l.id == link_id));
        carried
            && self
                .record(n)
                .is_some_and(|r| unmuted(&r)(&kept.event) && shared(&r, &kept.event))
    }

    /// An event a linked peer sent into `n` with `ttl` hops left: checked,
    /// kept once, and passed on to the other links carrying `n` while hops
    /// and policy allow. `true` when it was taken (new or already held).
    pub async fn receive(&self, n: NeighborhoodId, event: Event, from_link: &str, ttl: u8) -> bool {
        let Some(record) = self.record(n) else {
            return false;
        };
        if event.verify().is_err() || !unmuted(&record)(&event) {
            return false;
        }
        // Kept with no more hops than this side would ever give: a peer's
        // 255 must not live on in provenance for later code to trust.
        let ttl = ttl.min(record.policy.publish_hops.saturating_add(1));
        let provenance = Provenance::pushed(ttl, record.policy.pass_on);
        let added = match self
            .scoped
            .add(n, event.clone(), provenance, Some(from_link))
            .await
        {
            Ok(added) => added,
            Err(_) => return false,
        };
        if matches!(added, Added::New | Added::Upgraded) && provenance.hops > 0 {
            let hops = (ttl - 1).min(record.policy.publish_hops);
            self.forward(n, event, Some(from_link), hops);
        }
        added.taken()
    }

    /// Send `event` into `n` on every link carrying it but `except`, each
    /// peer allowed `ttl` more hops. Spawned: a slow peer holds up nobody.
    /// Nothing of an extension this device does not share in `n`.
    pub(super) fn forward(&self, n: NeighborhoodId, event: Event, except: Option<&str>, ttl: u8) {
        let Some(links) = &self.links else {
            return;
        };
        if !self.record(n).is_some_and(|r| shared(&r, &event)) {
            return;
        }
        for link in links.carrying(n) {
            if Some(link.id.as_str()) == except {
                continue;
            }
            let event = event.clone();
            let links = links.clone();
            tokio::spawn(async move {
                match super::wire::send(&link, n, ttl, &event).await {
                    Ok(true) => links.mark_seen(&link.id),
                    Ok(false) => {
                        links.mark_seen(&link.id);
                        tracing::debug!(link = %link.id, "peer did not take the event")
                    }
                    Err(e) => tracing::debug!(link = %link.id, error = %e, "could not reach peer"),
                }
            });
        }
    }

    /// Ask every link carrying `n` for what matches, all at once, and wait
    /// up to [`ASK_LINKS_FOR`] for the answers. Nothing is kept or passed on:
    /// the caller decides what to do with it (an app's manifest, say).
    pub(super) async fn ask_links(&self, n: NeighborhoodId, filters: &[Filter]) -> Vec<Event> {
        let Some(links) = &self.links else {
            return Vec::new();
        };
        let asks = links.carrying(n).into_iter().map(|link| {
            let links = links.clone();
            async move {
                match super::wire::pull(&link, n, filters).await {
                    Ok(events) => {
                        links.mark_seen(&link.id);
                        // Only what was asked for.
                        events
                            .into_iter()
                            .filter(|e| matches_any(filters, e))
                            .collect::<Vec<_>>()
                    }
                    Err(e) => {
                        tracing::debug!(link = %link.id, error = %e, "could not ask peer");
                        Vec::new()
                    }
                }
            }
        });
        tokio::time::timeout(ASK_LINKS_FOR, futures_util::future::join_all(asks))
            .await
            .map(|answers| answers.into_iter().flatten().collect())
            .unwrap_or_default()
    }

    /// Follow `filters` on every link carrying `n`, for as long as the
    /// returned guard lives: a subscription on each peer stays open, one
    /// opens on a link the moment it starts carrying `n` or comes back in
    /// reach, and closes when the link ends or stops carrying `n`. Each event
    /// is kept in `n` and reaches live subscribers as it arrives. Not passed
    /// on.
    ///
    /// An event already kept, which a peer sends again because it came (back)
    /// in reach, goes to the returned receiver: the follower's own subscriber
    /// sees it again, as a sign it can be fetched from someone now, without
    /// learning who.
    pub(super) fn follow_links(
        &self,
        n: NeighborhoodId,
        filters: Vec<Filter>,
    ) -> Option<(AbortOnDrop, tokio::sync::mpsc::UnboundedReceiver<Event>)> {
        let links = self.links.clone()?;
        let scoped = self.scoped.clone();
        // Bounded: a peer streaming faster than events are kept waits on its
        // socket instead of growing memory.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(FOLLOW_QUEUE);
        let (again_tx, again) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let wanted = filters.clone();
        let keep = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // Only what was asked for is kept.
                if !matches_any(&wanted, &event) {
                    continue;
                }
                if scoped.has(n, &event) {
                    let _ = again_tx.send(event);
                } else {
                    let _ = scoped.add(n, event, Provenance::pulled(), None).await;
                }
            }
        });
        let supervise = tokio::spawn(async move {
            let _keep = AbortOnDrop(keep);
            let mut changes = links.watch();
            // Told when a follow ends (and whether it held a while), so it
            // is tried again without waiting for anything else to change.
            let (ended_tx, mut ended) = tokio::sync::mpsc::unbounded_channel::<(String, bool)>();
            let mut following: BTreeMap<String, AbortOnDrop> = BTreeMap::new();
            let mut failures: BTreeMap<String, u32> = BTreeMap::new();
            loop {
                let carrying = links.carrying(n);
                following.retain(|id, task| {
                    !task.0.is_finished() && carrying.iter().any(|l| &l.id == id)
                });
                failures.retain(|id, _| carrying.iter().any(|l| &l.id == id));
                for link in carrying {
                    if following.contains_key(&link.id) || !links.reachable(&link.id) {
                        continue;
                    }
                    let id = link.id.clone();
                    let fails = failures.entry(id.clone()).or_default();
                    let task = tokio::spawn(follow_link(
                        links.clone(),
                        link,
                        n,
                        filters.clone(),
                        tx.clone(),
                        retry_after(*fails),
                        ended_tx.clone(),
                    ));
                    *fails = fails.saturating_add(1);
                    following.insert(id, AbortOnDrop(task));
                }
                // A link that drops out is retried when it is back in reach,
                // one that ended after its wait, and a quiet one now and then
                // in case it never said.
                tokio::select! {
                    _ = changes.changed() => {}
                    Some((id, held)) = ended.recv() => {
                        if held {
                            failures.remove(&id);
                        }
                    }
                    _ = tokio::time::sleep(RETRY_FOLLOW) => {}
                }
            }
        });
        Some((AbortOnDrop(supervise), again))
    }
}

/// The most events one linked peer's REQ is answered with.
pub const MAX_LINK_BACKLOG: usize = 1000;

/// Events a follow holds between the links' sockets and the keeper.
const FOLLOW_QUEUE: usize = 256;

/// How long a follow that ended `failures` times in a row waits before the
/// next try: soon at first — a link just set up may not be finished on the
/// peer's side yet — then longer, up to [`RETRY_FOLLOW`].
fn retry_after(failures: u32) -> std::time::Duration {
    std::time::Duration::from_millis(250)
        .saturating_mul(1u32 << failures.min(10))
        .min(RETRY_FOLLOW)
}

/// Follow one link until it fails or ends, wait `wait` unless it held a
/// while, then say so on `ended`: the supervisor in
/// [`Neighbors::follow_links`] decides whether to try again.
async fn follow_link(
    links: Arc<super::link::Links>,
    link: super::link::Link,
    n: NeighborhoodId,
    filters: Vec<Filter>,
    tx: tokio::sync::mpsc::Sender<Event>,
    wait: std::time::Duration,
    ended: tokio::sync::mpsc::UnboundedSender<(String, bool)>,
) {
    let id = link.id.clone();
    let seen = links.clone();
    let started = std::time::Instant::now();
    let result = super::wire::follow(&link, n, &filters, tx, || {
        tracing::info!(link = %id, "following peer");
        seen.mark_seen(&id)
    })
    .await;
    match result {
        Ok(()) => tracing::info!(link = %link.id, "stopped following peer"),
        Err(e) => tracing::info!(link = %link.id, error = %e, "could not follow peer"),
    }
    let held = started.elapsed() >= RETRY_FOLLOW;
    if !held {
        tokio::time::sleep(wait).await;
    }
    let _ = ended.send((link.id, held));
}

/// How long before a quiet follow is looked at again, and the longest an
/// ended one waits to be tried again.
const RETRY_FOLLOW: std::time::Duration = std::time::Duration::from_secs(15);

/// Aborts its task when dropped.
pub(super) struct AbortOnDrop(pub(super) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// How long a peer may take to say whether it has a blob.
const HAS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The largest blob taken from a link.
const MAX_LINK_BLOB: u64 = 64 * 1024 * 1024;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A response body, read until `cap` (or the length it announced, if less)
/// and no further: `None` past it, or when it broke off.
async fn read_capped(mut resp: reqwest::Response, cap: u64) -> Option<Vec<u8>> {
    let cap = resp.content_length().unwrap_or(cap);
    if cap > MAX_LINK_BLOB {
        return None;
    }
    let mut body = Vec::with_capacity(cap.min(1 << 20) as usize);
    while let Some(chunk) = resp.chunk().await.ok()? {
        if (body.len() + chunk.len()) as u64 > cap {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

/// Drop events by muted authors.
fn unmuted(record: &Record) -> impl Fn(&Event) -> bool + Send + Sync + 'static {
    let muted = record.muted.clone();
    move |e: &Event| !muted.contains(&e.pubkey.to_hex())
}

/// Whether `event` may go to peers in `record`'s neighborhood as far as the
/// share switches go: anything but an update of an extension switched off
/// there.
fn shared(record: &Record, event: &Event) -> bool {
    super::updates::extension_for(event.kind.as_u16()).is_none_or(|ext| record.policy.shares(ext))
}

fn matches_any(filters: &[Filter], event: &Event) -> bool {
    let any = nostr::filter::MatchEventOptions::new();
    filters.iter().any(|f| f.match_event(event, any))
}

fn failed(e: anyhow::Error) -> NeighborError {
    tracing::warn!(error = %e, "neighborhood carriage failed");
    NeighborError::Denied
}

#[async_trait]
impl NeighborData for Neighbors {
    async fn publish(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        event: Event,
    ) -> Result<(), NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        match target.peer {
            // To that one peer, with no hops to pass it on.
            Some(peer) => {
                let link = self
                    .links
                    .as_ref()
                    .and_then(|l| l.by_peer(&peer))
                    .filter(|l| l.neighborhoods.contains(&record.id))
                    .ok_or(NeighborError::NotFound)?;
                // Kept here for this device; meant for that peer alone, so
                // not given to anyone else.
                let added = self
                    .scoped
                    .add(record.id, event.clone(), Provenance::pulled(), None)
                    .await
                    .map_err(failed)?;
                if !added.taken() {
                    return Err(NeighborError::Denied);
                }
                let n = record.id;
                tokio::spawn(async move {
                    if let Err(e) = super::wire::send(&link, n, 0, &event).await {
                        tracing::debug!(error = %e, "could not reach the peer");
                    }
                });
                Ok(())
            }
            None => {
                let hops = record.policy.publish_hops;
                let added = self
                    .scoped
                    .add(record.id, event.clone(), Provenance::mine(hops), None)
                    .await
                    .map_err(failed)?;
                if !added.taken() {
                    return Err(NeighborError::Denied);
                }
                if hops > 0 {
                    self.forward(record.id, event, None, hops - 1);
                }
                Ok(())
            }
        }
    }

    async fn held(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        filters: Vec<Filter>,
    ) -> Result<Vec<Event>, NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        if target.peer.is_some() {
            return Err(NeighborError::Unsupported);
        }
        let keep = unmuted(&record);
        Ok(self
            .scoped
            .backlog(record.id, &filters)
            .await
            .map_err(failed)?
            .into_iter()
            .map(|(e, _)| e)
            .filter(|e| keep(e))
            .collect())
    }

    async fn subscribe(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        filters: Vec<Filter>,
    ) -> Result<Feed, NeighborError> {
        use futures_util::StreamExt as _;
        let (target, record) = self.target(napplet, address)?;
        let keep = unmuted(&record);
        match target.peer {
            None => {
                let backlog = self
                    .scoped
                    .backlog(record.id, &filters)
                    .await
                    .map_err(failed)?;
                // Subscribed before peers are asked, so nothing they bring
                // is missed.
                let live = self.scoped.feed(record.id);
                // Peers followed for as long as the napplet listens.
                let (following, again) = match (record.policy.backlog_hops > 0)
                    .then(|| self.follow_links(record.id, filters.clone()))
                    .flatten()
                {
                    Some((guard, again)) => (Some(guard), Some(again)),
                    None => (None, None),
                };
                // What a peer coming in reach sends again, next to what is new.
                let again = futures_util::stream::unfold(again, |rx| async move {
                    let mut rx = rx?;
                    let event = rx.recv().await?;
                    Some((event, Some(rx)))
                });
                let live = futures_util::stream::select(live, again);
                Ok(Feed {
                    backlog: backlog
                        .into_iter()
                        .map(|(e, _)| e)
                        .filter(|e| keep(e))
                        .collect(),
                    live: Box::pin(live.filter(move |e| {
                        let _following = &following;
                        std::future::ready(keep(e))
                    })),
                })
            }
            // Reading one peer's view is not supported yet.
            Some(_) => Err(NeighborError::Unsupported),
        }
    }

    async fn blob(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        if target.peer.is_none() {
            if let Some(bytes) = self.scoped.blob(record.id, sha256).await.map_err(failed)? {
                return Ok(Some(bytes));
            }
        }
        Ok(self
            .blob_from_links(record.id, sha256, target.peer.as_deref())
            .await)
    }

    async fn has(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        sha256: &str,
    ) -> Result<Option<bool>, NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        Ok(self.has_in(record.id, sha256, target.peer.as_deref()).await)
    }

    /// Kept here in the neighborhood's scope.
    async fn upload(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        bytes: Vec<u8>,
        sha256: &str,
    ) -> Result<(), NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        if target.peer.is_some() {
            return Err(NeighborError::NotFound);
        }
        // `blob_hops` pushes nothing yet: links do not forward blobs, so an
        // upload stays here until a peer asks for it.
        self.scoped
            .add_blob(record.id, &bytes, sha256)
            .await
            .map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::{Ask, Asker, PeerDirectory, PeerView, Seams};
    use super::*;
    use myco_napplet_runtime::nap::neighbor::{CreateRequest, NeighborHost};

    struct Yes;
    #[async_trait]
    impl Asker for Yes {
        async fn confirm(&self, _: Ask) -> bool {
            true
        }
    }

    /// Bob carries every neighborhood.
    struct Bob;
    impl PeerDirectory for Bob {
        fn peers(&self, _: &Record) -> Vec<PeerView> {
            vec![PeerView {
                key: "npub1bob".into(),
                name: None,
                reachable: true,
                link: None,
            }]
        }
    }

    fn open() -> Neighbors {
        let d = std::env::temp_dir().join(format!("myco-nb-data-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        let n = Neighbors::open(
            &d,
            Seams {
                peers: Arc::new(Bob),
                asker: Arc::new(Yes),
                scoped_stores: Arc::new(crate::neighbor::data::MemStores::default()),
                scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
                links: None,
            },
        );
        n.set_nearby(false);
        n
    }

    fn app() -> NappletIdentity {
        NappletIdentity::new("chat", "hash")
    }

    fn note(text: &str) -> Event {
        nostr::EventBuilder::text_note(text)
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap()
    }

    fn kind1() -> Vec<Filter> {
        vec![Filter::new().kind(nostr::Kind::TextNote)]
    }

    /// A new neighborhood's address.
    async fn create(n: &Neighbors, name: &str) -> String {
        n.create(
            &app(),
            CreateRequest {
                name: name.into(),
                auto_accept: false,
            },
        )
        .await
        .unwrap()
        .address
        .unwrap()
    }

    async fn texts(n: &Neighbors, address: &str) -> Vec<String> {
        n.subscribe(&app(), address, kind1())
            .await
            .unwrap()
            .backlog
            .into_iter()
            .map(|e| e.content)
            .collect()
    }

    #[tokio::test]
    async fn two_neighborhoods_do_not_see_each_other() {
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        n.publish(&app(), &club, note("club")).await.unwrap();
        n.publish(&app(), &other, note("other")).await.unwrap();
        assert_eq!(texts(&n, &club).await, ["club"]);
        assert_eq!(texts(&n, &other).await, ["other"]);
    }

    #[tokio::test]
    async fn new_events_reach_a_live_subscription_in_their_scope_only() {
        use futures_util::StreamExt as _;
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        let mut feed = n.subscribe(&app(), &club, kind1()).await.unwrap();
        n.publish(&app(), &other, note("elsewhere")).await.unwrap();
        n.publish(&app(), &club, note("live")).await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), feed.live.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.content, "live");
    }

    #[tokio::test]
    async fn muted_authors_are_dropped() {
        let n = open();
        let club = create(&n, "Club").await;
        let loud = note("loud");
        n.publish(&app(), &club, loud.clone()).await.unwrap();
        let id = n.list(&app()).await.remove(0).id;
        n.mute(&app(), &id, loud.pubkey, true).await.unwrap();
        assert!(texts(&n, &club).await.is_empty());
    }

    #[tokio::test]
    async fn a_peer_address_asks_only_that_peer() {
        let n = open();
        create(&n, "Club").await;
        let id = n.list(&app()).await.remove(0).id;
        let bob = n.peers(&app(), &id).await.unwrap().remove(0);
        let addr = bob.address.as_deref().unwrap();
        assert_eq!(
            n.subscribe(&app(), addr, kind1()).await.err(),
            Some(NeighborError::Unsupported)
        );
        // No link to Bob: nobody to ask for a blob, nobody to send to.
        assert_eq!(n.blob(&app(), addr, "ab").await.unwrap(), None);
        assert_eq!(
            n.publish(&app(), addr, note("psst")).await.unwrap_err(),
            NeighborError::NotFound
        );
    }

    #[tokio::test]
    async fn an_upload_is_kept_in_its_scope_only() {
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        let bytes = b"club picture".to_vec();
        let sha = nsite_deck::sha256_hex(&bytes);
        n.upload(&app(), &club, bytes.clone(), &sha).await.unwrap();
        assert_eq!(n.blob(&app(), &club, &sha).await.unwrap(), Some(bytes));
        assert_eq!(n.blob(&app(), &other, &sha).await.unwrap(), None);
    }

    fn open_at(d: &Path) -> Neighbors {
        let n = Neighbors::open(
            d,
            Seams {
                peers: Arc::new(Bob),
                asker: Arc::new(Yes),
                scoped_stores: Arc::new(crate::neighbor::data::MemStores::default()),
                scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
                links: None,
            },
        );
        n.set_nearby(false);
        n
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("myco-nb-data-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn how_each_event_got_here_is_kept_and_survives_a_restart() {
        let d = dir();
        let n = open_at(&d);
        let club = create(&n, "Club").await;
        let id = n.records()[0].id;
        let mine = note("mine");
        n.publish(&app(), &club, mine.clone()).await.unwrap();
        let pushed = note("pushed");
        n.scoped
            .add(id, pushed.clone(), Provenance::pushed(2, true), Some("l"))
            .await
            .unwrap();
        let pulled = note("pulled");
        n.scoped
            .add(id, pulled.clone(), Provenance::pulled(), None)
            .await
            .unwrap();
        assert_eq!(n.scoped.provenance(id, &mine), Some(Provenance::mine(3)));
        assert_eq!(n.scoped.provenance(id, &pulled), Some(Provenance::pulled()));
        // Pushed later with hops: it may travel now.
        let again = n
            .scoped
            .add(id, pulled.clone(), Provenance::pushed(1, true), None)
            .await
            .unwrap();
        assert_eq!(again, Added::Upgraded);
        n.scoped.flush().await;
        let stores = n.scoped.stores.clone();
        drop(n);

        let blobs: Arc<dyn nsite_deck::BlobStore> = Arc::new(nsite_deck::testing::MemBlobs::new());
        let back = Scoped::open(&d, stores, blobs);
        assert_eq!(back.provenance(id, &mine), Some(Provenance::mine(3)));
        assert_eq!(
            back.provenance(id, &pushed),
            Some(Provenance::pushed(2, true))
        );
        assert_eq!(
            back.provenance(id, &pulled),
            Some(Provenance::pushed(1, true))
        );
    }

    #[test]
    fn a_scope_table_from_before_provenance_reads_as_nothing_passed_on() {
        let d = dir();
        let id = NeighborhoodId::random();
        let e = note("old");
        let old = serde_json::json!({ e.id.to_hex(): [id], "blob:ab": [id] });
        std::fs::write(d.join(SCOPES_FILE), old.to_string()).unwrap();
        let scoped = Scoped::open(
            &d,
            Arc::new(MemStores::default()),
            Arc::new(nsite_deck::testing::MemBlobs::new()),
        );
        assert_eq!(scoped.provenance(id, &e), Some(Provenance::pulled()));
        assert!(scoped.scopes_blob(id, "ab"));
    }

    /// A peer pushing past the quota evicts its own kind first: what this
    /// device published stays, however old.
    #[tokio::test]
    async fn eviction_takes_others_events_before_ours() {
        let n = open();
        let club = create(&n, "Club").await;
        let id = n.records()[0].id;
        n.scoped.set_quota(Quota {
            events: 10,
            bytes: 100,
        });
        n.publish(&app(), &club, note("ours, oldest"))
            .await
            .unwrap();
        for i in 0..20 {
            n.receive(id, note(&format!("theirs {i}")), "link-x", 1)
                .await;
        }
        let left = texts(&n, &club).await;
        assert!(left.len() <= 10, "{left:?}");
        assert!(left.iter().any(|t| t == "ours, oldest"), "{left:?}");
    }

    #[tokio::test]
    async fn a_neighborhood_past_its_quota_drops_its_oldest() {
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        n.scoped.set_quota(Quota {
            events: 10,
            bytes: 100,
        });
        n.publish(&app(), &other, note("elsewhere")).await.unwrap();
        for i in 0..12 {
            n.publish(&app(), &club, note(&format!("post {i}")))
                .await
                .unwrap();
        }
        let left = texts(&n, &club).await;
        assert!(left.len() <= 10, "{left:?}");
        assert!(!left.iter().any(|t| t == "post 0"), "the oldest went");
        assert!(left.iter().any(|t| t == "post 11"), "the newest stayed");
        assert_eq!(texts(&n, &other).await, ["elsewhere"], "others untouched");

        for i in 0..3u8 {
            let bytes = vec![i; 40];
            let sha = nsite_deck::sha256_hex(&bytes);
            n.upload(&app(), &club, bytes, &sha).await.unwrap();
        }
        let first = nsite_deck::sha256_hex(&[0u8; 40]);
        assert_eq!(n.blob(&app(), &club, &first).await.unwrap(), None);
        let big = vec![9u8; 101];
        let sha = nsite_deck::sha256_hex(&big);
        assert!(n.upload(&app(), &club, big, &sha).await.is_err());
    }

    /// Records the blobs it was told to delete.
    #[derive(Default)]
    struct Pruned(Mutex<Vec<String>>);
    #[async_trait]
    impl Prune for Pruned {
        async fn remove_blob(&self, sha256: &str) {
            self.0.lock().unwrap().push(sha256.to_string());
        }
    }

    #[tokio::test]
    async fn forgetting_a_neighborhood_deletes_what_only_it_kept() {
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        let pruned = Arc::new(Pruned::default());
        n.scoped.prune.set(pruned.clone()).ok().unwrap();
        let both = note("both");
        n.publish(&app(), &club, both.clone()).await.unwrap();
        n.publish(&app(), &other, both.clone()).await.unwrap();
        n.publish(&app(), &club, note("club only")).await.unwrap();
        let bytes = b"club picture".to_vec();
        let sha = nsite_deck::sha256_hex(&bytes);
        n.upload(&app(), &club, bytes, &sha).await.unwrap();

        let id = n.records()[0].id;
        n.forget_content(id).await;
        assert!(texts(&n, &club).await.is_empty());
        assert_eq!(texts(&n, &other).await, ["both"]);
        assert_eq!(*pruned.0.lock().unwrap(), [sha]);
        // Its store went with it: keeping again starts from nothing.
        let fresh = n.scoped.stores.store(id).unwrap();
        assert!(fresh.query(&[Filter::new()]).await.unwrap().is_empty());
    }

    /// A keep in flight when a neighborhood is left can't bring it back.
    #[tokio::test]
    async fn nothing_is_kept_in_a_neighborhood_once_left() {
        let n = open();
        create(&n, "Club").await;
        let id = n.records()[0].id;
        n.scoped.leave(id).await;
        let late = note("late");
        n.scoped
            .add(id, late.clone(), Provenance::pulled(), None)
            .await
            .unwrap();
        assert!(!n.scoped.has(id, &late));
        n.scoped.rejoin(id);
        n.scoped
            .add(id, late.clone(), Provenance::pulled(), None)
            .await
            .unwrap();
        assert!(n.scoped.has(id, &late), "joined again: kept again");
    }

    /// The shell's Leave arrives on the JNI thread, outside any runtime:
    /// the content still goes, on the runtime the host was given.
    #[test]
    fn leaving_from_outside_a_runtime_deletes_the_content() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let n = open();
        let (club, kept) = rt.block_on(async {
            let club = create(&n, "Club").await;
            let kept = note("club only");
            n.publish(&app(), &club, kept.clone()).await.unwrap();
            (club, kept)
        });
        let id = n.records()[0].id;
        assert!(n.scoped.has(id, &kept));
        n.set_runtime(rt.handle().clone());
        assert!(tokio::runtime::Handle::try_current().is_err());
        assert!(n.forget(id));
        rt.block_on(async {
            for _ in 0..200 {
                if !n.scoped.has(id, &kept) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("{club}: content kept after leaving");
        });
    }

    #[tokio::test]
    async fn replaceable_and_deleted_stay_inside_their_neighborhood() {
        let d = dir();
        let stores = Arc::new(LmdbStores::open(
            d.join("neighborhood-relay"),
            &d.join(SCOPES_FILE),
        ));
        let scoped = Scoped::open(&d, stores, Arc::new(nsite_deck::testing::MemBlobs::new()));
        let (club, home) = (NeighborhoodId::random(), NeighborhoodId::random());
        let keys = nostr::Keys::generate();
        let list = |secs: u64| {
            nostr::EventBuilder::new(nostr::Kind::from(10002u16), "")
                .custom_created_at(nostr::Timestamp::from(secs))
                .sign_with_keys(&keys)
                .unwrap()
        };
        let keep = |n, e: Event| {
            let scoped = scoped.clone();
            async move { scoped.add(n, e, Provenance::mine(0), None).await.unwrap() }
        };
        let ids = |kept: Vec<(Event, Provenance)>| {
            kept.into_iter().map(|(e, _)| e.id).collect::<Vec<_>>()
        };

        // A newer list in Home leaves Club's older one alone.
        let (older, newer) = (list(1_000), list(2_000));
        assert_eq!(keep(club, older.clone()).await, Added::New);
        assert_eq!(keep(home, newer.clone()).await, Added::New);
        let lists = [Filter::new().kind(nostr::Kind::from(10002u16))];
        assert_eq!(ids(scoped.backlog(club, &lists).await.unwrap()), [older.id]);
        assert_eq!(ids(scoped.backlog(home, &lists).await.unwrap()), [newer.id]);
        // Within one neighborhood the newer still replaces the older.
        assert_eq!(keep(club, newer.clone()).await, Added::New);
        assert_eq!(ids(scoped.backlog(club, &lists).await.unwrap()), [newer.id]);
        assert!(
            scoped.provenance(club, &older).is_none(),
            "the replaced one left the scope"
        );

        // An author's deletion kept in Home leaves Club's copy alone.
        let hello = nostr::EventBuilder::text_note("hello")
            .sign_with_keys(&keys)
            .unwrap();
        keep(club, hello.clone()).await;
        keep(home, hello.clone()).await;
        let delete = nostr::EventBuilder::delete(
            nostr::nips::nip09::EventDeletionRequest::new().id(hello.id),
        )
        .sign_with_keys(&keys)
        .unwrap();
        keep(home, delete).await;
        let notes = [Filter::new().id(hello.id)];
        assert_eq!(scoped.backlog(club, &notes).await.unwrap().len(), 1);
        assert!(scoped.backlog(home, &notes).await.unwrap().is_empty());
        // And the table agrees: Home no longer has it, Club still does.
        assert!(!scoped.has(home, &hello), "no phantom left in Home");
        assert!(scoped.has(club, &hello));
    }

    #[tokio::test]
    async fn what_the_store_does_not_keep_is_not_in_the_neighborhood() {
        let n = open();
        let club = create(&n, "Club").await;
        let keys = nostr::Keys::generate();
        let at = |secs: u64| {
            nostr::EventBuilder::new(nostr::Kind::from(10002u16), "")
                .custom_created_at(nostr::Timestamp::from(secs))
                .sign_with_keys(&keys)
                .unwrap()
        };
        n.publish(&app(), &club, at(2_000)).await.unwrap();
        let older = at(1_000);
        assert_eq!(
            n.publish(&app(), &club, older.clone()).await.unwrap_err(),
            NeighborError::Denied
        );
        assert!(!n.scoped.has(n.records()[0].id, &older));
    }

    #[tokio::test]
    async fn another_napplets_address_is_refused() {
        let n = open();
        let club = create(&n, "Club").await;
        let mut other = app();
        other.d_tag = "map".into();
        assert_eq!(
            n.publish(&other, &club, note("x")).await.unwrap_err(),
            NeighborError::NotFound
        );
    }
}
