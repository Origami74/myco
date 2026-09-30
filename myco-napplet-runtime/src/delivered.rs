//! What this napplet has been given: the event ids and blob hashes the runtime
//! delivered to it, remembered so that a napplet can **keep** or
//! **rebroadcast** only what it was actually shown (NAP-LOCAL,
//! `docs/design/napplet/NAP-LOCAL.md`).
//!
//! A set of every id would grow without bound, so this is a **scalable bloom
//! filter** (Almeida et al.): a chain of filters, each twice the capacity of
//! the last and with a tighter false-positive rate, so the chain's overall
//! rate stays under [`TARGET_FPR`] however many ids arrive. The hashing
//! follows the fips routing filter's shape — two base hashes, `k` indexes
//! derived as `h1 + i·h2` — keyed with per-filter random keys, so a napplet
//! cannot grind ids that collide in someone's filter.
//!
//! A false positive lets a napplet keep or rebroadcast an event it was not
//! shown, at about [`TARGET_FPR`]; the event must still be validly signed. No
//! false negatives: what was delivered is always recognised until the chain
//! outgrows its share of [`MAX_BYTES`] and its oldest filter is dropped.
//!
//! Growth stops at a bound: once a doubled filter would take more than half
//! the chain's share, new filters repeat the last one's size and rate, and
//! the chain becomes a sliding window — the oldest deliveries age out, the
//! memory stays put.
//!
//! Lives as long as the napplet has a window open; the host shares one
//! ledger across a napplet's windows and drops it with the last.

use std::collections::hash_map::RandomState;
use std::collections::VecDeque;
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex};

/// The chain's overall false-positive target.
pub const TARGET_FPR: f64 = 0.001;
/// Ids the first filter is sized for.
const BASE_CAPACITY: usize = 1024;
/// Each filter's rate is this times the previous one's, so the sum stays
/// under `TARGET_FPR` (a geometric series: `p0 / (1 - r) = TARGET_FPR`).
const TIGHTENING: f64 = 0.5;
/// The most one napplet's ledger may occupy — its event chain and its blob
/// chain together, half each. Past it the oldest filter goes: the oldest
/// deliveries are forgotten, the newest never are.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// One bloom filter in the chain.
struct Filter {
    bits: Vec<u64>,
    num_bits: u64,
    hashes: u32,
    capacity: usize,
    fpr: f64,
    count: usize,
    keys: (RandomState, RandomState),
}

impl Filter {
    /// Bits for `capacity` items at `fpr` — the standard optimum.
    fn bits_for(capacity: usize, fpr: f64) -> u64 {
        let ln2 = std::f64::consts::LN_2;
        ((-(capacity as f64) * fpr.ln()) / (ln2 * ln2))
            .ceil()
            .max(64.0) as u64
    }

    /// The bytes a filter of this shape would take, without allocating it.
    fn bytes_for(capacity: usize, fpr: f64) -> usize {
        Self::bits_for(capacity, fpr).div_ceil(64) as usize * 8
    }

    fn new(capacity: usize, fpr: f64) -> Self {
        let num_bits = Self::bits_for(capacity, fpr);
        let hashes = ((num_bits as f64 / capacity as f64) * std::f64::consts::LN_2)
            .round()
            .max(1.0) as u32;
        Self {
            bits: vec![0; num_bits.div_ceil(64) as usize],
            num_bits,
            hashes,
            capacity,
            fpr,
            count: 0,
            keys: (RandomState::new(), RandomState::new()),
        }
    }

    fn indexes(&self, item: &[u8; 32]) -> impl Iterator<Item = u64> + '_ {
        let h1 = self.keys.0.hash_one(item);
        let h2 = self.keys.1.hash_one(item) | 1;
        (0..self.hashes as u64).map(move |i| h1.wrapping_add(i.wrapping_mul(h2)) % self.num_bits)
    }

    fn insert(&mut self, item: &[u8; 32]) {
        let idx: Vec<u64> = self.indexes(item).collect();
        for i in idx {
            self.bits[(i / 64) as usize] |= 1 << (i % 64);
        }
        self.count += 1;
    }

    fn contains(&self, item: &[u8; 32]) -> bool {
        self.indexes(item)
            .all(|i| self.bits[(i / 64) as usize] & (1 << (i % 64)) != 0)
    }

    fn bytes(&self) -> usize {
        self.bits.len() * 8
    }
}

/// A scalable bloom filter over 32-byte keys. See the module docs.
pub struct Scalable {
    filters: VecDeque<Filter>,
    /// The false-positive rate the next filter is built with.
    next_fpr: f64,
    next_capacity: usize,
    max_bytes: usize,
}

impl Default for Scalable {
    fn default() -> Self {
        Self {
            filters: VecDeque::new(),
            next_fpr: TARGET_FPR * (1.0 - TIGHTENING),
            next_capacity: BASE_CAPACITY,
            max_bytes: MAX_BYTES / 2,
        }
    }
}

impl Scalable {
    pub fn insert(&mut self, item: &[u8; 32]) {
        if self.contains(item) {
            return;
        }
        let full = self.filters.back().is_none_or(|f| f.count >= f.capacity);
        if full {
            let grown = Filter::bytes_for(self.next_capacity, self.next_fpr);
            let filter = match self.filters.back() {
                // At the bound: repeat the last shape — a sliding window.
                Some(last) if grown > self.max_bytes / 2 => Filter::new(last.capacity, last.fpr),
                _ => {
                    let f = Filter::new(self.next_capacity, self.next_fpr);
                    self.next_capacity = self.next_capacity.saturating_mul(2);
                    self.next_fpr *= TIGHTENING;
                    f
                }
            };
            self.filters.push_back(filter);
            while self.bytes() > self.max_bytes && self.filters.len() > 1 {
                self.filters.pop_front();
            }
        }
        self.filters.back_mut().expect("just ensured").insert(item);
    }

