//! The event cache: a second LMDB (the same `nostr-lmdb` store the local
//! relay uses, so NIP-01 filters are index walks here too) behind a
//! segmented-LRU index that holds it to a byte budget.
//!
//! **The budget is disk, estimated.** An event costs far more on disk than
//! its JSON: `nostr-lmdb` writes several index keys per event, and pads every
//! single-letter tag value to a fixed width in three more. [`disk_cost`] is
//! a fit to measured store growth, so a 500 MB budget is roughly 500 MB of
//! database. Should the store still run out of room first, the cache evicts
//! further and retries rather than stop accepting events.
//!
//! **Writes are serialised.** Replaceable slots and deletions make one save
//! drop other events; the index follows that by checking what the save could
//! have displaced, which is only sound while nothing else writes in between.
//! A batch is saved concurrently, though, so the store commits it in one go.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use nostr::{Event, EventId, Filter, JsonUtil, Kind};
use nostr_database::{DatabaseEventStatus, NostrDatabase, SaveEventStatus};
use nostr_lmdb::NostrLMDB;
use nsite_deck::seams::RelayBackend;

use crate::slru::{write_snapshot, Slru};
use crate::CacheStats;

/// The index snapshot, next to the LMDB directory.
const SNAPSHOT: &str = "index.bin";

/// Left by "Clear cache": the next open deletes the database files, which is
/// the only way to hand LMDB's high-water mark back to the disk (an open
/// environment is never unmapped in-process).
const COMPACT_MARKER: &str = "compact-on-open";

/// Ids per delete filter / lookup, so one filter never carries thousands.
const CHUNK: usize = 256;

/// Per-event disk cost fit to measured `nostr-lmdb` growth: a fixed cost for
/// the id/time/author/kind index keys, twice the JSON (the event itself plus
/// B-tree slack), and a fixed cost per indexed (single-letter) tag.
const BASE_COST: u64 = 1_536;
const TAG_COST: u64 = 2_432;

/// The event's estimated share of the database, in bytes.
pub fn disk_cost(event: &Event) -> u64 {
    let indexed_tags = event
        .tags
        .iter()
        .filter(|t| {
            let s = t.as_slice();
            s.len() >= 2 && s[0].len() == 1
        })
        .count() as u64;
    BASE_COST + 2 * event.as_json().len() as u64 + TAG_COST * indexed_tags
}

/// The largest budget accepted; anything above is treated as this.
pub const MAX_BUDGET: u64 = 64 * 1024 * 1024 * 1024;

/// A bounded, evicting event store. See the module docs.
pub struct EventCache {
    db: NostrLMDB,
    /// The LMDB map size this cache was opened with. It cannot change while
    /// the process runs, so the budget in force never exceeds half of it: a
    /// map that fills cannot even delete (LMDB copies pages on write), so the
    /// cache must evict well before that point.
    map_size: u64,
    dir: PathBuf,
    index: Mutex<Slru>,
    /// Held across every write that changes what the store holds.
    writes: tokio::sync::Mutex<()>,
    scratch: bool,
}

/// What one concurrent save of a batch came to.
struct Saved {
    ids: HashSet<EventId>,
    /// Some save failed because the LMDB map is full.
    map_full: bool,
}

/// Whether a save error may be LMDB running out of map (`MDB_MAP_FULL`).
///
/// `nostr-lmdb` saves in batched write transactions, and when one fails it
/// reports every operation in it — the one that failed included — as a
/// generic "Batched transaction failed"; a full map most often surfaces at
/// commit, the same way. So that error counts too.
fn is_map_full(error: &impl std::fmt::Display) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("map_full")
        || text.contains("mapfull")
        || text.contains("map full")
        || text.contains("batched transaction failed")
}

