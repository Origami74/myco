//! Process-global bridge between the Android `VpnService`'s TUN fd and the FIPS
//! node's app-owned-TUN channels (from [`fips::Node::enable_app_owned_tun`]).
//!
//! The node's channel ends are [`install`]ed when it starts; the VpnService pump
//! (via the `tun_bridge_jni` exports) calls [`send_packet`] for each IPv6 packet
//! read from the fd (app → mesh) and [`next_packet`] to pull packets destined for
//! the fd (mesh → app). The ends live in statics, not on `AppRuntime`, so the
//! blocking `next_packet` never holds the reducer lock — mirroring the BLE bridge.

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
    fips::upper::tcp_mss::clamp_tcp_mss(&mut packet, MAX_MSS.load(Ordering::Relaxed));
    match outbound().lock().unwrap().as_ref() {
        Some(tx) => tx.try_send(packet).is_ok(),
        None => false,
    }
}

/// mesh → app: pull the next IPv6 packet for the TUN fd, blocking up to
/// `timeout`. `None` = timed out (loop again) or no TUN installed.
pub fn next_packet(timeout: Duration) -> Option<Vec<u8>> {
    let guard = inbound().lock().ok()?;
    let rx = guard.as_ref()?;
    rx.recv_timeout(timeout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn install_send_next_roundtrip() {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        install(out_tx, in_rx, 1143);

        // app → mesh: send_packet pushes into the node-side outbound channel.
        // (A short non-TCP packet is left unchanged by the MSS clamp.)
        assert!(send_packet(vec![0x60, 0, 0, 0]));
        assert_eq!(out_rx.recv().await.unwrap(), vec![0x60, 0, 0, 0]);

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
