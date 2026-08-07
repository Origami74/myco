//! npub → observed lane record — the lane disambiguation seam for D-19's
//! merged peer diagnostics row (01-02).
//!
//! Wi-Fi Aware and the LAN/AP lane both ride fips's plain UDP transport and
//! share one JNI push site (`aware_bridge_jni.rs`'s `TRANSPORT_TYPE =
//! "udp"`), so fips itself structurally cannot tell them apart — only the
//! Kotlin radio that observed a given peer knows which lane carried it. This
//! module holds that record: plain lock-based state with no JNI or Android
//! dependency, so it is unit-testable on the host even though
//! `aware_bridge_jni.rs` (Android-only) is its sole real caller, pushing on
//! every `awarePeerFound`/`awarePeerLost`. `AppRuntime::state()` reads
//! [`snapshot`] into `merge_peers()`'s `lane_by_npub` parameter (01-01's
//! seam).
//!
//! Never inferred from address shape (e.g. link-local vs. routable) — only
//! ever set from an explicit Kotlin push, per this phase's prohibition on
//! presenting inference as observation.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static OBSERVED_LANE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn observed_lane() -> &'static Mutex<HashMap<String, String>> {
    OBSERVED_LANE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record that `npub` was last observed reachable via `lane` (`"aware"` or
/// `"udp"`) — the found side of the pair.
pub(crate) fn set_lane(npub: &str, lane: &str) {
    observed_lane()
        .lock()
        .unwrap()
        .insert(npub.to_string(), lane.to_string());
}

/// Clear `npub`'s recorded lane, but only if it still equals `lane` — a lost
/// event from a lane that has since been superseded by a fresher found from
/// the other lane must not erase the newer record.
pub(crate) fn clear_lane(npub: &str, lane: &str) {
    let mut map = observed_lane().lock().unwrap();
    if map.get(npub).map(String::as_str) == Some(lane) {
        map.remove(npub);
    }
}

/// A snapshot of every npub's currently observed lane, for
/// `merge_peers()`'s `lane_by_npub` parameter. Cloned rather than held
/// locked across the merge call.
pub(crate) fn snapshot() -> HashMap<String, String> {
    observed_lane().lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test below uses its own npub — the map is a process-global shared
    // across every test in this binary, so a shared key would let one test
    // observe another's writes and flake under parallel `cargo test`.

    #[test]
    fn set_then_snapshot_reports_the_pushed_lane() {
        set_lane("npub-set-snapshot", "aware");
        assert_eq!(
            snapshot().get("npub-set-snapshot").map(String::as_str),
            Some("aware")
        );
    }

    #[test]
    fn a_second_set_overwrites_the_first() {
        set_lane("npub-overwrite", "udp");
        set_lane("npub-overwrite", "aware");
        assert_eq!(
            snapshot().get("npub-overwrite").map(String::as_str),
            Some("aware")
        );
    }

    #[test]
    fn clear_with_matching_lane_removes_the_entry() {
        set_lane("npub-clear-match", "aware");
        clear_lane("npub-clear-match", "aware");
        assert_eq!(snapshot().get("npub-clear-match"), None);
    }

    #[test]
    fn clear_with_a_stale_lane_does_not_clobber_a_fresher_record() {
        // aware found, then udp found (fresher) for the same npub, then a
        // late-arriving aware lost must not erase the udp record.
        set_lane("npub-clear-stale", "aware");
        set_lane("npub-clear-stale", "udp");
        clear_lane("npub-clear-stale", "aware");
        assert_eq!(
            snapshot().get("npub-clear-stale").map(String::as_str),
            Some("udp"),
            "a stale lost from the superseded lane must not clear the fresher record"
        );
    }

    #[test]
    fn unset_npub_is_absent_from_the_snapshot() {
        assert_eq!(snapshot().get("npub-never-pushed-xyz"), None);
    }
}