impl EventCache {
    /// Open the cache under `dir` with a budget of `limit` bytes.
    ///
    /// Nothing is read but the directory, so opening stays cheap. Call
    /// [`EventCache::startup`] once from a background task: it reads the index
    /// snapshot and repairs it against the store.
    pub fn open(dir: impl AsRef<Path>, limit: u64) -> anyhow::Result<Self> {
        Self::open_with_map(dir, limit, map_size_for(limit))
    }

    /// [`EventCache::open`] with an explicit LMDB map size — production passes
    /// [`map_size_for`]; tests shrink it to make the store fill first.
    fn open_with_map(dir: impl AsRef<Path>, limit: u64, map_size: usize) -> anyhow::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let lmdb_dir = dir.join("lmdb");
        if dir.join(COMPACT_MARKER).exists() {
            let _ = std::fs::remove_dir_all(&lmdb_dir);
            let _ = std::fs::remove_file(dir.join(SNAPSHOT));
            let _ = std::fs::remove_file(dir.join(COMPACT_MARKER));
        }
        std::fs::create_dir_all(&lmdb_dir)?;
        let db = NostrLMDB::builder(&lmdb_dir)
            .map_size(map_size)
            .build()
            .map_err(|e| anyhow::anyhow!("open event cache at {}: {e}", lmdb_dir.display()))?;
        Ok(Self {
            db,
            dir,
            map_size: map_size as u64,
            index: Mutex::new(Slru::new(budget_within(limit, map_size as u64))),
            writes: tokio::sync::Mutex::new(()),
            scratch: false,
        })
    }

    /// A throwaway cache under the temp dir, removed on drop — for tests.
    pub fn scratch(limit: u64) -> Self {
        let mut cache =
            Self::open(crate::scratch_dir("events"), limit).expect("open scratch cache");
        cache.scratch = true;
        cache
    }

    pub fn stats(&self) -> CacheStats {
        let index = self.index.lock().unwrap();
        CacheStats {
            count: index.len() as u64,
            bytes: index.bytes(),
            limit: index.limit(),
        }
    }

    /// Change the budget, evicting down to it at once.
    /// Change the budget, evicting down to it at once. A budget beyond what
    /// the map opened at launch can hold is held to half the map until the
    /// next launch, which sizes the map for it.
    pub async fn set_limit(&self, limit: u64) {
        let limit = budget_within(limit, self.map_size);
        self.index.lock().unwrap().set_limit(limit);
        self.evict().await;
    }

    pub fn contains(&self, id: &[u8; 32]) -> bool {
        self.index.lock().unwrap().contains(id)
    }

    /// Cache `events`. Returns how many were new here.
    ///
    /// Events must already be verified. Ephemeral and already-expired ones are
    /// skipped; an id already cached is left where it is (being seen again is
    /// not an access). NIP-01 replaceable slots and NIP-09 deletions are the
    /// store's, as in the local relay; the index follows whatever the store
    /// dropped.
    pub async fn insert(&self, events: &[Event]) -> usize {
        let now = now_secs();
        let writing = self.writes.lock().await;
        let batch: Vec<&Event> = {
            let index = self.index.lock().unwrap();
            let mut seen = HashSet::new();
            events
                .iter()
                .filter(|e| {
                    !e.kind.is_ephemeral()
                        && expiration(e).is_none_or(|exp| exp > now)
                        && !index.contains(&e.id.to_bytes())
                        && seen.insert(e.id)
                })
                .collect()
        };
        if batch.is_empty() {
            return 0;
        }
        let first = self.save_batch(&batch).await;
        let mut fresh = first.ids;
        if first.map_full {
            // The map filled before the budget said it would: the estimate ran
            // low for what this cache holds. Learn from it — hold the budget
            // to 80% of what is held now, so eviction runs early enough that
            // LMDB has pages left to delete with — and try once more.
            let victims = {
                let mut index = self.index.lock().unwrap();
                let learned = index.bytes() / 10 * 8;
                if learned < index.limit() {
                    index.set_limit(learned);
                }
                index.evict_over_limit()
            };
            tracing::info!(
                evicted = victims.len(),
                "event cache: store full before the budget; evicting further"
            );
            self.delete(&victims).await;
            let retry: Vec<&Event> = batch
                .iter()
                .copied()
                .filter(|e| !fresh.contains(&e.id))
                .collect();
            fresh.extend(self.save_batch(&retry).await.ids);
        }
        let victims = self.index.lock().unwrap().evict_over_limit();
        self.delete(&victims).await;
        drop(writing);
        fresh.len()
    }

    /// Save `batch` concurrently (the store commits concurrent saves
    /// together), index what landed, and forget whatever the saves displaced.
    /// The caller holds the write lock.
    async fn save_batch(&self, batch: &[&Event]) -> Saved {
        let mut displaced: Vec<[u8; 32]> = Vec::new();
        for event in batch {
            displaced.extend(self.displaced_by(event).await);
        }
        let results =
            futures_util::future::join_all(batch.iter().map(|e| self.db.save_event(e))).await;
        let mut saved = Saved {
            ids: HashSet::new(),
            map_full: false,
        };
        let mut deletions = false;
        {
            let mut index = self.index.lock().unwrap();
            for (event, result) in batch.iter().zip(results) {
                match result {
                    Ok(SaveEventStatus::Success) => {
                        index.insert(event.id.to_bytes(), disk_cost(event), expiration(event));
                        saved.ids.insert(event.id);
                        // A later event in the same batch may have replaced
                        // this one; checked with the rest below.
                        if event.kind.is_replaceable() || event.kind.is_addressable() {
                            displaced.push(event.id.to_bytes());
                        }
                        deletions |= event.kind == Kind::EventDeletion;
                    }
                    Ok(SaveEventStatus::Rejected(_)) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, id = %event.id, "event cache: save failed");
                        saved.map_full |= is_map_full(&e);
                    }
                }
            }
        }
        // A deletion may have removed an event saved beside it in this very
        // batch, which `displaced_by` could not see before the save.
        if deletions {
            displaced.extend(saved.ids.iter().map(|id| id.to_bytes()));
        }
        self.forget_if_gone(&displaced).await;
        saved.ids.retain(|id| self.contains(&id.to_bytes()));
        saved
    }

    /// Ids an event could supersede or delete on save, among those cached.
    async fn displaced_by(&self, event: &Event) -> Vec<[u8; 32]> {
        let kind = event.kind;
        if kind == Kind::EventDeletion {
            let mut out: Vec<[u8; 32]> = event
                .tags
                .event_ids()
                .map(|id| id.to_bytes())
                .filter(|id| self.contains(id))
                .collect();
            // Deletions by address: whatever the cache holds in that slot.
            for coordinate in event.tags.coordinates() {
                let mut slot = Filter::new()
                    .kind(coordinate.kind)
                    .author(coordinate.public_key);
                if coordinate.kind.is_addressable() {
                    slot = slot.identifier(coordinate.identifier.clone());
                }
                out.extend(self.slot_ids(slot).await);
            }
            return out;
        }
        if !(kind.is_replaceable() || kind.is_addressable()) {
            return Vec::new();
        }
        let mut slot = Filter::new().kind(kind).author(event.pubkey);
        if kind.is_addressable() {
            let d = event.tags.identifier().unwrap_or_default().to_string();
            slot = slot.identifier(d);
        }
        self.slot_ids(slot).await
    }

    async fn slot_ids(&self, filter: Filter) -> Vec<[u8; 32]> {
        match self.db.negentropy_items(filter).await {
            Ok(items) => items.into_iter().map(|(id, _)| id.to_bytes()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Drop from the index any of `ids` the store no longer holds.
    async fn forget_if_gone(&self, ids: &[[u8; 32]]) {
        for id in ids {
            let status = self.db.check_id(&EventId::from_byte_array(*id)).await;
            if !matches!(status, Ok(DatabaseEventStatus::Saved)) {
                self.index.lock().unwrap().remove(id);
            }
        }
    }

    /// Events matching any of `filters`, newest first, capped at the smallest
    /// `limit` — without counting as an access. For a caller that merges this
    /// with another store and then [`EventCache::touch`]es what it returned.
    pub async fn peek(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        let now = now_secs();
        // Expired but not yet swept events are skipped below; ask for that
        // many more so they cannot take a live event's place under a `limit`.
        // Reads stay reads: deleting is the upkeep's job.
        let due = self.index.lock().unwrap().expired(now).len();
        let mut out: Vec<Event> = Vec::new();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        for filter in filters {
            let mut filter = filter.clone();
            if let (Some(limit), true) = (filter.limit, due > 0) {
                filter.limit = Some(limit + due);
            }
            let found = self
                .db
                .query(filter)
                .await
                .map_err(|e| anyhow::anyhow!("event cache: query failed: {e}"))?;
            for event in found {
                if expiration(&event).is_some_and(|exp| exp <= now) {
                    continue;
                }
                if seen.insert(event.id.to_bytes()) {
                    out.push(event);
                }
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        nsite_deck::cap_per_filter(&mut out, filters);
        Ok(out)
    }

    /// Count an access to each of `ids` that is cached.
    pub fn touch<'a>(&self, ids: impl IntoIterator<Item = &'a EventId>) {
        let mut index = self.index.lock().unwrap();
        for id in ids {
            index.touch(&id.to_bytes());
        }
    }

    /// [`EventCache::peek`], counting every hit as an access.
    pub async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        let out = self.peek(filters).await?;
        self.touch(out.iter().map(|e| &e.id));
        Ok(out)
    }

    /// Remove `ids` from the cache — they are now in the local relay, or gone.
    pub async fn remove(&self, ids: &[[u8; 32]]) {
        let _writing = self.writes.lock().await;
        let held: Vec<[u8; 32]> = {
            let mut index = self.index.lock().unwrap();
            ids.iter()
                .filter(|id| index.remove(id).is_some())
                .copied()
                .collect()
        };
        self.delete(&held).await;
    }

    /// Delete everything past its NIP-40 expiry. Returns how many went.
    pub async fn sweep_expired(&self) -> usize {
        let expired = self.index.lock().unwrap().expired(now_secs());
        if !expired.is_empty() {
            self.remove(&expired).await;
        }
        expired.len()
    }

    /// Evict down to the budget.
    pub async fn evict(&self) {
        let _writing = self.writes.lock().await;
        let victims = self.index.lock().unwrap().evict_over_limit();
        if !victims.is_empty() {
            tracing::debug!(evicted = victims.len(), "event cache: evicted");
            self.delete(&victims).await;
        }
    }

    async fn delete(&self, ids: &[[u8; 32]]) {
        for chunk in ids.chunks(CHUNK) {
            let filter = Filter::new().ids(chunk.iter().map(|id| EventId::from_byte_array(*id)));
            if let Err(e) = self.db.delete(filter).await {
                tracing::warn!(error = %e, "event cache: delete failed");
            }
        }
    }

    /// Drop everything ("Clear cache"). The database is emptied now; its file
    /// shrinks at the next launch.
    pub async fn clear(&self) -> anyhow::Result<()> {
        {
            let _writing = self.writes.lock().await;
            self.index.lock().unwrap().clear();
            self.db
                .wipe()
                .await
                .map_err(|e| anyhow::anyhow!("event cache: wipe failed: {e}"))?;
            let _ = std::fs::write(self.dir.join(COMPACT_MARKER), b"");
        }
        self.save_snapshot().await;
        Ok(())
    }

    /// Persist the index if it changed since the last save. Encoded under the
    /// lock, written off the async threads.
    pub async fn save_snapshot(&self) {
        let bytes = {
            let mut index = self.index.lock().unwrap();
            if !index.take_dirty() {
                return;
            }
            index.encode()
        };
        let path = self.dir.join(SNAPSHOT);
        let written = tokio::task::spawn_blocking(move || write_snapshot(&path, &bytes)).await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::warn!("event cache: could not save the index");
        }
    }

    /// Once per launch, in the background: read the index snapshot (folding in
    /// anything cached since open), then repair it against the store.
    pub async fn startup(&self) {
        let path = self.dir.join(SNAPSHOT);
        let limit = self.index.lock().unwrap().limit();
        let loaded = tokio::task::spawn_blocking(move || Slru::load(&path, limit))
            .await
            .ok()
            .flatten();
        if let Some(mut snapshot) = loaded {
            let mut index = self.index.lock().unwrap();
            snapshot.absorb(&index);
            *index = snapshot;
        }
        self.reconcile().await;
    }

    /// Bring the index back in step with the store: forget ids the store no
    /// longer has, and index events it holds that the index missed (oldest
    /// first, so they are the first to go).
    ///
    /// Always compares the two sets of ids — a listing, not a load — because
    /// equal counts are not equal sets: a kill after an eviction and an insert
    /// at budget leaves both at the same size and each holding one the other
    /// lacks. Only missing events are read.
    pub async fn reconcile(&self) {
        {
            let _writing = self.writes.lock().await;
            let mut items = match self.db.negentropy_items(Filter::new()).await {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!(error = %e, "event cache: listing failed; not reconciling");
                    return;
                }
            };
            let in_store: HashSet<[u8; 32]> = items.iter().map(|(id, _)| id.to_bytes()).collect();
            let forgot = {
                let mut index = self.index.lock().unwrap();
                let stale: Vec<[u8; 32]> = index
                    .keys()
                    .into_iter()
                    .filter(|id| !in_store.contains(id))
                    .collect();
                for id in &stale {
                    index.remove(id);
                }
                items.retain(|(id, _)| !index.contains(&id.to_bytes()));
                stale.len()
            };
            if forgot == 0 && items.is_empty() {
                return;
            }
            items.sort_by_key(|(_, at)| *at);
            let mut added = 0usize;
            for chunk in items.chunks(CHUNK) {
                let filter = Filter::new().ids(chunk.iter().map(|(id, _)| *id));
                let Ok(found) = self.db.query(filter).await else {
                    continue;
                };
                let mut found: Vec<Event> = found.into_iter().collect();
                found.sort_by_key(|e| e.created_at);
                let mut index = self.index.lock().unwrap();
                for event in found {
                    if index.insert(event.id.to_bytes(), disk_cost(&event), expiration(&event)) {
                        added += 1;
                    }
                }
            }
            tracing::info!(
                forgot,
                indexed = added,
                "event cache: index reconciled with the store"
            );
            let victims = self.index.lock().unwrap().evict_over_limit();
            self.delete(&victims).await;
        }
        self.save_snapshot().await;
    }
}

