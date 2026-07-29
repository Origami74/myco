//! System-wide `.fips` DNS on the phone.
//!
//! The Android `VpnService` advertises [`SENTINEL_STR`] (`fd00::53`) as the DNS
//! server for every app on the tunnel. Because that address is inside the routed
//! `fd00::/8` range but is **not** the node's own TUN address, the OS resolver's
//! query packets are handed to the app-owned-TUN pump instead of being delivered
//! locally. [`handle_query`] recognises those packets in the app→mesh path and
//! synthesises a reply, so any app can resolve `<npub>.fips` (and, via a host map
//! later, aliases) to a mesh `fd00::` address — without a real DNS server socket.
//!
//! Resolution itself is pure computation: `<npub>.fips` → `fd00::` is derived from
//! the public key alone (see [`fips::upper::dns::handle_dns_packet`]), so this
//! works with no network and no upstream resolver.
//!
//! Because the sentinel is the *only* resolver the tunnel advertises, this
//! module owns every name the device looks up, not just mesh ones: non-`.fips`
//! queries are relayed to a real resolver and their replies injected back into
//! the TUN. Advertising the real resolvers alongside the sentinel instead does
//! not work — the OS may send a `.fips` query to any server in the list, and a
//! real one denies it authoritatively, so mesh names would resolve or not
//! depending on which server happened to be picked.

use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use fips::upper::dns::{handle_dns_packet, DnsIdentityTx, DnsResolvedIdentity};
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

/// Teach the node an npub's address→pubkey mapping without going through a DNS
/// lookup, so a packet sent to that mesh address can open a session.
///
/// Dialling a peer by raw `fd00::` literal skips resolution, so nothing
/// registers the identity and the node has no pubkey to open a session with —
/// the send is dropped and the address looks unroutable. That is invisible for
/// a *direct* neighbour, whose identity the node already holds from the
/// handshake, which is why only adjacent peers used to be reachable. Every
/// address is derived from the public key alone, so this needs no network.
pub fn warm_route(npub: &str) {
    let Ok(peer) = fips::PeerIdentity::from_npub(npub) else {
        return;
    };
    let id = DnsResolvedIdentity {
        node_addr: *peer.node_addr(),
        pubkey: peer.pubkey_full(),
    };
    if let Some(tx) = identity_tx().lock().unwrap().as_ref() {
        let _ = tx.try_send(id);
    }
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

/// What [`handle_query`] did with a packet.
pub enum Dns {
    /// A `.fips` query, answered here — write this reply to the TUN.
    Answered(Vec<u8>),
    /// Not ours to answer, sent to a real resolver instead; the reply will be
    /// delivered later through [`crate::tun_bridge::push_local`]. The caller
    /// must consume the packet either way.
    Forwarded,
    /// Not a DNS query to the sentinel — forward it into the mesh unchanged.
    NotOurs,
}

/// Handle a packet the TUN pump read, if it is a UDP DNS query to
/// `[fd00::53]:53`. `.fips` names are answered from the public key alone;
/// everything else is relayed to a real resolver (see [`set_upstream`]),
/// because the sentinel is the tunnel's *only* advertised server — anything we
/// decline here would simply fail to resolve on the device.
pub fn handle_query(packet: &[u8]) -> Dns {
    match parse_query(packet) {
        Some((src_addr, src_port, dns_query)) => {
            if is_fips_name(dns_query) {
                match answer_fips(src_addr, src_port, dns_query) {
                    Some(reply) => Dns::Answered(reply),
                    // Malformed enough that even SERVFAIL can't be built.
                    None => Dns::Forwarded,
                }
            } else {
                forward_upstream(src_addr, src_port, dns_query);
                Dns::Forwarded
            }
        }
        None => Dns::NotOurs,
    }
}

/// Split a TUN packet into `(querier addr, querier port, DNS payload)` if it is
/// a UDP DNS query addressed to `[fd00::53]:53`. `None` for anything else.
fn parse_query(packet: &[u8]) -> Option<(&[u8], u16, &[u8])> {
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
    if u16::from_be_bytes([packet[42], packet[43]]) != DNS_PORT {
        return None;
    }
    let src_port = u16::from_be_bytes([packet[40], packet[41]]);
    let dns_query = &packet[IPV6_HEADER_LEN + UDP_HEADER_LEN..IPV6_HEADER_LEN + payload_len];
    Some((&packet[8..24], src_port, dns_query))
}

/// Answer a `.fips` query from the public key alone, and warm the route.
fn answer_fips(src_addr: &[u8], src_port: u16, dns_query: &[u8]) -> Option<Vec<u8>> {
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

/// Real resolvers to relay non-`.fips` queries to, set by the platform from the
/// underlying network's DNS servers (see the Android `MycoVpnService`).
static UPSTREAM: OnceLock<Mutex<Vec<SocketAddr>>> = OnceLock::new();

fn upstream() -> &'static Mutex<Vec<SocketAddr>> {
    UPSTREAM.get_or_init(|| Mutex::new(Vec::new()))
}

/// Install the upstream resolvers. Replaces any previous set — the platform
/// re-supplies these whenever the underlying network changes.
pub fn set_upstream(servers: Vec<SocketAddr>) {
    *upstream().lock().unwrap() = servers;
}

/// How long to wait for an upstream resolver before giving up on a query. The
/// OS resolver has its own, longer timeout, so a lost query costs a retry
/// rather than a hang.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(4);

/// Relay a non-`.fips` query to a real resolver and inject the reply into the
/// TUN when it comes back.
///
/// Runs on its own thread: the caller is the TUN read loop, and blocking it
/// would stall every other packet on the device. DNS volume is low enough that
/// a thread per outstanding query is cheaper than the machinery to avoid it.
///
/// The socket is deliberately plain: the tunnel routes no IPv4 and claims IPv6
/// only when the network underneath has none, so a query to an IPv4 resolver
/// leaves via the real network without needing to be `protect()`ed.
fn forward_upstream(src_addr: &[u8], src_port: u16, dns_query: &[u8]) {
    let servers = upstream().lock().unwrap().clone();
    if servers.is_empty() {
        return; // nothing to relay to; the querier will time out and retry
    }
    let mut querier = [0u8; 16];
    querier.copy_from_slice(src_addr);
    let query = dns_query.to_vec();

    std::thread::spawn(move || {
        let Ok(sock) = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)) else {
            return;
        };
        if sock.set_read_timeout(Some(UPSTREAM_TIMEOUT)).is_err() {
            return;
        }
        for server in servers {
            if sock.send_to(&query, server).is_err() {
                continue;
            }
            let mut buf = [0u8; 1500];
            match sock.recv_from(&mut buf) {
                // Only accept a reply from the server we just asked, and only
                // if the transaction id matches the query we sent.
                Ok((n, from)) if from.ip() == server.ip() && n >= 2 && buf[..2] == query[..2] => {
                    crate::tun_bridge::push_local(build_reply(&querier, src_port, &buf[..n]));
                    return;
                }
                _ => continue,
            }
        }
    });
}

