//! The **local relay**: the NIP-01 WebSocket front door serving the in-app
//! WebView at `ws://localhost:4870` (loopback only — other phones never reach
//! it), with a relay store behind it.
//!
//! This is the only Myco-specific code on the content path. It keeps **live
//! subscriptions** (a `REQ` stays open; newly-stored events that match are pushed
//! as they arrive), and a napplet's subscriptions are fed from the same live bus
//! ([`RelayHub::live_events`]), so an nsite and a napplet see the same events by
//! the same route.
//!
//! It lives here rather than in `myco-relay` so the store behind it stays a plain
//! NIP-01 relay with no Myco concepts in it, and can be swapped for any other
//! relay later. See `reference/thinning-custom-relay.md`.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use nostr::Event;
use nostr::Filter;
use nsite_deck::seams::RelayBackend;
use tokio::sync::broadcast;

use myco_relay::matches_filter;
#[cfg(test)]
use myco_relay::RelayStore;

/// How soon the same event may be passed on again through the same door (a
/// napplet's relay rebroadcast). A rebroadcast needs no signature, so
/// nothing else paces a napplet that loops on one.
pub(crate) const PASS_ON_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a non-expiring event's id is remembered as seen. Only has to
/// outlast a copy arriving by a second path (a publish and a pull of it).
const SEEN_RETENTION_SECS: u64 = 30 * 60;
/// Floor for an event that carries a NIP-40 expiry in the past or very near
/// future — remember it briefly regardless.
const SEEN_FLOOR_SECS: u64 = 60;
/// Cap on remembered ids. At ~40 bytes an entry this is a few hundred KB worst
/// case, and chat (the high-rate kind) expires itself out well before the cap.
const SEEN_CAPACITY: usize = 4096;

/// The ids this hub has already delivered live, so a copy arriving by a
/// second path is stored but not delivered twice.
///
/// Deliberately **not** the store's dedup: storage answers "do I hold this?",
/// and an id GC'd at NIP-40 expiry looks new again on the next pull.
#[derive(Default)]
struct SeenSet {
    inner: Mutex<SeenInner>,
}
#[derive(Default)]
struct SeenInner {
    /// id -> the second after which the id may be forgotten.
    until: HashMap<[u8; 32], u64>,
    /// Insertion order, so the oldest id is the one evicted at capacity.
    order: VecDeque<[u8; 32]>,
}

impl SeenSet {
    /// Record an id, returning `true` if this is the **first** time we have seen
    /// it — the caller's signal to deliver it live.
    fn insert(&self, event: &Event) -> bool {
        let now = crate::content::now_secs();
        // Remember an expiring event until it expires; anything else for a
        // fixed window. Either way at least the floor.
        let until = myco_relay::expiration(event)
            .unwrap_or(now + SEEN_RETENTION_SECS)
            .max(now + SEEN_FLOOR_SECS);

        let id = event.id.to_bytes();
        let mut inner = self.inner.lock().unwrap();
        inner.gc(now);
        if inner.until.contains_key(&id) {
            return false;
        }
        while inner.order.len() >= SEEN_CAPACITY {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            inner.until.remove(&oldest);
        }
        inner.until.insert(id, until);
        inner.order.push_back(id);
        true
    }
}

impl SeenInner {
    /// Drop ids whose retention has run out. Cheap in the common case: the queue
    /// is in insertion order and retention is near-uniform, so this stops at the
    /// first live entry.
    fn gc(&mut self, now: u64) {
        while let Some(id) = self.order.front() {
            match self.until.get(id) {
                Some(&until) if until > now => break,
                Some(_) => {
                    let id = *id;
                    self.until.remove(&id);
                    self.order.pop_front();
                }
                // Already evicted by capacity; drop the stale queue entry.
                None => {
                    self.order.pop_front();
                }
            }
        }
    }
}

