//! [`RemoteBackend`] — a [`RelayBackend`] backed by **someone else's** NIP-01
//! relay, reached over a WebSocket.
//!
//! This is what makes the seam's promise real: with the store behind an ordinary
//! NIP-01 surface, the thing answering can be Citrine on the same phone, a strfry
//! on the LAN, or anything else that speaks the protocol. Myco keeps its mesh
//! behaviour in the proxy in front and asks the backend only for `EVENT` and
//! `REQ`.
//!
//! **Nothing Myco-specific goes over this link.** No `MESH` envelope, no ttl, no
//! circle. That is boundary 2 (`reference/thinning-custom-relay.md`), and it is
//! the reason an unmodified relay can serve here at all.
//!
//! One connection, held open and reopened on demand. The gateway hits the backend
//! on every page load, so a socket per request would be wasteful even against
//! localhost — and against a relay on the LAN it would be a round trip per blob
//! lookup. A single actor task owns the socket and multiplexes publishes and
//! queries over it by subscription id, the same shape
//! [`crate::peer_relay`] uses for peers.
//!
//! Reads are **not** re-verified. NIP-01 makes signature checking mandatory for a
//! relay accepting an `EVENT`, so a conforming relay has already done it, and
//! paying Schnorr again per event on a phone buys nothing. Pointing Myco at a
//! relay is therefore an act of trust, which is why configuring one warns
//! (`reference/thinning-custom-relay.md`, D7).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use nostr::{Event, Filter};
use nsite_deck::seams::RelayBackend;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Cap on a single connect. A misconfigured URL must fail fast rather than
/// hanging the gateway on SYN retransmits.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Cap on one query. A relay that accepts the `REQ` and never sends `EOSE` would
/// otherwise stall a page load indefinitely.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on waiting for an `OK` after publishing. Storing is idempotent and the
/// seam reports nothing back, so this only bounds how long we hold the caller.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);

/// Keepalive, so a silently half-open socket is noticed rather than swallowing
/// writes. Same hazard the peer pool documents: a write to a dead-but-unreset
/// socket buffers and looks like success for as long as the OS retransmits.
const PING_INTERVAL: Duration = Duration::from_secs(30);

enum Command {
    Publish {
        event: Box<Event>,
        reply: oneshot::Sender<bool>,
    },
    Query {
        filters: Vec<Filter>,
        reply: oneshot::Sender<Vec<Event>>,
    },
}

/// An in-flight `REQ`: events so far, and where to deliver them on `EOSE`.
struct Pending {
    events: Vec<Event>,
    reply: oneshot::Sender<Vec<Event>>,
}

/// A relay we do not own, used as the event store.
pub struct RemoteBackend {
    url: String,
    tx: Mutex<Option<mpsc::UnboundedSender<Command>>>,
}

impl RemoteBackend {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            tx: Mutex::new(None),
        }
    }

    /// The URL this backend talks to, for diagnostics and the settings screen.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A live command channel, starting the connection actor if the last one
    /// exited. Lazy on purpose: a backend configured but unreachable should cost
    /// nothing until something actually reads or writes.
    fn sender(&self) -> mpsc::UnboundedSender<Command> {
        let mut slot = self.tx.lock().unwrap();
        if let Some(tx) = slot.as_ref() {
            if !tx.is_closed() {
                return tx.clone();
            }
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let url = self.url.clone();
        tokio::spawn(async move { run(url, rx).await });
        *slot = Some(tx.clone());
        tx
    }
}

#[async_trait]
impl RelayBackend for RemoteBackend {
    async fn publish(&self, event: Event) -> anyhow::Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = Command::Publish {
            event: Box::new(event),
            reply: reply_tx,
        };
        if self.sender().send(cmd).is_err() {
            anyhow::bail!("relay backend unavailable: {}", self.url);
        }
        match tokio::time::timeout(PUBLISH_TIMEOUT, reply_rx).await {
            Ok(Ok(true)) => Ok(()),
            // The relay answered `OK false`, or the socket died mid-write. Either
            // way the event is not stored, and the caller should know: unlike the
            // embedded store, this can fail for reasons outside our control.
            Ok(Ok(false)) => anyhow::bail!("relay backend rejected the event"),
            Ok(Err(_)) | Err(_) => anyhow::bail!("relay backend did not acknowledge in time"),
        }
    }

    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = Command::Query {
            filters: filters.to_vec(),
            reply: reply_tx,
        };
        if self.sender().send(cmd).is_err() {
            anyhow::bail!("relay backend unavailable: {}", self.url);
        }
        match tokio::time::timeout(QUERY_TIMEOUT, reply_rx).await {
            Ok(Ok(events)) => Ok(events),
            // A read failure is an error rather than an empty result: an empty
            // set means "the relay has nothing", and the gateway would render
            // that as a missing site.
            Ok(Err(_)) | Err(_) => anyhow::bail!("relay backend did not answer in time"),
        }
    }
}