/// Walk the QNAME in `dns_query` (wire format, labels after the 12-byte
/// header). Returns the offset just past the terminating zero label, plus
/// whether the name's last label is `fips` (case-insensitive).
fn scan_qname(dns_query: &[u8]) -> Option<(usize, bool)> {
    let mut i = 12; // skip the fixed header
    let mut last = Vec::new();
    loop {
        let len = *dns_query.get(i)? as usize;
        if len == 0 {
            return Some((i + 1, last.eq_ignore_ascii_case(b"fips")));
        }
        // Compression pointers never appear in a question's QNAME.
        if len > 63 {
            return None;
        }
        last = dns_query.get(i + 1..i + 1 + len)?.to_vec();
        i += 1 + len;
    }
}

/// True if the query asks for a name under the `.fips` pseudo-TLD.
fn is_fips_name(dns_query: &[u8]) -> bool {
    scan_qname(dns_query)
        .map(|(_, is_fips)| is_fips)
        .unwrap_or(false)
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
        use simple_dns::{Name, Packet, Question, QCLASS, QTYPE, TYPE};
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
        let Dns::Answered(reply) = handle_query(&pkt) else {
            panic!("a .fips query must be answered here, not forwarded");
        };

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
    fn non_fips_name_is_relayed_not_answered() {
        // We are the tunnel's only resolver, so a non-`.fips` name must be
        // relayed to a real one rather than answered (or denied) here —
        // denying it would leave the device able to resolve nothing else.
        let pkt = make_query("google.com", SENTINEL);
        assert!(
            matches!(handle_query(&pkt), Dns::Forwarded),
            "non-.fips must be relayed upstream"
        );
    }

    #[test]
    fn ignores_wrong_destination() {
        let npub = "npub1mqelkzqp4659fws35h2wvr7z9caka5ml8qddj3ssnwaulwpxdd9sdc3esw";
        // Same query but to a non-sentinel address → not ours, forward to mesh.
        let other = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9];
        let pkt = make_query(&format!("{npub}.fips"), other);
        assert!(matches!(handle_query(&pkt), Dns::NotOurs));
    }

    #[test]
    fn ignores_non_dns_udp() {
        let mut pkt = make_query("whatever.fips", SENTINEL);
        // Change dst port away from 53.
        pkt[42..44].copy_from_slice(&4870u16.to_be_bytes());
        assert!(matches!(handle_query(&pkt), Dns::NotOurs));
    }
}
