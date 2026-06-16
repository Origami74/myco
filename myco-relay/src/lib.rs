//! `myco-relay` — a generic embedded **Nostr relay**: a NIP-01 event store
//! implementing `nsite-deck`'s [`RelayBackend`] seam. A plain store + (P2 M2) a
//! `ws://localhost:4869` socket — no fanout of its own (the propagator is a
//! separate P3 process). See `docs/design/nsite-layer.md` §2.1.
//!
//! The query surface Myco uses is tiny (`{kinds, authors, #d}`), so this is a
//! hand-rolled store over the rust-`nostr` `Event` type rather than a relay
//! framework; negentropy/NIP-77 is **not** here (it lands in P3). Replaceable
//! semantics: keep only the newest event per `(kind, author)` for replaceable
//! kinds (e.g. 15128) and per `(kind, author, d-tag)` for addressable kinds
//! (e.g. 35128). Persistence is a single small JSON file rewritten on change —
//! the manifest set is a handful of events.
//!
//! [`RelayBackend`]: nsite_deck::seams::RelayBackend

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use nostr::{Event, PublicKey};
use nsite_deck::seams::{ManifestFilter, RelayBackend};

/// A replaceable-slot key: `(kind, author, d-tag)`. For replaceable kinds the
/// d-tag is `None`; for addressable kinds it is the `d` tag value.
type Slot = (u16, [u8; 32], Option<String>);

/// An embedded NIP-01 event store with replaceable semantics + JSON persistence.
pub struct RelayStore {
    path: Option<PathBuf>,
    events: Mutex<HashMap<Slot, Event>>,
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

        let mut map: HashMap<Slot, Event> = HashMap::new();
        if path.is_file() {
            let raw = std::fs::read(&path)?;
            match serde_json::from_slice::<Vec<Event>>(&raw) {
                Ok(events) => {
                    for event in events {
                        let slot = slot_of(&event);
                        // Apply replaceable dedup on load (newest wins).
                        match map.get(&slot) {
                            Some(existing) if existing.created_at >= event.created_at => {}
                            _ => {
                                map.insert(slot, event);
                            }
                        }
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

    /// Number of stored events (for diagnostics).
    pub fn count(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    /// Persist the current event set (full rewrite; the set is tiny). Caller must
    /// not hold the lock.
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

/// Whether a kind is addressable / parameterized-replaceable (one per
/// `(author, d-tag)`): range `30000..40000`. Kind 35128 is here.
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

fn slot_of(event: &Event) -> Slot {
    let kind = event.kind.as_u16();
    let d = if is_addressable(kind) {
        event_d_tag(event)
    } else {
        None
    };
    // Replaceable, addressable, and (for Myco's manifest-only use) any other kind
    // collapse to a (kind, author, d) slot. Myco stores only manifests, so this
    // is sufficient; a general social relay would key regular kinds by id.
    (kind, event.pubkey.to_bytes(), d)
}

#[async_trait]
impl RelayBackend for RelayStore {
    async fn store_event(&self, event: Event) -> anyhow::Result<bool> {
        let slot = slot_of(&event);
        let snapshot = {
            let mut map = self.events.lock().unwrap();
            let accept = match map.get(&slot) {
                Some(existing) => event.created_at > existing.created_at,
                None => true,
            };
            if !accept {
                return Ok(false);
            }
            map.insert(slot, event);
            map.values().cloned().collect::<Vec<_>>()
        };
        self.persist(&snapshot);
        Ok(true)
    }

    async fn get_manifest(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let slot = (kind, author.to_bytes(), d_tag.map(str::to_string));
        Ok(self.events.lock().unwrap().get(&slot).cloned())
    }

    async fn query(&self, filter: &ManifestFilter) -> anyhow::Result<Vec<Event>> {
        let map = self.events.lock().unwrap();
        let mut out: Vec<Event> = map
            .values()
            .filter(|e| filter.kinds.is_empty() || filter.kinds.contains(&e.kind.as_u16()))
            .filter(|e| filter.authors.is_empty() || filter.authors.contains(&e.pubkey))
            .filter(|e| {
                filter.d_tags.is_empty()
                    || event_d_tag(e).is_some_and(|d| filter.d_tags.contains(&d))
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use nsite_deck::model::{KIND_NAMED, KIND_ROOT};
    use nsite_deck::testing::build_test_site_with_keys;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-relay-test-{}-{}", std::process::id(), tag))
    }

    #[tokio::test]
    async fn stores_and_gets_root_and_named() {
        let store = RelayStore::in_memory();
        let keys = nostr::Keys::generate();

        let root = build_test_site_with_keys(&keys, &[("/index.html", b"r")], None, None);
        let named = build_test_site_with_keys(&keys, &[("/index.html", b"n")], Some("blog"), None);

        assert!(store.store_event(root.manifest.clone()).await.unwrap());
        assert!(store.store_event(named.manifest.clone()).await.unwrap());

        // Root + named live in distinct slots for the same author.
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
    async fn persists_across_reopen() {
        let dir = tmp("persist");
        let _ = std::fs::remove_dir_all(&dir);
        let keys = nostr::Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"x")], None, None);

        {
            let store = RelayStore::open(&dir).unwrap();
            store.store_event(site.manifest.clone()).await.unwrap();
        }
        // Reopen: the manifest is still queryable.
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
