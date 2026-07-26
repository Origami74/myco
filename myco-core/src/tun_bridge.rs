//! Process-global bridge between the Android `VpnService`'s TUN fd and the FIPS
//! node's app-owned-TUN channels (from [`fips::Node::enable_app_owned_tun`]).
//!
//! The node's channel ends are [`install`]ed when it starts; the VpnService pump
//! (via the `tun_bridge_jni` exports) calls [`send_packet`] for each IPv6 packet
//! read from the fd (app → mesh) and [`next_packet`] to pull packets destined for
//! the fd (mesh → app). The ends live in statics, not on `AppRuntime`, so the
//! blocking `next_packet` never holds the reducer lock — mirroring the BLE bridge.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc::Sender as MeshSender;

/// Clamp outbound TCP SYNs to FIPS's MSS (the app-owned path bypasses the
/// system-TUN reader's clamp; see `fips` `Node::enable_app_owned_tun`). Set on
/// install to `effective_ipv6_mtu - 60`; the default is FIPS's effective MTU for
/// the 1280 transport floor (`1280 - 77 - 60`).
static MAX_MSS: AtomicU16 = AtomicU16::new(1143);

#[allow(clippy::type_complexity)]
static OUTBOUND: OnceLock<Mutex<Option<MeshSender<Vec<u8>>>>> = OnceLock::new();
#[allow(clippy::type_complexity)]
static INBOUND: OnceLock<Mutex<Option<std::sync::mpsc::Receiver<Vec<u8>>>>> = OnceLock::new();

fn outbound() -> &'static Mutex<Option<MeshSender<Vec<u8>>>> {
    OUTBOUND.get_or_init(|| Mutex::new(None))
}

fn inbound() -> &'static Mutex<Option<std::sync::mpsc::Receiver<Vec<u8>>>> {
    INBOUND.get_or_init(|| Mutex::new(None))
}

/// Locally-synthesised packets to deliver to the TUN fd ahead of mesh traffic —
/// currently `.fips` DNS replies (see [`crate::dns_intercept`]). Kept separate
/// from the node's inbound channel so a reply is injected without a round-trip
/// through the mesh.
static LOCAL: OnceLock<Mutex<VecDeque<Vec<u8>>>> = OnceLock::new();

fn local() -> &'static Mutex<VecDeque<Vec<u8>>> {
    LOCAL.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Install the node's app-owned-TUN channel ends and the MSS ceiling
/// (`effective_ipv6_mtu - 60`). Replaces any prior install (the node is rebuilt on
/// a BLE off→on cycle, yielding fresh channels).
pub fn install(
    outbound_tx: MeshSender<Vec<u8>>,
    inbound_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    max_mss: u16,
) {
    *outbound().lock().unwrap() = Some(outbound_tx);
    *inbound().lock().unwrap() = Some(inbound_rx);
    MAX_MSS.store(max_mss, Ordering::Relaxed);
}

/// app → mesh: clamp the TCP MSS, then route an IPv6 packet read from the TUN fd
/// into the mesh. Returns `false` if no TUN is installed or the queue is full.
pub fn send_packet(mut packet: Vec<u8>) -> bool {
    // System-wide `.fips` DNS: if this is a query to the sentinel resolver,
    // answer it locally, queue the reply for the TUN, and don't forward it.
    if let Some(reply) = crate::dns_intercept::try_answer(&packet) {
        local().lock().unwrap().push_back(reply);
        return true;
    }
    // Only mesh (fd00::/8) traffic belongs on the app-owned TUN. The VpnService
    // also routes ::/0 so the OS reports the tunnel as IPv6-capable (without
    // that, browsers never issue the AAAA query that resolves `<npub>.fips` —
    // see MycoVpnService), which drags in unrelated IPv6 packets. Drop them here
    // rather than pushing them into FIPS.
    if !is_mesh_bound(&packet) {
        return true; // consumed = dropped
    }
    fips::upper::tcp_mss::clamp_tcp_mss(&mut packet, MAX_MSS.load(Ordering::Relaxed));
    match outbound().lock().unwrap().as_ref() {
        Some(tx) => tx.try_send(packet).is_ok(),
        None => false,
    }
}

/// True if `packet` is an IPv6 packet destined for the mesh ULA (`fd00::/8`).
/// FIPS mesh addresses all fall under the `fd` prefix; anything else (IPv4, or
/// public IPv6) does not belong on the app-owned TUN.
fn is_mesh_bound(packet: &[u8]) -> bool {
    packet.len() >= 40 && packet[0] >> 4 == 6 && packet[24] == 0xfd
}

/// mesh → app: pull the next IPv6 packet for the TUN fd, blocking up to
/// `timeout`. `None` = timed out (loop again) or no TUN installed.
pub fn next_packet(timeout: Duration) -> Option<Vec<u8>> {
    // Locally-synthesised packets (DNS replies) jump ahead of mesh traffic.
    if let Some(pkt) = local().lock().unwrap().pop_front() {
        return Some(pkt);
    }
    let guard = inbound().lock().ok()?;
    let rx = guard.as_ref()?;
    rx.recv_timeout(timeout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal 40-byte IPv6 header with an `fd00::/8` destination, so it
    /// passes [`is_mesh_bound`]. `tag` is the low byte of the destination, to
    /// tell packets apart. Payload/L4 omitted — the MSS clamp leaves a
    /// non-TCP packet unchanged.
    fn mesh_packet(tag: u8) -> Vec<u8> {
        let mut p = vec![0u8; 40];
        p[0] = 0x60; // version 6
        p[24] = 0xfd; // dst in fd00::/8
        p[39] = tag;
        p
    }

    #[tokio::test]
    async fn install_send_next_roundtrip() {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        install(out_tx, in_rx, 1143);

        // app → mesh: a mesh-bound packet reaches the node outbound channel.
        let pkt = mesh_packet(1);
        assert!(send_packet(pkt.clone()));
        assert_eq!(out_rx.recv().await.unwrap(), pkt);

        // A non-mesh packet (the ::/0 route drags these in) is dropped, not
        // forwarded into the mesh.
        let mut public = mesh_packet(2);
        public[24] = 0x20; // 2000::/3 — public IPv6
        assert!(send_packet(public)); // consumed…
        assert!(out_rx.try_recv().is_err()); // …but nothing reached the mesh

        // mesh → app: a packet the node writes inbound is pulled by next_packet.
        in_tx.send(vec![0x60, 1, 2, 3]).unwrap();
        assert_eq!(
            next_packet(Duration::from_millis(200)),
            Some(vec![0x60, 1, 2, 3])
        );

        // Nothing pending → timeout.
        assert_eq!(next_packet(Duration::from_millis(20)), None);
    }
}
