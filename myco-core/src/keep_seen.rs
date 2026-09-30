//! Keep what we see: a handful of event kinds that arrive from outside the
//! local relay — a public relay, a Circle peer's relay, a multi-hop pull —
//! are written into the local relay on the way past, so the next lookup
//! answers here, works offline, and is served on to the Circle.
//!
//! Only kinds another lookup will want again, and only replaceable ones, so
//! each author (and `d` tag) costs one event however often it is seen:
//!
//! - **Profiles** (kind 0) and the author's relay lists: **10002** (NIP-65,
//!   where to find their notes and manifests), **10050** (NIP-17, where to
//!   send them a DM) and **10063** (BUD-03, their Blossom servers).
//! - **nsite manifests**, 15128 / 35128. Legacy 34128 is not kept: nothing
//!   here reads it, and it is one event per *file*.
//! - **napplet manifests**, 15129 / 35129. Snapshots (5129) are not kept:
//!   nothing here reads them, and a regular kind is never replaced.
//!
//! What this is not: it never touches blobs (a manifest is kept, the bytes it
//! names are not fetched), and it never installs anything. A newer manifest
//! kept for an installed app sits in the store beside the **pinned** version
//! this phone has the files for; the gateway, a napplet's open and the Circle
//! relay all read through the pin (`docs/design/nsite/nsite-layer.md` §2.1,
//! "Events kept as they pass").
//!
//! **Signatures.** Every caller hands in events that were verified where they
//! entered the process — `query_relay_filters` for a public relay, the peer
//! pool for a mesh relay — so they are not verified again here
//! (`reference/thinning-custom-relay.md`, D7).
//!
//! **Off the hot path.** [`KeepSeen::offer`] filters and clones, then spawns
//! the writes; the caller answers without waiting. At most [`MAX_PER_BATCH`]
//! events are kept per offer and at most [`MAX_BATCHES_IN_FLIGHT`] offers
//! write at once. An offer beyond that is dropped, not queued.
//!
//! **Growth** is bounded in practice by the kinds: small, replaceable, one per
//! author and app the user actually came across. Nothing prunes them yet
//! besides "Clear local database" (`docs/roadmap.md`, pruning of kept events).

use std::sync::Arc;

use nostr::Event;
use nsite_deck::seams::RelayBackend;
use tokio::sync::Semaphore;

/// The kinds kept when seen. See the module docs for why each is here.
pub(crate) const KEPT_KINDS: &[u16] = &[
    0,
    10_002,
    10_050,
    10_063,
    nsite_deck::KIND_ROOT,
    nsite_deck::KIND_NAMED,
    myco_napplet_runtime::KIND_ROOT,
    myco_napplet_runtime::KIND_NAMED,
];

/// An event larger than this is not kept — the size common public relays
/// accept. A kind 0 with a picture inlined as base64 is the usual offender;
/// the lookup that found it still has it.
const MAX_EVENT_BYTES: usize = 64 * 1024;

/// At most this many events are kept from one offer. A napplet paging through
/// a directory asks for a page at a time; a page larger than this is a flood.
pub(crate) const MAX_PER_BATCH: usize = 128;

/// At most this many offers write at once; further ones are dropped.
pub(crate) const MAX_BATCHES_IN_FLIGHT: usize = 2;

/// Whether `event` is of a kind, and a size, this device keeps.
pub(crate) fn is_kept(event: &Event) -> bool {
    KEPT_KINDS.contains(&event.kind.as_u16()) && approx_size(event) <= MAX_EVENT_BYTES
}

/// The event's size, near enough: its content and every tag value. Counting
/// the bytes without serialising the event again.
fn approx_size(event: &Event) -> usize {
    event.content.len()
        + event
            .tags
            .iter()
            .map(|t| t.as_slice().iter().map(String::len).sum::<usize>())
            .sum::<usize>()
}

/// The tap: one per content layer, so every path shares one bound. Always
/// over the **embedded** store (read through the cache, so a kept event
/// leaves it): with a custom relay configured there is no tap at all (see
/// `Content::keep_seen`), so browsing is never written to someone else's
/// relay.
#[derive(Clone)]
pub struct KeepSeen {
    store: Arc<dyn RelayBackend>,
    permits: Arc<Semaphore>,
}

impl KeepSeen {
    pub fn new(store: Arc<dyn RelayBackend>) -> Self {
        Self {
            store,
            permits: Arc::new(Semaphore::new(MAX_BATCHES_IN_FLIGHT)),
        }
    }

