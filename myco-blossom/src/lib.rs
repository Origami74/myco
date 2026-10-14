//! `myco-blossom` — a generic embedded **Blossom blob store**: a content-
//! addressed filesystem store keyed by sha256, implementing `nsite-deck`'s
//! [`BlobStore`] seam. See `docs/design/nsite/nsite-layer.md` §2.2.
//!
//! This is currently the **only** implementation of that seam, and the default.
//! A second one that reads through to an external Blossom server is planned but
//! **not built** (`reference/thinning-custom-relay.md`, D9); the read fast path
//! below is written on the assumption that this store owns its own directory,
//! and that assumption is what a shared store would break.
//!
//! Storage shape (matching the Go reference's `blobs/` dir): each blob is a file
//! named by its lowercase-hex sha256 under `root/`. Blobs are immutable and
//! self-authenticating — the hash *is* the identity — so the store verifies
//! `sha256(bytes) == name` **on write** and writes atomically (temp + rename).
//! Reads return the named file directly: the only way bytes enter is a verified
//! write, so re-hashing on every gateway request would be wasted work.
//!
//! The `http://localhost:24243` BUD-01 HTTP server (GET/PUT/HEAD) is bound by
//! `myco-core` over the shared Tokio runtime (P2 M2); this crate is the store.
//!
//! [`BlobStore`]: nsite_deck::seams::BlobStore

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use async_trait::async_trait;
use nsite_deck::seams::BlobStore;
use sha2::{Digest, Sha256};

pub mod server;

/// A content-addressed blob store rooted at a directory.
pub struct FsBlobStore {
    root: PathBuf,
    /// Running `(count, bytes)` figures, so the app's once-a-second state poll
    /// (which holds the reducer lock) never walks the blob dir. See [`Stats`].
    stats: Arc<StatsCell>,
}

/// How many blobs a store holds and how many bytes they take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlobStats {
    pub count: usize,
    pub bytes: u64,
}

/// The figures behind [`FsBlobStore::try_stats`].
///
/// They are learned once by walking the directory (off the caller's thread,
/// unless a caller asks to wait) and from then on kept up to date by every
/// mutation: `put` of a new blob adds it, `remove` subtracts it, `wipe` and
/// `retain_blobs` set them outright from what they kept. A mutation that
/// lands while the walk runs is recorded in `pending` and folded in when it
/// ends, so the walk racing a sync does not count a blob twice or miss it.
///
/// The figures are diagnostics and may drift slightly: a `put` racing a
/// `wipe`/`retain_blobs`, or a file changed behind the store's back, is not
/// reflected until the next launch re-walks the directory.
#[derive(Default)]
struct Stats {
    known: Option<BlobStats>,
    scanning: bool,
    /// Bumped by `wipe`/`retain_blobs`: a walk that started before one of
    /// them is stale and its result is dropped.
    generation: u64,
    /// While a walk runs: each blob mutated meanwhile, and its size after the
    /// last mutation (`None` = removed).
    pending: HashMap<String, Option<u64>>,
}

#[derive(Default)]
struct StatsCell {
    state: Mutex<Stats>,
    done: Condvar,
}

impl StatsCell {
    /// Learn the figures by walking `root`, unless they are already known; if
    /// a walk is already running, wait for it instead.
    fn scan(&self, root: &Path) {
        let generation = {
            let mut st = self.state.lock().unwrap();
            loop {
                if st.known.is_some() {
                    return;
                }
                if !st.scanning {
                    break;
                }
                st = self.done.wait(st).unwrap();
            }
            st.claim()
        };
        self.walk(root, generation);
    }

    /// The walk itself, claimed by [`Stats::claim`]. Runs without the lock.
    fn walk(&self, root: &Path, generation: u64) {
        let mut found: HashMap<String, u64> = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(root) {
            for entry in rd.filter_map(Result::ok).filter(is_blob_name) {
                if let Some(name) = entry.file_name().to_str() {
                    let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    found.insert(name.to_ascii_lowercase(), len);
                }
            }
        }
        let mut st = self.state.lock().unwrap();
        st.scanning = false;
        let pending = std::mem::take(&mut st.pending);
        // A `wipe`/`retain_blobs` meanwhile already set the figures outright.
        if st.generation == generation && st.known.is_none() {
            for (hash, after) in pending {
                match after {
                    Some(len) => found.insert(hash, len),
                    None => found.remove(&hash),
                };
            }
            st.known = Some(BlobStats {
                count: found.len(),
                bytes: found.values().sum(),
            });
        }
        self.done.notify_all();
    }
}

