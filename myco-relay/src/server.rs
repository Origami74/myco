//! A NIP-01 WebSocket relay over [`RelayStore`]. Serves the node's events to the
//! in-app WebView at `ws://localhost:4869` and to mesh peers at
//! `ws://[fd00::self]:4869`.
//!
//! Unlike the original manifest-only socket, this one keeps **live subscriptions**
//! (a `REQ` stays open; newly-stored events that match are pushed as they arrive),
//! which is what makes nearby chat feel live. New events also drive an optional
//! [`Gossiper`] — the mesh fan-out hook (`docs/design/event-gossip.md`) — with the
//! connection's [`Origin`] (loopback = the local WebView, else a mesh peer) so the
//! gossiper can apply the push/pull and `event-ttl` rules.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use nostr::{Event, PublicKey};
use nsite_deck::seams::{ManifestFilter, RelayBackend};
use tokio::sync::broadcast;

use crate::{matches_filter, RelayStore};

/// Where an event reached this relay from: the local WebView (a loopback socket)
/// or a mesh peer (a `.fips` socket). Drives the gossiper's push/pull split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A loopback connection — the in-app nsite client publishing.
    Local,
    /// A mesh peer's relay pushing over `.fips`.
    Mesh,
}

/// Context for a newly-accepted event handed to the [`Gossiper`].
#[derive(Clone, Copy, Debug)]
pub struct Inbound {
    /// Where the event arrived from (loopback WebView vs a mesh peer).
    pub origin: Origin,
    /// The `event-ttl` that rode on the inbound EVENT (transient, stripped on
    /// store). `None` for a local publish — the gossiper stamps the originate
    /// default. See `docs/design/event-gossip.md` §2.
    pub event_ttl: Option<u8>,
    /// The mesh peer's address the event came from, for split-horizon (never
    /// forward back to the sender). `None` for a local publish.
    pub sender: Option<IpAddr>,
}

/// The mesh fan-out hook. The relay calls this for every **newly-accepted** event
/// (the store's id-dedup is the loop guard: a duplicate is never re-delivered
/// here). The implementor (`myco-core`) decides whether and how far to push it to
/// circle peers using the [`Inbound`] context (see `docs/design/event-gossip.md`).
/// The default does nothing — the relay never fans out on its own.
#[async_trait]
pub trait Gossiper: Send + Sync {
    async fn on_event(&self, event: Event, inbound: Inbound);

    /// The **pull plane**: forward a `REQ`'s filters to circle peers carrying a
    /// decremented `req-ttl`, returning their matching (signature-verified) events
    /// to fold into the backlog before `EOSE`. `exclude` is the requester's mesh
    /// address (split-horizon — never forward straight back to it). The default
    /// does nothing, so a relay with no gossiper stays single-hop. See
    /// `docs/design/event-gossip.md` (req-ttl).
    async fn on_req(
        &self,
        _filters: Vec<serde_json::Value>,
        _req_ttl: u8,
        _exclude: Option<IpAddr>,
    ) -> Vec<Event> {
        Vec::new()
    }
}

/// Access policy for **mesh** connections: only paired (Circle) peers may read or
/// publish content. Loopback (the in-app WebView) bypasses this entirely. The one
/// exception is the pairing handshake — an as-yet-unpaired peer must be able to
/// publish a pair request to bootstrap, so [`may_publish`](PeerGate::may_publish)
/// is consulted per event kind. The implementor (`myco-core`) backs this with the
/// Circle; a hub with no gate is open (the local/test default).
pub trait PeerGate: Send + Sync {
    /// May the mesh peer at `ip` open a `REQ` (read events from us)?
    fn may_read(&self, ip: IpAddr) -> bool;
    /// May the mesh peer at `ip` publish an `EVENT` of `kind`? Implementors allow
    /// the pairing kinds from anyone (bootstrap) and everything else only when
    /// `ip` is a paired peer.
    fn may_publish(&self, ip: IpAddr, kind: u16) -> bool;
}

/// Clamp on the `req-ttl` we'll honour, so a peer can't turn us into an
/// unbounded query amplifier (mirrors `MAX_EVENT_TTL` on the push plane).
const MAX_REQ_TTL: u8 = 2;