    /// Keep the kept kinds among `events`, behind the caller's back.
    ///
    /// `events` must already be verified — see the module docs. The store
    /// applies replaceable / addressable newest-wins and NIP-09 on its own, so
    /// an older event than the one held is a no-op.
    ///
    /// Returns the write task, for tests to wait on; `None` when there was
    /// nothing to keep, no runtime to write on, or the bound was reached.
    pub fn offer<'a>(
        &self,
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let batch: Vec<Event> = events
            .into_iter()
            .filter(|e| is_kept(e))
            .take(MAX_PER_BATCH)
            .cloned()
            .collect();
        if batch.is_empty() {
            return None;
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            tracing::debug!(
                dropped = batch.len(),
                "keep-seen: busy; not keeping this batch"
            );
            return None;
        };
        let store = self.store.clone();
        Some(runtime.spawn(async move {
            let _permit = permit;
            for event in batch {
                if let Err(e) = store.publish(event).await {
                    tracing::debug!(error = %e, "keep-seen: could not store an event");
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_relay::RelayStore;
    use nostr::{EventBuilder, Filter, Keys, Kind, Tag, Timestamp};

    fn event(keys: &Keys, kind: u16, content: &str, at: u64, d: Option<&str>) -> Event {
        let tags: Vec<Tag> = d
            .map(|d| Tag::identifier(d.to_string()))
            .into_iter()
            .collect();
        EventBuilder::new(Kind::from(kind), content)
            .tags(tags)
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn scratch() -> Arc<RelayStore> {
        Arc::new(RelayStore::in_memory())
    }

    async fn all(store: &RelayStore) -> Vec<Event> {
        store.query(&[Filter::new()]).await.unwrap()
    }

    #[tokio::test]
    async fn kept_kinds_are_stored_and_nothing_else() {
        let store = scratch();
        let keep = KeepSeen::new(store.clone());
        let keys = Keys::generate();
        let kept: Vec<Event> = [
            (0, None),
            (10_002, None),
            (10_050, None),
            (10_063, None),
            (15_128, None),
            (35_128, Some("blog")),
            (15_129, None),
            (35_129, Some("game")),
        ]
        .into_iter()
        .map(|(kind, d)| event(&keys, kind, "", 1_000, d))
        .collect();
        let others: Vec<Event> = [1u16, 3, 7, 1059, 5_129, 30_023, 34_128]
            .into_iter()
            .map(|kind| event(&keys, kind, "", 1_000, Some("x")))
            .collect();

        keep.offer(kept.iter().chain(&others))
            .unwrap()
            .await
            .unwrap();

        let mut got: Vec<u16> = all(&store).await.iter().map(|e| e.kind.as_u16()).collect();
        got.sort_unstable();
        let mut want: Vec<u16> = kept.iter().map(|e| e.kind.as_u16()).collect();
        want.sort_unstable();
        assert_eq!(got, want, "only the kept kinds reach the store");
    }

    #[tokio::test]
    async fn nothing_to_keep_spawns_nothing() {
        let store = scratch();
        let keep = KeepSeen::new(store.clone());
        let note = event(&Keys::generate(), 1, "hi", 1_000, None);
        assert!(keep.offer([&note]).is_none());
        assert!(all(&store).await.is_empty());
    }

    /// Replaceable semantics are the store's: an older profile seen after a
    /// newer one does not replace it, and a newer one does.
    #[tokio::test]
    async fn the_newest_profile_wins() {
        let store = scratch();
        let keep = KeepSeen::new(store.clone());
        let keys = Keys::generate();
        let newer = event(&keys, 0, r#"{"name":"new"}"#, 2_000, None);
        let older = event(&keys, 0, r#"{"name":"old"}"#, 1_000, None);
        let newest = event(&keys, 0, r#"{"name":"newest"}"#, 3_000, None);

        keep.offer([&newer]).unwrap().await.unwrap();
        keep.offer([&older]).unwrap().await.unwrap();
        let held = store
            .query(&[Filter::new().kind(Kind::Metadata)])
            .await
            .unwrap();
        assert_eq!(held.iter().map(|e| e.id).collect::<Vec<_>>(), [newer.id]);

        keep.offer([&newest]).unwrap().await.unwrap();
        let held = store
            .query(&[Filter::new().kind(Kind::Metadata)])
            .await
            .unwrap();
        assert_eq!(held.iter().map(|e| e.id).collect::<Vec<_>>(), [newest.id]);
    }

    #[tokio::test]
    async fn an_oversized_event_is_not_kept() {
        let store = scratch();
        let keep = KeepSeen::new(store.clone());
        let keys = Keys::generate();
        let huge = event(&keys, 0, &"x".repeat(MAX_EVENT_BYTES + 1), 1_000, None);
        assert!(keep.offer([&huge]).is_none());
        assert!(all(&store).await.is_empty());
    }

    /// A flood is cut to one batch, and offers past the in-flight bound are
    /// dropped rather than queued.
    #[tokio::test]
    async fn a_flood_is_cut_to_one_batch() {
        let store = scratch();
        let keep = KeepSeen::new(store.clone());
        let flood: Vec<Event> = (0..MAX_PER_BATCH + 50)
            .map(|_| event(&Keys::generate(), 0, "{}", 1_000, None))
            .collect();

        // Hold every permit, as offers still writing would.
        let held: Vec<_> = (0..MAX_BATCHES_IN_FLIGHT)
            .map(|_| keep.permits.clone().try_acquire_owned().unwrap())
            .collect();
        assert!(keep.offer(&flood).is_none(), "an offer past the bound ran");
        drop(held);

        keep.offer(&flood).unwrap().await.unwrap();
        assert_eq!(all(&store).await.len(), MAX_PER_BATCH);
    }
}
