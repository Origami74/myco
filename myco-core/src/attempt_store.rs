//! Crash-surviving persistence for the BLE connect-attempt log (D-13, D-14).
//!
//! The fips transport's attempt log is an in-memory ring, so a force-stop takes
//! the evidence with it — and a force-stop is exactly what happens between
//! noticing a peering fault and sitting down to read the diagnostics. This store
//! keeps that history on disk across launches.
//!
//! **The read path is the point.** `CONCERNS.md`'s CORE-03 records what happens
//! when a whole-document JSON store meets one corrupt byte: the entire file
//! becomes unreadable and every record in it is lost. This file is newline
//! delimited and parsed one line at a time, so a truncated final line costs one
//! entry and a garbled line costs that line. Nothing here panics, unwraps or
//! rewrites the source file on a read failure, and a file whose majority of
//! lines failed to parse is copied aside before the store is ever allowed to
//! write — a still-good file we merely failed to understand must not be replaced
//! by a shorter one.
//!
//! Size is bounded by peer count rather than uptime: at most
//! [`MAX_ATTEMPTS_PER_PEER`] entries per address, and addresses whose newest
//! attempt is older than [`EVICT_AFTER`] are dropped on the next write. No
//! background job, no rotation logic.
//!
//! Threading: [`AttemptStore::observe`] runs inside `AppRuntime::state()` on the
//! FFI thread and does no I/O at all. [`AttemptStore::flush`] does the I/O and is
//! spawned onto the tokio runtime, never called inline, and never holds the lock
//! across the write.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::ble_diag::{BleAttempt, BlePeerAttempts, MAX_ATTEMPTS_PER_PEER};

/// File name under the app-private data dir.
const FILE_NAME: &str = "ble-attempts.jsonl";

/// Sibling written when a file's majority of lines failed to parse, so the
/// original is preserved rather than replaced by the next write.
const CORRUPT_SUFFIX: &str = "ble-attempts.jsonl.corrupt";

/// A peer whose newest attempt is older than this is dropped from disk on the
/// next write. Bounds the file by peer count instead of uptime, and bounds how
/// far back the record of which devices were physically near this phone reaches
/// (T-03-03).
const EVICT_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Minimum gap between flushes. `state()` is polled far more often than the log
/// changes meaningfully, so this keeps the disk quiet.
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// One attempt as persisted. Mirrors [`BleAttempt`]; carries no
/// peer-supplied free text, only locally generated values (T-03-03).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedAttempt {
    /// Wall-clock milliseconds since the Unix epoch at which the attempt resolved.
    pub at_ms: u64,
    /// The peer's BLE address.
    pub ble_addr: String,
    /// The peer's node address hex, or empty when the attempt never learned one.
    pub node_addr_hex: String,
    /// `central` or `peripheral`.
    pub role: String,
    /// Milliseconds from discovery to resolution; `0` when not measured.
    pub discovery_ms: u64,
    /// Stable outcome label, e.g. `lost-tiebreaker`.
    pub outcome: String,
}

impl PersistedAttempt {
    fn from_live(a: &BleAttempt) -> Self {
        Self {
            at_ms: a.at_ms,
            ble_addr: a.ble_addr.clone(),
            node_addr_hex: a.node_addr_hex.clone(),
            role: a.role.as_str().to_string(),
            discovery_ms: a.discovery_ms,
            outcome: a.outcome.as_str().to_string(),
        }
    }
}

#[derive(Default)]
struct Inner {
    /// Per-address rings, oldest first, capped at [`MAX_ATTEMPTS_PER_PEER`].
    by_addr: HashMap<String, VecDeque<PersistedAttempt>>,
    /// Learned address-to-node-address pairs, so attribution survives a restart.
    node_addrs: HashMap<String, String>,
    /// Live per-address send-failure counts. Deliberately not persisted: the
    /// counter is a property of the current process's link, and a stale count
    /// carried across a restart would read as current evidence.
    send_failures: HashMap<String, u64>,
    /// Something changed since the last successful flush.
    dirty: bool,
    /// When the last flush ran, so flushes stay rate limited.
    last_flush: Option<Instant>,
    /// The loaded file was mostly unparseable and has already been copied aside.
    /// Kept so the copy is never made twice.
    preserved: bool,
}

/// Bounded, corruption-tolerant on-disk home for the attempt log.
pub struct AttemptStore {
    path: PathBuf,
    corrupt_path: PathBuf,
    inner: Mutex<Inner>,
}

impl AttemptStore {
    /// Read `<data_dir>/ble-attempts.jsonl`, keeping every line that parses.
    ///
    /// Infallible by construction: a missing file is an empty history, not an
    /// error, and an unreadable one degrades to an empty history rather than
    /// taking startup down or surfacing an app-level banner — a diagnostics file
    /// is not worth failing a launch for.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE_NAME);
        let corrupt_path = data_dir.join(CORRUPT_SUFFIX);
        let mut inner = Inner::default();

