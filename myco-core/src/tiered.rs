//! Reading through the local stores and the shell cache as one, and choosing
//! which of the two a write lands in.
//!
//! The local relay and Blossom hold what this device **keeps**; the cache
//! (`myco-cache`) holds everything else that passed through. A reader should
//! not care which: [`TieredRelay`] and [`TieredBlobs`] answer from both,
//! deduplicated, and a cache hit counts as an access to it.
//!
//! What differs between them is where a write goes, fixed at construction:
//!
//! - [`Tier::Kept`] — the local store, and the copy in the cache (if any) is
//!   dropped: an event is held in one place, not two. This is the view the
//!   content layer, the hub and the gateway use.
//! - [`Tier::Cache`] — the cache, except the kinds `keep_seen` keeps, which go
//!   to the local store as they always have. This is the view pulled backlog,
//!   mesh pass-through and fetched blobs are written through.
//!
//! Two more rules hold for both:
//!
//! - Private messages ([`is_private`]) are never **cached**: they go to the
//!   local store, as every event did before the cache existed. A gift wrap
//!   addressed to this phone that only passed through the cache would be
//!   evicted before anyone opened it — or, dropped, acknowledged to its sender
//!   and then lost.
//! - A deletion (kind 5) reaches both stores: one published here also clears
//!   the cached copies of what it names, and one passing through also reaches
//!   what the local store keeps — when it names something kept there, so the
//!   relay does not collect every stranger's deletions.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use myco_cache::{BlobCache, EventCache};
use nostr::{Event, Filter, Kind};
use nsite_deck::seams::{BlobStore, RelayBackend};

/// Kinds addressed to one person — NIP-04 DMs, NIP-17 seals and NIP-59 gift
/// wraps. Kept rather than cached (see the module docs); never taken from a
/// query answer into either store.
pub(crate) fn is_private(event: &Event) -> bool {
    matches!(event.kind.as_u16(), 4 | 13 | 14 | 1059 | 1060)
}

/// Where a tiered view writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The local store; the cache's copy is dropped.
    Kept,
    /// The cache. `keep_kinds` routes `keep_seen`'s kinds to the local store
    /// instead — off when a custom relay is configured, since browsing is not
    /// written to someone else's relay.
    Cache { keep_kinds: bool },
}

/// The local relay and the event cache, read as one.
pub struct TieredRelay {
    kept: Arc<dyn RelayBackend>,
    cache: Arc<EventCache>,
    tier: Tier,
}

impl TieredRelay {
    pub fn new(kept: Arc<dyn RelayBackend>, cache: Arc<EventCache>, tier: Tier) -> Self {
        Self { kept, cache, tier }
    }

    async fn keep(&self, event: Event) -> anyhow::Result<()> {
        let id = event.id.to_bytes();
        let deletion = event.kind == Kind::EventDeletion;
        self.kept.publish(event.clone()).await?;
        if deletion {
            // What it deletes may be cached (the user's older notes, pulled
            // from another device); the cache applies NIP-09 itself.
            self.cache.insert(std::slice::from_ref(&event)).await;
        } else if self.cache.contains(&id) {
            self.cache.remove(&[id]).await;
        }
        Ok(())
    }

    /// Whether the local store holds anything `deletion` names, by id or by
    /// address — the only case in which a passing deletion is worth keeping.
    async fn names_something_kept(&self, deletion: &Event) -> bool {
        let ids: Vec<nostr::EventId> = deletion.tags.event_ids().copied().collect();
        let mut filters: Vec<Filter> = Vec::new();
        if !ids.is_empty() {
            filters.push(Filter::new().ids(ids));
        }
        for coordinate in deletion.tags.coordinates() {
            let mut slot = Filter::new()
                .kind(coordinate.kind)
                .author(coordinate.public_key);
            if coordinate.kind.is_addressable() {
                slot = slot.identifier(coordinate.identifier.clone());
            }
            filters.push(slot);
        }
        if filters.is_empty() {
            return false;
        }
        self.kept
            .query(&filters)
            .await
            .is_ok_and(|found| found.iter().any(|e| e.pubkey == deletion.pubkey))
    }
}

