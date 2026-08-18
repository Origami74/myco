//! Bounded queue and drainer for platform-discovered peers.
//!
//! Wi-Fi Aware and the `!FIPS` AP lane are discovered by Kotlin, not by the
//! mesh stack, so the app has to tell a *running* node "peer `<npub>` is
//! reachable at `<address>`". fips used to expose a process-global queue for
//! exactly this; it is now the control socket's `connect` command, and that
//! changes the threading contract enough to need an adapter on this side.
//!
//! `connect` is a mutation, so it does not render off a snapshot — it goes
//! through the rx loop's control arm and is awaited inside the packet loop,
//! behind whatever that loop is doing. The push, meanwhile, arrives on
//! `AwareRadio`'s or `ApRadio`'s single `HandlerThread`, which also serves
//! every `NetworkCallback` and discovery callback for that lane. Blocking one
//! of those for a control round-trip would serialise every subsequent NDP-up,
//! NDP-lost and network-change event behind it — during a discovery burst,
//! which is precisely when several peers are coming up at once.
//!
//! So: [`push`] never blocks and never touches the socket, and a tokio task
//! owns the client and issues `connect`.
//!
//! **Overflow policy: drop-new.** A full queue discards the arriving push
//! rather than the oldest queued one. Both are defensible; drop-new is what a
//! bounded `try_send` gives, and it favours peers already waiting to be dialled
//! over a burst that is, by definition, still arriving. Losing a push is
//! acceptable either way — platform discovery re-fires periodically.
//!
//! There is no withdrawal counterpart, deliberately. Myco's old `peer_lost`
//! call was a no-op (the UDP transport does not override
//! `close_connection`), and the control socket's `disconnect` keys on npub
//! alone and does a full peer teardown — so a routine Aware NDP drop would
//! kill a live BLE session to the same peer and suppress its reconnect.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{Receiver, Sender};

use crate::control_client::ControlClient;

/// Queued pushes held before the drainer gets to them.
///
/// Sized for a discovery burst, not for a backlog: a peer that does not make it
/// out of here within a few seconds is better re-discovered than re-dialled
/// from a stale address.
const QUEUE_CAP: usize = 64;

/// How long the same `(npub, address)` is suppressed after being enqueued.
///
/// Kotlin re-pushes on every rediscovery, on the assumption that the mesh
/// dedups. That is true of the *handshake*, not of the dial: `api_connect`
/// either refreshes an already-connected peer's path or calls
/// `initiate_peer_connection` outright, with no freshness gate and no connect
/// budget of the kind the old in-tree queue had. Without this, an Aware
/// discovery burst becomes a dial burst.
const REPUSH_SUPPRESSION: Duration = Duration::from_secs(10);

/// Attempts per queued push, covering the window where the node is up but the
/// control socket is not yet accepting — fips binds it inside `run_rx_loop`,
/// which only starts after `node.start()` completes.
const CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_RETRY_DELAY: Duration = Duration::from_secs(2);

/// One platform-discovered peer, as Kotlin observed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformPeer {
    /// The peer's npub, from the Aware service-info or the mDNS TXT record. A
    /// routing hint only — Noise IK is what authenticates.
    pub npub: String,
    /// A fully-formatted socket address. Link-locals carry a *numeric* scope
    /// (`"[fe80::x%3]:4871"`); interface-name scopes do not parse.
    pub address: String,
    /// The fips transport that will carry it, qualified with the instance
    /// name of the lane's own UDP socket (`"udp/aware"`, `"udp/lan"`). Both
    /// lanes ride UDP, but each has its own socket pinned to its own
    /// `android.net.Network`, and a dial down the wrong one is unroutable.
    pub transport: String,
}

struct Queue {
    tx: Sender<PlatformPeer>,
    /// Taken once, by the drainer.
    rx: Mutex<Option<Receiver<PlatformPeer>>>,
    /// When each `(npub, address)` was last accepted, for re-push suppression.
    last_push: Mutex<HashMap<(String, String), Instant>>,
}

static QUEUE: OnceLock<Queue> = OnceLock::new();

fn queue() -> &'static Queue {
    QUEUE.get_or_init(|| {
        let (tx, rx) = tokio::sync::mpsc::channel(QUEUE_CAP);
        Queue {
            tx,
            rx: Mutex::new(Some(rx)),
            last_push: Mutex::new(HashMap::new()),
        }
    })
}

