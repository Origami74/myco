//! Pure merge of the peer/advert/circle/pairing snapshots into one ordered,
//! npub-or-address-keyed `peers` array (D-19, DIAG-01/03/04/06).
//!
//! No I/O, no locks, no [`crate::runtime::AppRuntime`] dependency — everything
//! here is a plain transform over already-fetched slices, so it is
//! unit-testable on the host and runs allocation-only inside
//! `AppRuntime::state()`.
//!
//! Merge order follows RESEARCH.md's Pitfall 4: npub-first grouping silently
//! loses the "seen but not yet resolved" rows D-09 requires. The base
//! identity set is built from the radio-side peer views first (npub may be
//! empty), adverts are unioned in second, and only then is Circle/pairing
//! data left-joined by npub onto the rows that have one.

use std::collections::HashMap;

use fips::control::read_handle::PeerView;

use crate::content::{CircleContact, OutboundPairView, PairRequestView};
use crate::state::{BleAdvert, BlePeer, PeerDiagnosticView};

/// D-11 ordering weight for a row's `state` — lower sorts first.
fn peer_state_rank(state: &str) -> u8 {
    match state {
        "connected" => 0,
        "reachable-via-relay" => 1,
        "seen-unidentified" => 2,
        "paired-offline" => 3,
        // "unreachable" and any state string this module never emits.
        _ => 4,
    }
}

/// Fixed D-04 display order for `also_reachable_via` — never snapshot order.
const TRANSPORT_ORDER: [&str; 4] = ["ble", "aware", "udp", "tcp"];

/// Sort a set of transport names into the fixed D-04 order. Unknown transport
/// names sort after the four known ones, stably by their original order.
fn order_transports(mut transports: Vec<String>) -> Vec<String> {
    transports.sort_by_key(|t| {
        TRANSPORT_ORDER
            .iter()
            .position(|known| *known == t)
            .unwrap_or(TRANSPORT_ORDER.len())
    });
    transports
}

/// Truncate a peer-supplied string to at most `max_chars` characters, on a
/// UTF-8 char boundary, before it crosses the FFI (T-01-02: an oversized
/// Circle/display name must not reach a fixed-width Dev-tab row).
fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => s[..byte_idx].to_string(),
        None => s.to_string(),
    }
}

/// Shortened display fallback for a hex/npub string, matching the Kotlin
/// `short()` helper's `take(10)…takeLast(4)` convention above 18 characters.
fn short(s: &str) -> String {
    let len = s.chars().count();
    if len > 18 {
        let head: String = s.chars().take(10).collect();
        let tail: String = s.chars().skip(len - 4).collect();
        format!("{head}…{tail}")
    } else {
        s.to_string()
    }
}

