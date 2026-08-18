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

use crate::control_client::PeerView;

use crate::ble_diag::{BlePeerAttempts, MAX_ATTEMPTS_PER_PEER};

use crate::content::{CircleContact, OutboundPairView, PairRequestView};
use crate::state::{BleAdvert, BlePeer, PeerAttemptView, PeerDiagnosticView};

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
/// `advert_names` is BLE-address-keyed self-advertised display names, read out
/// of peers' scan responses. It is joined on after adverts have been attributed
/// to rows, so a row only carries one if a scan actually saw that address. The
/// value is unauthenticated (see [`crate::advert_names`]) and is therefore only
/// ever surfaced as a fallback below the Circle/pairing names.
///
/// `ble_attempts` is the per-peer BLE connect-attempt log, keyed by BLE
/// address. It supplies three things: the role/discovery/outcome history joined
/// onto each row, the per-peer link send-failure count, and the learned
/// address-to-node-address pairs that let step 2 attribute an advert to an
/// existing peer row instead of emitting a second one (D-09). Empty on the host
/// build, where there is no radio.
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
    advert_names: &HashMap<String, String>,
    ble_attempts: &[BlePeerAttempts],
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
        let authenticated_at_ms = pv.map(|p| p.authenticated_at_ms).unwrap_or(0);
        // `None` both when there is no PeerView and when MMP has not measured
        // the link yet — the row cannot tell those apart and must not pretend.
        let srtt_ms = pv.and_then(|p| p.srtt_ms);
        // A BLE peer's link address is its scan address, so recording it here
        // is what lets steps 2 and 5b attribute that device's adverts — its
        // RSSI and the name it broadcasts — to this row. Only for BLE: on the
        // IP transports the same field is a socket address, which would key
        // into nothing and match nothing.
        let ble_addr = match pv {
            Some(p) if p.transport == "ble" => p.transport_addr.clone(),
            _ => String::new(),
        };
        rows.push(PeerDiagnosticView {
            key,
            npub: bp.npub.clone(),
            node_addr_hex: bp.node_addr_hex.clone(),
            ble_addr,
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
            authenticated_at_ms,
            advertised_name: String::new(),
            srtt_ms,
            rssi: bp.rssi,
            psm: bp.psm,
            pair_state: String::new(),
            in_circle: false,
            role: String::new(),
            discovery_ms: 0,
            send_drops: 0,
            attempts: Vec::new(),
        });
    }

    // Address-to-node-address pairs the attempt log learned from attempts that
    // got far enough to see a peer pubkey. This is the mapping 01-01 lacked; it
    // is what lets step 2 below collapse a raw advert into the peer row it
    // actually belongs to (D-09) rather than emitting a second row for the same
    // device. Only pairs the log actually learned appear here — nothing is
    // inferred from address shape.
    let learned_node_addr: HashMap<&str, &str> = ble_attempts
        .iter()
        .filter(|a| !a.node_addr_hex.is_empty())
        .map(|a| (a.ble_addr.as_str(), a.node_addr_hex.as_str()))
        .collect();

    // Step 2: union in adverts as additional not-yet-resolved rows keyed by
    // BLE address, but first attach any advert already attributed to an
    // existing row (its rssi, psm and ble_addr) instead of creating a second
    // row. An advert is attributed either because the row already carries that
    // address, or because the attempt log has learned which node address that
    // BLE address belongs to.
    for adv in ble_adverts {
        let learned = learned_node_addr.get(adv.addr.as_str()).copied();
        if let Some(row) = rows.iter_mut().find(|r| {
            r.ble_addr == adv.addr
                || r.key == adv.addr
                || learned
                    .is_some_and(|node| !r.node_addr_hex.is_empty() && r.node_addr_hex == node)
        }) {
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
                authenticated_at_ms: 0,
                advertised_name: String::new(),
                srtt_ms: None,
                rssi: Some(adv.rssi),
                psm: adv.psm,
                pair_state: String::new(),
                in_circle: false,
                role: String::new(),
                discovery_ms: 0,
                send_drops: 0,
                attempts: Vec::new(),
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
            authenticated_at_ms: 0,
            advertised_name: String::new(),
            srtt_ms: None,
            rssi: None,
            psm: 0,
            pair_state: String::new(),
            in_circle: false,
            role: String::new(),
            discovery_ms: 0,
            send_drops: 0,
            attempts: Vec::new(),
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

    // Step 5b: join self-advertised names on by BLE address. Runs after step 2
    // has attributed adverts to rows, so `ble_addr` is populated wherever a
    // scan actually saw the device. Nothing is written for a row we never saw
    // an advert for — the absence of a broadcast name is a fact, and inventing
    // one from the npub here would rob the display layer of the distinction.
    for row in rows.iter_mut() {
        if row.ble_addr.is_empty() {
            continue;
        }
        if let Some(name) = advert_names.get(&row.ble_addr) {
            row.advertised_name = name.clone();
        }
    }

    // Step 6: join the recorded attempt history onto each row. Rows are matched
    // by BLE address, or by the node address the attempt log learned for that
    // address. A row with no recorded attempts keeps an empty list, an empty
    // role and zero counters — never a fabricated entry (DIAG-01/03).
    for row in rows.iter_mut() {
        let recorded = if !row.ble_addr.is_empty() {
            ble_attempts.iter().find(|a| a.ble_addr == row.ble_addr)
        } else if !row.node_addr_hex.is_empty() {
            ble_attempts
                .iter()
                .find(|a| a.node_addr_hex == row.node_addr_hex)
        } else {
            None
        };
        let Some(rec) = recorded else { continue };

        row.send_drops = rec.send_failures;
        // Newest first for display. The log already caps each address at
        // MAX_ATTEMPTS_PER_PEER; taking it again bounds what crosses the FFI
        // regardless of what the transport hands over.
        row.attempts = rec
            .attempts
            .iter()
            .rev()
            .take(MAX_ATTEMPTS_PER_PEER)
            .map(|a| PeerAttemptView {
                at_ms: a.at_ms,
                role: a.role.as_str().to_string(),
                discovery_ms: a.discovery_ms,
                outcome: a.outcome.as_str().to_string(),
            })
            .collect();
        // The row's headline role/duration are the newest attempt's, so a peer
        // that has never resolved an attempt shows neither.
        if let Some(newest) = row.attempts.first() {
            row.role = newest.role.clone();
            row.discovery_ms = newest.discovery_ms;
        }
        // A row reached only through the learned node-address pair has no BLE
        // address of its own yet; adopt the one its attempts were recorded under.
        if row.ble_addr.is_empty() {
            row.ble_addr = rec.ble_addr.clone();
        }
    }

    // Step 7: sort by state rank, then last_seen_ms descending, then key
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
            authenticated_at_ms: 0,
            transport: transport.to_string(),
            transport_addr: String::new(),
            srtt_ms: None,
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

    /// An inbound BLE peer — one we never dialled, so the attempt log knows
    /// nothing about it — must still be attributed its own adverts. Its link
    /// address is what supplies that, and without it the RSSI and the
    /// self-advertised name both go missing on exactly the peers most likely to
    /// have connected that way.
    #[test]
    fn an_inbound_ble_peer_is_keyed_by_its_link_address() {
        let mut view = pv("a1", "npub-inbound", true, 1_000, "ble");
        view.transport_addr = "ble0/77:B5:98:5E:D1:E6".to_string();
        let mut ip = pv("a2", "npub-over-ip", true, 1_000, "udp");
        ip.transport_addr = "[::ffff:192.168.8.238]:2121".to_string();
        let peers = vec![
            bp("a1", "npub-inbound", true),
            bp("a2", "npub-over-ip", true),
        ];
        let adverts = vec![BleAdvert {
            addr: "ble0/77:B5:98:5E:D1:E6".to_string(),
            psm: 196,
            rssi: -61,
        }];
        let names = HashMap::from([(
            "ble0/77:B5:98:5E:D1:E6".to_string(),
            "orchid eero".to_string(),
        )]);
        let out = merge_peers(
            &[view, ip],
            &peers,
            &adverts,
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &names,
            // Deliberately empty: this is the no-dial-history case.
            &[],
            0,
        );
        assert_eq!(out.len(), 2, "the advert must not become a second row");
        let inbound = out.iter().find(|r| r.npub == "npub-inbound").expect("row");
        assert_eq!(inbound.ble_addr, "ble0/77:B5:98:5E:D1:E6");
        assert_eq!(inbound.rssi, Some(-61));
        assert_eq!(inbound.advertised_name, "orchid eero");
        // A socket address is not a scan address and must key into nothing.
        let over_ip = out.iter().find(|r| r.npub == "npub-over-ip").expect("row");
        assert_eq!(over_ip.ble_addr, "");
        assert_eq!(over_ip.advertised_name, "");
    }

    /// A self-advertised name is joined on by BLE address, and only for a row
    /// an advert was actually attributed to. A peer seen over another lane
    /// entirely carries none — the display layer has to be able to tell "chose
    /// no name" from "we never heard one".
    #[test]
    fn an_advertised_name_lands_on_the_row_its_advert_was_attributed_to() {
        let views = vec![
            pv("a1", "npub-ble-seen", true, 1_000, "ble"),
            pv("a2", "npub-udp-only", true, 1_000, "udp"),
        ];
        let peers = vec![
            bp("a1", "npub-ble-seen", true),
            bp("a2", "npub-udp-only", true),
        ];
        // The advert's address is learned onto the first row by the attempt log.
        let attempts = vec![BlePeerAttempts {
            ble_addr: "ble0/AA:01".to_string(),
            node_addr_hex: "a1".to_string(),
            send_failures: 0,
            attempts: Vec::new(),
        }];
        let adverts = vec![BleAdvert {
            addr: "ble0/AA:01".to_string(),
            psm: 133,
            rssi: -55,
        }];
        let names = HashMap::from([("ble0/AA:01".to_string(), "DC-1".to_string())]);
        let out = merge_peers(
            &views,
            &peers,
            &adverts,
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &names,
            &attempts,
            0,
        );
        let seen = out.iter().find(|r| r.npub == "npub-ble-seen").expect("row");
        assert_eq!(seen.advertised_name, "DC-1");
        let other = out.iter().find(|r| r.npub == "npub-udp-only").expect("row");
        assert_eq!(
            other.advertised_name, "",
            "a row with no attributed advert must not borrow another's name"
        );
    }

    /// The ping the status panel shows is the peer's MMP SRTT, carried through
    /// untouched. A row with no `PeerView` behind it (a Circle member who is
    /// merely paired-offline) carries `None` rather than inheriting anyone's.
    #[test]
    fn srtt_rides_the_peer_view_onto_the_row() {
        let mut view = pv("a1", "npub1connected", true, 1_000, "udp");
        view.srtt_ms = Some(37.5);
        let peers = vec![bp("a1", "npub1connected", true)];
        let members = vec![circle("npub2offline", "Offline Friend")];
        let out = merge_peers(
            &[view],
            &peers,
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
        assert_eq!(out[0].srtt_ms, Some(37.5));
        assert_eq!(out[1].srtt_ms, None);
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
            &HashMap::new(),
            &[],
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
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
            &HashMap::new(),
            &[],
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
            &HashMap::new(),
            &[],
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
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
            &HashMap::new(),
            &[],
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
        let out = merge_peers(
            &[],
            &[],
            &adverts,
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
        let out = merge_peers(
            &[],
            &[],
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
            &HashMap::new(),
            &[],
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
            &HashMap::new(),
            &[],
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
            &HashMap::new(),
            &[],
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
        let out = merge_peers(
            &[],
            &[],
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
        let out = merge_peers(
            &[],
            &[],
            &[],
            &members,
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name.chars().count(), 64);
    }

    #[test]
    fn empty_inputs_produce_empty_vec_not_panic() {
        let out = merge_peers(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
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
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &lane_by_npub,
            &HashMap::new(),
            &[],
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transport, "aware");
    }

    #[test]
    fn npub_absent_from_lane_map_falls_back_to_raw_fips_transport() {
        let views = vec![pv("a1", "npub-plain-udp", true, 1_000, "udp")];
        let peers = vec![bp("a1", "npub-plain-udp", true)];
        let mut lane_by_npub = HashMap::new();
        lane_by_npub.insert("some-other-npub".to_string(), "aware".to_string());
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &lane_by_npub,
            &HashMap::new(),
            &[],
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transport, "udp");
    }

    /// The far end of the tracer: a lost outbound tiebreaker recorded inside the
    /// fips BLE transport must survive the merge and reach the serialized
    /// `AppState` JSON carrying its role, discovery duration and outcome.
    #[test]
    fn recorded_lost_tiebreaker_reaches_the_serialized_row() {
        use crate::ble_diag::{BleAttempt, BleAttemptOutcome, BleRole};

        let views = vec![pv("beef", "npub-tiebreak", false, 1_000, "ble")];
        let peers = vec![bp("beef", "npub-tiebreak", false)];
        let recorded = vec![BlePeerAttempts {
            ble_addr: "ble0/AA:BB:CC:DD:EE:FF".to_string(),
            node_addr_hex: "beef".to_string(),
            send_failures: 3,
            attempts: vec![BleAttempt {
                at_ms: 1_700_000_000_000,
                ble_addr: "ble0/AA:BB:CC:DD:EE:FF".to_string(),
                node_addr_hex: "beef".to_string(),
                role: BleRole::Central,
                discovery_ms: 742,
                outcome: BleAttemptOutcome::LostTiebreaker,
            }],
        }];

        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &recorded,
            0,
        );
        assert_eq!(out.len(), 1);

        let row = &out[0];
        assert_eq!(row.role, "central");
        assert_eq!(row.discovery_ms, 742);
        assert_eq!(row.send_drops, 3);
        assert_eq!(row.attempts.len(), 1);
        // The row adopts the address its attempts were recorded under.
        assert_eq!(row.ble_addr, "ble0/AA:BB:CC:DD:EE:FF");

        // Prove it survives serialization, in the camelCase the Dev tab reads.
        let json = serde_json::to_string(row).expect("row serializes");
        assert!(json.contains(r#""role":"central""#), "{json}");
        assert!(json.contains(r#""discoveryMs":742"#), "{json}");
        assert!(json.contains(r#""sendDrops":3"#), "{json}");
        assert!(json.contains(r#""outcome":"lost-tiebreaker""#), "{json}");
        assert!(json.contains(r#""atMs":1700000000000"#), "{json}");
    }

    /// A peer with no recorded attempts renders as having none — never as
    /// having succeeded or failed, and never with a guessed role.
    #[test]
    fn peer_without_recorded_attempts_shows_no_history() {
        let views = vec![pv("a1", "npub-quiet", true, 1_000, "ble")];
        let peers = vec![bp("a1", "npub-quiet", true)];
        let out = merge_peers(
            &views,
            &peers,
            &[],
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "");
        assert_eq!(out[0].discovery_ms, 0);
        assert_eq!(out[0].send_drops, 0);
        assert!(out[0].attempts.is_empty());
    }

    /// Step 2's completed attribution: an advert whose BLE address the log has
    /// mapped to a known node address collapses into that peer's row instead of
    /// producing a second one (D-09).
    #[test]
    fn advert_with_learned_node_addr_collapses_into_the_peer_row() {
        use crate::ble_diag::{BleAttempt, BleAttemptOutcome, BleRole};

        let views = vec![pv("beef", "npub-known", true, 1_000, "ble")];
        let peers = vec![bp("beef", "npub-known", true)];
        let adverts = vec![BleAdvert {
            addr: "ble0/AA:BB:CC:DD:EE:FF".to_string(),
            psm: 131,
            rssi: -55,
        }];
        let recorded = vec![BlePeerAttempts {
            ble_addr: "ble0/AA:BB:CC:DD:EE:FF".to_string(),
            node_addr_hex: "beef".to_string(),
            send_failures: 0,
            attempts: vec![BleAttempt {
                at_ms: 1,
                ble_addr: "ble0/AA:BB:CC:DD:EE:FF".to_string(),
                node_addr_hex: "beef".to_string(),
                role: BleRole::Peripheral,
                discovery_ms: 10,
                outcome: BleAttemptOutcome::Connected,
            }],
        }];

        let out = merge_peers(
            &views,
            &peers,
            &adverts,
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &recorded,
            0,
        );

        // One row, not two: the advert was attributed via the learned pair.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].npub, "npub-known");
        assert_eq!(out[0].ble_addr, "ble0/AA:BB:CC:DD:EE:FF");
        assert_eq!(out[0].psm, 131);
        assert_eq!(out[0].rssi, Some(-55));
        assert_eq!(out[0].role, "peripheral");
    }

    /// Without a learned pair the same advert must still produce its own row —
    /// attribution is driven by recorded facts, never guessed.
    #[test]
    fn advert_without_learned_node_addr_stays_a_separate_row() {
        let views = vec![pv("beef", "npub-known", true, 1_000, "ble")];
        let peers = vec![bp("beef", "npub-known", true)];
        let adverts = vec![BleAdvert {
            addr: "ble0/AA:BB:CC:DD:EE:FF".to_string(),
            psm: 131,
            rssi: -55,
        }];
        let out = merge_peers(
            &views,
            &peers,
            &adverts,
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            0,
        );
        assert_eq!(out.len(), 2);
    }
}
