//! `myco-blossom` — a generic embedded **Blossom blob store**: a content-
//! addressed filesystem store keyed by sha256, implementing `nsite-deck`'s
//! [`BlobStore`] seam. **Always embedded** (there is no good Android Blossom app
//! to forward to). See `docs/design/nsite-layer.md` §2.2.
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

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nsite_deck::seams::BlobStore;
use sha2::{Digest, Sha256};

pub mod server;

/// A content-addressed blob store rooted at a directory.
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Open (creating if needed) a blob store rooted at `root`.
    pub fn open(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn blob_path(&self, sha256_hex: &str) -> PathBuf {
        self.root.join(sha256_hex)
    }

    /// Number of stored blobs (for diagnostics / cache status).
    pub fn count(&self) -> usize {
        std::fs::read_dir(&self.root)
            .map(|rd| rd.filter_map(Result::ok).filter(is_blob_name).count())
            .unwrap_or(0)
    }

    /// Delete every blob whose hash is **not** in `keep`. Used by the selective
    /// cache wipe to retain the blobs backing pinned nsites while clearing the rest.
    /// `keep` holds lowercase-hex sha256 names (the manifest's `path -> hash` values).
    pub fn retain_blobs(&self, keep: &std::collections::HashSet<String>) {
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for entry in rd.filter_map(Result::ok).filter(is_blob_name) {
                let kept = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| keep.contains(name));
                if !kept {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Total bytes on disk (for the LRU cap accounting; eviction itself is P5).
    pub fn total_bytes(&self) -> u64 {
        std::fs::read_dir(&self.root)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter(is_blob_name)
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
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

    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let hash = sha256_hex(bytes);
        let dest = self.blob_path(&hash);
        if dest.is_file() {
            return Ok(hash); // immutable + content-addressed → already stored
        }
        // Atomic write: a unique temp file (pid + hash) then rename into place.
        let tmp = self
            .root
            .join(format!(".tmp-{}-{}", std::process::id(), &hash));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &dest)?;
        Ok(hash)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(&self.root)?.filter_map(Result::ok) {
            let _ = std::fs::remove_file(entry.path());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-blossom-test-{}-{}", std::process::id(), tag))
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
}
