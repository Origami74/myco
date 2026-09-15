//! `myco-relay` — a generic embedded **Nostr relay**: a NIP-01 event store
//! implementing `nsite-deck`'s [`RelayBackend`] seam. See
//! `docs/design/nsite/nsite-layer.md` §2.1 and `docs/design/core/event-gossip.md`.
//!
//! This crate holds no Myco concepts — no mesh, no ttl, no circles. The
//! WebSocket front door that applies those is `myco-core::mesh_relay`, which
//! keeps this store swappable for any other NIP-01 relay
//! (`reference/thinning-custom-relay.md`).
//!
//! Two kinds of event live here, in two places:
//!
//! - **Durable** events — manifests, a user's replaceable kinds, notes a
//!   napplet published or pulled — live in an **LMDB** database
//!   ([`nostr_lmdb`], rust-nostr's store). It indexes by id, kind, author,
//!   `d` tag, tags and time, so a query is an index walk rather than a scan
//!   of everything held; it applies NIP-01 replaceable / addressable
//!   semantics and NIP-09 deletions itself; and it is an mmap, so a read is a
//!   page-cache hit and an idle store costs nothing. Every write is one small
//!   ACID transaction — never a rewrite of the whole set, which is what the
//!   JSON file this replaced did on every event.
//! - **Expiring** events (a NIP-40 `expiration` tag — chat, pair traffic;
//!   `docs/design/core/event-gossip.md` §5) stay **in memory**, GC'd on
//!   expiry and never written to disk. That is a design property, not a
//!   shortcut: a conversation in the room is not a record on the phone.
//!
//! Ephemeral kinds (20000–29999) are stored by neither, as NIP-01 says; the
//! front door still delivers them live.
//!
//! [`RelayBackend`]: nsite_deck::seams::RelayBackend

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::FutureExt;
use nostr::filter::MatchEventOptions;
use nostr::{Event, Filter};
use nostr_database::{NostrDatabase, SaveEventStatus};
use nostr_lmdb::NostrLMDB;
use nsite_deck::seams::{AdminBackend, RelayBackend};

/// The LMDB map size: an upper bound on the file, reserved as address space
/// and grown as sparse pages, not allocated. 1 GiB is far beyond what a
/// phone's relay should hold and small enough that a 64-bit Android process
/// reserves it without complaint.
const MAP_SIZE: usize = 1024 * 1024 * 1024;

/// The JSON file the store persisted to before LMDB. Read once and renamed
/// aside on the first open that finds it; never written again.
const LEGACY_FILE: &str = "events.json";

/// An embedded NIP-01 event store: durable events in LMDB, expiring events in
/// memory. See the crate docs for why the two are apart.
pub struct RelayStore {
    /// Durable events. `None` only for a store that failed to open, which
    /// `open` refuses to construct — so always present in practice; the
    /// `Option` is what lets `Drop` on a test store remove its directory.
    db: NostrLMDB,
    /// Where the LMDB lives, for `wipe` and diagnostics.
    dir: PathBuf,
    /// Expiring events, by id. Memory-only by design.
    expiring: Mutex<HashMap<[u8; 32], Event>>,
    /// Events read from a pre-LMDB `events.json`, waiting to be saved on the
    /// first async call. `open` is synchronous and the LMDB save is not, so
    /// the file is read there and drained here; it is moved aside once
    /// everything in it is in the database.
    pending_legacy: Mutex<Vec<Event>>,
    /// A store made by [`RelayStore::in_memory`] removes its directory when
    /// dropped: it exists to be as good as no persistence for tests.
    scratch: bool,
}

impl RelayStore {
    /// A throwaway store — for tests. LMDB has no memory-only mode, so this is
    /// a fresh directory under the temp dir, removed when the store drops.
    pub fn in_memory() -> Self {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "myco-relay-scratch-{}-{}-{n}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut store = Self::open(&dir).expect("open a scratch relay store");
        store.scratch = true;
        store
    }