#[async_trait]
impl RelayBackend for TieredRelay {
    async fn publish(&self, event: Event) -> anyhow::Result<()> {
        match self.tier {
            Tier::Kept => self.keep(event).await,
            Tier::Cache { keep_kinds } if keep_kinds && crate::keep_seen::is_kept(&event) => {
                self.keep(event).await
            }
            Tier::Cache { .. } if is_private(&event) => self.keep(event).await,
            Tier::Cache { keep_kinds } => {
                if keep_kinds
                    && event.kind == Kind::EventDeletion
                    && self.names_something_kept(&event).await
                {
                    // The author's deletion reaches what the relay keeps of
                    // theirs; the relay checks it is theirs to delete.
                    self.kept.publish(event.clone()).await?;
                }
                self.cache.insert(std::slice::from_ref(&event)).await;
                Ok(())
            }
        }
    }

    /// Both stores at once, merged: one copy per id, the newest per
    /// replaceable slot, newest first, each filter capped at its own `limit`. Only
    /// the cached events that survive the merge count as accessed.
    ///
    /// A cache that fails only costs its hits. A local store that fails is an
    /// error only if the cache had nothing either: a custom relay that is down
    /// should not hide what this phone already has.
    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        let (kept, cached) = tokio::join!(self.kept.query(filters), self.cache.peek(filters));
        let cached = cached.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "event cache: query failed; answering from the relay");
            Vec::new()
        });
        let kept = match kept {
            Ok(kept) => kept,
            Err(e) if !cached.is_empty() => {
                tracing::warn!(error = %e, "relay query failed; answering from the cache");
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let merged = merge(kept, cached, filters);
        self.cache.touch(merged.iter().map(|e| &e.id));
        Ok(merged)
    }
}

