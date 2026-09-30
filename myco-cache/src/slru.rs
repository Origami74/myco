//! A segmented LRU index over 32-byte keys (event ids, blob sha256s).
//!
//! Two segments, each in recency order:
//!
//! - **probation** — where every new entry starts. A burst of one-off
//!   entries (a napplet scrolling a long feed once) lands here and is what
//!   gets evicted first. A first access moves an entry to the young end of
//!   probation; only a **second** access promotes it.
//! - **protected** — entries accessed at least twice. It holds at most
//!   [`PROTECTED_SHARE_PCT`] of the budget; past that its oldest entries are
//!   demoted back to the young end of probation, not dropped.
//!
//! Eviction takes the oldest probation entry, and only reaches into
//! protected once probation is empty. So what the user keeps coming back to
//! survives a scan, and what was seen once ages out first.
//!
//! The index only tracks keys and sizes; the bytes live in the store behind
//! it. Being re-seen on ingest is **not** an access — [`Slru::insert`] of a
//! key already held changes nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;

/// The share of the budget the protected segment may hold, in percent.
pub const PROTECTED_SHARE_PCT: u64 = 80;

/// Which segment an entry is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Segment {
    Probation,
    Protected,
}

#[derive(Clone, Copy, Debug)]
struct Meta {
    tick: u64,
    size: u64,
    segment: Segment,
    /// Accesses while on probation: the first refreshes, the second promotes.
    hits: u8,
    /// NIP-40 expiry (unix seconds), for the expiry sweep. `None` for blobs
    /// and for events that never expire.
    expires: Option<u64>,
}

/// The index. Not thread-safe on its own; the caches hold it in a mutex.
#[derive(Default)]
pub struct Slru {
    entries: HashMap<[u8; 32], Meta>,
    probation: BTreeMap<u64, [u8; 32]>,
    protected: BTreeMap<u64, [u8; 32]>,
    /// Entries that expire, by `(expiry, key)`, so the sweep and the "anything
    /// due?" check never walk the whole index.
    expiring: BTreeSet<(u64, [u8; 32])>,
    next_tick: u64,
    probation_bytes: u64,
    protected_bytes: u64,
    /// The budget, in bytes. Protected is held to its share of it on every
    /// promotion; [`Slru::evict_to`] holds the total to it.
    limit: u64,
    /// Changed since the last snapshot, so an idle cache writes nothing.
    dirty: bool,
}

impl Slru {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
        self.rebalance();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes accounted across both segments.
    pub fn bytes(&self) -> u64 {
        self.probation_bytes + self.protected_bytes
    }

    pub fn contains(&self, key: &[u8; 32]) -> bool {
        self.entries.contains_key(key)
    }

    fn tick(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }

    /// Add `key` at the young end of probation. Returns `false` (and changes
    /// nothing, not even recency) when it is already held.
    pub fn insert(&mut self, key: [u8; 32], size: u64, expires: Option<u64>) -> bool {
        if self.entries.contains_key(&key) {
            return false;
        }
        let tick = self.tick();
        self.entries.insert(
            key,
            Meta {
                tick,
                size,
                segment: Segment::Probation,
                hits: 0,
                expires,
            },
        );
        self.probation.insert(tick, key);
        self.probation_bytes += size;
        if let Some(exp) = expires {
            self.expiring.insert((exp, key));
        }
        self.dirty = true;
        true
    }

    /// Record an access. A probation entry's first access moves it to the
    /// young end of probation; its second promotes it to protected. A
    /// protected entry moves to the young end. Returns whether `key` is held.
    pub fn touch(&mut self, key: &[u8; 32]) -> bool {
        let Some(meta) = self.entries.get(key).copied() else {
            return false;
        };
        let tick = self.tick();
        let promote = match meta.segment {
            Segment::Probation => {
                self.probation.remove(&meta.tick);
                meta.hits >= 1
            }
            Segment::Protected => {
                self.protected.remove(&meta.tick);
                true
            }
        };
        let segment = if promote {
            if meta.segment == Segment::Probation {
                self.probation_bytes -= meta.size;
                self.protected_bytes += meta.size;
            }
            self.protected.insert(tick, *key);
            Segment::Protected
        } else {
            self.probation.insert(tick, *key);
            Segment::Probation
        };
        self.entries.insert(
            *key,
            Meta {
                tick,
                segment,
                hits: meta.hits.saturating_add(1),
                ..meta
            },
        );
        self.dirty = true;
        if promote {
            self.rebalance();
        }
        true
    }

