//! `myco-relay` — a generic embedded **Nostr relay**: a NIP-01 event store
//! implementing `nsite-deck`'s [`RelayBackend`] seam, plus a `ws://…:4869` socket
//! ([`server`]). See `docs/design/nsite-layer.md` §2.1 and
//! `docs/design/event-gossip.md`.
//!
//! Two kinds of event live here:
//!
//! - **Replaceable / addressable** (manifests: 15128 replaceable, 35128
//!   addressable) — newest-per-slot, persisted to a small JSON file. The slot is
//!   `(kind, author)` for replaceable and `(kind, author, d-tag)` for addressable.
//! - **Regular** (e.g. kind 9 chat) — kept **by event id** (many per author), so a
//!   second message does not overwrite the first. These are typically ephemeral
//!   (a NIP-40 `expiration` tag, `docs/design/event-gossip.md` §5): they are GC'd
//!   on expiry and **not persisted** — chat is memory-only by design.
//!
//! The query surface Myco uses is tiny (`{kinds, authors, #d, limit}`), so this is
//! a hand-rolled store over the rust-`nostr` `Event` type. Negentropy/NIP-77 is
//! **not** here (it lands later).
//!
//! [`RelayBackend`]: nsite_deck::seams::RelayBackend

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use nostr::{Event, PublicKey};
use nsite_deck::seams::{ManifestFilter, RelayBackend};

pub mod server;

/// An embedded NIP-01 event store, keyed by event id, with replaceable/addressable
/// dedup, NIP-40 expiry, and JSON persistence of the non-expiring (manifest) set.
pub struct RelayStore {
    path: Option<PathBuf>,
    events: Mutex<HashMap<[u8; 32], Event>>,
}

impl RelayStore {
    /// An in-memory store (no persistence) — for tests.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            events: Mutex::new(HashMap::new()),
        }
    }

    /// Open a store persisted at `<dir>/events.json`, loading any prior events.
    pub fn open(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("events.json");

        let mut map: HashMap<[u8; 32], Event> = HashMap::new();
        if path.is_file() {
            let raw = std::fs::read(&path)?;
            match serde_json::from_slice::<Vec<Event>>(&raw) {
                Ok(events) => {
                    let now = now_secs();
                    for event in events {
                        // Apply the same admission rules on load (newest wins,
                        // skip expired).
                        admit(&mut map, event, now);
                    }
                }
                Err(e) => tracing::warn!(error = %e, "relay store: ignoring corrupt events.json"),
            }
        }

        Ok(Self {
            path: Some(path),
            events: Mutex::new(map),
        })
    }

    /// Number of stored (non-expired) events (for diagnostics).
    pub fn count(&self) -> usize {
        let now = now_secs();
        self.events
            .lock()
            .unwrap()
            .values()
            .filter(|e| !is_expired(e, now))
            .count()
    }

    /// Persist the current **persistable** (non-expiring) event set — a full
    /// rewrite (the manifest set is tiny). Expiring events (chat) are memory-only,
    /// so storing one does not touch disk. Caller must not hold the lock.
    fn persist(&self, snapshot: &[Event]) {
        let Some(path) = &self.path else { return };
        let json = match serde_json::to_vec(snapshot) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!(error = %e, "relay store: serialize failed");
                return;
            }
        };
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path)) {
            tracing::error!(error = %e, "relay store: persist failed");
        }
    }
}

/// Replaceable kinds: 0, 3, and `10000..20000` (kind 15128 manifests live here).
fn is_replaceable(kind: u16) -> bool {
    kind == 0 || kind == 3 || (10_000..20_000).contains(&kind)
}

/// Addressable / parameterized-replaceable kinds: `30000..40000` (kind 35128).
fn is_addressable(kind: u16) -> bool {
    (30_000..40_000).contains(&kind)
}

fn event_d_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d"))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

/// The replaceable/addressable slot an event collapses into: `(kind, author, d)`,
/// with `d` only for addressable kinds. Regular kinds do not use a slot (kept by
/// id), so this is only consulted for replaceable/addressable events.
fn slot_of(event: &Event) -> (u16, [u8; 32], Option<String>) {
    let kind = event.kind.as_u16();
    let d = if is_addressable(kind) {
        event_d_tag(event)
    } else {
        None
    };
    (kind, event.pubkey.to_bytes(), d)
}

