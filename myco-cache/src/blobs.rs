//! The blob cache: a second content-addressed directory (the same
//! [`FsBlobStore`] the local Blossom uses — hash-verified, atomic writes)
//! behind a segmented-LRU index that holds it to a byte budget.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use myco_blossom::FsBlobStore;
use nsite_deck::seams::BlobStore;

use crate::slru::{write_snapshot, Slru};
use crate::CacheStats;

/// The index snapshot, beside the blob directory.
const SNAPSHOT: &str = "index.bin";

/// A blob larger than this share of the budget is not cached: one such blob
/// would push out a large part of everything else for a single use.
const MAX_SHARE_DIVISOR: u64 = 8;

/// A bounded, evicting blob store. See the module docs.
pub struct BlobCache {
    store: Arc<FsBlobStore>,
    dir: PathBuf,
    index: Mutex<Slru>,
    scratch: bool,
}

impl BlobCache {
    /// Open the cache under `dir` (blobs in `<dir>/blobs`) with a budget of
    /// `limit` bytes. As with the event cache, nothing is read here;
    /// [`BlobCache::startup`] loads the index and repairs it against the
    /// directory.
    pub fn open(dir: impl AsRef<Path>, limit: u64) -> anyhow::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let store = Arc::new(FsBlobStore::open(dir.join("blobs"))?);
        Ok(Self {
            store,
            dir,
            index: Mutex::new(Slru::new(limit)),
            scratch: false,
        })
    }

    /// A throwaway cache under the temp dir, removed on drop — for tests.
    pub fn scratch(limit: u64) -> Self {
        let mut cache = Self::open(crate::scratch_dir("blobs"), limit).expect("open scratch cache");
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
    pub fn set_limit(&self, limit: u64) {
        self.index.lock().unwrap().set_limit(limit);
        self.evict();
    }

    /// Whether the index holds `sha256_hex` — no disk access.
    pub fn contains(&self, sha256_hex: &str) -> bool {
        parse_key(sha256_hex).is_some_and(|k| self.index.lock().unwrap().contains(&k))
    }

    /// Remove blobs from the cache — they are now in the local Blossom.
    pub fn remove(&self, hashes: &[String]) {
        for hash in hashes {
            let Some(key) = parse_key(hash) else {
                continue;
            };
            if self.index.lock().unwrap().remove(&key).is_some() {
                self.store.remove(hash);
            }
        }
    }

    /// Evict down to the budget.
    pub fn evict(&self) {
        let victims = self.index.lock().unwrap().evict_over_limit();
        if !victims.is_empty() {
            tracing::debug!(evicted = victims.len(), "blob cache: evicted");
        }
        for key in victims {
            self.store.remove(&hex::encode(key));
        }
    }

    /// Drop everything ("Clear cache"). Files are deleted, so the disk is
    /// freed at once.
    pub async fn clear(&self) -> anyhow::Result<()> {
        self.index.lock().unwrap().clear();
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            for (hash, _, _) in store.list() {
                store.remove(&hash);
            }
        })
        .await?;
        self.save_snapshot().await;
        Ok(())
    }

    /// Persist the index if it changed since the last save.
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
            tracing::warn!("blob cache: could not save the index");
        }
    }

    /// Once per launch, in the background: read the snapshot (folding in
    /// anything cached since open), then bring it in step with the directory
    /// — forget what is not on disk, index what is (oldest file first).
    pub async fn startup(&self) {
        let path = self.dir.join(SNAPSHOT);
        let limit = self.index.lock().unwrap().limit();
        let store = self.store.clone();
        let Ok((loaded, mut on_disk)) =
            tokio::task::spawn_blocking(move || (Slru::load(&path, limit), store.list())).await
        else {
            return;
        };
        let (forgot, added) = {
            let mut index = self.index.lock().unwrap();
            if let Some(mut snapshot) = loaded {
                snapshot.absorb(&index);
                *index = snapshot;
            }
            let present: std::collections::HashSet<[u8; 32]> = on_disk
                .iter()
                .filter_map(|(h, _, _)| parse_key(h))
                .collect();
            let stale: Vec<[u8; 32]> = index
                .keys()
                .into_iter()
                .filter(|k| !present.contains(k))
                .collect();
            for key in &stale {
                index.remove(key);
            }
            on_disk.sort_by_key(|(_, _, modified)| *modified);
            let mut added = 0usize;
            for (hash, size, _) in &on_disk {
                if let Some(key) = parse_key(hash) {
                    if index.insert(key, *size, None) {
                        added += 1;
                    }
                }
            }
            (stale.len(), added)
        };
        if forgot + added > 0 {
            tracing::info!(forgot, indexed = added, "blob cache: index reconciled");
        }
        self.evict();
        self.save_snapshot().await;
    }
}