impl Drop for EventCache {
    fn drop(&mut self) {
        if self.scratch {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[async_trait]
impl RelayBackend for EventCache {
    async fn publish(&self, event: Event) -> anyhow::Result<()> {
        self.insert(std::slice::from_ref(&event)).await;
        Ok(())
    }

    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        EventCache::query(self, filters).await
    }
}

/// The budget a cache with a `map_size` map can hold: at most half the map,
/// and never more than [`MAX_BUDGET`].
fn budget_within(limit: u64, map_size: u64) -> u64 {
    limit.min(MAX_BUDGET).min(map_size / 2)
}

/// LMDB map size for a budget: address space, not allocation. Twice the
/// (already disk-estimated) budget, never under 1 GiB, capped, and rounded up
/// to 1 MiB — a multiple of every page size LMDB meets, which it requires.
/// Eviction keeps the store under the budget, so the map is a margin, not the
/// limit; a budget raised far past it takes full effect at the next launch.
fn map_size_for(limit: u64) -> usize {
    const MIB: u64 = 1024 * 1024;
    let floor = 1024 * MIB;
    let size = limit.min(MAX_BUDGET).saturating_mul(2).max(floor);
    (size.div_ceil(MIB) * MIB) as usize
}

/// The NIP-40 `expiration` tag value, if present.
pub fn expiration(event: &Event) -> Option<u64> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("expiration"))
            .then(|| s.get(1).and_then(|v| v.parse::<u64>().ok()))
            .flatten()
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip01::Coordinate;
    use nostr::{EventBuilder, Keys, Tag, Timestamp};
    use std::sync::Arc;