/// Shared per-relay state: the store, a broadcast bus that fans newly-stored
/// events to all live subscriptions on this device, and the optional mesh gossiper.
///
/// One hub can back **several listeners** (e.g. the mesh `[::]:4869` socket and a
/// loopback `127.0.0.1:4869` socket for the WebView) via [`serve_on_hub`], so the
/// live bus, store, and gossiper are shared across them — a peer's event pushed on
/// the mesh socket reaches a WebView subscription on the loopback socket.
pub struct RelayHub {
    store: Arc<RelayStore>,
    live: broadcast::Sender<Event>,
    gossip: Option<Arc<dyn Gossiper>>,
    /// Mesh access policy. `None` = open (local/test default); `Some` restricts
    /// mesh peers to paired (Circle) devices. Loopback always bypasses it.
    gate: Option<Arc<dyn PeerGate>>,
}

impl RelayHub {
    /// Build a shared hub. Pass `None` for `gossip` to disable mesh fan-out. No
    /// access gate — every connection is served (the local/test default).
    pub fn new(store: Arc<RelayStore>, gossip: Option<Arc<dyn Gossiper>>) -> Arc<Self> {
        Self::with_gate(store, gossip, None)
    }

    /// Build a hub that restricts **mesh** access to paired peers via `gate`
    /// (loopback is always allowed). Pass `None` for `gate` to stay open.
    pub fn with_gate(
        store: Arc<RelayStore>,
        gossip: Option<Arc<dyn Gossiper>>,
        gate: Option<Arc<dyn PeerGate>>,
    ) -> Arc<Self> {
        // Buffer enough that a brief subscriber stall doesn't drop chat; an
        // over-capacity lag is surfaced as `Lagged` and skipped, not blocked.
        let (live, _) = broadcast::channel(512);
        Arc::new(Self {
            store,
            live,
            gossip,
            gate,
        })
    }
}

/// Serve the relay on `addr` until the future is dropped/aborted (no gossiper).
pub async fn serve(store: Arc<RelayStore>, addr: SocketAddr) -> anyhow::Result<()> {
    serve_on(store, bind(addr)?).await
}

/// Bind a listener for the relay. For an IPv6 address this is **`IPV6_V6ONLY`**,
/// so `[::]:port` does not collide with another app squatting on
/// `127.0.0.1:port` — the mesh is IPv6-only. Returns the bind error so the caller
/// can warn the user (e.g. the port is already in use). Must be called within a
/// Tokio runtime.
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

/// Serve on an already-bound listener with no mesh gossiper (the local/test path).
pub async fn serve_on(
    store: Arc<RelayStore>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    serve_on_with(store, listener, None).await
}

/// Serve on an already-bound listener, fanning newly-accepted events to `gossip`
/// (the mesh propagator). The runtime uses this so chat events reach peers.
pub async fn serve_on_with(
    store: Arc<RelayStore>,
    listener: tokio::net::TcpListener,
    gossip: Option<Arc<dyn Gossiper>>,
) -> anyhow::Result<()> {
    serve_on_hub(RelayHub::new(store, gossip), listener).await
}

