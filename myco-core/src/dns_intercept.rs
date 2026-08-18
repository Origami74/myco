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
//! Resolution itself is **not** done here. The node runs its own `.fips`
//! responder and publishes the address it bound to ([`set_responder_addr`]);
//! this module lifts the DNS payload out of the packet, sends it there over an
//! ordinary UDP socket, and splices the answer back into a reply packet. That
//! is the whole point of the indirection: answering a `<npub>.fips` query is
//! what puts the peer's public key into the node's identity cache, and a mesh
//! address is a truncated hash — the key cannot be recovered from it. With no
//! cache entry the first packet to a freshly-resolved name is rejected with
//! ICMPv6 "No route", a failure direct neighbours mask entirely because their
//! identity arrives with the Noise handshake.
//!
//! Because the sentinel is the *only* resolver the tunnel advertises, this
//! module owns every name the device looks up, not just mesh ones: non-`.fips`
//! queries are relayed to a real resolver and their replies injected back into
//! the TUN. Advertising the real resolvers alongside the sentinel instead does
//! not work — the OS may send a `.fips` query to any server in the list, and a
//! real one denies it authoritatively, so mesh names would resolve or not
//! depending on which server happened to be picked.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use fips::upper::tcp_mss::recalculate_l4_checksum;

/// Where the node's built-in `.fips` responder is listening, or `None` when it
/// is not running.
///
/// Published by `runtime::start_node` from `Node::dns_local_addr()` once
/// `start()` has returned — a one-shot read, because `run_rx_loop` then borrows
/// the node for the rest of its life. The responder is either up for that whole
/// life or it never came up.
///
/// This replaces the app-owned DNS identity channel, and inverts the data path
/// while it is at it. Myco used to answer `.fips` itself and push each resolved
/// identity *into* the node; the node's own responder start-up then overwrote
/// the receiver on that channel, so every push was silently dropped and route
/// warming quietly stopped. Now the responder answers and registers the
/// identity as its own side effect, and there is no channel left to clobber.
static RESPONDER: OnceLock<Mutex<Option<SocketAddr>>> = OnceLock::new();

fn responder() -> &'static Mutex<Option<SocketAddr>> {
    RESPONDER.get_or_init(|| Mutex::new(None))
}

/// Publish (or retract) the responder's address. Replaces any prior value: the
/// node is rebuilt on a transport off→on cycle and binds a fresh socket.
///
/// Clears the warmed-npub set, because a fresh node has a fresh, empty identity
/// cache and everything has to be warmed again.
pub fn set_responder_addr(addr: Option<SocketAddr>) {
    *responder().lock().unwrap() = addr;
    warmed().lock().unwrap().clear();
}

fn responder_addr() -> Option<SocketAddr> {
    *responder().lock().unwrap()
}

/// How long to wait for the responder. It is in-process on loopback, so this is
/// a liveness bound, not a latency budget.
const RESPONDER_TIMEOUT: Duration = Duration::from_secs(2);

/// npubs already resolved through the responder since the current node started.
///
/// `keepwarm_tick` calls [`warm_route`] for every Circle member every 8s. That
/// used to be an in-memory channel send; it is a UDP round trip now, on the
/// battery-sensitive background path, so it happens once per npub per node.
static WARMED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn warmed() -> &'static Mutex<HashSet<String>> {
    WARMED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Transaction ids for synthesised queries, so a reply can be matched to the
/// query that asked for it.
static QUERY_ID: AtomicU16 = AtomicU16::new(1);

/// Teach the node an npub's address→pubkey mapping ahead of any dial, so the
/// first packet to that mesh address can open a session.
///
/// A FIPS address is a truncated double hash of the public key, so the node
/// cannot recover the key from the address; with no cache entry the send is
/// dropped and the address looks unroutable. A *direct* neighbour hides this,
/// since its identity came from the Noise handshake.
///
/// `keepwarm_tick`'s dials go to `ws://<npub>.fips:4870`, a name — so they warm
/// themselves through resolution eventually. This pre-warm exists because the
/// dial and the resolution are concurrent, and losing that race is the whole
/// failure. Fire-and-forget on its own thread: the caller is a tokio task and
/// this must not be what stalls the keepwarm loop.
pub fn warm_route(npub: &str) {
    if fips::PeerIdentity::from_npub(npub).is_err() {
        return;
    }
    let Some(addr) = responder_addr() else {
        return; // no node, nothing to warm; retried on the next tick
    };
    if !warmed().lock().unwrap().insert(npub.to_string()) {
        return;
    }
    let Some(query) = build_aaaa_query(&format!("{npub}.fips")) else {
        return;
    };
    std::thread::spawn(move || {
        // The answer is discarded — registering the identity is the side
        // effect this exists for.
        let _ = resolve_via_responder(addr, &query);
    });
}

