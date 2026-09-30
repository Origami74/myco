//! `myco-cache` — the shell-wide cache, kept apart from the local relay and
//! the local Blossom.
//!
//! The local relay and Blossom hold what this device **keeps**: its own
//! publishes, installed apps, the handful of kinds kept as they pass, and
//! private messages. Everything else that passes through —
//! query answers, pulled backlog, mesh pass-through, fetched blobs — lands
//! here instead, in two bounded stores:
//!
//! - [`EventCache`]: a second `nostr-lmdb`, indexed NIP-01 queries.
//! - [`BlobCache`]: a second content-addressed blob directory.
//!
//! Both evict by segmented LRU ([`slru`]): new entries start on probation and
//! are the first to go; a second access promotes them, so what the user keeps
//! coming back to survives a one-off scan. Being re-seen on ingest is not an
//! access.
//!
//! This crate holds no Myco concepts beyond that; `myco-core` decides what is
//! kept and what is cached, and reads through both.

pub mod blobs;
pub mod events;
pub mod slru;

pub use blobs::BlobCache;
pub use events::EventCache;

/// Default event cache budget: 500 MB.
pub const DEFAULT_EVENT_CACHE_BYTES: u64 = 500 * 1024 * 1024;

/// Default blob cache budget: 1.5 GB.
pub const DEFAULT_BLOB_CACHE_BYTES: u64 = 1536 * 1024 * 1024;

/// How full a cache is. `bytes` and `limit` are the budget's accounting —
/// estimated database size for events (`events::disk_cost`), file size for
/// blobs — not a measurement of the disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct CacheStats {
    pub count: u64,
    pub bytes: u64,
    pub limit: u64,
}

/// A fresh directory under the temp dir, for scratch caches.
pub(crate) fn scratch_dir(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "myco-cache-{tag}-{}-{nanos}-{n}",
        std::process::id()
    ))
}