    /// Open the store under `dir`: the LMDB at `<dir>/lmdb`, and any events a
    /// pre-LMDB build left in `<dir>/events.json` migrated in on the way.
    pub fn open(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let lmdb_dir = dir.join("lmdb");
        std::fs::create_dir_all(&lmdb_dir)?;
        let db = NostrLMDB::builder(&lmdb_dir)
            .map_size(MAP_SIZE)
            .build()
            .map_err(|e| anyhow::anyhow!("open relay store at {}: {e}", lmdb_dir.display()))?;
        let pending_legacy = read_legacy_file(&dir.join(LEGACY_FILE));
        Ok(Self {
            db,
            dir,
            expiring: Mutex::new(HashMap::new()),
            pending_legacy: Mutex::new(pending_legacy),
            scratch: false,
        })
    }

    /// Save whatever `open` read from a pre-LMDB `events.json`, then move the
    /// file aside so it is read once. Runs at the start of every async entry
    /// point and is a no-op after the first.
    async fn flush_legacy(&self) {
        let pending = std::mem::take(&mut *self.pending_legacy.lock().unwrap());
        if pending.is_empty() {
            return;
        }
        let mut moved = 0usize;
        for event in pending {
            if matches!(
                self.db.save_event(&event).await,
                Ok(SaveEventStatus::Success)
            ) {
                moved += 1;
            }
        }
        let path = self.dir.join(LEGACY_FILE);
        if let Err(e) = std::fs::rename(&path, path.with_extension("json.migrated")) {
            tracing::warn!(error = %e, "relay store: migrated events.json but could not move it aside");
        }
        tracing::info!(moved, "relay store: migrated legacy events.json into LMDB");
    }

    /// Number of stored (non-expired) events (for diagnostics).
    pub fn count(&self) -> usize {
        let now = now_secs();
        let pending = self.pending_legacy.lock().unwrap().len();
        let durable = self
            .db
            .count(Filter::new())
            // The LMDB count is a read transaction with no await inside, so
            // the future is ready the moment it is made; `now_or_never` is a
            // poll, not a block.
            .now_or_never()
            .and_then(|r| r.ok())
            .unwrap_or(0);
        let live = self
            .expiring
            .lock()
            .unwrap()
            .values()
            .filter(|e| !is_expired(e, now))
            .count();
        durable + live + pending
    }

    /// Drop every stored event whose id is **not** in `keep`. Used by the
    /// selective cache wipe to retain the events backing pinned apps while
    /// clearing everything else (chat included).
    pub async fn retain_events(&self, keep: &HashSet<[u8; 32]>) {
        self.flush_legacy().await;
        self.expiring
            .lock()
            .unwrap()
            .retain(|id, _| keep.contains(id));
        let all = match self.db.query(Filter::new()).await {
            Ok(events) => events,
            Err(e) => {
                tracing::error!(error = %e, "relay store: retain could not list events");
                return;
            }
        };
        let drop: Vec<nostr::EventId> = all
            .into_iter()
            .filter(|e| !keep.contains(&e.id.to_bytes()))
            .map(|e| e.id)
            .collect();
        if drop.is_empty() {
            return;
        }
        // Deleting by id filter; chunked so one filter never carries thousands
        // of ids.
        for chunk in drop.chunks(256) {
            if let Err(e) = self
                .db
                .delete(Filter::new().ids(chunk.iter().copied()))
                .await
            {
                tracing::error!(error = %e, "relay store: retain could not delete");
            }
        }
    }