impl Stats {
    /// Mark a walk as running; returns the generation it reads.
    fn claim(&mut self) -> u64 {
        self.scanning = true;
        self.pending.clear();
        self.generation
    }

    /// A blob that was not there is now stored with `len` bytes.
    fn added(&mut self, hash: &str, len: u64) {
        if let Some(k) = &mut self.known {
            k.count += 1;
            k.bytes += len;
        }
        if self.scanning {
            self.pending.insert(hash.to_string(), Some(len));
        }
    }

    /// A blob of `len` bytes is gone.
    fn removed(&mut self, hash: &str, len: u64) {
        if let Some(k) = &mut self.known {
            k.count = k.count.saturating_sub(1);
            k.bytes = k.bytes.saturating_sub(len);
        }
        if self.scanning {
            self.pending.insert(hash.to_string(), None);
        }
    }

    /// A bulk operation measured what is left: take that, outright.
    fn reset(&mut self, to: BlobStats) {
        self.known = Some(to);
        self.generation += 1;
        self.pending.clear();
    }
}

impl FsBlobStore {
    /// Open (creating if needed) a blob store rooted at `root`. Nothing is
    /// read here; the figures are learned on first ask.
    pub fn open(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            stats: Arc::default(),
        })
    }

    fn blob_path(&self, sha256_hex: &str) -> PathBuf {
        self.root.join(sha256_hex)
    }

    /// The store's figures **without touching the disk** — safe under a lock
    /// the UI waits on. `None` until they are first known; the first call
    /// starts a background walk of the directory to learn them.
    pub fn try_stats(&self) -> Option<BlobStats> {
        let mut st = self.stats.state.lock().unwrap();
        if st.known.is_none() && !st.scanning {
            // Claimed under the lock, so one walk at most is ever running.
            let generation = st.claim();
            let cell = self.stats.clone();
            let root = self.root.clone();
            let spawned = std::thread::Builder::new()
                .name("blob-stats".into())
                .spawn(move || cell.walk(&root, generation));
            if spawned.is_err() {
                // Release the claim; the next ask tries again.
                st.scanning = false;
                tracing::warn!("blob store: could not start the stats walk");
            }
        }
        st.known
    }

    /// The store's figures, walking the directory (on this thread) if they are
    /// not known yet. Never call it where a slow disk would stall the UI —
    /// that is what [`FsBlobStore::try_stats`] is for.
    pub fn stats(&self) -> BlobStats {
        self.stats.scan(&self.root);
        self.stats.state.lock().unwrap().known.unwrap_or_default()
    }

    /// Number of stored blobs. May walk the directory: see [`FsBlobStore::stats`].
    pub fn count(&self) -> usize {
        self.stats().count
    }

    /// Total bytes on disk. May walk the directory: see [`FsBlobStore::stats`].
    pub fn total_bytes(&self) -> u64 {
        self.stats().bytes
    }

    /// Delete every blob whose hash is **not** in `keep`. Used by the selective
    /// cache wipe to retain the blobs backing pinned nsites while clearing the rest.
    /// `keep` holds lowercase-hex sha256 names (the manifest's `path -> hash` values).
    pub fn retain_blobs(&self, keep: &std::collections::HashSet<String>) {
        let mut left = BlobStats::default();
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for entry in rd.filter_map(Result::ok).filter(is_blob_name) {
                let kept = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| keep.contains(name));
                if !kept && std::fs::remove_file(entry.path()).is_ok() {
                    continue;
                }
                left.count += 1;
                left.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        self.stats.state.lock().unwrap().reset(left);
    }

    /// Delete one blob. Returns whether it was there.
    pub fn remove(&self, sha256_hex: &str) -> bool {
        let hash = sha256_hex.to_ascii_lowercase();
        let path = self.blob_path(&hash);
        // Under the stats lock, so the size read and the delete are one step
        // for the accounting (a racing `put` of the same hash cannot slip in).
        let mut st = self.stats.state.lock().unwrap();
        let tracked = st.known.is_some() || st.scanning;
        let len = if tracked {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let removed = std::fs::remove_file(&path).is_ok();
        if removed {
            st.removed(&hash, len);
        }
        removed
    }

    /// Every stored blob as `(sha256 hex, bytes, modified)`, for a cache
    /// rebuilding its index from the directory.
    pub fn list(&self) -> Vec<(String, u64, std::time::SystemTime)> {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        rd.filter_map(Result::ok)
            .filter(is_blob_name)
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                let name = entry.file_name().to_str()?.to_string();
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                Some((name, meta.len(), modified))
            })
            .collect()
    }
}