/// Shared per-relay state: the store, and a broadcast bus that fans
/// newly-stored events to all live subscriptions on this device.
///
/// One hub can back several listeners via [`serve_on_hub`], so the live bus
/// and store are shared across them, and napplets publish into the same hub
/// in-process.
pub struct RelayHub {
    /// Whatever is answering NIP-01 behind us: the embedded store by default,
    /// or a relay the user pointed us at. The proxy never needs to know which.
    store: Arc<dyn RelayBackend>,
    /// Where events that are only passing through are written: pulled
    /// backlog. The shell cache in production (`Content::cache_relay`);
    /// `store` itself unless one is given.
    transient: Arc<dyn RelayBackend>,
    live: broadcast::Sender<Event>,
    /// Ids already delivered live here. Shared across every listener on this
    /// hub.
    seen: SeenSet,
    /// When each event was last passed on, per door, for [`PASS_ON_COOLDOWN`].
    passed_on: Mutex<HashMap<([u8; 32], &'static str), std::time::Instant>>,
}

impl RelayHub {
    /// Build a shared hub over one store.
    #[cfg(test)]
    pub fn new(store: Arc<dyn RelayBackend>) -> Arc<Self> {
        let transient = store.clone();
        Self::with_cache(store, transient)
    }

    /// As [`RelayHub::new`], writing passing events to `transient` rather
    /// than `store`: what this device publishes is kept, what it pulls is
    /// cached.
    pub fn with_cache(store: Arc<dyn RelayBackend>, transient: Arc<dyn RelayBackend>) -> Arc<Self> {
        // Buffer enough that a brief subscriber stall doesn't drop chat; an
        // over-capacity lag is surfaced as `Lagged` and skipped, not blocked.
        let (live, _) = broadcast::channel(512);
        Arc::new(Self {
            store,
            transient,
            live,
            seen: SeenSet::default(),
            passed_on: Mutex::new(HashMap::new()),
        })
    }
}

impl RelayHub {
    /// Every event this hub accepts, from any source.
    ///
    /// The same stream the loopback WebSocket serves to nsites. A napplet's
    /// subscriptions are fed from here too, so a napplet and an nsite see the
    /// same events by the same route rather than through two mechanisms that
    /// can drift apart.
    pub fn live_events(&self) -> broadcast::Receiver<Event> {
        self.live.subscribe()
    }

    /// Whether a napplet may pass `event` on through `door` now, recording it
    /// if so. Refused when it went out through the same door under
    /// [`PASS_ON_COOLDOWN`] ago.
    pub(crate) fn claim_pass_on(&self, event: &Event, door: &'static str) -> anyhow::Result<()> {
        let id = event.id.to_bytes();
        let now = std::time::Instant::now();
        let mut passed = self.passed_on.lock().unwrap();
        passed.retain(|_, at| now.duration_since(*at) < PASS_ON_COOLDOWN);
        if passed.contains_key(&(id, door)) {
            anyhow::bail!("it was passed on moments ago; try again shortly");
        }
        passed.insert((id, door), now);
        Ok(())
    }

    /// Accept an event this device published: dedupe, store (kept), and wake
    /// live subscriptions.
    ///
    /// A napplet's `relay.publish` is bound for relays (NAP-RELAY), and still
    /// has to reach this phone's own subscriptions. Backlog pulled from
    /// elsewhere goes through [`RelayHub::accept_pulled`] instead.
    ///
    /// Returns whether this was the first sighting.
    pub async fn accept_unforwarded(&self, event: Event) -> anyhow::Result<bool> {
        self.accept_into(&self.store, event).await
    }

    /// As [`RelayHub::accept_unforwarded`], for backlog *pulled* from another
    /// relay: written to the cache, not kept.
    pub async fn accept_pulled(&self, event: Event) -> anyhow::Result<bool> {
        self.accept_into(&self.transient, event).await
    }

