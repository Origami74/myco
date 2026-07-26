//! System-wide `.fips` DNS on the phone.
//!
//! The Android `VpnService` advertises [`SENTINEL_STR`] (`fd00::53`) as the DNS
//! server for every app on the tunnel. Because that address is inside the routed
//! `fd00::/8` range but is **not** the node's own TUN address, the OS resolver's
//! query packets are handed to the app-owned-TUN pump instead of being delivered
//! locally. [`try_answer`] recognises those packets in the app→mesh path and
//! synthesises a reply, so any app can resolve `<npub>.fips` (and, via a host map
//! later, aliases) to a mesh `fd00::` address — without a real DNS server socket.
//!
//! Resolution itself is pure computation: `<npub>.fips` → `fd00::` is derived from
//! the public key alone (see [`fips::upper::dns::handle_dns_packet`]), so this
//! works with no network and no upstream resolver. Non-`.fips` names get NXDOMAIN
//! — the exit-node demo routes web traffic through an HTTP proxy, which does its
//! own DNS on the far side, so the phone never needs to resolve public names.

use std::sync::{Mutex, OnceLock};

use fips::upper::dns::{handle_dns_packet, DnsIdentityTx};
use fips::upper::hosts::HostMap;
use fips::upper::tcp_mss::recalculate_l4_checksum;

/// Sender that feeds each DNS-resolved identity to the node's rx-loop so it
/// populates its identity cache — the route-warming side effect the built-in
/// desktop DNS responder provides. Without it, a resolved `<npub>.fips` answers
/// the AAAA but the first packet to that address has no cached pubkey to open a
/// session with, so it is dropped and the connection silently hangs. Installed
/// from `runtime.rs` via [`fips::Node::enable_app_owned_dns`].
static IDENTITY_TX: OnceLock<Mutex<Option<DnsIdentityTx>>> = OnceLock::new();

fn identity_tx() -> &'static Mutex<Option<DnsIdentityTx>> {
    IDENTITY_TX.get_or_init(|| Mutex::new(None))
}

/// Install the node's DNS-identity sender. Replaces any prior install (the node
/// is rebuilt on a transport off→on cycle, yielding a fresh channel).
pub fn set_identity_tx(tx: DnsIdentityTx) {
    *identity_tx().lock().unwrap() = Some(tx);
}

/// The sentinel DNS-server address, `fd00::53`. Chosen inside the routed
/// `fd00::/8` prefix but astronomically unlikely to collide with an
/// npub-derived node address (those fill the whole 128 bits from a hash).
const SENTINEL: [u8; 16] = [0xfd, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x53];

/// String form used by the Kotlin VPN builder's `addDnsServer`.
pub const SENTINEL_STR: &str = "fd00::53";

/// TTL (seconds) on synthesised AAAA answers. Short: a node's mesh address is
/// stable, but keeping it low means a stale mapping self-heals quickly.
const ANSWER_TTL: u32 = 30;

const IPV6_HEADER_LEN: usize = 40;
const UDP_HEADER_LEN: usize = 8;
const NEXT_HEADER_UDP: u8 = 17;
const DNS_PORT: u16 = 53;

/// If `packet` is a UDP DNS query to `[fd00::53]:53`, resolve it and return the
/// reply packet (IPv6+UDP+DNS) to write back to the TUN. Returns `None` for any
/// packet that is not such a query, so the caller forwards it into the mesh
/// unchanged.
pub fn try_answer(packet: &[u8]) -> Option<Vec<u8>> {
    // IPv6 only, single UDP header (no extension headers — mesh DNS has none).
    if packet.len() < IPV6_HEADER_LEN + UDP_HEADER_LEN {
        return None;
    }
    if packet[0] >> 4 != 6 || packet[6] != NEXT_HEADER_UDP {
        return None;
    }
    if packet[24..40] != SENTINEL {
        return None;
    }
    let payload_len = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    if payload_len < UDP_HEADER_LEN || IPV6_HEADER_LEN + payload_len > packet.len() {
        return None;
    }

    let src_addr = &packet[8..24];
    let src_port = u16::from_be_bytes([packet[40], packet[41]]);
    let dst_port = u16::from_be_bytes([packet[42], packet[43]]);
    if dst_port != DNS_PORT {
        return None;
    }

    let dns_query = &packet[IPV6_HEADER_LEN + UDP_HEADER_LEN..IPV6_HEADER_LEN + payload_len];
    // Empty host map: `<npub>.fips` resolves by pure computation without it.
    let (dns_reply, identity) = handle_dns_packet(dns_query, ANSWER_TTL, &HostMap::new())?;

    // Warm the route: hand the resolved identity to the node so it caches the
    // pubkey and can open a session to this address (see [`set_identity_tx`]).
    if let Some(id) = identity {
        if let Some(tx) = identity_tx().lock().unwrap().as_ref() {
            let _ = tx.try_send(id);
        }
    }

    Some(build_reply(src_addr, src_port, &dns_reply))
}