/// One connection's lifetime: connect, then multiplex commands over it until the
/// socket dies. Exiting drops the command channel, which is how [`sender`] knows
/// to start a fresh actor on the next call.
///
/// [`sender`]: RemoteBackend::sender
async fn run(url: String, mut rx: mpsc::UnboundedReceiver<Command>) {
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url));
    let ws = match connect.await {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => {
            tracing::warn!(url, error = %e, "relay backend: connect failed");
            return;
        }
        Err(_) => {
            tracing::warn!(url, "relay backend: connect timed out");
            return;
        }
    };
    tracing::info!(url, "relay backend: connected");

    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<String, Pending> = HashMap::new();
    // Publishes awaiting an `OK`, keyed by event id.
    let mut publishes: HashMap<String, oneshot::Sender<bool>> = HashMap::new();
    let mut next_sub: u64 = 0;
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await; // the first tick is immediate

    let reason = loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                None => break "pool dropped",
                Some(Command::Publish { event, reply }) => {
                    let id = event.id.to_hex();
                    let frame = serde_json::json!(["EVENT", *event]).to_string();
                    if sink.send(Message::Text(frame.into())).await.is_err() {
                        let _ = reply.send(false);
                        break "write failed (publish)";
                    }
                    publishes.insert(id, reply);
                }
                Some(Command::Query { filters, reply }) => {
                    let sub_id = format!("q{next_sub}");
                    next_sub += 1;
                    let mut req: Vec<serde_json::Value> =
                        vec![serde_json::Value::from("REQ"), sub_id.clone().into()];
                    req.extend(filters.iter().filter_map(|f| serde_json::to_value(f).ok()));
                    let frame = serde_json::Value::Array(req).to_string();
                    if sink.send(Message::Text(frame.into())).await.is_err() {
                        let _ = reply.send(Vec::new());
                        break "write failed (query)";
                    }
                    pending.insert(sub_id, Pending { events: Vec::new(), reply });
                }
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(txt))) => {
                    if let Some(done) = handle_inbound(&txt, &mut pending, &mut publishes) {
                        // Close the satisfied subscription so it does not linger
                        // on the relay's side.
                        let close = serde_json::json!(["CLOSE", done]).to_string();
                        if sink.send(Message::Text(close.into())).await.is_err() {
                            break "write failed (close)";
                        }
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    if sink.send(Message::Pong(p)).await.is_err() {
                        break "write failed (pong)";
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    tracing::debug!(url, error = %e, "relay backend: read error");
                    break "read error";
                }
                None => break "closed by relay",
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break "write failed (ping)";
                }
            }
        }
    };

    // Release anything still waiting, so a caller gets its error rather than
    // sitting until its own timeout.
    for (_, p) in pending.drain() {
        let _ = p.reply.send(Vec::new());
    }
    for (_, reply) in publishes.drain() {
        let _ = reply.send(false);
    }
    tracing::info!(url, reason, "relay backend: connection closed");
}