    fn note(keys: &Keys, body: &str, at: u64) -> Event {
        EventBuilder::text_note(body)
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    #[test]
    fn map_sizes_are_page_aligned_and_capped() {
        for limit in [0, 300_000_000, 500 * 1024 * 1024 + 7, u64::MAX] {
            let size = map_size_for(limit) as u64;
            assert_eq!(size % (1024 * 1024), 0, "{limit} gave an unaligned map");
            assert!(size <= MAX_BUDGET * 2);
        }
    }

    #[test]
    fn disk_cost_counts_indexed_tags() {
        let keys = Keys::generate();
        let bare = note(&keys, "x", 1);
        let tagged = EventBuilder::text_note("x")
            .tags([
                Tag::public_key(Keys::generate().public_key()),
                Tag::parse(["client", "y"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        assert!(disk_cost(&bare) > bare.as_json().len() as u64);
        assert!(disk_cost(&tagged) >= disk_cost(&bare) + TAG_COST);
        assert!(disk_cost(&tagged) < disk_cost(&bare) + 2 * TAG_COST + 400);
    }

    #[tokio::test]
    async fn inserts_queries_and_counts_bytes() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let a = note(&keys, "a", 1_000);
        let b = note(&keys, "b", 2_000);
        assert_eq!(cache.insert(&[a.clone(), b.clone()]).await, 2);
        assert_eq!(cache.insert(std::slice::from_ref(&a)).await, 0, "dedup");
        let got = cache
            .query(&[Filter::new().author(keys.public_key())])
            .await
            .unwrap();
        assert_eq!(
            got.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![b.id, a.id]
        );
        let stats = cache.stats();
        assert_eq!(stats.count, 2);
        assert_eq!(stats.bytes, disk_cost(&a) + disk_cost(&b));
    }

    #[tokio::test]
    async fn evicts_to_the_budget_keeping_what_was_used() {
        let keys = Keys::generate();
        let first = note(&keys, "used", 1);
        let size = disk_cost(&first);
        // Room for about four notes.
        let cache = EventCache::scratch(size * 4 + 10);
        cache.insert(std::slice::from_ref(&first)).await;
        cache.query(&[Filter::new().id(first.id)]).await.unwrap();
        cache.query(&[Filter::new().id(first.id)]).await.unwrap();
        for i in 0..20 {
            cache
                .insert(&[note(&keys, &format!("scan{i:02}"), 100 + i)])
                .await;
        }
        assert!(cache.stats().bytes <= size * 4 + 10);
        let kept = cache.peek(&[Filter::new().id(first.id)]).await.unwrap();
        assert_eq!(kept.len(), 1, "a twice-used event was evicted by a scan");
        let all = cache.peek(&[Filter::new()]).await.unwrap();
        assert_eq!(
            all.len() as u64,
            cache.stats().count,
            "index and store disagree"
        );
    }

    #[tokio::test]
    async fn replaced_events_leave_the_index() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let old = EventBuilder::new(Kind::Metadata, "{}")
            .custom_created_at(Timestamp::from(1_000))
            .sign_with_keys(&keys)
            .unwrap();
        let new = EventBuilder::new(Kind::Metadata, "{\"name\":\"x\"}")
            .custom_created_at(Timestamp::from(2_000))
            .sign_with_keys(&keys)
            .unwrap();
        cache.insert(std::slice::from_ref(&old)).await;
        cache.insert(std::slice::from_ref(&new)).await;
        assert!(!cache.contains(&old.id.to_bytes()));
        assert_eq!(cache.stats().count, 1);
        assert_eq!(cache.stats().bytes, disk_cost(&new));

        // Both versions in one batch: only the newest stays indexed.
        let cache = EventCache::scratch(1_000_000);
        cache.insert(&[old.clone(), new.clone()]).await;
        assert_eq!(cache.stats().count, 1);
    }

    /// Concurrent writers to one replaceable slot leave the index agreeing
    /// with the store.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_replaceable_inserts_do_not_drift() {
        let cache = Arc::new(EventCache::scratch(100_000_000));
        let keys = Keys::generate();
        for round in 0..20u64 {
            let tasks: Vec<_> = (0..4u64)
                .map(|i| {
                    let cache = cache.clone();
                    let event = EventBuilder::new(Kind::from(10_000u16), format!("{round}-{i}"))
                        .custom_created_at(Timestamp::from(round * 10 + i))
                        .sign_with_keys(&keys)
                        .unwrap();
                    tokio::spawn(async move { cache.insert(&[event]).await })
                })
                .collect();
            for task in tasks {
                task.await.unwrap();
            }
        }
        let held = cache.peek(&[Filter::new()]).await.unwrap().len();
        assert_eq!(held, 1);
        assert_eq!(cache.stats().count, 1, "the index kept replaced events");
    }

    #[tokio::test]
    async fn a_deletion_by_address_leaves_the_index() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let article = EventBuilder::new(Kind::LongFormTextNote, "text")
            .tags([Tag::identifier("post")])
            .custom_created_at(Timestamp::from(1_000))
            .sign_with_keys(&keys)
            .unwrap();
        cache.insert(std::slice::from_ref(&article)).await;
        let coordinate =
            Coordinate::new(Kind::LongFormTextNote, keys.public_key()).identifier("post");
        let deletion = EventBuilder::delete(
            nostr::nips::nip09::EventDeletionRequest::new().coordinate(coordinate),
        )
        .sign_with_keys(&keys)
        .unwrap();
        cache.insert(std::slice::from_ref(&deletion)).await;
        assert!(!cache.contains(&article.id.to_bytes()));
    }

    /// A note and its deletion in one batch leave neither in the index.
    #[tokio::test]
    async fn a_note_deleted_in_its_own_batch_leaves_no_phantom() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let doomed = note(&keys, "doomed", 1);
        let deletion =
            EventBuilder::delete(nostr::nips::nip09::EventDeletionRequest::new().id(doomed.id))
                .sign_with_keys(&keys)
                .unwrap();
        cache.insert(&[doomed.clone(), deletion]).await;
        assert!(!cache.contains(&doomed.id.to_bytes()));
        let held = cache.peek(&[Filter::new()]).await.unwrap().len() as u64;
        assert_eq!(held, cache.stats().count, "index and store disagree");
    }