/// Assemble the response IPv6/UDP packet: swap the query's src/dst and ports,
/// carry the DNS reply as the UDP payload, and fix lengths + checksum.
fn build_reply(orig_src_addr: &[u8], orig_src_port: u16, dns_reply: &[u8]) -> Vec<u8> {
    let udp_len = UDP_HEADER_LEN + dns_reply.len();
    let mut pkt = vec![0u8; IPV6_HEADER_LEN + udp_len];

    // IPv6 header: version 6, our sentinel as source, the querier as destination.
    pkt[0] = 0x60;
    pkt[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes()); // payload length
    pkt[6] = NEXT_HEADER_UDP;
    pkt[7] = 64; // hop limit
    pkt[8..24].copy_from_slice(&SENTINEL);
    pkt[24..40].copy_from_slice(orig_src_addr);

    // UDP header: from :53 back to the querier's port.
    pkt[40..42].copy_from_slice(&DNS_PORT.to_be_bytes());
    pkt[42..44].copy_from_slice(&orig_src_port.to_be_bytes());
    pkt[44..46].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // checksum left zero, recomputed below.
    pkt[IPV6_HEADER_LEN + UDP_HEADER_LEN..].copy_from_slice(dns_reply);

    recalculate_l4_checksum(&mut pkt);
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal IPv6/UDP DNS query packet for `qname` to `[fd00::53]:53`.
    fn make_query(qname: &str, dst: [u8; 16]) -> Vec<u8> {
        use simple_dns::{Name, Packet, QCLASS, QTYPE, Question, TYPE};
        let mut q = Packet::new_query(0x1234);
        q.questions.push(Question::new(
            Name::new_unchecked(qname),
            QTYPE::TYPE(TYPE::AAAA),
            QCLASS::CLASS(simple_dns::CLASS::IN),
            false,
        ));
        let dns = q.build_bytes_vec().unwrap();
        let udp_len = UDP_HEADER_LEN + dns.len();
        let mut pkt = vec![0u8; IPV6_HEADER_LEN + udp_len];
        pkt[0] = 0x60;
        pkt[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
        pkt[6] = NEXT_HEADER_UDP;
        pkt[7] = 64;
        // src = some client addr, dst = sentinel
        pkt[8..24].copy_from_slice(&[0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        pkt[24..40].copy_from_slice(&dst);
        pkt[40..42].copy_from_slice(&40000u16.to_be_bytes()); // src port
        pkt[42..44].copy_from_slice(&DNS_PORT.to_be_bytes()); // dst port 53
        pkt[44..46].copy_from_slice(&(udp_len as u16).to_be_bytes());
        pkt[IPV6_HEADER_LEN + UDP_HEADER_LEN..].copy_from_slice(&dns);
        pkt
    }

    #[test]
    fn resolves_npub_fips_query() {
        // A valid npub (from fips test vectors would be ideal; use a well-formed one).
        let npub = "npub1mqelkzqp4659fws35h2wvr7z9caka5ml8qddj3ssnwaulwpxdd9sdc3esw";
        let pkt = make_query(&format!("{npub}.fips"), SENTINEL);
        let reply = try_answer(&pkt).expect("should answer .fips query");

        // Reply is IPv6/UDP, from :53, back to the querier, addressed to the client.
        assert_eq!(reply[0] >> 4, 6);
        assert_eq!(reply[6], NEXT_HEADER_UDP);
        assert_eq!(&reply[8..24], &SENTINEL); // src = sentinel
        assert_eq!(&reply[24..40], &pkt[8..24]); // dst = original client
        assert_eq!(u16::from_be_bytes([reply[40], reply[41]]), DNS_PORT);
        assert_eq!(u16::from_be_bytes([reply[42], reply[43]]), 40000);

        // The DNS answer carries an AAAA in the mesh prefix.
        let dns = &reply[IPV6_HEADER_LEN + UDP_HEADER_LEN..];
        let parsed = simple_dns::Packet::parse(dns).unwrap();
        assert_eq!(parsed.answers.len(), 1);
    }

    #[test]
    fn ignores_wrong_destination() {
        let npub = "npub1mqelkzqp4659fws35h2wvr7z9caka5ml8qddj3ssnwaulwpxdd9sdc3esw";
        // Same query but to a non-sentinel address → not ours, forward to mesh.
        let other = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9];
        let pkt = make_query(&format!("{npub}.fips"), other);
        assert!(try_answer(&pkt).is_none());
    }

    #[test]
    fn ignores_non_dns_udp() {
        let mut pkt = make_query("whatever.fips", SENTINEL);
        // Change dst port away from 53.
        pkt[42..44].copy_from_slice(&4870u16.to_be_bytes());
        assert!(try_answer(&pkt).is_none());
    }
}