    pub fn contains(&self, item: &[u8; 32]) -> bool {
        self.filters.iter().any(|f| f.contains(item))
    }

    /// Bytes held across the chain.
    pub fn bytes(&self) -> usize {
        self.filters.iter().map(Filter::bytes).sum()
    }
}

/// Everything delivered to one napplet: event ids and blob hashes.
#[derive(Default)]
pub struct Delivered {
    events: Scalable,
    blobs: Scalable,
}

impl std::fmt::Debug for Delivered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delivered")
            .field("event_bytes", &self.events.bytes())
            .field("blob_bytes", &self.blobs.bytes())
            .finish()
    }
}

impl Delivered {
    pub fn record_event(&mut self, id: &[u8; 32]) {
        self.events.insert(id);
    }

    pub fn record_blob(&mut self, sha256: &[u8; 32]) {
        self.blobs.insert(sha256);
    }

    pub fn has_event(&self, id: &[u8; 32]) -> bool {
        self.events.contains(id)
    }

    pub fn has_blob(&self, sha256: &[u8; 32]) -> bool {
        self.blobs.contains(sha256)
    }
}

/// One napplet's ledger, shared by its windows' sessions.
pub type Ledger = Arc<Mutex<Delivered>>;

/// Record every event found in `value` — an outgoing envelope's payload.
///
/// Walks the JSON rather than knowing each result's shape: an event is an
/// object with an `id`, a `pubkey` and a `sig`, wherever it sits
/// (`result.event`, an `events` array, an outbox answer). Anything that is
/// not a well-formed id is ignored.
pub fn record_events_in(ledger: &mut Delivered, value: &serde_json::Value) {
    record_walk(ledger, value, 0);
}

fn record_walk(ledger: &mut Delivered, value: &serde_json::Value, depth: u8) {
    if depth > 6 {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("sig") && map.contains_key("pubkey") {
                if let Some(id) = map.get("id").and_then(|v| v.as_str()).and_then(parse_hex32) {
                    ledger.record_event(&id);
                    return;
                }
            }
            for v in map.values() {
                record_walk(ledger, v, depth + 1);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                record_walk(ledger, v, depth + 1);
            }
        }
        _ => {}
    }
}

/// A 64-hex string (an event id, a sha256) as 32 bytes.
pub fn parse_hex32(hex: &str) -> Option<[u8; 32]> {
    nostr::EventId::from_hex(hex).ok().map(|id| id.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u64) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[..8].copy_from_slice(&n.to_le_bytes());
        k[8..16].copy_from_slice(&n.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
        k
    }

    #[test]
    fn no_false_negatives_across_growth() {
        let mut s = Scalable::default();
        for n in 0..20_000 {
            s.insert(&key(n));
        }
        assert!((0..20_000).all(|n| s.contains(&key(n))));
        assert!(s.filters.len() > 1, "the chain never grew");
    }

    #[test]
    fn false_positives_stay_near_target_as_it_grows() {
        for size in [BASE_CAPACITY, 4 * BASE_CAPACITY, 16 * BASE_CAPACITY] {
            let mut s = Scalable::default();
            for n in 0..size as u64 {
                s.insert(&key(n));
            }
            let probes = 200_000u64;
            let hits = (0..probes)
                .filter(|n| s.contains(&key(1_000_000_000 + n)))
                .count();
            let rate = hits as f64 / probes as f64;
            assert!(rate < TARGET_FPR * 2.0, "{size} ids gave {rate}");
        }
    }

    #[test]
    fn the_cap_holds_and_turns_the_chain_into_a_window() {
        let cap = 120 * 1024;
        let mut s = Scalable {
            max_bytes: cap,
            ..Scalable::default()
        };
        // Far past the point where a doubled filter would exceed the cap.
        for n in 0..300_000 {
            s.insert(&key(n));
            assert!(s.bytes() <= cap, "{} bytes after {n} ids", s.bytes());
        }
        assert!(
            !s.contains(&key(0)),
            "the oldest deliveries were not dropped"
        );
        assert!(s.contains(&key(299_999)), "the newest delivery was lost");
        assert!(s.filters.len() >= 2, "the window kept one filter only");
    }

    #[test]
    fn events_are_found_wherever_they_sit() {
        let id = "ab".repeat(32);
        let other = "cd".repeat(32);
        let payload = serde_json::json!({
            "subId": "s",
            "result": { "event": { "id": id, "pubkey": "p", "sig": "s" } },
            "events": [ { "id": other, "pubkey": "p", "sig": "s" } ],
            "noise": { "id": "ef".repeat(32) },
        });
        let mut d = Delivered::default();
        record_events_in(&mut d, &payload);
        assert!(d.has_event(&parse_hex32(&id).unwrap()));
        assert!(d.has_event(&parse_hex32(&other).unwrap()));
        assert!(
            !d.has_event(&parse_hex32(&"ef".repeat(32)).unwrap()),
            "a bare id counted as a delivered event"
        );
    }
}