/// Merge every peer/advert/circle/pairing snapshot into one ordered,
/// npub-or-address-keyed `peers` array. See the module doc for merge order.
///
/// `lane_by_npub` is a lane-origin override (npub → observed lane, e.g.
/// `"aware"`), consulted in preference to the raw fips-reported transport
/// name. It exists because Wi-Fi Aware and the LAN/AP lane both ride fips's
/// plain UDP transport and are indistinguishable from `PeerView.transport`
/// alone — only the Kotlin radio push site knows which one carried a given
/// peer. Empty in plan 01-01 (every transport passes through as fips
/// reported it, unmodified); plan 01-02 populates it from
/// `aware_bridge_jni.rs`. Never inferred from address shape (e.g.
/// link-local vs. routable) — that would be exactly the sort of
/// inference-presented-as-observation this phase prohibits.
///
/// `now_ms` is reserved for future staleness-based state work and unused
/// today (state is derived purely from connectivity/pairing facts, per D-10).
#[allow(clippy::too_many_arguments)]
pub fn merge_peers(
    peer_views: &[PeerView],
    ble_peers: &[BlePeer],
    ble_adverts: &[BleAdvert],
    circle: &[CircleContact],
    pending_pairs: &[PairRequestView],
    outbound_pairs: &[OutboundPairView],
    reachable_npubs: &[String],
    lane_by_npub: &HashMap<String, String>,
    _now_ms: u64,
) -> Vec<PeerDiagnosticView> {
    let mut rows: Vec<PeerDiagnosticView> = Vec::new();

    // Step 1: base identity set from ble_peers (npub may be empty — D-09),
    // enriched with last_seen_ms/transport/display_name from the matching
    // PeerView by node_addr_hex (ble_peers is built 1:1 from peer_views).
    // `transport` is the lane override when the npub has one, else passed
    // through exactly as fips observed it — an empty string means "no
    // resolved link", never a guessed default.
    for bp in ble_peers {
        let pv = peer_views
            .iter()
            .find(|p| p.node_addr_hex == bp.node_addr_hex);
        let key = if !bp.npub.is_empty() {
            bp.npub.clone()
        } else {
            bp.node_addr_hex.clone()
        };
        let name = pv
            .map(|p| truncate_chars(&p.display_name, 64))
            .unwrap_or_default();
        let transport = lane_by_npub
            .get(&bp.npub)
            .cloned()
            .or_else(|| pv.map(|p| p.transport.clone()))
            .unwrap_or_default();
        let last_seen_ms = pv.map(|p| p.last_seen_ms).unwrap_or(0);
        rows.push(PeerDiagnosticView {
            key,
            npub: bp.npub.clone(),
            node_addr_hex: bp.node_addr_hex.clone(),
            ble_addr: String::new(),
            name,
            // Only "connected" is decided here (the one state a row can
            // already know for certain); every other state is assigned in
            // step 5 once Circle/pairing/reachability data has been joined.
            state: if bp.connected {
                "connected".to_string()
            } else {
                String::new()
            },
            transport,
            also_reachable_via: Vec::new(),
            last_seen_ms,
            rssi: bp.rssi,
            psm: bp.psm,
            pair_state: String::new(),
            in_circle: false,
        });
    }

    // Step 2: union in adverts as additional not-yet-resolved rows keyed by
    // BLE address, but first attach any advert whose address is already
    // attributed to an existing row (its rssi, psm and ble_addr) instead of
    // creating a second row — a duplicate advert for the same address is the
    // one attribution case this plan's data can exercise; the fuller
    // address-to-node-address map lands in plan 01-03.
    for adv in ble_adverts {
        if let Some(row) = rows
            .iter_mut()
            .find(|r| r.ble_addr == adv.addr || r.key == adv.addr)
        {
            row.ble_addr = adv.addr.clone();
            row.rssi = Some(adv.rssi);
            row.psm = adv.psm;
        } else {
            rows.push(PeerDiagnosticView {
                key: adv.addr.clone(),
                npub: String::new(),
                node_addr_hex: String::new(),
                ble_addr: adv.addr.clone(),
                name: String::new(),
                state: String::new(),
                transport: String::new(),
                also_reachable_via: Vec::new(),
                last_seen_ms: 0,
                rssi: Some(adv.rssi),
                psm: adv.psm,
                pair_state: String::new(),
                in_circle: false,
            });
        }
    }

    // Step 3: union in every npub that appears only in the Circle, the
    // incoming pair requests or the outbound invites, so a pairing with no
    // radio contact yet still has a row.
    let mut known_npubs: std::collections::HashSet<String> = rows
        .iter()
        .filter(|r| !r.npub.is_empty())
        .map(|r| r.npub.clone())
        .collect();
    let mut extra_npubs: Vec<String> = Vec::new();
    for npub in circle
        .iter()
        .map(|c| &c.npub)
        .chain(pending_pairs.iter().map(|p| &p.npub))
        .chain(outbound_pairs.iter().map(|o| &o.npub))
    {
        if known_npubs.insert(npub.clone()) {
            extra_npubs.push(npub.clone());
        }
    }
    for npub in extra_npubs {
        rows.push(PeerDiagnosticView {
            key: npub.clone(),
            npub,
            node_addr_hex: String::new(),
            ble_addr: String::new(),
            name: String::new(),
            state: String::new(),
            transport: String::new(),
            also_reachable_via: Vec::new(),
            last_seen_ms: 0,
            rssi: None,
            psm: 0,
            pair_state: String::new(),
            in_circle: false,
        });
    }

    // Step 4: left-join Circle name, pair state and relay reachability onto
    // rows that have an npub. This module never reads
    // `PairRequestView`'s one-time credential field — only `npub`/`name`.
    for row in rows.iter_mut() {
        if row.npub.is_empty() {
            continue;
        }
        if let Some(c) = circle.iter().find(|c| c.npub == row.npub) {
            row.in_circle = true;
            if row.name.is_empty() {
                row.name = if c.name.is_empty() {
                    short(&row.npub)
                } else {
                    truncate_chars(&c.name, 64)
                };
            }
        }
        let incoming = pending_pairs.iter().any(|p| p.npub == row.npub);
        let outbound = outbound_pairs.iter().any(|o| o.npub == row.npub);
        row.pair_state = match (incoming, outbound) {
            (true, true) => "incoming-waiting+outbound-waiting".to_string(),
            (true, false) => "incoming-waiting".to_string(),
            (false, true) => "outbound-waiting".to_string(),
            (false, false) if row.in_circle => "paired".to_string(),
            (false, false) => String::new(),
        };
        row.also_reachable_via = order_transports(std::mem::take(&mut row.also_reachable_via));
    }

    // Step 5: assign the final state last (D-10's five-state vocabulary).
    let reachable: std::collections::HashSet<&str> =
        reachable_npubs.iter().map(|s| s.as_str()).collect();
    for row in rows.iter_mut() {
        row.state = if row.state == "connected" {
            "connected".to_string()
        } else if !row.npub.is_empty() && reachable.contains(row.npub.as_str()) {
            "reachable-via-relay".to_string()
        } else if row.npub.is_empty() {
            "seen-unidentified".to_string()
        } else if row.in_circle || !row.pair_state.is_empty() {
            "paired-offline".to_string()
        } else {
            "unreachable".to_string()
        };
    }

    // Step 6: sort by state rank, then last_seen_ms descending, then key
    // ascending — a total order, so two polls over the same data always
    // produce the same sequence (D-11).
    rows.sort_by(|a, b| {
        peer_state_rank(&a.state)
            .cmp(&peer_state_rank(&b.state))
            .then_with(|| b.last_seen_ms.cmp(&a.last_seen_ms))
            .then_with(|| a.key.cmp(&b.key))
    });

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pv(
        node_addr_hex: &str,
        npub: &str,
        connected: bool,
        last_seen_ms: u64,
        transport: &str,
    ) -> PeerView {
        PeerView {
            node_addr_hex: node_addr_hex.to_string(),
            npub: npub.to_string(),
            connected,
            last_seen_ms,
            transport: transport.to_string(),
            display_name: String::new(),
        }
    }

    fn bp(node_addr_hex: &str, npub: &str, connected: bool) -> BlePeer {
        BlePeer {
            node_addr_hex: node_addr_hex.to_string(),
            npub: npub.to_string(),
            connected,
            psm: 0,
            rssi: None,
        }
    }

    fn circle(npub: &str, name: &str) -> CircleContact {
        CircleContact {
            npub: npub.to_string(),
            name: name.to_string(),
            added_at: 0,
        }
    }

    fn pending(npub: &str, name: &str) -> PairRequestView {
        PairRequestView {
            npub: npub.to_string(),
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn outbound(npub: &str, name: &str) -> OutboundPairView {
        OutboundPairView {
            npub: npub.to_string(),
            name: name.to_string(),
            since: 0,
        }
    }

    #[test]
    fn connected_sorts_before_paired_offline_regardless_of_last_heard() {
        let views = vec![pv("a1", "npub1connected", true, 1_000, "udp")];
        let peers = vec![bp("a1", "npub1connected", true)];
        let members = vec![circle("npub2offline", "Offline Friend")];
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            0,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].npub, "npub1connected");
        assert_eq!(out[0].state, "connected");
        assert_eq!(out[1].npub, "npub2offline");
        assert_eq!(out[1].state, "paired-offline");
    }

    #[test]
    fn same_state_orders_by_last_heard_descending() {
        let views = vec![
            pv("a1", "npub-older", true, 1_000, "udp"),
            pv("a2", "npub-newer", true, 9_000, "udp"),
        ];
        let peers = vec![bp("a1", "npub-older", true), bp("a2", "npub-newer", true)];
        let out = merge_peers(&views, &peers, &[], &[], &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out[0].npub, "npub-newer");
        assert_eq!(out[1].npub, "npub-older");
    }

    #[test]
    fn same_state_and_last_heard_ties_break_on_key_ascending() {
        let views = vec![
            pv("a1", "npub-zzz", true, 5_000, "udp"),
            pv("a2", "npub-aaa", true, 5_000, "udp"),
        ];
        let peers = vec![bp("a1", "npub-zzz", true), bp("a2", "npub-aaa", true)];
        let out = merge_peers(&views, &peers, &[], &[], &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out[0].npub, "npub-aaa");
        assert_eq!(out[1].npub, "npub-zzz");
    }

    #[test]
    fn sort_is_stable_across_shuffled_input() {
        let views_a = vec![
            pv("a1", "npub-a", true, 5_000, "udp"),
            pv("a2", "npub-b", false, 1_000, ""),
            pv("a3", "npub-c", true, 5_000, "ble"),
        ];
        let peers_a = vec![
            bp("a1", "npub-a", true),
            bp("a2", "npub-b", false),
            bp("a3", "npub-c", true),
        ];
        // The same three entries in a different input order.
        let views_b = vec![
            pv("a3", "npub-c", true, 5_000, "ble"),
            pv("a1", "npub-a", true, 5_000, "udp"),
            pv("a2", "npub-b", false, 1_000, ""),
        ];
        let peers_b = vec![
            bp("a3", "npub-c", true),
            bp("a1", "npub-a", true),
            bp("a2", "npub-b", false),
        ];
        let members = vec![circle("npub-b", "Offline Friend")];

        let out_a = merge_peers(
            &views_a,
            &peers_a,
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            0,
        );
        let out_b = merge_peers(
            &views_b,
            &peers_b,
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            0,
        );
        let keys_a: Vec<&str> = out_a.iter().map(|r| r.key.as_str()).collect();
        let keys_b: Vec<&str> = out_b.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys_a, keys_b,
            "shuffled input must not change output order"
        );
    }

    #[test]
    fn ble_peer_with_empty_npub_is_seen_unidentified_keyed_by_node_addr() {
        let views = vec![pv("addrhex1", "", false, 0, "")];
        let peers = vec![bp("addrhex1", "", false)];
        let out = merge_peers(&views, &peers, &[], &[], &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, "addrhex1");
        assert_eq!(out[0].state, "seen-unidentified");
    }

    #[test]
    fn unmatched_advert_creates_seen_unidentified_row_with_rssi_psm() {
        let advert = BleAdvert {
            addr: "adapter/AA:BB:CC:DD:EE:FF".to_string(),
            psm: 129,
            rssi: -55,
        };
        let out = merge_peers(
            &[],
            &[],
            std::slice::from_ref(&advert),
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, advert.addr);
        assert_eq!(out[0].state, "seen-unidentified");
        assert_eq!(out[0].rssi, Some(-55));
        assert_eq!(out[0].psm, 129);
    }

    #[test]
    fn matched_advert_attaches_to_existing_row_no_duplicate() {
        let adverts = vec![
            BleAdvert {
                addr: "adapter/AA:BB".to_string(),
                psm: 1,
                rssi: -70,
            },
            BleAdvert {
                addr: "adapter/AA:BB".to_string(),
                psm: 2,
                rssi: -40,
            },
        ];
        let out = merge_peers(&[], &[], &adverts, &[], &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(
            out.len(),
            1,
            "duplicate advert address must not produce a second row"
        );
        assert_eq!(out[0].psm, 2);
        assert_eq!(out[0].rssi, Some(-40));
    }

    #[test]
    fn circle_only_npub_with_no_radio_or_pairing_is_paired_offline() {
        let members = vec![circle("npub-offline", "Friend")];
        let out = merge_peers(&[], &[], &[], &members, &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].state, "paired-offline");
        assert!(out[0].in_circle);
    }

    #[test]
    fn circle_npub_in_reachable_npubs_is_reachable_via_relay() {
        let members = vec![circle("npub-relay", "Friend")];
        let reachable = vec!["npub-relay".to_string()];
        let out = merge_peers(
            &[],
            &[],
            &[],
            &members,
            &[],
            &[],
            &reachable,
            &HashMap::new(),
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].state, "reachable-via-relay");
    }

    #[test]
    fn pending_pair_only_npub_has_incoming_waiting_pair_state() {
        let pending_pairs = vec![pending("npub-inbound", "Requester")];
        let out = merge_peers(
            &[],
            &[],
            &[],
            &[],
            &pending_pairs,
            &[],
            &[],
            &HashMap::new(),
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pair_state, "incoming-waiting");
    }

    #[test]
    fn npub_with_incoming_and_outbound_pair_produces_one_row_naming_both() {
        let pending_pairs = vec![pending("npub-both", "Requester")];
        let outbound_pairs = vec![outbound("npub-both", "Requester")];
        let out = merge_peers(
            &[],
            &[],
            &[],
            &[],
            &pending_pairs,
            &outbound_pairs,
            &[],
            &HashMap::new(),
            0,
        );
        assert_eq!(out.len(), 1, "one row, not two");
        assert!(out[0].pair_state.contains("incoming-waiting"));
        assert!(out[0].pair_state.contains("outbound-waiting"));
    }

    #[test]
    fn empty_circle_name_falls_back_to_shortened_npub() {
        let members = vec![circle(
            "npub1verylongidentifierthatexceedseighteenchars",
            "",
        )];
        let out = merge_peers(&[], &[], &[], &members, &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out.len(), 1);
        assert!(!out[0].name.is_empty(), "must never render an empty name");
        assert!(
            out[0].name.contains('…'),
            "must fall back to the shortened npub"
        );
    }

    #[test]
    fn long_name_is_truncated_to_64_chars() {
        let long_name = "x".repeat(200);
        let members = vec![circle("npub-longname", &long_name)];
        let out = merge_peers(&[], &[], &[], &members, &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name.chars().count(), 64);
    }

    #[test]
    fn empty_inputs_produce_empty_vec_not_panic() {
        let out = merge_peers(&[], &[], &[], &[], &[], &[], &[], &HashMap::new(), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn also_reachable_via_orders_ble_aware_udp_tcp_regardless_of_input_order() {
        let shuffled = vec![
            "tcp".to_string(),
            "ble".to_string(),
            "udp".to_string(),
            "aware".to_string(),
        ];
        let ordered = order_transports(shuffled);
        assert_eq!(ordered, vec!["ble", "aware", "udp", "tcp"]);
    }

    #[test]
    fn connected_transport_passes_through_without_fabricating_a_default() {
        // A connected peer whose PeerView carries no resolved link_info must
        // render an empty transport, never a guessed "ble" default — the
        // plan's own must_haves forbid presenting an inferred value as an
        // observed fact.
        let views = vec![pv("a1", "npub-no-transport", true, 1_000, "")];
        let peers = vec![bp("a1", "npub-no-transport", true)];
        let out = merge_peers(&views, &peers, &[], &[], &[], &[], &[], &HashMap::new(), 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].state, "connected");
        assert_eq!(
            out[0].transport, "",
            "must not fabricate a transport fips did not observe"
        );
    }

    #[test]
    fn lane_override_takes_precedence_over_raw_fips_transport() {
        // Scope handoff to 01-02: both Wi-Fi Aware and the LAN/AP lane ride
        // fips's plain UDP transport and share one JNI push site today, so
        // fips reports "udp" for both. Once 01-02 threads a real npub→lane
        // map through from the Kotlin push site, the override must win.
        let views = vec![pv("a1", "npub-aware", true, 1_000, "udp")];
        let peers = vec![bp("a1", "npub-aware", true)];
        let mut lane_by_npub = HashMap::new();
        lane_by_npub.insert("npub-aware".to_string(), "aware".to_string());
        let out = merge_peers(&views, &peers, &[], &[], &[], &[], &[], &lane_by_npub, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transport, "aware");
    }

    #[test]
    fn npub_absent_from_lane_map_falls_back_to_raw_fips_transport() {
        let views = vec![pv("a1", "npub-plain-udp", true, 1_000, "udp")];
        let peers = vec![bp("a1", "npub-plain-udp", true)];
        let mut lane_by_npub = HashMap::new();
        lane_by_npub.insert("some-other-npub".to_string(), "aware".to_string());
        let out = merge_peers(&views, &peers, &[], &[], &[], &[], &[], &lane_by_npub, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transport, "udp");
    }
}