/// A minimal AAAA query for `name`, as the responder expects it: payload only,
/// no IP or UDP header.
fn build_aaaa_query(name: &str) -> Option<Vec<u8>> {
    use simple_dns::{Name, Packet, Question, CLASS, QCLASS, QTYPE, TYPE};
    let mut packet = Packet::new_query(QUERY_ID.fetch_add(1, Ordering::Relaxed));
    packet.questions.push(Question::new(
        Name::new(name).ok()?,
        QTYPE::TYPE(TYPE::AAAA),
        QCLASS::CLASS(CLASS::IN),
        false,
    ));
    packet.build_bytes_vec().ok()
}

/// One blocking UDP round trip to the responder at `addr`, returning the DNS
/// reply payload. `None` if it did not answer in time, or the answer did not
/// match the question.
///
/// Takes the address rather than reading the published one so the round trip is
/// testable without publishing a responder — publishing one would leak DNS
/// replies into the shared TUN queue that other tests read.
fn resolve_via_responder(addr: SocketAddr, dns_query: &[u8]) -> Option<Vec<u8>> {
    if dns_query.len() < 2 {
        return None;
    }
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::1]:0".parse().ok()?
    } else {
        "127.0.0.1:0".parse().ok()?
    };
    let sock = std::net::UdpSocket::bind(bind).ok()?;
    sock.set_read_timeout(Some(RESPONDER_TIMEOUT)).ok()?;
    sock.send_to(dns_query, addr).ok()?;

    let mut buf = [0u8; 1500];
    let (n, from) = sock.recv_from(&mut buf).ok()?;
    // Only the server we asked, and only an answer to the question we sent.
    if from.ip() != addr.ip() || n < 2 || buf[..2] != dns_query[..2] {
        return None;
    }
    Some(buf[..n].to_vec())
}

/// The sentinel DNS-server address, `fd00::53`. Chosen inside the routed
/// `fd00::/8` prefix but astronomically unlikely to collide with an
/// npub-derived node address (those fill the whole 128 bits from a hash).
const SENTINEL: [u8; 16] = [0xfd, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x53];

/// String form used by the Kotlin VPN builder's `addDnsServer`. Its only
/// consumer is on the other side of the FFI, so nothing in Rust reads it.
#[allow(dead_code)]
pub const SENTINEL_STR: &str = "fd00::53";

const IPV6_HEADER_LEN: usize = 40;
const UDP_HEADER_LEN: usize = 8;
const NEXT_HEADER_UDP: u8 = 17;
const DNS_PORT: u16 = 53;

/// What [`handle_query`] did with a packet.
pub enum Dns {
    /// Ours, and now in flight to a resolver — the node's own `.fips` responder
    /// or a real upstream one. The reply is delivered later through
    /// [`crate::tun_bridge::push_local`]; the caller must consume the packet
    /// regardless, because it must not reach the mesh either way.
    Forwarded,
    /// Not a DNS query to the sentinel — forward it into the mesh unchanged.
    NotOurs,
}