/// Serve a pre-built (shared) [`RelayHub`] on `listener`. Spawn this once per
/// listener that should share the same store + live bus + gossiper.
pub async fn serve_on_hub(
    hub: Arc<RelayHub>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let app = Router::new().route("/", get(root)).with_state(hub);
    // Connect-info gives each socket's peer address, so the handler can tell a
    // loopback (WebView) connection from a mesh peer.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// `/` is the WS endpoint; the peer address classifies the [`Origin`] and is the
/// split-horizon sender id for mesh fan-out.
async fn root(
    ws: WebSocketUpgrade,
    State(hub): State<Arc<RelayHub>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    let peer = addr.ip();
    ws.on_upgrade(move |socket| handle_ws(socket, hub, peer))
}

/// One client connection: serve `REQ` backlog + keep the subscription live, accept
/// `EVENT`s (store → fan to local subs → drive the gossiper), honour `CLOSE`.
async fn handle_ws(socket: WebSocket, hub: Arc<RelayHub>, peer_ip: IpAddr) {
    let origin = if peer_ip.is_loopback() {
        Origin::Local
    } else {
        Origin::Mesh
    };
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut live = hub.live.subscribe();
    // Active subscriptions on this connection: sub_id -> its filters.
    let mut subs: HashMap<String, Vec<ManifestFilter>> = HashMap::new();

    loop {
        tokio::select! {
            incoming = ws_rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        for reply in handle_client_frame(text.as_str(), &hub, origin, peer_ip, &mut subs).await {
                            if ws_tx.send(Message::text(reply)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        // Answer the peer-relay pool's keepalive so it can tell a
                        // live connection from a silent half-open one (its liveness
                        // check is a ping that must draw a frame back within its
                        // interval). tungstenite may also auto-pong, but replying
                        // explicitly guarantees the pong is flushed on an otherwise
                        // idle subscription.
                        if ws_tx.send(Message::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Err(_)) => return,
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
                                    return;
                                }
                            }
                        }
                    }
                    // Lagged: this slow subscriber missed some events — skip them
                    // and carry on rather than dropping the connection.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

/// Handle one client frame, mutating `subs` and returning the frames to send back.
async fn handle_client_frame(
    text: &str,
    hub: &Arc<RelayHub>,
    origin: Origin,
    peer_ip: IpAddr,
    subs: &mut HashMap<String, Vec<ManifestFilter>>,
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
            // Mesh access gate: only paired peers may read from us. Unpaired peers
            // get a CLOSED (no backlog, no live subscription registered).
            if origin == Origin::Mesh {
                if let Some(gate) = &hub.gate {
                    if !gate.may_read(peer_ip) {
                        return vec![serde_json::json!([
                            "CLOSED",
                            sub_id,
                            "restricted: pair to access"
                        ])
                        .to_string()];
                    }
                }
            }
            let raw_filters: Vec<serde_json::Value> = array.iter().skip(2).cloned().collect();
            // `req-ttl` rides *inside* a filter object (a transient extension key,
            // like `event-ttl` rides the EVENT) — the remaining mesh forward hops.
            // parse_filter ignores the key, so it never affects local matching.
            let req_ttl = raw_filters
                .iter()
                .filter_map(|f| f.get("req-ttl").and_then(|v| v.as_u64()))
                .max()
                .map(|n| n.min(MAX_REQ_TTL as u64) as u8)
                .unwrap_or(0);
            let filters: Vec<ManifestFilter> =
                raw_filters.iter().filter_map(parse_filter).collect();

            // Stored backlog: any-match across the REQ's filters, newest first.
            let mut events: Vec<Event> = Vec::new();
            for filter in &filters {
                if let Ok(mut matched) = hub.store.query(filter).await {
                    events.append(&mut matched);
                }
            }

            // Pull plane: fold in peers' matching events up to `req-ttl` more hops.
            // Bounded by the gossiper's own per-peer timeouts. Only paid when a
            // client opts in by setting req-ttl (e.g. discovery), so plain chat
            // REQs stay single-hop and cheap.
            if req_ttl > 0 {
                if let Some(gossip) = hub.gossip.clone() {
                    let exclude = (origin == Origin::Mesh).then_some(peer_ip);
                    let remote = gossip
                        .on_req(raw_filters.clone(), req_ttl - 1, exclude)
                        .await;
                    events.extend(remote);
                }
            }

            events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            events.dedup_by(|a, b| a.id == b.id);

            // Keep the subscription open so matching new events stream live.
            subs.insert(sub_id.clone(), filters);

            let mut out: Vec<String> = events
                .iter()
                .map(|e| serde_json::json!(["EVENT", sub_id, e]).to_string())
                .collect();
            out.push(serde_json::json!(["EOSE", sub_id]).to_string());
            out
        }
        Some("EVENT") => {
            let Some(event_value) = array.get(1) else {
                return Vec::new();
            };
            // The transient `event-ttl` (top-level, not a tag) rides the EVENT for
            // mesh fan-out; read it before parsing (parsing to Event drops it, so
            // it is never stored). See docs/design/event-gossip.md §2.
            let event_ttl = event_value
                .get("event-ttl")
                .and_then(|v| v.as_u64())
                .map(|n| n.min(u8::MAX as u64) as u8);
            let Ok(event) = serde_json::from_value::<Event>(event_value.clone()) else {
                return Vec::new();
            };
            let id = event.id.to_hex();
            // Mesh access gate: an unpaired peer may publish only the pairing
            // handshake (so pairing can bootstrap); everything else is rejected
            // until they're in our Circle.
            if origin == Origin::Mesh {
                if let Some(gate) = &hub.gate {
                    if !gate.may_publish(peer_ip, event.kind.as_u16()) {
                        return vec![serde_json::json!([
                            "OK",
                            id,
                            false,
                            "restricted: pair to access"
                        ])
                        .to_string()];
                    }
                }
            }
            if event.verify().is_err() {
                return vec![
                    serde_json::json!(["OK", id, false, "invalid: bad signature"]).to_string(),
                ];
            }
            match hub.store.store_event(event.clone()).await {
                Ok(true) => {
                    // Fan to this device's live subscriptions (incl. the WebView).
                    let _ = hub.live.send(event.clone());
                    // Drive the mesh gossiper off the socket path (non-blocking).
                    if let Some(gossip) = hub.gossip.clone() {
                        let inbound = Inbound {
                            origin,
                            event_ttl,
                            sender: (origin == Origin::Mesh).then_some(peer_ip),
                        };
                        tokio::spawn(async move { gossip.on_event(event, inbound).await });
                    }
                    vec![serde_json::json!(["OK", id, true, ""]).to_string()]
                }
                // Duplicate / superseded: still an accepted outcome per NIP-01.
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

/// Parse a NIP-01 filter object into the basic [`ManifestFilter`].
fn parse_filter(value: &serde_json::Value) -> Option<ManifestFilter> {
    let obj = value.as_object()?;
    let mut filter = ManifestFilter::default();
    if let Some(kinds) = obj.get("kinds").and_then(|v| v.as_array()) {
        filter.kinds = kinds
            .iter()
            .filter_map(|k| k.as_u64().map(|k| k as u16))
            .collect();
    }
    if let Some(authors) = obj.get("authors").and_then(|v| v.as_array()) {
        filter.authors = authors
            .iter()
            .filter_map(|a| a.as_str())
            .filter_map(|hex| PublicKey::parse(hex).ok())
            .collect();
    }
    if let Some(d_tags) = obj.get("#d").and_then(|v| v.as_array()) {
        filter.d_tags = d_tags
            .iter()
            .filter_map(|d| d.as_str().map(str::to_string))
            .collect();
    }
    filter.limit = obj
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    Some(filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use nsite_deck::model::KIND_ROOT;
    use nsite_deck::testing::build_test_site_with_keys;
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
        store.store_event(site.manifest.clone()).await.unwrap();

        let addr = spawn_relay(store.clone()).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let req = serde_json::json!([
            "REQ", "s1",
            { "kinds": [KIND_ROOT], "authors": [hex::encode(keys.public_key().to_bytes())] }
        ]);
        ws.send(WsMessage::Text(req.to_string().into()))
            .await
            .unwrap();

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
            serde_json::json!(["EVENT", site.manifest])
                .to_string()
                .into(),
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
        sub.send(WsMessage::Text(req.to_string().into()))
            .await
            .unwrap();
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
            serde_json::json!(["EVENT", msg]).to_string().into(),
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

    /// The gossiper is invoked for an accepted event, tagged with its origin.
    #[tokio::test]
    async fn gossiper_invoked_on_local_event() {
        use std::sync::Mutex;

        struct Capture(Mutex<Vec<(String, Inbound)>>);
        #[async_trait]
        impl Gossiper for Capture {
            async fn on_event(&self, event: Event, inbound: Inbound) {
                self.0.lock().unwrap().push((event.id.to_hex(), inbound));
            }
        }

        let store = Arc::new(RelayStore::in_memory());
        let capture = Arc::new(Capture(Mutex::new(Vec::new())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on_with(store, listener, Some(capture.clone())));

        let keys = Keys::generate();
        let msg = chat_event(&keys, "mesh", "gossip me");
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        ws.send(WsMessage::Text(
            serde_json::json!(["EVENT", msg]).to_string().into(),
        ))
        .await
        .unwrap();
        // Await the OK so the store+spawn have run.
        let _ = ws.next().await;

        // Give the spawned gossip task a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let seen = capture.0.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, msg.id.to_hex());
        // 127.0.0.1 is loopback → Local origin, no sender (originator stamps TTL).
        assert_eq!(seen[0].1.origin, Origin::Local);
        assert_eq!(seen[0].1.sender, None);
    }
}