    /// Forget `key`. Returns its size when it was held.
    pub fn remove(&mut self, key: &[u8; 32]) -> Option<u64> {
        let meta = self.entries.remove(key)?;
        match meta.segment {
            Segment::Probation => {
                self.probation.remove(&meta.tick);
                self.probation_bytes -= meta.size;
            }
            Segment::Protected => {
                self.protected.remove(&meta.tick);
                self.protected_bytes -= meta.size;
            }
        }
        if let Some(exp) = meta.expires {
            self.expiring.remove(&(exp, *key));
        }
        self.dirty = true;
        Some(meta.size)
    }

    /// Demote the oldest protected entries while protected holds more than its
    /// share. Demoted entries go to the **young** end of probation, with one
    /// access to their name: they were wanted, so one more promotes them again.
    fn rebalance(&mut self) {
        let cap = self.limit / 100 * PROTECTED_SHARE_PCT;
        while self.protected_bytes > cap {
            let Some((&old_tick, &key)) = self.protected.iter().next() else {
                break;
            };
            self.protected.remove(&old_tick);
            let tick = self.tick();
            let meta = self.entries.get_mut(&key).expect("indexed key");
            meta.tick = tick;
            meta.segment = Segment::Probation;
            meta.hits = 1;
            let size = meta.size;
            self.protected_bytes -= size;
            self.probation_bytes += size;
            self.probation.insert(tick, key);
            self.dirty = true;
        }
    }

    /// Remove and return the keys to evict until the total fits the limit:
    /// oldest probation first, then oldest protected.
    pub fn evict_over_limit(&mut self) -> Vec<[u8; 32]> {
        self.evict_to(self.limit)
    }