    async fn accept_into(
        &self,
        store: &Arc<dyn RelayBackend>,
        event: Event,
    ) -> anyhow::Result<bool> {
        let first_sighting = self.seen.insert(&event);
        store.publish(event.clone()).await?;
        if first_sighting {
            let _ = self.live.send(event);
        }
        Ok(first_sighting)
    }
}

/// Bind a listener for the relay. For an IPv6 address this is **`IPV6_V6ONLY`**.
/// Returns the bind error so the caller can warn the user (e.g. the port is
/// already in use). Must be called within a Tokio runtime.
pub fn bind(addr: SocketAddr) -> anyhow::Result<tokio::net::TcpListener> {
    let domain = if addr.is_ipv6() {
        socket2::Domain::IPV6
    } else {
        socket2::Domain::IPV4
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(tokio::net::TcpListener::from_std(socket.into())?)
}

/// Serve on an already-bound listener (the test path).
#[cfg(test)]
pub async fn serve_on(
    store: Arc<dyn RelayBackend>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    serve_on_hub(RelayHub::new(store), listener).await
}

/// Serve a pre-built (shared) [`RelayHub`] on `listener`. Spawn this once per
/// listener that should share the same store + live bus.
pub async fn serve_on_hub<L>(hub: Arc<RelayHub>, listener: L) -> anyhow::Result<()>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    let app = Router::new().route("/", get(root)).with_state(hub);
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

/// `/` is the WS endpoint.
async fn root(ws: WebSocketUpgrade, State(hub): State<Arc<RelayHub>>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, hub))
}

/// One client connection: serve `REQ` backlog + keep the subscription live, accept
/// `EVENT`s (store → fan to live subs), honour `CLOSE`.
async fn handle_ws(socket: WebSocket, hub: Arc<RelayHub>) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut live = hub.live.subscribe();
    // Active subscriptions on this connection: sub_id -> its filters.
    let mut subs: HashMap<String, Vec<Filter>> = HashMap::new();

    'conn: loop {
        tokio::select! {
            incoming = ws_rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        for reply in handle_client_frame(text.as_str(), &hub, &mut subs).await {
                            if ws_tx.send(Message::text(reply)).await.is_err() {
                                break 'conn;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if ws_tx.send(Message::Pong(payload)).await.is_err() {
                            break 'conn;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break 'conn,
                    Some(Err(_)) => break 'conn,
                    _ => {}
                }
            }
            event = live.recv() => {
                match event {
                    Ok(ev) => {
                        for (sub_id, filters) in subs.iter() {
                            if filters.iter().any(|f| matches_filter(&ev, f)) {
                                let frame = serde_json::json!(["EVENT", sub_id, ev]).to_string();
                                if ws_tx.send(Message::text(frame)).await.is_err() {
                                    break 'conn;
                                }
                            }
                        }
                    }
                    // Lagged: this slow subscriber missed some events — skip them
                    // and carry on rather than dropping the connection.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break 'conn,
                }
            }
        }
    }
}

