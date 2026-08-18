//! Node-address prefix → self-advertised display name.
//!
//! The name a device chose for itself is otherwise invisible until you have
//! exchanged pair traffic with it: the fips handshake does not carry one, and
//! `PeerView.display_name` is an abbreviated npub. This module holds the one
//! place it *is* observable before pairing — the string a peer puts in its BLE
//! scan response — keyed by the leading bytes of the peer's own `node_addr`,
//! which it advertises alongside the name.
//!
//! Keyed on the node address rather than the BLE MAC it arrived on, because
//! those are not the same question. A device is routinely *discovered* by BLE
//! advert and then *carried* over the LAN lane, and its peer row is keyed by
//! node address; a MAC-keyed name could only ever be attached to peers
//! currently on BLE, which is the minority case. The advertiser therefore says
//! who it is, and no address mapping is needed at all.
//!
//! Myco-owned on purpose. The name is a Myco-layer nicety with no bearing on
//! routing, so it stays out of the fips bridge's `deliver_scan` path and rides
//! its own JNI push instead, exactly as [`crate::lane_observation`] does for
//! lane origin. Plain lock-based state with no JNI or Android dependency, so it
//! is unit-testable on the host.
//!
//! **This name is unauthenticated.** It arrives in a plaintext broadcast that
//! anyone in radio range can forge, so it is only ever a fallback *below* every
//! name learned from signed pair traffic — never an override. Callers must
//! preserve that ordering.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Longest advertised name accepted. A BLE scan response is 31 bytes, the
/// service-data header eats four and the node-address prefix six, so nothing
/// longer can arrive intact; truncating here as well means a hostile or buggy
/// advertiser cannot push an oversized string into the map either.
pub(crate) const MAX_ADVERT_NAME_BYTES: usize = 21;

/// Cap on distinct devices remembered. A node address is stable, so this grows
/// far slower than a MAC-keyed map would — but it is still unbounded input from
/// the air. Well above any plausible room; a bound, not a policy.
const MAX_ENTRIES: usize = 512;

static ADVERT_NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn advert_names() -> &'static Mutex<HashMap<String, String>> {
    ADVERT_NAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record the name the device whose `node_addr` starts with `node_prefix` (hex)
/// is advertising for itself.
///
/// A blank name clears the entry rather than storing an empty string — a peer
/// that stopped advertising a name has no name, which is not the same as
/// having one that is empty.
pub(crate) fn set_name(node_prefix: &str, name: &str) {
    let trimmed = truncate_bytes(name.trim(), MAX_ADVERT_NAME_BYTES);
    let mut map = advert_names().lock().unwrap();
    if trimmed.is_empty() {
        map.remove(node_prefix);
        return;
    }
    // Only refuse *new* addresses at the cap, so an already-tracked device can
    // still update its name in a saturated map.
    if map.len() >= MAX_ENTRIES && !map.contains_key(node_prefix) {
        return;
    }
    map.insert(node_prefix.to_ascii_lowercase(), trimmed);
}

/// A snapshot of every advertised name, for `merge_peers()`'s
/// `advert_names` parameter. Cloned rather than held locked across the merge.
pub(crate) fn snapshot() -> HashMap<String, String> {
    advert_names().lock().unwrap().clone()
}

/// Truncate to at most `max` bytes on a UTF-8 character boundary.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses its own key — the map is a process-global shared across
    // every test in this binary.

    /// Keys are compared against lowercase hex from the peer row, so an
    /// uppercase push must still be findable.
    #[test]
    fn keys_are_normalised_to_lowercase_hex() {
        set_name("AABB00000005", "Shouty");
        assert_eq!(
            snapshot().get("aabb00000005").map(String::as_str),
            Some("Shouty")
        );
    }

    #[test]
    fn a_pushed_name_reaches_the_snapshot() {
        set_name("aabb00000001", "DC-1");
        assert_eq!(
            snapshot().get("aabb00000001").map(String::as_str),
            Some("DC-1")
        );
    }

    #[test]
    fn a_blank_name_clears_rather_than_storing_an_empty_string() {
        set_name("aabb00000002", "gone");
        set_name("aabb00000002", "   ");
        assert_eq!(snapshot().get("aabb00000002"), None);
    }

    /// A multi-byte name cut at the cap must stay valid UTF-8 — the truncation
    /// point is a character boundary, not a byte offset.
    #[test]
    fn an_oversized_name_is_cut_on_a_character_boundary() {
        let long = "é".repeat(40); // 80 bytes
        set_name("aabb00000003", &long);
        let stored = snapshot().get("aabb00000003").cloned().expect("stored");
        assert!(stored.len() <= MAX_ADVERT_NAME_BYTES);
        assert!(stored.chars().all(|c| c == 'é'), "{stored}");
    }

    #[test]
    fn a_later_push_replaces_the_earlier_name() {
        set_name("aabb00000004", "old");
        set_name("aabb00000004", "new");
        assert_eq!(
            snapshot().get("aabb00000004").map(String::as_str),
            Some("new")
        );
    }
}
