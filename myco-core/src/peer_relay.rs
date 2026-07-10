//! A pool of **persistent, bidirectional** WebSocket connections to peers' mesh
//! relays — **one socket per peer**, shared by both propagation planes:
//!
//! - **push** ([`PeerRelayPool::send`]): fan an `["EVENT", …]` frame to a peer
//!   (fire-and-forget) — the multi-hop flood of `docs/design/event-gossip.md`.
//! - **pull** ([`PeerRelayPool::request`]): open a `REQ`, collect the peer's
//!   matching events until `EOSE`/`CLOSED`, then close the subscription.
//!
//! A Nostr relay connection is two-way, so there is no reason to hold more than one
//! socket per destination: event fan-out, every REQ + its event replies, and the
//! keepalive all multiplex over the single connection via subscription ids (NIP-01).
//! Over BLE a TCP+WS handshake is several round-trips and concurrent connects
//! contend for the one radio, so a fresh socket per message (or a second socket for
//! pulls) is exactly what we avoid.
//!
//! Each peer gets a long-lived actor task ([`run`]) owning the split socket. It
//! `select!`s over (a) commands from the pool, (b) inbound frames, and (c) a
//! keepalive ping. **The read half is the point.** The previous pool was
//! write-only, so it never noticed a *half-open* connection: after a mesh flap the
//! peer keeps the same identity-derived `fd00::` address, but the pre-flap socket is
//! dead — and a write to a dead-but-not-yet-reset TCP socket *buffers and
//! "succeeds"* for as long as the OS retransmits (minutes on mobile). Fan-out then
//! vanished into a black hole while the mesh looked healthy. Reading the socket
//! surfaces a peer close/error at once, and a ping the peer never answers within one
//! interval catches the silent half-open; either drops the task, and the next
//! command lazily reopens a fresh connection.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nostr::Event;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Keepalive cadence: send a WS ping this often on an otherwise-idle connection.
/// If a whole interval passes with **no inbound frame at all** (not even the pong),
/// the connection is treated as dead and the task exits so it can reconnect. Short
/// enough to catch a silent half-open in seconds (a middle-node flap blackholes the
/// route with no RST), not the TCP retransmit horizon; long enough not to chatter
/// over BLE. The runtime's keepwarm loop respawns the dropped task promptly.
const PING_INTERVAL: Duration = Duration::from_secs(10);

/// A command sent to a peer's connection actor over its channel.
enum Command {
    /// Fire-and-forget: write a pre-built `["EVENT", …]` frame (the push plane).
    Publish(String),
    /// Open a `REQ` for `filters`, accumulate matching events until `EOSE`/`CLOSED`,
    /// then reply once with the batch and close the subscription (the pull plane).
    Request {
        filters: Vec<serde_json::Value>,
        reply: oneshot::Sender<Vec<Event>>,
    },
}

/// An in-flight `REQ` on a connection: events seen so far, and where to deliver them
/// on `EOSE`. The events are returned unverified — callers verify (matching the old
/// `query_relay` contract).
struct Pending {
    events: Vec<Event>,
    reply: oneshot::Sender<Vec<Event>>,
}

#[derive(Default)]
pub struct PeerRelayPool {
    /// Per-peer command channel to its live actor task. A closed sender means the
    /// task exited (connection died / connect failed); the next use respawns it.
    peers: Mutex<HashMap<String, mpsc::UnboundedSender<Command>>>,
    /// npubs whose actor currently holds a **live, connected** socket — set once
    /// `connect_async` succeeds, cleared when the task exits. The runtime diffs this
    /// each keepwarm tick: a peer transitioning absent→present is the "reappeared"
    /// edge that drives a resubscribe. Distinct from `peers` (which has an entry
    /// during the connecting window too).
    connected: Arc<Mutex<HashSet<String>>>,
}