        // Read line by line and deserialize each line on its own. Never
        // `serde_json::from_slice` over the whole file — that is precisely the
        // failure mode where one corrupt byte costs the entire history.
        let mut parsed: Vec<PersistedAttempt> = Vec::new();
        let mut failed = 0usize;
        let mut non_empty = 0usize;

        if let Ok(file) = std::fs::File::open(&path) {
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else {
                    // An I/O error mid-file (or invalid UTF-8) ends the read;
                    // whatever parsed before it is still good.
                    failed += 1;
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                non_empty += 1;
                match serde_json::from_str::<PersistedAttempt>(&line) {
                    Ok(rec) => parsed.push(rec),
                    Err(_) => failed += 1,
                }
            }
        }

        // A file we mostly failed to understand is preserved before the store is
        // ever allowed to write, so a still-good file cannot be lost to a
        // rewrite. Copy, never move: the original bytes stay exactly as found.
        if non_empty > 0 && failed * 2 > non_empty {
            let _ = std::fs::copy(&path, &corrupt_path);
            inner.preserved = true;
        }

        for rec in parsed {
            if !rec.node_addr_hex.is_empty() {
                inner
                    .node_addrs
                    .insert(rec.ble_addr.clone(), rec.node_addr_hex.clone());
            }
            inner
                .by_addr
                .entry(rec.ble_addr.clone())
                .or_default()
                .push_back(rec);
        }
        for ring in inner.by_addr.values_mut() {
            ring.make_contiguous().sort_by_key(|r| r.at_ms);
            while ring.len() > MAX_ATTEMPTS_PER_PEER {
                ring.pop_front();
            }
        }