/// Route one inbound frame. Returns a subscription id that has just been
/// satisfied, so the caller can `CLOSE` it.
fn handle_inbound(
    txt: &str,
    pending: &mut HashMap<String, Pending>,
    publishes: &mut HashMap<String, oneshot::Sender<bool>>,
) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(txt).ok()?;
    let arr = val.as_array()?;
    match arr.first().and_then(|v| v.as_str())? {
        "EVENT" => {
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
        "OK" => {
            // ["OK", <event_id>, <accepted>, <message>]
            let id = arr.get(1)?.as_str()?;
            let accepted = arr.get(2)?.as_bool().unwrap_or(false);
            if let Some(reply) = publishes.remove(id) {
                let _ = reply.send(accepted);
            }
            None
        }
        _ => None, // NOTICE, AUTH, … — nothing to route
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsite_deck::seams::AdminBackend;
    use std::sync::Arc;

    /// Stand up the embedded relay and drive it through `RemoteBackend`, which is
    /// the arrangement P6 is for: Myco talking to a relay it does not own, over
    /// nothing but NIP-01.
    async fn spawn_relay() -> (Arc<myco_relay::RelayStore>, String) {
        let store = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(crate::mesh_relay::serve_on(store.clone(), listener));
        (store, format!("ws://{addr}"))
    }

    fn chat(keys: &nostr::Keys, content: &str) -> Event {
        nostr::EventBuilder::new(nostr::Kind::from(9u16), content)
            .tags([nostr::Tag::identifier("mesh".to_string())])
            .sign_with_keys(keys)
            .unwrap()
    }

    /// Publish and read back over a socket, with the same connection reused —
    /// the round trip the gateway depends on.
    #[tokio::test]
    async fn publishes_and_queries_over_one_connection() {
        let (store, url) = spawn_relay().await;
        let backend = RemoteBackend::new(&url);
        let keys = nostr::Keys::generate();

        let first = chat(&keys, "one");
        let second = chat(&keys, "two");
        backend.publish(first.clone()).await.unwrap();
        backend.publish(second.clone()).await.unwrap();
        assert_eq!(store.count(), 2, "both reached the relay");

        let got = backend
            .query(&[Filter::new().kind(nostr::Kind::from(9u16))])
            .await
            .unwrap();
        assert_eq!(got.len(), 2);

        // Re-publishing is idempotent, exactly as the seam promises.
        backend.publish(first).await.unwrap();
        assert_eq!(store.count(), 2);
    }

    /// The full filter surface has to survive the trip, or the embedded and
    /// remote backends would disagree about what a query means.
    #[tokio::test]
    async fn filters_are_honoured_by_the_remote_relay() {
        let (_store, url) = spawn_relay().await;
        let backend = RemoteBackend::new(&url);
        let keys = nostr::Keys::generate();

        let old = nostr::EventBuilder::new(nostr::Kind::from(9u16), "old")
            .custom_created_at(nostr::Timestamp::from(1_000))
            .sign_with_keys(&keys)
            .unwrap();
        let recent = nostr::EventBuilder::new(nostr::Kind::from(9u16), "recent")
            .custom_created_at(nostr::Timestamp::from(9_000))
            .sign_with_keys(&keys)
            .unwrap();
        backend.publish(old.clone()).await.unwrap();
        backend.publish(recent.clone()).await.unwrap();

        let windowed = backend
            .query(&[Filter::new().since(nostr::Timestamp::from(5_000))])
            .await
            .unwrap();
        assert_eq!(windowed.len(), 1, "since must reach the relay");
        assert_eq!(windowed[0].id, recent.id);
    }

    /// A relay that is not there must fail, not hang and not look empty.
    ///
    /// An empty result would read as "the site does not exist" and the gateway
    /// would render a 404 for content that is merely unreachable.
    #[tokio::test]
    async fn an_unreachable_relay_is_an_error_not_an_empty_answer() {
        // Port 1 is reserved and nothing listens there.
        let backend = RemoteBackend::new("ws://127.0.0.1:1");
        let err = backend
            .query(&[Filter::new().kind(nostr::Kind::from(9u16))])
            .await;
        assert!(err.is_err(), "an unreachable relay is an error");

        let keys = nostr::Keys::generate();
        assert!(backend.publish(chat(&keys, "nobody home")).await.is_err());
    }

    /// The actor reconnects once its socket dies, rather than the backend going
    /// permanently cold after the first blip.
    #[tokio::test]
    async fn a_dropped_connection_is_reopened_on_the_next_call() {
        let (store, url) = spawn_relay().await;
        let backend = RemoteBackend::new(&url);
        let keys = nostr::Keys::generate();

        backend.publish(chat(&keys, "before")).await.unwrap();

        // Drop the actor the way a dead socket would.
        {
            let mut slot = backend.tx.lock().unwrap();
            *slot = None;
        }

        backend.publish(chat(&keys, "after")).await.unwrap();
        assert_eq!(store.count(), 2, "the second publish reconnected");

        // The store is ours in this test, so tidy up through the admin seam.
        AdminBackend::wipe(store.as_ref()).await.unwrap();
    }
}