/// The NIP-40 `expiration` tag value (a unix timestamp), if present.
fn expiration(event: &Event) -> Option<u64> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("expiration"))
            .then(|| s.get(1).and_then(|v| v.parse::<u64>().ok()))
            .flatten()
    })
}

/// Whether an event has passed its NIP-40 expiry. Events with no `expiration` tag
/// (manifests) never expire.
pub fn is_expired(event: &Event, now: u64) -> bool {
    expiration(event).is_some_and(|exp| exp <= now)
}

/// Does an event satisfy a basic `{kinds, authors, #d}` filter? Used by both the
/// stored `query` and the server's live-subscription forwarding.
pub fn matches_filter(event: &Event, filter: &ManifestFilter) -> bool {
    (filter.kinds.is_empty() || filter.kinds.contains(&event.kind.as_u16()))
        && (filter.authors.is_empty() || filter.authors.contains(&event.pubkey))
        && (filter.d_tags.is_empty()
            || event_d_tag(event).is_some_and(|d| filter.d_tags.contains(&d)))
}

/// Admit an event into `map`, applying replaceable/addressable dedup (newest wins)
/// and skipping anything already expired or a stale duplicate. Returns `true` if
/// the event is now stored as new (so the caller can persist / fan it out).
fn admit(map: &mut HashMap<[u8; 32], Event>, event: Event, now: u64) -> bool {
    if is_expired(&event, now) {
        return false;
    }
    let kind = event.kind.as_u16();
    if is_replaceable(kind) || is_addressable(kind) {
        let slot = slot_of(&event);
        // Find the current holder of this slot, if any.
        let existing = map
            .iter()
            .find(|(_, e)| slot_of(e) == slot)
            .map(|(id, e)| (*id, e.created_at));
        match existing {
            Some((_, ts)) if ts >= event.created_at => return false,
            Some((old_id, _)) => {
                map.remove(&old_id);
            }
            None => {}
        }
        map.insert(event.id.to_bytes(), event);
        true
    } else {
        // Regular (and any other) kind: keyed by id, many per author.
        if map.contains_key(&event.id.to_bytes()) {
            return false;
        }
        map.insert(event.id.to_bytes(), event);
        true
    }
}