    /// As [`Slru::evict_over_limit`], down to `target` bytes — below the
    /// limit when the store behind has run out of room early.
    pub fn evict_to(&mut self, target: u64) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        while self.bytes() > target {
            let next = self
                .probation
                .values()
                .next()
                .or_else(|| self.protected.values().next())
                .copied();
            let Some(key) = next else {
                break;
            };
            self.remove(&key);
            out.push(key);
        }
        out
    }

    /// Keys whose expiry is at or before `now`. Not removed; the caller
    /// removes what it managed to delete.
    pub fn expired(&self, now: u64) -> Vec<[u8; 32]> {
        self.expiring
            .range(..(now + 1, [0u8; 32]))
            .map(|(_, k)| *k)
            .collect()
    }

    /// Every key, in no particular order.
    pub fn keys(&self) -> Vec<[u8; 32]> {
        self.entries.keys().copied().collect()
    }

    pub fn clear(&mut self) {
        let limit = self.limit;
        *self = Self::new(limit);
        self.dirty = true;
    }

    /// Whether the index changed since the last call, clearing the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Fold `newer` — entries indexed since this snapshot was taken — in at
    /// the young end, keeping their own order. Used when the snapshot is read
    /// after the cache has already started taking writes.
    pub fn absorb(&mut self, newer: &Slru) {
        for map in [&newer.probation, &newer.protected] {
            for key in map.values() {
                let meta = newer.entries[key];
                self.insert(*key, meta.size, meta.expires);
            }
        }
        self.dirty = true;
    }

    // --- snapshot ---
    //
    // A flat file: a magic header, then one record per entry, oldest first
    // within each segment (probation, then protected). Loading replays the
    // records in order, so recency survives a restart. The file is advisory:
    // the caches reconcile it against what the store really holds.

    const MAGIC: &'static [u8; 8] = b"MYCSLRU2";
    const RECORD: usize = 1 + 8 + 8 + 32;

    /// The index as snapshot bytes — built under the caller's lock, written
    /// outside it with [`write_snapshot`].
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.entries.len() * Self::RECORD);
        buf.extend_from_slice(Self::MAGIC);
        for map in [&self.probation, &self.protected] {
            for key in map.values() {
                let meta = &self.entries[key];
                let tag = match (meta.segment, meta.hits) {
                    (Segment::Probation, 0) => 0u8,
                    (Segment::Probation, _) => 1,
                    (Segment::Protected, _) => 2,
                };
                buf.push(tag);
                buf.extend_from_slice(&meta.size.to_le_bytes());
                buf.extend_from_slice(&meta.expires.unwrap_or(0).to_le_bytes());
                buf.extend_from_slice(key);
            }
        }
        buf
    }

    /// Read a snapshot. `None` for a missing or malformed file — the caller
    /// rebuilds from the store instead.
    pub fn load(path: &Path, limit: u64) -> Option<Self> {
        let raw = std::fs::read(path).ok()?;
        let body = raw.strip_prefix(Self::MAGIC.as_slice())?;
        let (records, rest) = body.as_chunks::<{ Self::RECORD }>();
        if !rest.is_empty() {
            return None;
        }
        let mut slru = Self::new(limit);
        for record in records {
            let tag = record[0];
            let size = u64::from_le_bytes(record[1..9].try_into().ok()?);
            let expires = u64::from_le_bytes(record[9..17].try_into().ok()?);
            let key: [u8; 32] = record[17..49].try_into().ok()?;
            slru.insert(key, size, (expires != 0).then_some(expires));
            let meta = slru.entries.get_mut(&key).expect("just inserted");
            match tag {
                0 => {}
                1 => meta.hits = 1,
                // Replayed oldest-first, so promoting in order rebuilds the
                // protected segment's recency too.
                _ => slru.promote_without_rebalance(&key),
            }
        }
        slru.rebalance();
        slru.dirty = false;
        Some(slru)
    }

    fn promote_without_rebalance(&mut self, key: &[u8; 32]) {
        let Some(meta) = self.entries.get(key).copied() else {
            return;
        };
        if meta.segment == Segment::Protected {
            return;
        }
        self.probation.remove(&meta.tick);
        self.probation_bytes -= meta.size;
        let tick = self.tick();
        self.protected.insert(tick, *key);
        self.protected_bytes += meta.size;
        self.entries.insert(
            *key,
            Meta {
                tick,
                segment: Segment::Protected,
                hits: meta.hits.max(2),
                ..meta
            },
        );
    }
}