    /// Store an event, reporting whether it was **new** to this store — `false`
    /// for a duplicate id, an event already superseded in its replaceable slot,
    /// one already expired, or an ephemeral kind (delivered, never stored).
    ///
    /// This answer describes *this store*, and nothing else should be built on
    /// it. In particular it is not a mesh loop-guard: an id GC'd at NIP-40 expiry
    /// looks new again on a later pull, so gossip novelty is the proxy's own
    /// seen-set instead (`reference/thinning-custom-relay.md`, D2). The
    /// [`RelayBackend`] seam therefore does not expose it — an arbitrary NIP-01
    /// relay could not answer it anyway.
    pub async fn admit_event(&self, event: Event) -> anyhow::Result<bool> {
        self.flush_legacy().await;
        let now = now_secs();
        if is_expired(&event, now) || event.kind.is_ephemeral() {
            return Ok(false);
        }
        if expiration(&event).is_some() {
            let mut map = self.expiring.lock().unwrap();
            // Opportunistic GC: drop anything that has expired since last touch.
            map.retain(|_, e| !is_expired(e, now));
            return Ok(admit_expiring(&mut map, event));
        }
        match self
            .db
            .save_event(&event)
            .await
            .map_err(|e| anyhow::anyhow!("relay store: save failed: {e}"))?
        {
            SaveEventStatus::Success => Ok(true),
            SaveEventStatus::Rejected(_) => Ok(false),
        }
    }
}