impl Drop for BlobCache {
    fn drop(&mut self) {
        if self.scratch {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[async_trait]
impl BlobStore for BlobCache {
    async fn has(&self, sha256_hex: &str) -> bool {
        self.contains(sha256_hex) && self.store.has(sha256_hex).await
    }

    /// A hit counts as an access.
    async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.contains(sha256_hex) {
            return Ok(None);
        }
        let Some(key) = parse_key(sha256_hex) else {
            return Ok(None);
        };
        let bytes = self.store.get(sha256_hex).await?;
        let mut index = self.index.lock().unwrap();
        match &bytes {
            Some(_) => {
                index.touch(&key);
            }
            None => {
                index.remove(&key);
            }
        }
        Ok(bytes)
    }

    async fn size(&self, sha256_hex: &str) -> anyhow::Result<Option<u64>> {
        if !self.contains(sha256_hex) {
            return Ok(None);
        }
        self.store.size(sha256_hex).await
    }

    /// Stores and indexes `bytes`, unless they are too large a share of the
    /// budget to be worth it — then only the hash is returned, as a store
    /// that already had them would.
    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let limit = self.index.lock().unwrap().limit();
        if bytes.len() as u64 > limit / MAX_SHARE_DIVISOR {
            return Ok(sha256_hex(bytes));
        }
        let hash = self.store.put(bytes).await?;
        let fresh = parse_key(&hash).is_some_and(|k| {
            self.index
                .lock()
                .unwrap()
                .insert(k, bytes.len() as u64, None)
        });
        if fresh {
            self.evict();
        }
        Ok(hash)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        self.clear().await
    }
}

/// A 64-hex sha256 as index key; `None` for anything else.
fn parse_key(sha256_hex: &str) -> Option<[u8; 32]> {
    let mut key = [0u8; 32];
    hex::decode_to_slice(sha256_hex.to_ascii_lowercase(), &mut key).ok()?;
    Some(key)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_and_evict_keep_what_was_used() {
        let cache = BlobCache::scratch(8_000);
        let used = cache.put(&[1u8; 1_000]).await.unwrap();
        assert!(cache.get(&used).await.unwrap().is_some());
        assert!(cache.get(&used).await.unwrap().is_some());
        for n in 2..30u8 {
            let other = cache.put(&[n; 1_000]).await.unwrap();
            cache.get(&other).await.unwrap();
        }
        assert!(cache.stats().bytes <= 8_000);
        assert!(
            cache.has(&used).await,
            "a twice-used blob was evicted by a scan"
        );
    }

    #[tokio::test]
    async fn an_oversized_blob_is_not_cached() {
        let cache = BlobCache::scratch(8_000);
        let small = cache.put(&[1u8; 500]).await.unwrap();
        let big = cache.put(&[2u8; 2_000]).await.unwrap();
        assert!(!cache.has(&big).await);
        assert!(cache.has(&small).await, "a big blob pushed out the rest");
    }

    #[tokio::test]
    async fn remove_clear_and_startup() {
        let dir = crate::scratch_dir("blob-reconcile");
        let cache = BlobCache::open(&dir, 1_000_000).unwrap();
        let a = cache.put(b"a").await.unwrap();
        let b = cache.put(b"b").await.unwrap();
        cache.remove(std::slice::from_ref(&a));
        assert!(!cache.has(&a).await);
        assert!(cache.has(&b).await);

        // Index lost (no snapshot saved): startup finds the file.
        *cache.index.lock().unwrap() = Slru::new(1_000_000);
        cache.startup().await;
        assert!(cache.contains(&b));
        assert_eq!(cache.stats().count, 1);

        cache.clear().await.unwrap();
        assert_eq!(cache.stats().count, 0);
        assert!(!cache.has(&b).await);
        assert!(cache.store.list().is_empty(), "clear left files behind");
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