/// Merge two answers to the same `filters`. See [`TieredRelay::query`].
pub(crate) fn merge(kept: Vec<Event>, cached: Vec<Event>, filters: &[Filter]) -> Vec<Event> {
    let mut seen: HashSet<[u8; 32]> = HashSet::new();
    let mut slots: HashMap<(u16, [u8; 32], String), usize> = HashMap::new();
    let mut out: Vec<Event> = Vec::with_capacity(kept.len() + cached.len());
    for event in kept.into_iter().chain(cached) {
        if !seen.insert(event.id.to_bytes()) {
            continue;
        }
        let kind = event.kind;
        if kind.is_replaceable() || kind.is_addressable() {
            let d = if kind.is_addressable() {
                event.tags.identifier().unwrap_or_default().to_string()
            } else {
                String::new()
            };
            let slot = (kind.as_u16(), event.pubkey.to_bytes(), d);
            if let Some(&at) = slots.get(&slot) {
                if event.created_at > out[at].created_at {
                    out[at] = event;
                }
                continue;
            }
            slots.insert(slot, out.len());
        }
        out.push(event);
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
    nsite_deck::cap_per_filter(&mut out, filters);
    out
}

/// The local Blossom and the blob cache, read as one.
pub struct TieredBlobs {
    kept: Arc<dyn BlobStore>,
    cache: Arc<BlobCache>,
    tier: Tier,
}

impl TieredBlobs {
    pub fn new(kept: Arc<dyn BlobStore>, cache: Arc<BlobCache>, tier: Tier) -> Self {
        Self { kept, cache, tier }
    }
}

#[async_trait]
impl BlobStore for TieredBlobs {
    // The cache is asked first: whether it holds a blob is an in-memory
    // lookup, while the local store may be someone else's server. Blobs are
    // content-addressed, so which copy answers makes no difference.

    async fn has(&self, sha256_hex: &str) -> bool {
        self.cache.has(sha256_hex).await || self.kept.has(sha256_hex).await
    }

    async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if self.cache.contains(sha256_hex) {
            if let Some(bytes) = self.cache.get(sha256_hex).await? {
                return Ok(Some(bytes));
            }
        }
        self.kept.get(sha256_hex).await
    }

    async fn size(&self, sha256_hex: &str) -> anyhow::Result<Option<u64>> {
        if let Some(size) = self.cache.size(sha256_hex).await? {
            return Ok(Some(size));
        }
        self.kept.size(sha256_hex).await
    }

    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        match self.tier {
            Tier::Kept => {
                let hash = self.kept.put(bytes).await?;
                self.cache.remove(std::slice::from_ref(&hash));
                Ok(hash)
            }
            Tier::Cache { .. } => self.cache.put(bytes).await,
        }
    }

    /// The local store only: clearing the cache is its own action.
    async fn wipe(&self) -> anyhow::Result<()> {
        self.kept.wipe().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_relay::RelayStore;
    use nostr::{EventBuilder, Keys, Kind, Timestamp};

    fn at(keys: &Keys, kind: Kind, body: &str, t: u64) -> Event {
        EventBuilder::new(kind, body)
            .custom_created_at(Timestamp::from(t))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn views() -> (Arc<RelayStore>, Arc<EventCache>, TieredRelay, TieredRelay) {
        let relay = Arc::new(RelayStore::in_memory());
        let cache = Arc::new(EventCache::scratch(10_000_000));
        let kept = TieredRelay::new(relay.clone(), cache.clone(), Tier::Kept);
        let cached = TieredRelay::new(
            relay.clone(),
            cache.clone(),
            Tier::Cache { keep_kinds: true },
        );
        (relay, cache, kept, cached)
    }

    #[tokio::test]
    async fn cache_writes_skip_the_relay_but_read_through() {
        let (relay, cache, kept, cached) = views();
        let keys = Keys::generate();
        let note = at(&keys, Kind::TextNote, "passing", 1);
        cached.publish(note.clone()).await.unwrap();
        assert_eq!(relay.count(), 0, "a passing note landed in the relay");
        assert!(cache.contains(&note.id.to_bytes()));
        let got = kept.query(&[Filter::new().id(note.id)]).await.unwrap();
        assert_eq!(got.len(), 1, "the merged view missed a cached event");
    }

    #[tokio::test]
    async fn kept_kinds_go_to_the_relay_from_the_cache_view() {
        let (relay, cache, _kept, cached) = views();
        let keys = Keys::generate();
        let profile = at(&keys, Kind::Metadata, "{}", 1);
        cached.publish(profile.clone()).await.unwrap();
        assert_eq!(relay.count(), 1);
        assert!(!cache.contains(&profile.id.to_bytes()));
    }

    #[tokio::test]
    async fn keeping_moves_an_event_out_of_the_cache() {
        let (relay, cache, kept, cached) = views();
        let keys = Keys::generate();
        let note = at(&keys, Kind::TextNote, "keep me", 1);
        cached.publish(note.clone()).await.unwrap();
        kept.publish(note.clone()).await.unwrap();
        assert!(!cache.contains(&note.id.to_bytes()));
        assert_eq!(relay.count(), 1);
        let got = kept.query(&[Filter::new()]).await.unwrap();
        assert_eq!(got.len(), 1, "held twice");
    }

    #[tokio::test]
    async fn private_messages_are_kept_not_cached() {
        let (relay, cache, _kept, cached) = views();
        let keys = Keys::generate();
        for kind in [4u16, 1059] {
            let dm = at(&keys, Kind::from(kind), "secret", 1);
            cached.publish(dm.clone()).await.unwrap();
            assert!(!cache.contains(&dm.id.to_bytes()));
        }
        assert_eq!(relay.count(), 2, "a passing gift wrap was stored nowhere");
    }

    /// A deletion published here clears the cached copy of what it names.
    #[tokio::test]
    async fn a_kept_deletion_reaches_a_cached_event() {
        let (_relay, cache, kept, cached) = views();
        let keys = Keys::generate();
        let note = at(&keys, Kind::TextNote, "pulled from another device", 1);
        cached.publish(note.clone()).await.unwrap();
        assert!(cache.contains(&note.id.to_bytes()));
        let deletion =
            EventBuilder::delete(nostr::nips::nip09::EventDeletionRequest::new().id(note.id))
                .sign_with_keys(&keys)
                .unwrap();
        kept.publish(deletion).await.unwrap();
        assert!(!cache.contains(&note.id.to_bytes()));
        assert!(kept
            .query(&[Filter::new().id(note.id)])
            .await
            .unwrap()
            .is_empty());
    }

    /// A stranger's deletion naming nothing kept is not kept.
    #[tokio::test]
    async fn a_passing_deletion_of_nothing_kept_stays_out_of_the_relay() {
        let (relay, _cache, _kept, cached) = views();
        let keys = Keys::generate();
        let elsewhere = at(&keys, Kind::TextNote, "never here", 1);
        let deletion =
            EventBuilder::delete(nostr::nips::nip09::EventDeletionRequest::new().id(elsewhere.id))
                .sign_with_keys(&keys)
                .unwrap();
        cached.publish(deletion).await.unwrap();
        assert_eq!(relay.count(), 0);
    }

    #[tokio::test]
    async fn a_passing_deletion_reaches_a_kept_event() {
        let (relay, _cache, kept, cached) = views();
        let keys = Keys::generate();
        let note = at(&keys, Kind::TextNote, "kept, then deleted", 1);
        kept.publish(note.clone()).await.unwrap();
        let deletion =
            EventBuilder::delete(nostr::nips::nip09::EventDeletionRequest::new().id(note.id))
                .sign_with_keys(&keys)
                .unwrap();
        cached.publish(deletion).await.unwrap();
        assert!(relay
            .query(&[Filter::new().id(note.id)])
            .await
            .unwrap()
            .is_empty());
    }

    /// A feed's `REQ` — notes with a limit, profiles without — gets every
    /// profile: each filter's limit is its own, not the smallest one's.
    #[tokio::test]
    async fn a_limit_on_one_filter_does_not_cap_another() {
        let (relay, _cache, kept, _cached) = views();
        let people: Vec<Keys> = (0..5).map(|_| Keys::generate()).collect();
        for (i, k) in people.iter().enumerate() {
            relay
                .publish(at(k, Kind::Metadata, "{}", 10))
                .await
                .unwrap();
            for n in 0..4 {
                relay
                    .publish(at(k, Kind::TextNote, &format!("{i}-{n}"), 100 + n))
                    .await
                    .unwrap();
            }
        }
        let authors: Vec<_> = people.iter().map(|k| k.public_key()).collect();
        let got = kept
            .query(&[
                Filter::new()
                    .authors(authors.clone())
                    .kind(Kind::TextNote)
                    .limit(3),
                Filter::new().authors(authors).kind(Kind::Metadata),
            ])
            .await
            .unwrap();
        assert_eq!(got.iter().filter(|e| e.kind == Kind::TextNote).count(), 3);
        assert_eq!(got.iter().filter(|e| e.kind == Kind::Metadata).count(), 5);
    }

    #[tokio::test]
    async fn merge_prefers_the_newest_in_a_slot_across_stores() {
        let keys = Keys::generate();
        let old = at(&keys, Kind::Metadata, "old", 1);
        let new = at(&keys, Kind::Metadata, "new", 2);
        let notes = [
            at(&keys, Kind::TextNote, "a", 3),
            at(&keys, Kind::TextNote, "b", 4),
        ];
        let merged = merge(
            vec![old, notes[0].clone()],
            vec![new.clone(), notes[0].clone(), notes[1].clone()],
            &[Filter::new()],
        );
        let ids: Vec<_> = merged.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![notes[1].id, notes[0].id, new.id]);
        let capped = merge(Vec::new(), notes.to_vec(), &[Filter::new().limit(1)]);
        assert_eq!(capped.len(), 1);
    }

    #[tokio::test]
    async fn blobs_read_through_and_keep_moves_them() {
        let dir = std::env::temp_dir().join(format!("myco-tiered-blobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let local = Arc::new(myco_blossom::FsBlobStore::open(&dir).unwrap());
        let cache = Arc::new(BlobCache::scratch(10_000_000));
        let kept = TieredBlobs::new(local.clone(), cache.clone(), Tier::Kept);
        let cached = TieredBlobs::new(
            local.clone(),
            cache.clone(),
            Tier::Cache { keep_kinds: true },
        );

        let hash = cached.put(b"fetched").await.unwrap();
        assert!(!local.has(&hash).await);
        assert_eq!(
            kept.get(&hash).await.unwrap().as_deref(),
            Some(&b"fetched"[..])
        );
        kept.put(b"fetched").await.unwrap();
        assert!(local.has(&hash).await);
        assert!(!cache.contains(&hash));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