impl Drop for RelayStore {
    fn drop(&mut self) {
        if self.scratch {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// The still-live, storable events of a pre-LMDB `events.json`, or nothing.
/// A file that will not parse is left where it is and logged.
fn read_legacy_file(path: &Path) -> Vec<Event> {
    if !path.is_file() {
        return Vec::new();
    }
    let events: Vec<Event> = match std::fs::read(path)
        .map_err(anyhow::Error::from)
        .and_then(|raw| serde_json::from_slice(&raw).map_err(anyhow::Error::from))
    {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(error = %e, "relay store: cannot read legacy events.json; leaving it");
            return Vec::new();
        }
    };
    let now = now_secs();
    events
        .into_iter()
        .filter(|e| !is_expired(e, now) && !e.kind.is_ephemeral())
        .collect()
}

/// Admit an expiring event into the in-memory map, applying replaceable /
/// addressable dedup (newest wins) and skipping a stale duplicate. Returns
/// `true` if the event is now stored as new.
fn admit_expiring(map: &mut HashMap<[u8; 32], Event>, event: Event) -> bool {
    let kind = event.kind.as_u16();
    if is_replaceable(kind) || is_addressable(kind) {
        let slot = slot_of(&event);
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
        if map.contains_key(&event.id.to_bytes()) {
            return false;
        }
        map.insert(event.id.to_bytes(), event);
        true
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
/// with `d` only for addressable kinds.
fn slot_of(event: &Event) -> (u16, [u8; 32], Option<String>) {
    let kind = event.kind.as_u16();
    let d = if is_addressable(kind) {
        event_d_tag(event)
    } else {
        None
    };
    (kind, event.pubkey.to_bytes(), d)
}

/// The NIP-40 `expiration` tag value (a unix timestamp), if present. Public so
/// the proxy in front can size its seen-set to how long an event can still be
/// circulating.
pub fn expiration(event: &Event) -> Option<u64> {
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

/// Does an event satisfy a NIP-01 filter? Used by the in-memory half of the
/// store and by the WebSocket front door for live-subscription matching.
pub fn matches_filter(event: &Event, filter: &Filter) -> bool {
    filter.match_event(event, MatchEventOptions::new())
}

#[async_trait]
impl RelayBackend for RelayStore {
    async fn publish(&self, event: Event) -> anyhow::Result<()> {
        self.admit_event(event).await.map(|_| ())
    }

    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        self.flush_legacy().await;
        let now = now_secs();
        let mut out: Vec<Event> = Vec::new();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        // Several filters are an any-match, as a multi-filter REQ is: each is
        // run against the index on its own (with its own limit), and the
        // results are merged by id.
        for filter in filters {
            let durable = self
                .db
                .query(filter.clone())
                .await
                .map_err(|e| anyhow::anyhow!("relay store: query failed: {e}"))?;
            for event in durable {
                if seen.insert(event.id.to_bytes()) {
                    out.push(event);
                }
            }
        }
        {
            let map = self.expiring.lock().unwrap();
            for event in map.values() {
                if is_expired(event, now) || seen.contains(&event.id.to_bytes()) {
                    continue;
                }
                if filters.iter().any(|f| matches_filter(event, f)) {
                    seen.insert(event.id.to_bytes());
                    out.push(event.clone());
                }
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        // Honour the smallest limit any filter asked for, matching how a relay
        // caps a multi-filter REQ.
        if let Some(limit) = filters.iter().filter_map(|f| f.limit).min() {
            out.truncate(limit);
        }
        Ok(out)
    }
}

#[async_trait]
impl AdminBackend for RelayStore {
    async fn wipe(&self) -> anyhow::Result<()> {
        self.pending_legacy.lock().unwrap().clear();
        let _ = std::fs::remove_file(self.dir.join(LEGACY_FILE));
        self.expiring.lock().unwrap().clear();
        self.db
            .wipe()
            .await
            .map_err(|e| anyhow::anyhow!("relay store: wipe failed: {e}"))
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

        assert!(store.admit_event(root.manifest.clone()).await.unwrap());
        assert!(store.admit_event(named.manifest.clone()).await.unwrap());

        let got_root =
            nsite_deck::seams::newest_in_slot(&store, KIND_ROOT, &keys.public_key(), None)
                .await
                .unwrap();
        let got_named =
            nsite_deck::seams::newest_in_slot(&store, KIND_NAMED, &keys.public_key(), Some("blog"))
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
        assert!(store.admit_event(m1.clone()).await.unwrap());
        assert!(store.admit_event(m2.clone()).await.unwrap());

        let filter = Filter::new().kind(Kind::from(9u16)).identifier("mesh");
        let got = store.query(std::slice::from_ref(&filter)).await.unwrap();
        assert_eq!(got.len(), 2, "both chat messages retained");

        // Re-storing the same id is a no-op (dedup), not a second copy.
        assert!(!store.admit_event(m1).await.unwrap());
        assert_eq!(
            store
                .query(std::slice::from_ref(&filter))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// The store answers the whole NIP-01 filter surface, not a hand-rolled
    /// subset of it.
    ///
    /// The old query took `{kinds, authors, #d, limit}` and silently ignored
    /// everything else, so a `since`/`until` window or an id lookup matched far
    /// more than the caller asked for. A backend swapped in behind this seam
    /// would have honoured them, so the two would have disagreed.
    #[tokio::test]
    async fn the_full_filter_surface_is_honoured() {
        let store = RelayStore::in_memory();
        let keys = Keys::generate();

        let old = EventBuilder::new(Kind::from(9u16), "old")
            .custom_created_at(nostr::Timestamp::from(1_000))
            .sign_with_keys(&keys)
            .unwrap();
        let recent = EventBuilder::new(Kind::from(9u16), "recent")
            .custom_created_at(nostr::Timestamp::from(9_000))
            .sign_with_keys(&keys)
            .unwrap();
        store.admit_event(old.clone()).await.unwrap();
        store.admit_event(recent.clone()).await.unwrap();

        // A time window: previously ignored, so both would have come back.
        let windowed = store
            .query(&[Filter::new().since(nostr::Timestamp::from(5_000))])
            .await
            .unwrap();
        assert_eq!(windowed.len(), 1, "since must exclude the older event");
        assert_eq!(windowed[0].id, recent.id);

        // An id lookup, likewise.
        let by_id = store.query(&[Filter::new().id(old.id)]).await.unwrap();
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].id, old.id);

        // Several filters are an any-match, the way a multi-filter REQ behaves.
        let either = store
            .query(&[
                Filter::new().id(old.id),
                Filter::new().since(nostr::Timestamp::from(5_000)),
            ])
            .await
            .unwrap();
        assert_eq!(either.len(), 2);
    }

    #[tokio::test]
    async fn expired_events_are_dropped_and_not_served() {
        let store = RelayStore::in_memory();
        let keys = Keys::generate();
        let now = now_secs();

        let live = chat_event(&keys, "mesh", "fresh", Some(now + 600));
        let dead = chat_event(&keys, "mesh", "stale", Some(now.saturating_sub(10)));

        assert!(store.admit_event(live.clone()).await.unwrap());
        assert!(
            !store.admit_event(dead).await.unwrap(),
            "an already-expired event is not stored"
        );

        let filter = Filter::new().kind(Kind::from(9u16));
        let got = store.query(std::slice::from_ref(&filter)).await.unwrap();
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
            store.admit_event(site.manifest.clone()).await.unwrap();
            store
                .admit_event(chat_event(&keys, "mesh", "ephemeral", Some(now + 600)))
                .await
                .unwrap();
            assert_eq!(store.count(), 2, "both live in memory");
        }
        // Reopen: only the manifest survives; the expiring chat event was never
        // written to disk.
        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(store.count(), 1, "only the manifest persists");
        let got = nsite_deck::seams::newest_in_slot(&store, KIND_ROOT, &keys.public_key(), None)
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
            store.admit_event(site.manifest.clone()).await.unwrap();
        }
        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(store.count(), 1);
        let got = nsite_deck::seams::newest_in_slot(&store, KIND_ROOT, &keys.public_key(), None)
            .await
            .unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));

        store.wipe().await.unwrap();
        assert_eq!(store.count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store written by a pre-LMDB build is brought in on the first open:
    /// its manifests are served, and the file is moved aside so it is read
    /// once.
    #[tokio::test]
    async fn a_legacy_events_json_is_migrated_once() {
        let dir = tmp("legacy-migrate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keys = Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"x")], None, None);
        let expired = chat_event(&keys, "mesh", "old", Some(1));
        std::fs::write(
            dir.join(LEGACY_FILE),
            serde_json::to_vec(&vec![site.manifest.clone(), expired]).unwrap(),
        )
        .unwrap();

        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(
            store.count(),
            1,
            "the manifest came across, the expired chat did not"
        );
        let got = nsite_deck::seams::newest_in_slot(&store, KIND_ROOT, &keys.public_key(), None)
            .await
            .unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));
        assert_eq!(store.count(), 1, "counted twice across the migration");
        assert!(
            !dir.join(LEGACY_FILE).exists(),
            "the legacy file was left in place"
        );
        assert!(dir.join("events.json.migrated").exists());
        drop(store);

        // Reopening finds the LMDB, not the file — nothing to migrate again.
        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(store.count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Durable regular events — a napplet's note — persist across reopen now
    /// that the store is a database rather than a rewritten file, and a NIP-09
    /// deletion by the author removes one.
    #[tokio::test]
    async fn notes_persist_and_deletions_apply() {
        let dir = tmp("notes-persist");
        let _ = std::fs::remove_dir_all(&dir);
        let keys = Keys::generate();
        let note = EventBuilder::text_note("kept")
            .sign_with_keys(&keys)
            .unwrap();
        {
            let store = RelayStore::open(&dir).unwrap();
            assert!(store.admit_event(note.clone()).await.unwrap());
        }
        let store = RelayStore::open(&dir).unwrap();
        assert_eq!(store.count(), 1, "a durable note did not survive reopen");

        let deletion =
            EventBuilder::delete(nostr::nips::nip09::EventDeletionRequest::new().id(note.id))
                .sign_with_keys(&keys)
                .unwrap();
        store.admit_event(deletion).await.unwrap();
        let left = store.query(&[Filter::new().id(note.id)]).await.unwrap();
        assert!(left.is_empty(), "the author's deletion was not applied");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An ephemeral kind is neither stored nor an error: the front door still
    /// delivers it live, the store simply has nothing to keep.
    #[tokio::test]
    async fn ephemeral_kinds_are_not_stored() {
        let store = RelayStore::in_memory();
        let keys = Keys::generate();
        let ping = EventBuilder::new(Kind::from(20_666u16), "ding")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(!store.admit_event(ping).await.unwrap());
        assert_eq!(store.count(), 0);
    }
}