    #[tokio::test]
    async fn expired_events_are_swept_and_never_served() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let now = now_secs();
        let soon = EventBuilder::text_note("soon")
            .tags([Tag::expiration(Timestamp::from(now + 1))])
            .sign_with_keys(&keys)
            .unwrap();
        let dead = EventBuilder::text_note("dead")
            .tags([Tag::expiration(Timestamp::from(now - 1))])
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(cache.insert(&[soon.clone(), dead]).await, 1);
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        // Never served, and a limited read still finds nothing in its place.
        assert!(cache.query(&[Filter::new()]).await.unwrap().is_empty());
        assert!(cache
            .query(&[Filter::new().limit(1)])
            .await
            .unwrap()
            .is_empty());
        // Deleting is the upkeep's: reads do not write.
        assert_eq!(cache.stats().count, 1);
        assert_eq!(cache.sweep_expired().await, 1);
        assert_eq!(cache.stats().count, 0);
    }

    #[tokio::test]
    async fn remove_and_clear() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let a = note(&keys, "a", 1);
        let b = note(&keys, "b", 2);
        cache.insert(&[a.clone(), b.clone()]).await;
        cache.remove(&[a.id.to_bytes()]).await;
        let left = cache.query(&[Filter::new()]).await.unwrap();
        assert_eq!(left.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b.id]);
        cache.clear().await.unwrap();
        assert_eq!(cache.stats().count, 0);
        assert!(cache.query(&[Filter::new()]).await.unwrap().is_empty());
        assert!(cache.dir.join(COMPACT_MARKER).exists());
    }

    /// Same size, different members: the index still ends up matching.
    #[tokio::test]
    async fn reconcile_repairs_equal_counts_with_different_members() {
        let cache = EventCache::scratch(1_000_000);
        let keys = Keys::generate();
        let a = note(&keys, "a", 1);
        let b = note(&keys, "b", 2);
        let c = note(&keys, "c", 3);
        cache.insert(&[a.clone(), b.clone()]).await;
        let snapshot = cache.index.lock().unwrap().encode();
        // After the snapshot: b goes, c arrives — then the process dies.
        cache.remove(&[b.id.to_bytes()]).await;
        cache.insert(std::slice::from_ref(&c)).await;
        let path = cache.dir.join(SNAPSHOT);
        write_snapshot(&path, &snapshot).unwrap();
        *cache.index.lock().unwrap() = Slru::new(1_000_000);
        cache.startup().await;
        assert!(cache.contains(&a.id.to_bytes()));
        assert!(
            cache.contains(&c.id.to_bytes()),
            "an event on disk was left unindexed"
        );
        assert!(
            !cache.contains(&b.id.to_bytes()),
            "a phantom entry survived"
        );
    }

    /// A budget far beyond the map is held to half of it, so the store never
    /// fills and the cache keeps accepting — evicting — rather than failing
    /// every save from then on.
    #[tokio::test]
    async fn a_budget_beyond_the_map_is_held_to_it() {
        let dir = crate::scratch_dir("map-full");
        // A budget far above what a 1 MiB map can hold.
        let cache = EventCache::open_with_map(&dir, 1_000_000_000, 1024 * 1024).unwrap();
        assert_eq!(cache.stats().limit, 512 * 1024);
        let keys = Keys::generate();
        let body = "x".repeat(4 * 1024);
        let mut accepted = 0;
        for i in 0..400 {
            accepted += cache
                .insert(&[note(&keys, &format!("{i}:{body}"), i)])
                .await;
        }
        assert!(
            accepted > 350,
            "the cache stopped accepting after the map filled: {accepted}"
        );
        let last = note(&keys, &format!("last:{body}"), 1_000);
        assert_eq!(cache.insert(std::slice::from_ref(&last)).await, 1);
        // Raising it at runtime is held the same way until the next launch.
        cache.set_limit(10_000_000_000).await;
        assert_eq!(cache.stats().limit, 512 * 1024);
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn startup_reads_the_snapshot_and_repairs_a_lost_one() {
        let dir = crate::scratch_dir("reconcile");
        let keys = Keys::generate();
        let a = note(&keys, "a", 1);
        let b = note(&keys, "b", 2);
        let cache = EventCache::open(&dir, 1_000_000).unwrap();
        cache.insert(&[a.clone(), b.clone()]).await;
        // No snapshot was saved; a fresh index knows nothing.
        *cache.index.lock().unwrap() = Slru::new(1_000_000);
        cache.startup().await;
        assert_eq!(cache.stats().count, 2);
        assert!(cache.contains(&a.id.to_bytes()));
        // The repair saved a snapshot; a second startup reads it.
        *cache.index.lock().unwrap() = Slru::new(1_000_000);
        cache.startup().await;
        assert_eq!(cache.stats().count, 2);
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
