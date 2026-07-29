//! Process-global bridge for the node's UDP transport raw fd (from
//! [`fips::Node::enable_app_owned_udp_fd`]) — lets the Android side pin that
//! socket to a specific `android.net.Network` (e.g. the `!FIPS` Wi-Fi AP, or
//! a Wi-Fi Aware NDP network) via `Network.bindSocket`.
//!
//! Platform-pushed peers (Wi-Fi Aware, the AP lane) live on a network that
//! never passes internet validation, so without an explicit bind the Noise
//! handshake's replies can be lost to routing/firewall rules that favour a
//! competing validated default network (e.g. cellular) — the send succeeds
//! locally, but nothing ever comes back.

use std::os::unix::io::RawFd;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static RX: OnceLock<Mutex<Option<Receiver<RawFd>>>> = OnceLock::new();

fn rx() -> &'static Mutex<Option<Receiver<RawFd>>> {
    RX.get_or_init(|| Mutex::new(None))
}

/// Install the node's fd-notification receiver. Replaces any prior install
/// (the node is rebuilt on a BLE/mesh off→on cycle, yielding a fresh channel).
pub fn install(receiver: Receiver<RawFd>) {
    *rx().lock().unwrap() = Some(receiver);
}

/// Pull the UDP transport's raw fd, blocking up to `timeout`. Returns `-1` on
/// timeout or if no receiver is installed — the fd is only ever sent once per
/// node lifetime (right after the transport starts), so callers poll this
/// once at startup rather than in a loop.
pub fn next_fd(timeout: Duration) -> RawFd {
    let guard = rx().lock().unwrap();
    match guard.as_ref() {
        Some(receiver) => receiver.recv_timeout(timeout).unwrap_or(-1),
        None => -1,
    }
}