/// Write snapshot bytes to `path` atomically (temp + rename). Deliberately
/// no fsync: the snapshot is advisory, and a torn one is refused on load and
/// rebuilt — cheaper than paying a sync every upkeep.
pub fn write_snapshot(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::File::create(&tmp)?.write_all(bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn evicts_oldest_probation_first() {
        let mut s = Slru::new(30);
        for n in 1..=4 {
            s.insert(k(n), 10, None);
        }
        assert_eq!(s.evict_over_limit(), vec![k(1)]);
        assert_eq!(s.bytes(), 30);
    }

    #[test]
    fn a_second_access_survives_a_scan() {
        let mut s = Slru::new(100);
        s.insert(k(1), 10, None);
        assert!(s.touch(&k(1)), "held entries report the touch");
        assert!(s.touch(&k(1)));
        // A scan of one-off entries, far larger than the budget, each
        // accessed once.
        for n in 2..=50 {
            s.insert(k(n), 10, None);
            s.touch(&k(n));
            s.evict_over_limit();
        }
        assert!(
            s.contains(&k(1)),
            "the twice-used entry was flushed by a scan"
        );
        assert!(s.bytes() <= 100);
    }

    #[test]
    fn one_access_does_not_protect() {
        let mut s = Slru::new(100);
        s.insert(k(1), 10, None);
        s.touch(&k(1));
        for n in 2..=20 {
            s.insert(k(n), 10, None);
            s.touch(&k(n));
            s.evict_over_limit();
        }
        assert!(!s.contains(&k(1)), "a single access protected an entry");
    }

    #[test]
    fn reinsert_is_not_an_access() {
        let mut s = Slru::new(20);
        s.insert(k(1), 10, None);
        s.insert(k(2), 10, None);
        assert!(!s.insert(k(1), 10, None));
        s.insert(k(3), 10, None);
        assert_eq!(
            s.evict_over_limit(),
            vec![k(1)],
            "re-seeing on ingest refreshed recency"
        );
    }

    #[test]
    fn protected_is_capped_by_demotion_not_loss() {
        let mut s = Slru::new(100);
        for n in 1..=10 {
            s.insert(k(n), 10, None);
            s.touch(&k(n));
            s.touch(&k(n));
        }
        // 100 bytes promoted; protected may hold 80.
        assert_eq!(s.len(), 10);
        assert!(s.protected_bytes <= 80);
        assert_eq!(s.bytes(), 100);
        assert!(s.evict_over_limit().is_empty());
        // The two demoted ones are the oldest promoted, and go first.
        s.insert(k(11), 10, None);
        assert_eq!(s.evict_over_limit(), vec![k(1)]);
    }

    #[test]
    fn remove_and_expired() {
        let mut s = Slru::new(100);
        s.insert(k(1), 10, Some(50));
        s.insert(k(2), 10, Some(500));
        s.insert(k(3), 10, None);
        assert_eq!(s.expired(100), vec![k(1)]);
        assert_eq!(s.remove(&k(1)), Some(10));
        assert_eq!(s.remove(&k(1)), None);
        assert!(s.expired(100).is_empty());
        assert_eq!(s.bytes(), 20);
    }

    #[test]
    fn snapshot_round_trips_order_segments_and_hits() {
        let dir = std::env::temp_dir().join(format!("myco-slru-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.bin");
        let mut s = Slru::new(40);
        s.insert(k(1), 10, None);
        s.insert(k(2), 10, Some(99));
        s.insert(k(3), 10, None);
        s.touch(&k(1));
        s.touch(&k(1));
        s.touch(&k(3));
        assert!(s.take_dirty());
        write_snapshot(&path, &s.encode()).unwrap();

        let mut back = Slru::load(&path, 40).unwrap();
        assert!(!back.take_dirty(), "a fresh load is not dirty");
        assert_eq!(back.len(), 3);
        assert_eq!(back.bytes(), 30);
        assert_eq!(back.expired(100), vec![k(2)]);
        // k(3) had one access: one more promotes it.
        back.touch(&k(3));
        back.insert(k(4), 10, None);
        back.insert(k(5), 10, None);
        assert_eq!(back.evict_over_limit(), vec![k(2)]);
        assert!(back.contains(&k(1)) && back.contains(&k(3)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absorb_keeps_what_arrived_after_the_snapshot() {
        let mut snap = Slru::new(100);
        snap.insert(k(1), 10, None);
        let mut newer = Slru::new(100);
        newer.insert(k(2), 10, None);
        snap.absorb(&newer);
        assert!(snap.contains(&k(1)) && snap.contains(&k(2)));
        snap.set_limit(10);
        assert_eq!(
            snap.evict_over_limit(),
            vec![k(1)],
            "the snapshot's older entry should have gone first"
        );
    }

    #[test]
    fn a_malformed_snapshot_is_refused() {
        let dir = std::env::temp_dir().join(format!("myco-slru-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.bin");
        std::fs::write(&path, b"MYCSLRU2abc").unwrap();
        assert!(Slru::load(&path, 10).is_none());
        assert!(Slru::load(&dir.join("missing"), 10).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