impl PeerRelayPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// npubs that currently hold a live connected socket (see [`Self::connected`]).
    pub fn connected_npubs(&self) -> HashSet<String> {
        self.connected.lock().unwrap().clone()
    }

    /// Ensure `npub` has a running actor (which lazily connects) at `url`, without
    /// sending anything. The runtime's keepwarm loop calls this for every Circle
    /// member so a dropped connection is respawned promptly — not lazily on the next
    /// outbound frame — restoring both directions fast after a mesh flap.
    pub fn ensure(&self, npub: &str, url: &str) {
        let mut peers = self.peers.lock().unwrap();
        let _ = self.spawn_or_get(&mut peers, npub, url);
    }

    /// Get a live command channel for `npub`'s connection at `url`, spawning the
    /// actor (which lazily connects) if there isn't a running one. Caller holds the
    /// map lock.
    fn spawn_or_get(
        &self,
        peers: &mut HashMap<String, mpsc::UnboundedSender<Command>>,
        npub: &str,
        url: &str,
    ) -> mpsc::UnboundedSender<Command> {
        if let Some(tx) = peers.get(npub) {
            if !tx.is_closed() {
                return tx.clone();
            }
            peers.remove(npub);
        }
        let (tx, rx) = mpsc::unbounded_channel::<Command>();
        peers.insert(npub.to_string(), tx.clone());
        tokio::spawn(run(
            url.to_string(),
            npub.to_string(),
            rx,
            self.connected.clone(),
        ));
        tx
    }

    /// Queue a pre-built relay frame (`["EVENT", {…}]`) to a peer's relay over the
    /// persistent connection (push plane). Non-blocking; must be called from within
    /// the Tokio runtime. A frame sent before the socket finishes connecting is
    /// buffered in the channel and flushed on connect.
    pub fn send(&self, npub: &str, url: &str, frame: String) {
        let mut peers = self.peers.lock().unwrap();
        let tx = self.spawn_or_get(&mut peers, npub, url);
        // Lost the race with a task that just exited: drop the entry so the next
        // publish respawns. Rare, and the push plane is best-effort (the pull plane
        // recovers backlog on reconnect).
        if tx.send(Command::Publish(frame)).is_err() {
            peers.remove(npub);
        }
    }

    /// Query a peer's relay (pull plane): open a `REQ` for `filters` over the shared
    /// connection and collect its matching events until `EOSE`. Bounded by
    /// `timeout` — a slow/dead peer yields an empty batch rather than stalling.
    /// Events are returned as received; the caller verifies signatures.
    pub async fn request(
        &self,
        npub: &str,
        url: &str,
        filters: Vec<serde_json::Value>,
        timeout: Duration,
    ) -> Vec<Event> {
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut peers = self.peers.lock().unwrap();
            let tx = self.spawn_or_get(&mut peers, npub, url);
            if tx
                .send(Command::Request {
                    filters,
                    reply: reply_tx,
                })
                .is_err()
            {
                peers.remove(npub);
                return Vec::new();
            }
        } // release the lock before awaiting the reply
        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(events)) => events,
            // Timed out, or the actor died before EOSE (connection dropped).
            _ => Vec::new(),
        }
    }
}

/// The per-peer connection actor: connect once, then multiplex push + pull + ping
/// over the one socket until it dies or the pool drops the command channel.
async fn run(
    url: String,
    npub: String,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    connected: Arc<Mutex<HashSet<String>>>,
) {
    let Ok((ws, _)) = tokio_tungstenite::connect_async(&url).await else {
        return; // connect failed → channel drops with this task → next command respawns
    };
    // Live now — mark reachable so the keepwarm loop sees the (re)connect edge.
    connected.lock().unwrap().insert(npub.clone());
    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut next_sub: u64 = 0;
    // First tick fires one interval out (not immediately) so a just-opened idle
    // connection isn't pinged before it's had a chance to carry traffic.
    let mut ping =
        tokio::time::interval_at(tokio::time::Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut awaiting_pong = false;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                None => break, // pool dropped the sender: shut the socket down
                Some(Command::Publish(frame)) => {
                    if sink.send(Message::Text(frame)).await.is_err() {
                        break;
                    }
                }
                Some(Command::Request { filters, reply }) => {
                    let sub_id = format!("r{next_sub}");
                    next_sub += 1;
                    let mut req: Vec<serde_json::Value> = Vec::with_capacity(filters.len() + 2);
                    req.push(serde_json::Value::from("REQ"));
                    req.push(serde_json::Value::from(sub_id.clone()));
                    req.extend(filters);
                    let frame = serde_json::Value::Array(req).to_string();
                    if sink.send(Message::Text(frame)).await.is_err() {
                        let _ = reply.send(Vec::new());
                        break;
                    }
                    pending.insert(sub_id, Pending { events: Vec::new(), reply });
                }
            },
            msg = stream.next() => match msg {
                // Any inbound frame proves the peer is alive this interval.
                Some(Ok(frame)) => {
                    awaiting_pong = false;
                    match frame {
                        Message::Text(txt) => {
                            if let Some(done) = handle_inbound(&txt, &mut pending) {
                                // Tell the relay to drop the (now-satisfied) sub, so a
                                // never-closed sub doesn't accumulate on its side over
                                // this long-lived connection.
                                let close = serde_json::json!(["CLOSE", done]).to_string();
                                let _ = sink.send(Message::Text(close)).await;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {} // pong / ping / binary — liveness already noted
                    }
                }
                Some(Err(_)) | None => break, // socket error or clean EOF → reconnect on next command
            },
            _ = ping.tick() => {
                // The previous ping went a whole interval unanswered by any frame →
                // treat the connection as dead (this is the half-open catch).
                if awaiting_pong {
                    break;
                }
                if sink.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
                awaiting_pong = true;
                // Reap subs whose caller already timed out and dropped the receiver,
                // so they don't linger until the connection closes.
                let dead: Vec<String> = pending
                    .iter()
                    .filter(|(_, p)| p.reply.is_closed())
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in dead {
                    pending.remove(&id);
                    let close = serde_json::json!(["CLOSE", id]).to_string();
                    let _ = sink.send(Message::Text(close)).await;
                }
            }
        }
    }
    // No longer reachable — clear the flag so the keepwarm loop respawns us and
    // sees a fresh (re)connect edge when the peer comes back.
    connected.lock().unwrap().remove(&npub);
    // Pending replies drop here → their `request` callers get an empty batch.
    let _ = sink.send(Message::Close(None)).await;
}