/// Enqueue a platform-discovered peer. Never blocks; safe from any thread,
/// including one with no tokio runtime.
///
/// Returns whether it was accepted, for logging only — no caller can do
/// anything useful with a rejection, and both rejection reasons (suppressed as
/// a duplicate, or queue full) are expected traffic rather than faults.
pub fn push(npub: &str, address: &str, transport: &str) -> bool {
    let key = (npub.to_string(), address.to_string());
    {
        let mut last = queue().last_push.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        // Prune while we hold the lock; the map is otherwise unbounded in the
        // number of addresses a peer has ever been seen at.
        last.retain(|_, at| now.duration_since(*at) < REPUSH_SUPPRESSION);
        if last.contains_key(&key) {
            return false;
        }
        last.insert(key, now);
    }
    queue()
        .tx
        .try_send(PlatformPeer {
            npub: npub.to_string(),
            address: address.to_string(),
            transport: transport.to_string(),
        })
        .is_ok()
}

/// Drain the queue onto the control socket, forever.
///
/// Held rather than dropped while the node is down: `ApRadio` arms its network
/// callback independently of node lifecycle, so pushes routinely precede
/// `StartNode` and outlive a BLE off→on rebuild. The queue is process-global
/// and the drainer is spawned once, so both survive a node being replaced
/// underneath them.
pub fn spawn_drainer(
    rt: &tokio::runtime::Runtime,
    control: ControlClient,
    node_live: Arc<AtomicBool>,
) {
    let Some(mut rx) = queue().rx.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return; // already spawned
    };
    rt.spawn(async move {
        loop {
            // Wait for a node before taking the item, so a push that raced
            // StartNode is held in the queue rather than binned on arrival.
            while !node_live.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let Some(peer) = rx.recv().await else {
                return; // sender is static, so only on shutdown
            };
            deliver(&control, &peer).await;
        }
    });
}

/// Issue one `connect`, retrying a few times so a push that arrives while the
/// rx loop is still coming up is not lost to the startup window.
async fn deliver(control: &ControlClient, peer: &PlatformPeer) {
    for attempt in 1..=CONNECT_ATTEMPTS {
        match control
            .connect_peer(&peer.npub, &peer.address, &peer.transport)
            .await
        {
            Ok(()) => return,
            Err(e) if attempt == CONNECT_ATTEMPTS => {
                tracing::warn!(
                    npub = %peer.npub,
                    address = %peer.address,
                    error = %e,
                    "platform peer push gave up; discovery will re-fire"
                );
            }
            Err(e) => {
                tracing::debug!(
                    npub = %peer.npub,
                    attempt,
                    error = %e,
                    "platform peer push failed; retrying"
                );
                tokio::time::sleep(CONNECT_RETRY_DELAY).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue is process-global, so the tests share it and must not assume
    /// they own it — and there is exactly one receiver to take, so this is one
    /// test rather than several racing for it.
    fn drain_now(rx: &mut Receiver<PlatformPeer>) -> Vec<PlatformPeer> {
        let mut out = Vec::new();
        while let Ok(p) = rx.try_recv() {
            out.push(p);
        }
        out
    }

    #[tokio::test]
    async fn the_queue_suppresses_duplicates_and_drops_rather_than_blocks() {
        let mut rx = queue()
            .rx
            .lock()
            .unwrap()
            .take()
            .expect("no drainer runs in tests");

        // Re-push suppression is per (npub, address): the same pair inside the
        // window is a duplicate dial, a new address for the same peer is a path
        // change and must get through.
        assert!(push("npub1sup", "[fe80::1%3]:4871", "udp"));
        assert!(
            !push("npub1sup", "[fe80::1%3]:4871", "udp"),
            "an immediate re-push of the same (npub, address) is a duplicate dial"
        );
        assert!(
            push("npub1sup", "[fe80::2%3]:4871", "udp"),
            "a new address for the same peer is a path change, not a duplicate"
        );

        let got = drain_now(&mut rx);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].address, "[fe80::1%3]:4871");
        assert_eq!(got[1].address, "[fe80::2%3]:4871");
        assert_eq!(got[0].transport, "udp");

        // Overflow must not block the caller: it is a framework callback thread
        // that also serves every other event on its lane. Nothing is drained
        // meanwhile, so the queue genuinely fills.
        let mut accepted = 0;
        for i in 0..(QUEUE_CAP * 2) {
            if push("npub1full", &format!("[fe80::{i}%3]:4871"), "udp") {
                accepted += 1;
            }
        }
        assert!(
            accepted <= QUEUE_CAP,
            "the queue is bounded at {QUEUE_CAP}, accepted {accepted}"
        );
        assert!(accepted > 0, "pushes must get through at all");
        assert_eq!(
            drain_now(&mut rx).len(),
            accepted,
            "everything accepted is queued, and nothing else is"
        );

        *queue().rx.lock().unwrap() = Some(rx);
    }
}
