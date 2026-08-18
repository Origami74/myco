//! Process-global bridge for the node's UDP transport raw fds (from
//! [`fips::Node::enable_app_owned_udp_fd`]) — lets the Android side pin each
//! socket to a specific `android.net.Network` (infrastructure Wi-Fi for the
//! `!FIPS`/LAN lane, a Wi-Fi Aware NDP for the Aware lane) via
//! `Network.bindSocket`.
//!
//! Platform-pushed peers live on networks that never pass internet validation,
//! so without an explicit bind the Noise handshake's replies can be lost to
//! routing rules that favour a competing validated default network (e.g.
//! cellular) — the send succeeds locally, but nothing ever comes back.
//!
//! # Why this is keyed by instance
//!
//! The node runs one UDP transport per lane (`runtime.rs`'s `"lan"` and
//! `"aware"` instances), so the seam delivers **two** fds. `Network.bindSocket`
//! is exclusive: a socket marked with the Wi-Fi netid cannot reach a Wi-Fi
//! Aware peer, whose NDP is a separate network with its own routing table, and
//! vice versa. Handing the wrong descriptor to a radio is therefore not a
//! cosmetic mix-up — it is exactly the bug this change fixes.
//!
//! Arrival order cannot be used to tell them apart: fips builds transports by
//! iterating a `HashMap`, so the order is arbitrary and may differ run to run.
//! Instead fips labels every delivery with the configured instance name
//! ([`fips::AppOwnedUdpSocket`]), and this module files each one under that
//! name. A radio asks for its own instance and can only ever receive that
//! instance's descriptor; a lane whose socket failed to bind gets `-1` rather
//! than a neighbour's fd.
//!
//! # Why a versioned latch and not a queue
//!
//! A queue serves a consumer that outlives every producer. These consumers do
//! not: `AwareRadio` is created and destroyed each time the user toggles the
//! Wi-Fi Aware lane, while the node — and its sockets — carry on running. A
//! radio started after its socket was announced must still be able to learn it,
//! so the latest descriptor per lane is *retained*, with a version that
//! increments on each new one. A caller passes back the version it last saw and
//! blocks only for something newer.

use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use fips::AppOwnedUdpSocket;

/// The retained descriptor for one lane.
///
/// `version` starts at 0, meaning "no socket announced yet", and increments on
/// every announcement — including a re-announcement of the same fd number,
/// which a node rebuild can easily produce and which still needs re-binding
/// because the socket behind the number is a new one.
#[derive(Clone, Copy)]
struct Announced {
    version: u64,
    fd: RawFd,
}

#[derive(Default)]
struct Lane {
    state: Mutex<Option<Announced>>,
    changed: Condvar,
}

impl Lane {
    fn announce(&self, fd: RawFd) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let version = state.map_or(0, |a| a.version) + 1;
        *state = Some(Announced { version, fd });
        self.changed.notify_all();
    }

    /// Block until this lane's version exceeds `since_version`, or `timeout`
    /// elapses. Returns the retained descriptor, or `None` on timeout.
    fn wait(&self, since_version: u64, timeout: Duration) -> Option<Announced> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |s| {
                s.is_none_or(|a| a.version <= since_version)
            })
            .unwrap_or_else(|e| e.into_inner());
        state.filter(|a| a.version > since_version)
    }
}

type Lanes = Mutex<HashMap<String, Arc<Lane>>>;

static LANES: OnceLock<Lanes> = OnceLock::new();

/// Bumped by every [`install`]; a fan-out thread exits once it is no longer the
/// current generation, so a node rebuild cannot leave two threads competing.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// How often a fan-out thread wakes to re-check whether it has been superseded.
const FANOUT_POLL: Duration = Duration::from_millis(250);

fn lanes() -> &'static Lanes {
    LANES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lane(instance: &str) -> Arc<Lane> {
    let mut map = lanes().lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(map.entry(instance.to_string()).or_default())
}