/// Route one inbound relay frame to its pending `REQ`. Returns the sub id that just
/// completed (on `EOSE`/`CLOSED`) so the caller can `CLOSE` it, else `None`.
fn handle_inbound(txt: &str, pending: &mut HashMap<String, Pending>) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(txt).ok()?;
    let arr = val.as_array()?;
    match arr.first().and_then(|v| v.as_str())? {
        "EVENT" => {
            // ["EVENT", <sub_id>, <event>]
            let sub_id = arr.get(1)?.as_str()?;
            let p = pending.get_mut(sub_id)?;
            let event = serde_json::from_value::<Event>(arr.get(2)?.clone()).ok()?;
            p.events.push(event);
            None
        }
        "EOSE" | "CLOSED" => {
            let sub_id = arr.get(1)?.as_str()?.to_string();
            let p = pending.remove(&sub_id)?;
            let _ = p.reply.send(p.events);
            Some(sub_id)
        }
        _ => None, // OK (for our EVENT publishes), NOTICE, … — nothing to route
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_relay::server::serve_on;
    use myco_relay::RelayStore;
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use std::sync::Arc;

    async fn spawn_relay() -> (Arc<RelayStore>, String) {
        let store = Arc::new(RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(store.clone(), listener));
        (store, format!("ws://{addr}"))
    }

    fn chat(keys: &Keys, content: &str) -> Event {
        EventBuilder::new(Kind::from(9u16), content)
            .tags([Tag::identifier("mesh".to_string())])
            .sign_with_keys(keys)
            .unwrap()
    }

    /// A pushed EVENT lands in the peer's store, and a later `request` reads it back
    /// over the **same** pooled connection (both planes, one socket).
    #[tokio::test]
    async fn push_then_pull_over_one_connection() {
        let (store, url) = spawn_relay().await;
        let pool = PeerRelayPool::new();
        let keys = Keys::generate();
        let ev = chat(&keys, "hello mesh");

        let frame = serde_json::json!(["EVENT", ev]).to_string();
        pool.send("peerX", &url, frame);

        // Poll the store until the pushed event is stored (the push is async).
        let stored = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if store.count() > 0 {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or(false);
        assert!(stored, "pushed event should reach the peer's store");

        let got = pool
            .request(
                "peerX",
                &url,
                vec![serde_json::json!({ "kinds": [9] })],
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(got.len(), 1, "request reads the event back");
        assert_eq!(got[0].id, ev.id);
    }

    /// A request to an unreachable peer returns empty within the timeout (no hang).
    #[tokio::test]
    async fn request_to_dead_peer_times_out_empty() {
        let pool = PeerRelayPool::new();
        // Port 1 is not listening; connect fails fast, actor exits, reply drops.
        let got = pool
            .request(
                "peerDead",
                "ws://127.0.0.1:1",
                vec![serde_json::json!({ "kinds": [9] })],
                Duration::from_secs(2),
            )
            .await;
        assert!(got.is_empty());
    }

    /// `ensure` connects without a frame to send, and marks the peer connected —
    /// this is the keepwarm reconnect-edge signal the runtime diffs.
    #[tokio::test]
    async fn ensure_connects_and_reports_connected() {
        let (_store, url) = spawn_relay().await;
        let pool = PeerRelayPool::new();
        assert!(!pool.connected_npubs().contains("peerZ"));
        pool.ensure("peerZ", &url);
        let up = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pool.connected_npubs().contains("peerZ") {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or(false);
        assert!(up, "ensure should connect and mark the peer connected");
    }

    /// `ensure` to an unreachable peer never reports it connected (connect fails, the
    /// actor exits without inserting into the connected set).
    #[tokio::test]
    async fn ensure_dead_peer_stays_absent() {
        let pool = PeerRelayPool::new();
        pool.ensure("peerNope", "ws://127.0.0.1:1");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!pool.connected_npubs().contains("peerNope"));
    }
}