/// Handle one client frame, mutating `subs` and returning the frames to send back.
async fn handle_client_frame(
    text: &str,
    hub: &Arc<RelayHub>,
    subs: &mut HashMap<String, Vec<Filter>>,
) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    match array.first().and_then(|v| v.as_str()) {
        Some("REQ") => {
            let sub_id = array
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Filters are parsed by the `nostr` crate, so the whole NIP-01
            // filter surface works rather than a hand-rolled subset.
            let filters: Vec<Filter> = array
                .iter()
                .skip(2)
                .filter_map(|f| serde_json::from_value::<Filter>(f.clone()).ok())
                .collect();

            // Stored backlog: any-match across the REQ's filters, newest first.
            let mut events: Vec<Event> = hub.store.query(&filters).await.unwrap_or_default();
            events.sort_by_key(|e| std::cmp::Reverse(e.created_at));
            events.dedup_by(|a, b| a.id == b.id);

            // Keep the subscription open so matching new events stream live.
            subs.insert(sub_id.clone(), filters);

            let mut replies: Vec<String> = events
                .iter()
                .map(|e| serde_json::json!(["EVENT", sub_id, e]).to_string())
                .collect();
            replies.push(serde_json::json!(["EOSE", sub_id]).to_string());
            replies
        }
        Some("EVENT") => {
            let Some(event_value) = array.get(1) else {
                return Vec::new();
            };
            let Ok(event) = serde_json::from_value::<Event>(event_value.clone()) else {
                return Vec::new();
            };
            let id = event.id.to_hex();
            if event.verify().is_err() {
                return vec![
                    serde_json::json!(["OK", id, false, "invalid: bad signature"]).to_string(),
                ];
            }
            // An in-app client's publish is this device's own and is kept.
            match hub.accept_unforwarded(event).await {
                Ok(true) => vec![serde_json::json!(["OK", id, true, ""]).to_string()],
                // Duplicate: still an accepted outcome per NIP-01.
                Ok(false) => vec![serde_json::json!(["OK", id, true, "duplicate:"]).to_string()],
                Err(e) => {
                    vec![serde_json::json!(["OK", id, false, format!("error: {e}")]).to_string()]
                }
            }
        }
        Some("CLOSE") => {
            if let Some(sub_id) = array.get(1).and_then(|v| v.as_str()) {
                subs.remove(sub_id);
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use nsite_deck::model::KIND_ROOT;
    use nsite_deck::testing::build_test_site_with_keys;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    async fn spawn_relay(store: Arc<RelayStore>) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(store, listener));
        addr
    }

    fn chat_event(keys: &Keys, room: &str, content: &str) -> Event {
        EventBuilder::new(Kind::from(9u16), content)
            .tags([Tag::identifier(room.to_string())])
            .sign_with_keys(keys)
            .unwrap()
    }

    #[tokio::test]
    async fn ws_relay_serves_req_then_eose() {
        let store = Arc::new(RelayStore::in_memory());
        let keys = Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"x")], None, None);
        store.admit_event(site.manifest.clone()).await.unwrap();

        let addr = spawn_relay(store.clone()).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let req = serde_json::json!([
            "REQ", "s1",
            { "kinds": [KIND_ROOT], "authors": [hex::encode(keys.public_key().to_bytes())] }
        ]);
        ws.send(WsMessage::Text(req.to_string())).await.unwrap();

        let mut got_event = false;
        let mut got_eose = false;
        while let Some(Ok(WsMessage::Text(txt))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
            match v[0].as_str() {
                Some("EVENT") => {
                    assert_eq!(
                        v[2]["id"].as_str(),
                        Some(site.manifest.id.to_hex().as_str())
                    );
                    got_event = true;
                }
                Some("EOSE") => {
                    got_eose = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(got_event, "relay should return the stored manifest");
        assert!(got_eose, "relay should send EOSE");
    }

    #[tokio::test]
    async fn ws_relay_accepts_event_and_rejects_bad_sig() {
        let store = Arc::new(RelayStore::in_memory());
        let addr = spawn_relay(store.clone()).await;

        let keys = Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"y")], None, None);

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        ws.send(WsMessage::Text(
            serde_json::json!(["EVENT", site.manifest]).to_string(),
        ))
        .await
        .unwrap();

        if let Some(Ok(WsMessage::Text(txt))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
            assert_eq!(v[0].as_str(), Some("OK"));
            assert_eq!(v[2].as_bool(), Some(true), "valid signed event accepted");
        } else {
            panic!("expected OK frame");
        }
        assert_eq!(store.count(), 1, "event stored");
    }

    /// A live subscriber receives matching events published after its REQ/EOSE.
    #[tokio::test]
    async fn live_subscription_delivers_new_events() {
        let store = Arc::new(RelayStore::in_memory());
        let addr = spawn_relay(store).await;
        let keys = Keys::generate();

        // Subscriber: REQ kind-9 #mesh, then read past EOSE.
        let (mut sub, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let req = serde_json::json!(["REQ", "s1", { "kinds": [9], "#d": ["mesh"] }]);
        sub.send(WsMessage::Text(req.to_string())).await.unwrap();
        // Drain until EOSE so we know the live subscription is registered.
        loop {
            if let Some(Ok(WsMessage::Text(txt))) = sub.next().await {
                let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
                if v[0].as_str() == Some("EOSE") {
                    break;
                }
            }
        }

        // Publisher: a second connection sends a new chat message.
        let (mut pubr, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let msg = chat_event(&keys, "mesh", "live hello");
        pubr.send(WsMessage::Text(
            serde_json::json!(["EVENT", msg]).to_string(),
        ))
        .await
        .unwrap();

        // The subscriber should receive it live as ["EVENT","s1",{…}].
        let received = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(Ok(WsMessage::Text(txt))) = sub.next().await {
                let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
                if v[0].as_str() == Some("EVENT") && v[1].as_str() == Some("s1") {
                    return v[2]["id"].as_str().map(str::to_string);
                }
            }
            None
        })
        .await
        .expect("did not receive live event in time");
        assert_eq!(
            received,
            Some(msg.id.to_hex()),
            "live event delivered to subscriber"
        );
    }

    /// A napplet cannot pass on the same event through the same door twice
    /// in a row.
    #[tokio::test]
    async fn pass_on_has_a_cooldown() {
        let hub = RelayHub::new(Arc::new(nsite_deck::testing::MemRelay::new()));
        let keys = Keys::generate();
        let local = chat_event(&keys, "mesh", "fine to pass on");
        hub.claim_pass_on(&local, "relays").unwrap();
        assert!(hub.claim_pass_on(&local, "relays").is_err());
    }

    /// Backlog pulled from elsewhere wakes this device's live subscriptions —
    /// which is how a napplet's subscription hears it — once.
    #[tokio::test]
    async fn a_pulled_event_is_delivered_live_once() {
        let store = Arc::new(RelayStore::in_memory());
        let hub = RelayHub::new(store.clone());
        let mut live = hub.live_events();

        let keys = Keys::generate();
        let msg = chat_event(&keys, "mesh", "from a relay's backlog");
        assert!(hub.accept_unforwarded(msg.clone()).await.unwrap());
        assert_eq!(live.recv().await.unwrap().id, msg.id);
        assert_eq!(store.count(), 1);

        // A copy by another path is stored idempotently and not re-delivered.
        assert!(!hub.accept_unforwarded(msg.clone()).await.unwrap());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), live.recv())
                .await
                .is_err(),
            "a duplicate pull was delivered twice"
        );
    }

    /// With a cache behind the hub, what is pulled lands in the cache and what
    /// this device publishes is kept; both wake live subscriptions.
    #[tokio::test]
    async fn pulls_are_cached_and_own_events_kept() {
        let kept = Arc::new(RelayStore::in_memory());
        let cache = Arc::new(RelayStore::in_memory());
        let hub = RelayHub::with_cache(kept.clone(), cache.clone());
        let mut live = hub.live_events();

        let keys = Keys::generate();
        let pulled = chat_event(&keys, "mesh", "from a peer's backlog");
        let own = chat_event(&keys, "mesh", "mine");
        assert!(hub.accept_pulled(pulled.clone()).await.unwrap());
        assert_eq!(live.recv().await.unwrap().id, pulled.id);
        assert!(hub.accept_unforwarded(own.clone()).await.unwrap());
        assert_eq!(live.recv().await.unwrap().id, own.id);

        let ids = |events: Vec<Event>| events.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(kept.query(&[Filter::new()]).await.unwrap()), [own.id]);
        assert_eq!(
            ids(cache.query(&[Filter::new()]).await.unwrap()),
            [pulled.id]
        );
    }
}