#[async_trait]
impl RelayBackend for RelayStore {
    async fn store_event(&self, event: Event) -> anyhow::Result<bool> {
        let now = now_secs();
        let persistable = expiration(&event).is_none();
        let snapshot = {
            let mut map = self.events.lock().unwrap();
            // Opportunistic GC: drop anything that has expired since last touch.
            map.retain(|_, e| !is_expired(e, now));
            if !admit(&mut map, event, now) {
                return Ok(false);
            }
            // Only the non-expiring (manifest) set is persisted; expiring chat
            // events stay in memory, so they never hit disk.
            persistable.then(|| {
                map.values()
                    .filter(|e| expiration(e).is_none())
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        if let Some(snapshot) = snapshot {
            self.persist(&snapshot);
        }
        Ok(true)
    }

    async fn get_manifest(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let now = now_secs();
        let map = self.events.lock().unwrap();
        Ok(map
            .values()
            .filter(|e| {
                e.kind.as_u16() == kind
                    && e.pubkey == *author
                    && event_d_tag(e).as_deref() == d_tag
                    && !is_expired(e, now)
            })
            .max_by_key(|e| e.created_at)
            .cloned())
    }

    async fn query(&self, filter: &ManifestFilter) -> anyhow::Result<Vec<Event>> {
        let now = now_secs();
        let map = self.events.lock().unwrap();
        let mut out: Vec<Event> = map
            .values()
            .filter(|e| !is_expired(e, now) && matches_filter(e, filter))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(limit) = filter.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        self.events.lock().unwrap().clear();
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }
}

/// Seconds since the Unix epoch.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use nsite_deck::model::{KIND_NAMED, KIND_ROOT};
    use nsite_deck::testing::build_test_site_with_keys;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-relay-test-{}-{}", std::process::id(), tag))
    }

    /// Build a signed kind-9 chat event with a room `d` tag and an optional
    /// `expiration` (absolute unix ts).
    fn chat_event(keys: &Keys, room: &str, content: &str, expiration: Option<u64>) -> Event {
        let mut tags = vec![Tag::identifier(room.to_string())];
        if let Some(exp) = expiration {
            tags.push(Tag::parse(["expiration", &exp.to_string()]).unwrap());
        }
        EventBuilder::new(Kind::from(9u16), content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign chat event")
    }

    #[tokio::test]
    async fn stores_and_gets_root_and_named() {
        let store = RelayStore::in_memory();
        let keys = Keys::generate();

        let root = build_test_site_with_keys(&keys, &[("/index.html", b"r")], None, None);
        let named = build_test_site_with_keys(&keys, &[("/index.html", b"n")], Some("blog"), None);

        assert!(store.store_event(root.manifest.clone()).await.unwrap());
        assert!(store.store_event(named.manifest.clone()).await.unwrap());

        let got_root = store
            .get_manifest(KIND_ROOT, &keys.public_key(), None)
            .await
            .unwrap();
        let got_named = store
            .get_manifest(KIND_NAMED, &keys.public_key(), Some("blog"))
            .await
            .unwrap();
        assert_eq!(got_root.map(|e| e.id), Some(root.manifest.id));
        assert_eq!(got_named.map(|e| e.id), Some(named.manifest.id));
        assert_eq!(store.count(), 2);
    }

    #[tokio::test]
    async fn regular_kinds_are_kept_by_id_not_replaced() {
        // Two chat messages from the same author must both survive (a manifest in
        // the same slot would replace — chat must not).
        let store = RelayStore::in_memory();
        let keys = Keys::generate();

        let m1 = chat_event(&keys, "mesh", "hello", None);
        let m2 = chat_event(&keys, "mesh", "world", None);
        assert!(store.store_event(m1.clone()).await.unwrap());
        assert!(store.store_event(m2.clone()).await.unwrap());

        let filter = ManifestFilter {
            kinds: vec![9],
            d_tags: vec!["mesh".to_string()],
            ..Default::default()
        };
        let got = store.query(&filter).await.unwrap();
        assert_eq!(got.len(), 2, "both chat messages retained");

        // Re-storing the same id is a no-op (dedup), not a second copy.
        assert!(!store.store_event(m1).await.unwrap());
        assert_eq!(store.query(&filter).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn expired_events_are_dropped_and_not_served() {
        let store = RelayStore::in_memory();
        let keys = Keys::generate();
        let now = now_secs();

        let live = chat_event(&keys, "mesh", "fresh", Some(now + 600));
        let dead = chat_event(&keys, "mesh", "stale", Some(now.saturating_sub(10)));

        assert!(store.store_event(live.clone()).await.unwrap());
        assert!(
            !store.store_event(dead).await.unwrap(),
            "an already-expired event is not stored"
        );

        let filter = ManifestFilter { kinds: vec![9], ..Default::default() };
        let got = store.query(&filter).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, live.id);
    }

    #[tokio::test]
    async fn chat_events_are_not_persisted_manifests_are() {
        let dir = tmp("persist-split");
        let _ = std::fs::remove_dir_all(&dir);
        let keys = Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"x")], None, None);
        let now = now_secs();

        {
            let store = RelayStore::open(&dir).unwrap();
            store.store_event(site.manifest.clone()).await.unwrap();
            store
                .store_event(chat_event(&keys, "mesh", "ephemeral", Some(now + 600)))
                .await
                .unwrap();
            assert_eq!(store.count(), 2, "both live in memory");
        }
        // Reopen: only the manifest survives; the expiring chat event was never
        // written to disk.
        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(store.count(), 1, "only the manifest persists");
        let got = store
            .get_manifest(KIND_ROOT, &keys.public_key(), None)
            .await
            .unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let dir = tmp("persist");
        let _ = std::fs::remove_dir_all(&dir);
        let keys = Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"x")], None, None);

        {
            let store = RelayStore::open(&dir).unwrap();
            store.store_event(site.manifest.clone()).await.unwrap();
        }
        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(store.count(), 1);
        let got = store
            .get_manifest(KIND_ROOT, &keys.public_key(), None)
            .await
            .unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));

        store.wipe().await.unwrap();
        assert_eq!(store.count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