/// Install the node's socket-notification receiver, replacing any prior install
/// (the node is rebuilt on a mesh off→on cycle, yielding a fresh channel and
/// fresh descriptors).
///
/// Spawns a small fan-out thread that owns the receiver and files each arriving
/// descriptor under its instance name. A thread rather than draining inside
/// [`next_fd`] because there are two independent consumers — one per Kotlin
/// radio, each on its own `HandlerThread` — and a shared single-consumer
/// channel would let either swallow the other's announcement.
pub fn install(receiver: Receiver<AppOwnedUdpSocket>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::Builder::new()
        .name("myco-udp-fd-fanout".to_string())
        .spawn(move || fan_out(receiver, generation))
        .expect("spawning the UDP fd fan-out thread");
}

fn fan_out(receiver: Receiver<AppOwnedUdpSocket>, generation: u64) {
    loop {
        if GENERATION.load(Ordering::SeqCst) != generation {
            return; // superseded by a newer node
        }
        match receiver.recv_timeout(FANOUT_POLL) {
            Ok(socket) => {
                // A `Single` UDP config has no instance name; Myco's own config
                // never produces one, but the seam allows it, so give it a key
                // of its own rather than silently filing it under a lane.
                let instance = socket.instance.unwrap_or_default();
                tracing::info!(
                    instance = %instance,
                    fd = socket.fd,
                    "UDP transport socket available to bind"
                );
                lane(&instance).announce(socket.fd);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Wait for a UDP socket on `instance` newer than `since_version`, blocking up
/// to `timeout`.
///
/// Returns `(version, fd)`, or `(since_version, -1)` if nothing newer arrived —
/// which is also what a lane whose socket never bound gets. Never another
/// lane's descriptor. Pass `0` the first time; pass back the returned version
/// afterwards, so a socket announced before the caller existed is still seen
/// exactly once, and a node restart's replacement socket is seen even when the
/// kernel hands out the same fd number again.
pub fn next_fd(instance: &str, since_version: u64, timeout: Duration) -> (u64, RawFd) {
    match lane(instance).wait(since_version, timeout) {
        Some(announced) => (announced.version, announced.fd),
        None => (since_version, -1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    /// The registry is process-global, so tests use instance names of their own
    /// rather than the real `"lan"` / `"aware"` ones.
    #[test]
    fn each_lane_only_ever_sees_its_own_descriptor() {
        let (tx, rx) = channel();
        install(rx);

        let send = |instance: &str, fd| {
            tx.send(AppOwnedUdpSocket {
                instance: Some(instance.to_string()),
                fd,
            })
            .unwrap()
        };
        send("test-aware", 41);
        send("test-lan", 42);

        // Deliberately read back in the opposite order to delivery: which fd a
        // lane gets is decided by the label, not by what arrived first.
        let (lan_v, lan_fd) = next_fd("test-lan", 0, Duration::from_secs(2));
        let (aware_v, aware_fd) = next_fd("test-aware", 0, Duration::from_secs(2));
        assert_eq!(lan_fd, 42);
        assert_eq!(aware_fd, 41);
        assert_eq!((lan_v, aware_v), (1, 1));

        // Nothing newer: the caller waits and is told nothing, rather than
        // being handed the other lane's socket or the same one twice.
        assert_eq!(
            next_fd("test-lan", lan_v, Duration::from_millis(50)),
            (lan_v, -1)
        );
        // A lane that never bound a socket is silent too.
        assert_eq!(
            next_fd("test-absent", 0, Duration::from_millis(50)),
            (0, -1)
        );

        // A node restart re-announces, and is seen even when the fd number
        // repeats — the version, not the number, is what says "re-bind this".
        send("test-lan", 42);
        assert_eq!(
            next_fd("test-lan", lan_v, Duration::from_secs(2)),
            (lan_v + 1, 42)
        );

        // A late consumer (a radio the user toggled on after the node started)
        // still learns the retained descriptor rather than blocking forever.
        assert_eq!(next_fd("test-aware", 0, Duration::from_millis(50)), (1, 41));

        // Held until here so the fan-out thread does not exit on a disconnect
        // before it has forwarded everything.
        drop(tx);
    }
}