fn is_blob_name(entry: &std::fs::DirEntry) -> bool {
    // Blob files are 64-char lowercase hex; skip temp files (`.tmp-*`) and dirs.
    entry
        .file_name()
        .to_str()
        .map(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn has(&self, sha256_hex: &str) -> bool {
        self.blob_path(&sha256_hex.to_ascii_lowercase()).is_file()
    }

    async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.blob_path(&sha256_hex.to_ascii_lowercase());
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// From the file's metadata: a size question must not cost a read of
    /// the whole blob, which is exactly what a caller with a cap asks it to
    /// avoid.
    async fn size(&self, sha256_hex: &str) -> anyhow::Result<Option<u64>> {
        let path = self.blob_path(&sha256_hex.to_ascii_lowercase());
        match std::fs::metadata(&path) {
            Ok(meta) => Ok(Some(meta.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let hash = sha256_hex(bytes);
        let dest = self.blob_path(&hash);
        if dest.is_file() {
            return Ok(hash); // immutable + content-addressed → already stored
        }
        // Atomic write: a unique temp file (pid + hash) then rename into place.
        let tmp = self
            .root
            .join(format!(".tmp-{}-{}", std::process::id(), hash));
        std::fs::write(&tmp, bytes)?;
        // The existence check and the rename are one step for the accounting,
        // so two racing puts of the same new blob count it once.
        let mut st = self.stats.state.lock().unwrap();
        let existed = dest.is_file();
        std::fs::rename(&tmp, &dest)?;
        if !existed {
            st.added(&hash, bytes.len() as u64);
        }
        Ok(hash)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(&self.root)?.filter_map(Result::ok) {
            let _ = std::fs::remove_file(entry.path());
        }
        self.stats.state.lock().unwrap().reset(BlobStats::default());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-blossom-test-{}-{}", std::process::id(), tag))
    }

    /// `size` answers from metadata, and agrees with what was put.
    #[tokio::test]
    async fn size_matches_what_was_put() {
        let dir = tmp("size");
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsBlobStore::open(&dir).unwrap();

        let bytes = vec![3u8; 12_345];
        let hash = store.put(&bytes).await.unwrap();
        assert_eq!(store.size(&hash).await.unwrap(), Some(12_345));
        assert_eq!(
            store.size(&hash.to_ascii_uppercase()).await.unwrap(),
            Some(12_345)
        );
        assert_eq!(store.size(&"0".repeat(64)).await.unwrap(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn put_get_has_round_trip() {
        let dir = tmp("rt");
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsBlobStore::open(&dir).unwrap();

        let hash = store.put(b"hello blossom").await.unwrap();
        assert_eq!(hash, sha256_hex(b"hello blossom"));
        assert!(store.has(&hash).await);
        assert_eq!(
            store.get(&hash).await.unwrap().as_deref(),
            Some(&b"hello blossom"[..])
        );

        // Idempotent: storing the same bytes returns the same hash, one file.
        store.put(b"hello blossom").await.unwrap();
        assert_eq!(store.count(), 1);

        // Missing blob → None, not an error.
        assert!(!store.has("deadbeef").await);
        assert_eq!(store.get("deadbeef").await.unwrap(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn persists_across_reopen_and_wipes() {
        let dir = tmp("persist");
        let _ = std::fs::remove_dir_all(&dir);

        let hash = {
            let store = FsBlobStore::open(&dir).unwrap();
            store.put(b"durable").await.unwrap()
        };
        // Re-open: blob survives (it's on disk).
        let store = FsBlobStore::open(&dir).unwrap();
        assert!(store.has(&hash).await);

        store.wipe().await.unwrap();
        assert!(!store.has(&hash).await);
        assert_eq!(store.count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn figures(count: usize, bytes: u64) -> BlobStats {
        BlobStats { count, bytes }
    }

    /// Wait for the background walk `try_stats` started.
    fn settled(store: &FsBlobStore) -> BlobStats {
        for _ in 0..500 {
            if let Some(s) = store.try_stats() {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the stats walk never finished");
    }

    /// Once known, the figures follow every mutation without a walk: a new
    /// blob adds, a duplicate does not, a delete subtracts, a wipe zeroes.
    #[tokio::test]
    async fn figures_follow_put_duplicate_remove_and_wipe() {
        let dir = tmp("figures");
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsBlobStore::open(&dir).unwrap();
        assert_eq!(store.stats(), figures(0, 0));

        let a = store.put(&[1u8; 100]).await.unwrap();
        assert_eq!(store.try_stats(), Some(figures(1, 100)));

        // Same bytes again: one file, counted once.
        store.put(&[1u8; 100]).await.unwrap();
        assert_eq!(store.try_stats(), Some(figures(1, 100)));

        store.put(&[2u8; 50]).await.unwrap();
        assert_eq!(store.try_stats(), Some(figures(2, 150)));

        // Delete subtracts; deleting what is gone changes nothing.
        assert!(store.remove(&a.to_ascii_uppercase()));
        assert!(!store.remove(&a));
        assert_eq!(store.try_stats(), Some(figures(1, 50)));

        store.wipe().await.unwrap();
        assert_eq!(store.try_stats(), Some(figures(0, 0)));
        store.put(&[3u8; 7]).await.unwrap();
        assert_eq!(store.try_stats(), Some(figures(1, 7)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `retain_blobs` sets the figures from what it kept.
    #[tokio::test]
    async fn retain_sets_the_figures_from_what_is_kept() {
        let dir = tmp("retain");
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsBlobStore::open(&dir).unwrap();
        let keep = store.put(&[1u8; 30]).await.unwrap();
        store.put(&[2u8; 40]).await.unwrap();

        store.retain_blobs(&[keep].into_iter().collect());
        assert_eq!(store.try_stats(), Some(figures(1, 30)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh store asks nothing of the disk on the polling path: the first
    /// `try_stats` is `None` (or, if the walk was quick, the answer), and the
    /// walk lands the true figures for what was already there.
    #[tokio::test]
    async fn first_figures_come_from_a_background_walk() {
        let dir = tmp("walk");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let store = FsBlobStore::open(&dir).unwrap();
            store.put(&[1u8; 10]).await.unwrap();
            store.put(&[2u8; 20]).await.unwrap();
        }
        // A stray temp file is not a blob.
        std::fs::write(dir.join(".tmp-1-x"), b"partial").unwrap();

        let store = FsBlobStore::open(&dir).unwrap();
        // A put before the walk is known must not be counted twice.
        store.put(&[3u8; 5]).await.unwrap();
        assert_eq!(settled(&store), figures(3, 35));
        assert_eq!(store.count(), 3);
        assert_eq!(store.total_bytes(), 35);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mutations that land while the walk runs are folded in once, whether or
    /// not the walk saw their file.
    #[test]
    fn a_walk_folds_in_what_changed_meanwhile() {
        let dir = tmp("fold");
        let _ = std::fs::remove_dir_all(&dir);
        let store = FsBlobStore::open(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let gone = rt.block_on(store.put(&[1u8; 10])).unwrap();
        let stays = rt.block_on(store.put(&[2u8; 20])).unwrap();
        // Forget the figures and claim a walk by hand, as `try_stats` would.
        let generation = {
            let mut st = store.stats.state.lock().unwrap();
            st.known = None;
            st.claim()
        };
        // Meanwhile: one removed, one added — both already on disk (or not)
        // by the time the walk reads the directory.
        assert!(store.remove(&gone));
        let fresh = rt.block_on(store.put(&[3u8; 30])).unwrap();
        store.stats.walk(&store.root, generation);
        assert_eq!(store.try_stats(), Some(figures(2, 50)));
        assert!(store.remove(&fresh));
        assert!(store.remove(&stays));
        assert_eq!(store.try_stats(), Some(figures(0, 0)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