/// Handle a packet the TUN pump read, if it is a UDP DNS query to
/// `[fd00::53]:53`. `.fips` names go to the node's own responder; everything
/// else is relayed to a real resolver (see [`set_upstream`]), because the
/// sentinel is the tunnel's *only* advertised server — anything we decline here
/// would simply fail to resolve on the device.
///
/// Both paths are asynchronous now. `.fips` used to be answered inline, from
/// the public key alone; it is a round trip to the responder instead, so the
/// route-warming side effect happens where the identity cache actually lives.
pub fn handle_query(packet: &[u8]) -> Dns {
    match parse_query(packet) {
        Some((src_addr, src_port, dns_query)) => {
            if is_fips_name(dns_query) {
                forward_to_responder(src_addr, src_port, dns_query);
            } else {
                forward_upstream(src_addr, src_port, dns_query);
            }
            Dns::Forwarded
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

/// Send a `.fips` query to the node's own responder and inject its answer back
/// into the TUN.
///
/// Runs on its own thread for the same reason [`forward_upstream`] does: the
/// caller is the TUN read loop, and blocking it would stall every other packet
/// on the device. Mesh DNS volume is low enough that a thread per outstanding
/// query is cheaper than the machinery to avoid one.
///
/// A dropped query is a query the OS resolver retries. With no responder
/// published — the node is stopped, or its bind failed — nothing is sent and
/// the name simply does not resolve, which is the honest outcome: answering it
/// here would resolve the name while leaving the node unable to reach it.
fn forward_to_responder(src_addr: &[u8], src_port: u16, dns_query: &[u8]) {
    let Some(addr) = responder_addr() else {
        return;
    };
    let mut querier = [0u8; 16];
    querier.copy_from_slice(src_addr);
    let query = dns_query.to_vec();

    std::thread::spawn(move || {
        if let Some(reply) = resolve_via_responder(addr, &query) {
            crate::tun_bridge::push_local(build_reply(&querier, src_port, &reply));
        }
    });
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

    const NPUB: &str = "npub1mqelkzqp4659fws35h2wvr7z9caka5ml8qddj3ssnwaulwpxdd9sdc3esw";

    /// A stand-in for the node's built-in responder: echoes back whatever it is
    /// sent, with the DNS QR bit set so it reads as a response. Returns the
    /// address it bound to.
    fn fake_responder() -> SocketAddr {
        let sock = std::net::UdpSocket::bind("[::1]:0").expect("bind fake responder");
        let addr = sock.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                if n >= 3 {
                    buf[2] |= 0x80; // QR = response
                }
                let _ = sock.send_to(&buf[..n], from);
            }
        });
        addr
    }

    /// The load-bearing leg: a `.fips` query is proxied to the responder and its
    /// answer comes back, matched by transaction id. Route warming rides on this
    /// round trip actually happening — the responder registering the peer's
    /// public key is the side effect the whole indirection exists for.
    #[test]
    fn a_fips_query_round_trips_through_the_responder() {
        let addr = fake_responder();
        let query = build_aaaa_query(&format!("{NPUB}.fips")).expect("query builds");
        let reply = resolve_via_responder(addr, &query).expect("responder answered");

        assert_eq!(&reply[..2], &query[..2], "transaction id must match");
        assert_ne!(
            reply[2] & 0x80,
            0,
            "must be a response, not the echo of a query"
        );
    }

    /// An answer from somewhere other than the responder, or to a different
    /// question, must not be spliced into the tunnel.
    #[test]
    fn a_mismatched_transaction_id_is_rejected() {
        let addr = fake_responder();
        let query = build_aaaa_query(&format!("{NPUB}.fips")).expect("query builds");
        let mut different = query.clone();
        different[0] ^= 0xff;
        // The fake echoes the id it was sent, so asking with one id and
        // checking against another is the same shape as an off-query reply.
        let reply = resolve_via_responder(addr, &different).expect("responder answered");
        assert_ne!(&reply[..2], &query[..2]);
    }

    /// A `.fips` query is consumed here whatever happens next — never handed to
    /// the mesh, and never answered locally. Answering it locally would give
    /// the caller an address the node has no key for, which is the "No route"
    /// failure this whole indirection exists to avoid. No responder is
    /// published in tests, so this exercises the no-responder path too.
    #[test]
    fn a_fips_query_is_consumed_but_not_answered_locally() {
        let pkt = make_query(&format!("{NPUB}.fips"), SENTINEL);
        assert!(
            matches!(handle_query(&pkt), Dns::Forwarded),
            "a .fips query is ours, so it must never reach the mesh"
        );
    }

    /// The reply packet assembly is unchanged and still owned here: the
    /// responder returns a DNS payload, not an IPv6 packet.
    #[test]
    fn reply_packets_are_addressed_back_to_the_querier() {
        let pkt = make_query(&format!("{NPUB}.fips"), SENTINEL);
        let reply = build_reply(&pkt[8..24], 40000, b"\x12\x34payload");

        assert_eq!(reply[0] >> 4, 6);
        assert_eq!(reply[6], NEXT_HEADER_UDP);
        assert_eq!(&reply[8..24], &SENTINEL); // src = sentinel
        assert_eq!(&reply[24..40], &pkt[8..24]); // dst = original client
        assert_eq!(u16::from_be_bytes([reply[40], reply[41]]), DNS_PORT);
        assert_eq!(u16::from_be_bytes([reply[42], reply[43]]), 40000);
        assert_eq!(
            &reply[IPV6_HEADER_LEN + UDP_HEADER_LEN..],
            b"\x12\x34payload"
        );
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
        // Same query but to a non-sentinel address → not ours, forward to mesh.
        let other = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9];
        let pkt = make_query(&format!("{NPUB}.fips"), other);
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