        Self {
            path,
            corrupt_path,
            inner: Mutex::new(inner),
        }
    }

    /// Fold a live fips snapshot into the in-memory history.
    ///
    /// Runs inside `state()` on the FFI thread, so it is allocation-and-compare
    /// only: no file I/O, no blocking, and the lock is released before the
    /// caller does anything else. Deduplicates on address plus timestamp, so
    /// re-observing the same live entry on every poll changes nothing and leaves
    /// the store clean.
    pub fn observe(&self, live: &[BlePeerAttempts]) {
        let mut inner = self.lock();
        for peer in live {
            // Send failures are a live counter, so track the newest value rather
            // than accumulating across polls.
            let prev = inner
                .send_failures
                .insert(peer.ble_addr.clone(), peer.send_failures);
            if prev != Some(peer.send_failures) {
                inner.dirty = true;
            }
            if !peer.node_addr_hex.is_empty()
                && inner.node_addrs.get(&peer.ble_addr) != Some(&peer.node_addr_hex)
            {
                inner
                    .node_addrs
                    .insert(peer.ble_addr.clone(), peer.node_addr_hex.clone());
                inner.dirty = true;
            }

            for attempt in &peer.attempts {
                let ring = inner.by_addr.entry(peer.ble_addr.clone()).or_default();
                if ring
                    .iter()
                    .any(|r| r.at_ms == attempt.at_ms && r.ble_addr == attempt.ble_addr)
                {
                    continue;
                }
                ring.push_back(PersistedAttempt::from_live(attempt));
                ring.make_contiguous().sort_by_key(|r| r.at_ms);
                while ring.len() > MAX_ATTEMPTS_PER_PEER {
                    ring.pop_front();
                }
                inner.dirty = true;
            }
        }
    }

    /// The merged history — everything loaded from disk plus everything observed
    /// this run — in the same shape `merge_peers` takes, attempts oldest first.
    pub fn snapshot(&self) -> Vec<BlePeerAttempts> {
        let inner = self.lock();
        let mut addrs: Vec<&String> = inner
            .by_addr
            .keys()
            .chain(inner.send_failures.keys())
            .collect();
        addrs.sort_unstable();
        addrs.dedup();
        addrs
            .into_iter()
            .map(|addr| BlePeerAttempts {
                ble_addr: addr.clone(),
                node_addr_hex: inner.node_addrs.get(addr).cloned().unwrap_or_default(),
                send_failures: inner.send_failures.get(addr).copied().unwrap_or(0),
                attempts: inner
                    .by_addr
                    .get(addr)
                    .map(|ring| ring.iter().map(to_live).collect())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Whether a flush is worth spawning: something changed and the rate limit
    /// has elapsed. Checked from `state()` so the common case costs one lock and
    /// no allocation.
    pub fn flush_due(&self) -> bool {
        let inner = self.lock();
        inner.dirty
            && inner
                .last_flush
                .map(|t| t.elapsed() >= FLUSH_INTERVAL)
                .unwrap_or(true)
    }

    /// Write the history out, one JSON record per line, replacing the file
    /// atomically. Addresses whose newest attempt is older than [`EVICT_AFTER`]
    /// are dropped here rather than by a background job.
    ///
    /// Runs on the tokio runtime, never the FFI thread. The lock is held only
    /// long enough to serialize; the write happens after it is released.
    pub fn flush(&self, now_ms: u64) {
        let (body, preserved) = {
            let mut inner = self.lock();
            let cutoff_ms = now_ms.saturating_sub(EVICT_AFTER.as_millis() as u64);
            inner
                .by_addr
                .retain(|_, ring| ring.back().is_some_and(|r| r.at_ms >= cutoff_ms));

            let mut lines = String::new();
            let mut addrs: Vec<&String> = inner.by_addr.keys().collect();
            addrs.sort_unstable();
            for addr in addrs {
                for rec in &inner.by_addr[addr] {
                    match serde_json::to_string(rec) {
                        Ok(line) => {
                            lines.push_str(&line);
                            lines.push('\n');
                        }
                        // A record that will not serialize is skipped rather
                        // than aborting the whole write.
                        Err(_) => continue,
                    }
                }
            }
            inner.dirty = false;
            inner.last_flush = Some(Instant::now());
            (lines, inner.preserved)
        };

        // Belt and braces: if load() found a mostly-unparseable file, the copy
        // was already made there. Re-checking costs nothing and documents that
        // no write happens before the original is safe.
        if preserved && !self.corrupt_path.exists() {
            let _ = std::fs::copy(&self.path, &self.corrupt_path);
        }

        let tmp = self.path.with_extension("jsonl.tmp");
        let _ =
            std::fs::write(&tmp, body.as_bytes()).and_then(|_| std::fs::rename(&tmp, &self.path));
    }

    /// Take the lock, recovering from poisoning rather than panicking: a
    /// diagnostics store must never be what takes the app down.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Rebuild the live record from a persisted one. Unknown role/outcome
/// labels from a hand-edited or future-version file are carried through as
/// recorded rather than being coerced into a wrong enum value.
fn to_live(rec: &PersistedAttempt) -> BleAttempt {
    use crate::ble_diag::{BleAttemptOutcome, BleRole};
    BleAttempt {
        at_ms: rec.at_ms,
        ble_addr: rec.ble_addr.clone(),
        node_addr_hex: rec.node_addr_hex.clone(),
        role: match rec.role.as_str() {
            "peripheral" => BleRole::Peripheral,
            _ => BleRole::Central,
        },
        discovery_ms: rec.discovery_ms,
        outcome: match rec.outcome.as_str() {
            "connected" => BleAttemptOutcome::Connected,
            "connect-timeout" => BleAttemptOutcome::ConnectTimeout,
            "pubkey-exchange-failed" => BleAttemptOutcome::PubkeyExchangeFailed,
            "lost-tiebreaker" => BleAttemptOutcome::LostTiebreaker,
            "pool-rejected" => BleAttemptOutcome::PoolRejected,
            "duplicate-node" => BleAttemptOutcome::DuplicateNode,
            _ => BleAttemptOutcome::ConnectError,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble_diag::{BleAttemptOutcome, BleRole};

    fn tmp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("myco-attempt-store-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn live(addr: &str, at_ms: u64, send_failures: u64) -> BlePeerAttempts {
        BlePeerAttempts {
            ble_addr: addr.to_string(),
            node_addr_hex: "beef".to_string(),
            send_failures,
            attempts: vec![BleAttempt {
                at_ms,
                ble_addr: addr.to_string(),
                node_addr_hex: "beef".to_string(),
                role: BleRole::Central,
                discovery_ms: 42,
                outcome: BleAttemptOutcome::LostTiebreaker,
            }],
        }
    }

    #[test]
    fn round_trips_through_save_then_load() {
        let dir = tmp_dir("roundtrip");
        let store = AttemptStore::load(&dir);
        store.observe(&[live("ble0/AA", 1_000, 3)]);
        store.flush(1_000);

        let reloaded = AttemptStore::load(&dir);
        let snap = reloaded.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].ble_addr, "ble0/AA");
        assert_eq!(snap[0].node_addr_hex, "beef");
        assert_eq!(snap[0].attempts.len(), 1);
        assert_eq!(snap[0].attempts[0].at_ms, 1_000);
        assert_eq!(snap[0].attempts[0].discovery_ms, 42);
        assert_eq!(
            snap[0].attempts[0].outcome,
            BleAttemptOutcome::LostTiebreaker
        );
        // Send failures are deliberately not persisted.
        assert_eq!(snap[0].send_failures, 0);
    }

    #[test]
    fn truncated_final_line_costs_only_that_entry() {
        let dir = tmp_dir("truncated");
        let store = AttemptStore::load(&dir);
        store.observe(&[live("ble0/AA", 1_000, 0)]);
        store.observe(&[live("ble0/AA", 2_000, 0)]);
        store.flush(2_000);

        // Lop the last line in half, as a crash mid-write would.
        let path = dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).unwrap();
        let cut = text.len() - 12;
        std::fs::write(&path, &text[..cut]).unwrap();

        let reloaded = AttemptStore::load(&dir);
        let snap = reloaded.snapshot();
        assert_eq!(snap.len(), 1);
        // The earlier entry survived; only the mangled one was lost.
        assert_eq!(snap[0].attempts.len(), 1);
        assert_eq!(snap[0].attempts[0].at_ms, 1_000);
    }

    #[test]
    fn mostly_garbage_file_is_preserved_and_leaves_the_original_intact() {
        let dir = tmp_dir("garbage");
        let path = dir.join(FILE_NAME);
        let garbage = "not json at all\n{{{ broken\n\u{1}\u{2}\u{3}\nalso not json\n";
        std::fs::write(&path, garbage).unwrap();

        let store = AttemptStore::load(&dir);
        assert!(
            store.snapshot().is_empty(),
            "no history from a garbage file"
        );

        // The original bytes are untouched...
        assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
        // ...and a preserved copy exists.
        let corrupt = dir.join(CORRUPT_SUFFIX);
        assert!(corrupt.exists(), "expected a .corrupt sibling");
        assert_eq!(std::fs::read_to_string(&corrupt).unwrap(), garbage);
    }

    #[test]
    fn missing_file_yields_no_history_and_no_preserved_copy() {
        let dir = tmp_dir("missing");
        let store = AttemptStore::load(&dir);
        assert!(store.snapshot().is_empty());
        assert!(!dir.join(CORRUPT_SUFFIX).exists());
        assert!(!dir.join(FILE_NAME).exists());
    }

    #[test]
    fn ring_caps_at_max_per_address() {
        let dir = tmp_dir("cap");
        let store = AttemptStore::load(&dir);
        for i in 0..(MAX_ATTEMPTS_PER_PEER as u64 + 7) {
            store.observe(&[live("ble0/AA", 1_000 + i, 0)]);
        }
        let snap = store.snapshot();
        assert_eq!(snap[0].attempts.len(), MAX_ATTEMPTS_PER_PEER);
        // Oldest fell off the front.
        assert_eq!(snap[0].attempts[0].at_ms, 1_007);
    }

    #[test]
    fn eviction_drops_a_stale_address_and_keeps_a_fresh_one() {
        let dir = tmp_dir("evict");
        let now_ms = 1_000_000_000_000u64;
        let day_ms = EVICT_AFTER.as_millis() as u64;

        let store = AttemptStore::load(&dir);
        store.observe(&[live("ble0/OLD", now_ms - day_ms - 60_000, 0)]);
        store.observe(&[live("ble0/NEW", now_ms - 60_000, 0)]);
        store.flush(now_ms);

        let reloaded = AttemptStore::load(&dir);
        let addrs: Vec<String> = reloaded
            .snapshot()
            .into_iter()
            .map(|p| p.ble_addr)
            .collect();
        assert_eq!(addrs, vec!["ble0/NEW".to_string()]);
    }

    #[test]
    fn observing_the_same_live_entry_twice_does_not_duplicate_or_dirty() {
        let dir = tmp_dir("dedup");
        let store = AttemptStore::load(&dir);
        store.observe(&[live("ble0/AA", 5_000, 1)]);
        store.flush(5_000);
        assert!(!store.flush_due(), "flush cleared the dirty flag");

        store.observe(&[live("ble0/AA", 5_000, 1)]);
        assert!(
            !store.flush_due(),
            "re-observing an identical snapshot is a no-op"
        );
        assert_eq!(store.snapshot()[0].attempts.len(), 1);
    }

    #[test]
    fn a_line_that_parses_survives_alongside_one_that_does_not() {
        let dir = tmp_dir("mixed");
        let path = dir.join(FILE_NAME);
        // One good record, one garbled — a minority failure, so no preserve.
        let good = r#"{"atMs":7,"bleAddr":"ble0/AA","nodeAddrHex":"beef","role":"peripheral","discoveryMs":9,"outcome":"connected"}"#;
        std::fs::write(&path, format!("{good}\n{{ nope\n{good}\n")).unwrap();

        let store = AttemptStore::load(&dir);
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].attempts.len(), 2, "both good lines kept");
        assert_eq!(snap[0].attempts[0].role, BleRole::Peripheral);
        assert!(
            !dir.join(CORRUPT_SUFFIX).exists(),
            "a minority failure must not trigger the preserve path"
        );
    }
}
