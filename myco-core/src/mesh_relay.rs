//! The **mesh relay proxy**: the NIP-01 WebSocket front door serving the in-app
//! WebView at `ws://localhost:4870` and mesh peers at `ws://[fd00::self]:4870`,
//! with a relay store behind it.
//!
//! This is the only Myco-specific code on the content path. It keeps **live
//! subscriptions** (a `REQ` stays open; newly-stored events that match are pushed
//! as they arrive), which is what makes nearby chat feel live, and it drives both
//! mesh planes: a [`Gossiper`] for fan-out (`docs/design/event-gossip.md`) and a
//! [`PeerGate`] for access, each keyed off the connection's [`Origin`] (loopback =
//! the local WebView, else a mesh peer).
//!
//! It lives here rather than in `myco-relay` so the store behind it stays a plain
//! NIP-01 relay with no Myco concepts in it, and can be swapped for any other
//! relay later. This is step P1 of `reference/thinning-custom-relay.md`. A proxy
//! built with no gossiper and no gate behaves as an ordinary NIP-01 relay, which
//! is what the tests use.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use nostr::{Event, PublicKey};
use nsite_deck::seams::{ManifestFilter, RelayBackend};
use tokio::sync::broadcast;

use myco_relay::{matches_filter, RelayStore};

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
    /// The hop budget that rode in the `MESH` envelope. `None` for a local
    /// publish — the gossiper stamps the originate default. The event itself is
    /// canonical NIP-01, so there is nothing to strip before storing.
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

    /// The **pull plane**, forwarding half: a mesh peer asked us with hops left,
    /// so pass its filters to our own circle peers carrying a decremented
    /// hop budget and return their matching events to fold into the backlog before
    /// `EOSE`. `exclude` is the requester's mesh address (split-horizon — never
    /// forward straight back to it).
    ///
    /// Only ever called for a **mesh-origin** `REQ`. A loopback client cannot
    /// reach this, so its `EOSE` never waits on a peer; the core drives multi-hop
    /// pull itself, through the peer pool. The default does nothing, so a relay
    /// with no gossiper stays single-hop. See `docs/design/event-gossip.md`
    /// and `reference/thinning-custom-relay.md` (D8).
    async fn on_req(
        &self,
        _filters: Vec<serde_json::Value>,
        _meta: crate::mesh_wire::MeshMeta,
        _exclude: Option<IpAddr>,
    ) -> Vec<Event> {
        Vec::new()
    }

    /// A **local** (loopback / in-app) client opened a `REQ`. The implementor
    /// records the raw `filters` so it can *recreate* this subscription against a
    /// Circle peer that (re)appears on the mesh — pulling that peer's matching
    /// backlog into the store so the client sees what it missed. The relay passes
    /// the filters verbatim; the core never interprets them (it has no notion of
    /// which kinds a given nsite cares about). `key` is unique per open
    /// subscription for the lifetime of the connection. Mesh-origin `REQ`s are
    /// never reported here — only local clients' own interests. Default: no-op.
    fn on_local_subscribe(&self, _key: &str, _filters: Vec<serde_json::Value>) {}

    /// A local client's subscription `key` closed (explicit `CLOSE`, or the
    /// connection dropped). Default: no-op.
    fn on_local_unsubscribe(&self, _key: &str) {}
}

/// Access policy for **mesh** connections: only paired (Circle) peers reach the
/// content plane at all, and what an admitted peer may do comes from its own
/// permission record. Loopback (the in-app WebView) bypasses this entirely.
///
/// There are no exceptions. Pairing used to need one, but the handshake now has
/// its own service (`crate::auth_service`), so nothing unpaired gets a socket.
/// The implementor (`myco-core`) backs this with the Circle; a hub with no gate
/// is open (the local/test default).
pub trait PeerGate: Send + Sync {
    /// May the mesh peer at `ip` open a connection at all?
    ///
    /// Checked **before the WebSocket upgrade**, so an unpaired peer is refused
    /// at the door rather than handed a socket and then told "no" per frame. That
    /// is cheaper over BLE — a stranger no longer costs an upgrade and a round of
    /// frames — and it means the content plane has one admission rule with no
    /// exceptions (`reference/thinning-custom-relay.md`, D6).
    ///
    /// Membership only. What an admitted peer may then do is [`may_read`] and
    /// [`may_publish`], from its own permission record.
    ///
    /// [`may_read`]: Self::may_read
    /// [`may_publish`]: Self::may_publish
    fn may_connect(&self, ip: IpAddr) -> bool;

    /// May the mesh peer at `ip` open a `REQ` (read events from us)?
    fn may_read(&self, ip: IpAddr) -> bool;
    /// May the mesh peer at `ip` publish an `EVENT` of `kind`? Implementors
    /// refuse the pairing kinds outright — those belong to the auth plane — and
    /// allow the rest per the peer's write grant.
    fn may_publish(&self, ip: IpAddr, kind: u16) -> bool;

    /// How far a `REQ` from `ip` may be forwarded on. `0` means answer from our
    /// own store only. Per-peer, so revoking multihop reads is a clamp on the
    /// budget the pull plane already carries rather than a separate branch
    /// (`reference/thinning-custom-relay.md`, D10).
    ///
    /// The push-side counterpart lives in the gossiper, which holds the same
    /// permission record and applies it where the hop budget is computed.
    fn max_req_ttl(&self, _ip: IpAddr) -> u8 {
        MAX_REQ_TTL
    }
}

/// Default clamp on the pull hop budget we'll honour, so a peer can't turn us
/// into an unbounded query amplifier (mirrors `MAX_EVENT_TTL` on the push plane).
/// A gate may lower it per peer — see [`PeerGate::max_req_ttl`].
pub(crate) const MAX_REQ_TTL: u8 = 2;

/// How long a non-expiring event's id is remembered as seen. A push wave lives
/// for seconds, so this only has to outlast retries and a slow multi-hop path.
const SEEN_RETENTION_SECS: u64 = 30 * 60;
/// Floor for an event that carries a NIP-40 expiry in the past or very near
/// future — remember it briefly regardless, so a wave in flight still terminates.
const SEEN_FLOOR_SECS: u64 = 60;
/// Cap on remembered ids. At ~40 bytes an entry this is a few hundred KB worst
/// case, and chat (the high-rate kind) expires itself out well before the cap.
const SEEN_CAPACITY: usize = 4096;

/// The ids this proxy has already handled, and therefore will not fan out again.
///
/// This is the mesh loop-guard, and it deliberately does **not** live in the
/// store. Storage answers "do I hold this?", which is a different question: an
/// id GC'd at NIP-40 expiry looks new again on the next pull, and a
/// store-triggered fan-out would start a fresh wave for an old message every
/// time someone new comes into range. Novelty is a property of this node's
/// history, so this node keeps it. See `reference/thinning-custom-relay.md` (D2)
/// and `docs/design/event-gossip.md` §4.
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
    /// it — the caller's signal to fan out.
    fn insert(&self, event: &Event) -> bool {
        let now = crate::content::now_secs();
        // Remember an expiring event until it expires everywhere; a manifest (no
        // expiry) for a fixed window. Either way at least the floor, so an event
        // that arrives already stale cannot be re-forwarded on every hop.
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

/// Query ids this node has already served.
///
/// Without it a pull multiplies: a circle is a graph, so the same query arrives
/// by several paths and each arrival re-fans it to every peer. Event-id dedup at
/// merge hides that in the *results* while the cost has already been paid — at
/// ttl 2 across a circle of ten, on the order of a hundred requests over BLE for
/// one query. It is also the amplification bound.
///
/// Bounded and FIFO-evicted. Queries are short-lived, so there is no expiry to
/// track beyond capacity — an id that falls out the back is one whose wave ended
/// long ago. See `reference/thinning-custom-relay.md` (D8).
#[derive(Default)]
struct SeenQueries {
    inner: Mutex<(std::collections::HashSet<String>, VecDeque<String>)>,
}

/// Cap on remembered query ids. A pull is rare next to an event, so this is
/// generous relative to the traffic.
const SEEN_QUERIES_CAPACITY: usize = 512;

impl SeenQueries {
    /// Record a query id, returning `true` if this node had not served it yet.
    fn insert_query(&self, qid: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let (set, order) = &mut *inner;
        if set.contains(qid) {
            return false;
        }
        while order.len() >= SEEN_QUERIES_CAPACITY {
            if let Some(oldest) = order.pop_front() {
                set.remove(&oldest);
            }
        }
        set.insert(qid.to_string());
        order.push_back(qid.to_string());
        true
    }
}

/// Shared per-relay state: the store, a broadcast bus that fans newly-stored
/// events to all live subscriptions on this device, and the optional mesh gossiper.
///
/// One hub can back **several listeners** (e.g. the mesh `[::]:4870` socket and a
/// loopback `127.0.0.1:4870` socket for the WebView) via [`serve_on_hub`], so the
/// live bus, store, and gossiper are shared across them — a peer's event pushed on
/// the mesh socket reaches a WebView subscription on the loopback socket.
pub struct RelayHub {
    store: Arc<RelayStore>,
    live: broadcast::Sender<Event>,
    gossip: Option<Arc<dyn Gossiper>>,
    /// Mesh access policy. `None` = open (local/test default); `Some` restricts
    /// mesh peers to paired (Circle) devices. Loopback always bypasses it.
    gate: Option<Arc<dyn PeerGate>>,
    /// Ids already handled here, so a copy arriving by a second path is stored
    /// but not re-flooded. Shared across every listener on this hub.
    seen: SeenSet,
    /// Query ids already served, so a pull that reaches us by several paths is
    /// answered once instead of re-fanned each time.
    seen_queries: SeenQueries,
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
            seen: SeenSet::default(),
            seen_queries: SeenQueries::default(),
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
    let app = Router::new()
        .route("/", get(root))
        .layer(axum::middleware::from_fn_with_state(hub.clone(), admit))
        .with_state(hub);
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

/// Admission, applied as a layer so it runs **before** the WebSocket upgrade is
/// even parsed: an unpaired mesh peer gets a plain 403 and no socket.
///
/// A stranger costs us a TCP accept and one small response, rather than an
/// upgrade plus a round of frames it was never allowed to send — which matters
/// on a BLE link. Loopback (the in-app WebView) always bypasses.
async fn admit(
    State(hub): State<Arc<RelayHub>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let peer = addr.ip();
    if !peer.is_loopback() {
        if let Some(gate) = &hub.gate {
            if !gate.may_connect(peer) {
                tracing::debug!(peer = %peer, "relay: refused an unpaired connection");
                return (
                    axum::http::StatusCode::FORBIDDEN,
                    "restricted: pair to access",
                )
                    .into_response();
            }
        }
    }
    next.run(req).await
}

/// Has this connection's peer lost access since it was admitted?
///
/// Admission is checked once, at the upgrade, so without re-checking here a
/// subscription opened while paired would keep streaming after the peer was
/// removed from the circle. Revocation has to reach connections that already
/// exist, not just the next one (`reference/thinning-custom-relay.md`, D6).
/// Loopback is the in-app WebView and is never gated.
fn revoked(hub: &RelayHub, origin: Origin, peer_ip: IpAddr) -> bool {
    origin == Origin::Mesh && hub.gate.as_ref().is_some_and(|g| !g.may_connect(peer_ip))
}

/// One client connection: serve `REQ` backlog + keep the subscription live, accept
/// `EVENT`s (store → fan to local subs → drive the gossiper), honour `CLOSE`.
async fn handle_ws(socket: WebSocket, hub: Arc<RelayHub>, peer_ip: IpAddr) {
    let origin = if peer_ip.is_loopback() {
        Origin::Local
    } else {
        Origin::Mesh
    };
    // A per-connection id so a local client's `REQ` sub_ids are globally unique in
    // the core's active-subscription registry (two connections can both use "s1").
    let conn_id = NEXT_CONN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut live = hub.live.subscribe();
    // Active subscriptions on this connection: sub_id -> its filters.
    let mut subs: HashMap<String, Vec<ManifestFilter>> = HashMap::new();

    'conn: loop {
        tokio::select! {
            incoming = ws_rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        for reply in handle_client_frame(text.as_str(), &hub, origin, peer_ip, conn_id, &mut subs).await {
                            if ws_tx.send(Message::text(reply)).await.is_err() {
                                break 'conn;
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
                        // Re-check membership before feeding an open subscription.
                        // Admission happens once, at the upgrade, so without this a
                        // subscription opened while paired would keep streaming
                        // after the peer was removed — revocation has to reach
                        // connections that already exist, not just the next one.
                        if revoked(&hub, origin, peer_ip) {
                            tracing::info!(peer = %peer_ip, "relay: dropping a revoked peer's connection");
                            break 'conn;
                        }
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
    finish_conn(&hub, origin, conn_id, &subs);
}

/// Tear down a connection's local subscriptions in the core's registry (so a
/// dropped in-app client stops being replayed to reappearing peers).
fn finish_conn(
    hub: &Arc<RelayHub>,
    origin: Origin,
    conn_id: u64,
    subs: &HashMap<String, Vec<ManifestFilter>>,
) {
    if origin != Origin::Local {
        return;
    }
    if let Some(gossip) = &hub.gossip {
        for sub_id in subs.keys() {
            gossip.on_local_unsubscribe(&sub_key(conn_id, sub_id));
        }
    }
}

/// The registry key for one local subscription: unique across connections.
fn sub_key(conn_id: u64, sub_id: &str) -> String {
    format!("{conn_id}:{sub_id}")
}

/// Monotonic per-connection id source (see [`handle_ws`]).
static NEXT_CONN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Handle one client frame, mutating `subs` and returning the frames to send back.
async fn handle_client_frame(
    text: &str,
    hub: &Arc<RelayHub>,
    origin: Origin,
    peer_ip: IpAddr,
    conn_id: u64,
    subs: &mut HashMap<String, Vec<ManifestFilter>>,
) -> Vec<String> {
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };

    // Unwrap a MESH envelope, if this is one: mesh state travels beside the NIP-01
    // message, never inside it, and what comes out is passed on untouched. A plain
    // frame is equally valid here and carries no metadata — store it, do not
    // forward it. See `reference/thinning-custom-relay.md` (D1).
    let (meta, value) = match crate::mesh_wire::unwrap(&frame) {
        // Boundary 1: the in-app client is an ordinary Nostr client and has no
        // business speaking our framing. Refusing it here is what stops an nsite
        // reaching the mesh planes through a hand-built frame.
        Some(_) if origin == Origin::Local => {
            tracing::debug!("relay: refusing a MESH frame from a local client");
            return vec![serde_json::json!([
                "NOTICE",
                "MESH framing is mesh-only; this relay speaks plain NIP-01 here"
            ])
            .to_string()];
        }
        Some((meta, inner)) => (meta, inner),
        None => (crate::mesh_wire::MeshMeta::default(), frame),
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
            // The forward budget comes from the envelope, clamped to what this
            // peer is granted. A loopback client cannot send an envelope at all,
            // so an nsite's REQ is always single-hop: one filter key must never
            // change a query's cost by orders of magnitude. Multi-hop pull is a
            // core-driven operation (discovery, update checks) that goes through
            // the peer pool directly. See `reference/thinning-custom-relay.md` (D8).
            let cap = hub
                .gate
                .as_ref()
                .map_or(MAX_REQ_TTL, |g| g.max_req_ttl(peer_ip));
            let hop = meta.clone().clamped(cap.min(MAX_REQ_TTL));
            // A circle is a graph, not a tree, so the same query reaches us by
            // several paths. Serve each query id once: answer from our own store
            // as usual, but do not fan it out again.
            let repeat = hop
                .qid
                .as_deref()
                .is_some_and(|qid| !hub.seen_queries.insert_query(qid));
            let filters: Vec<ManifestFilter> =
                raw_filters.iter().filter_map(parse_filter).collect();

            // Stored backlog: any-match across the REQ's filters, newest first.
            let mut events: Vec<Event> = Vec::new();
            for filter in &filters {
                if let Ok(mut matched) = hub.store.query(filter).await {
                    events.append(&mut matched);
                }
            }

            // Forwarding half of the pull plane: a peer asked with hops left, so
            // fold in our own peers' matching events before answering. Only a mesh
            // REQ reaches here, so a local client's `EOSE` is never held up by a
            // slow or unreachable peer — it arrives at local-store speed, and
            // anything a peer delivers later reaches the client through the live
            // subscription instead.
            if !repeat {
                if let Some(next) = hop.next_hop() {
                    if let Some(gossip) = hub.gossip.clone() {
                        // `next` carries the incoming query id onward, so every
                        // node downstream serves this query once.
                        let remote = gossip
                            .on_req(raw_filters.clone(), next, Some(peer_ip))
                            .await;
                        events.extend(remote);
                    }
                }
            }

            events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            events.dedup_by(|a, b| a.id == b.id);

            // Keep the subscription open so matching new events stream live.
            subs.insert(sub_id.clone(), filters);

            // Record a *local* client's interest so the core can recreate this
            // subscription against Circle peers that reappear on the mesh (pulling
            // their missed backlog). Mesh REQs are peers' interests, not ours, and
            // are never recorded. The filters are passed verbatim — the core does
            // not interpret them.
            if origin == Origin::Local {
                if let Some(gossip) = &hub.gossip {
                    gossip.on_local_subscribe(&sub_key(conn_id, &sub_id), raw_filters.clone());
                }
            }

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
            // The hop budget rides the envelope, so the event itself is canonical
            // NIP-01 and there is nothing to strip before storing it.
            let event_ttl = (origin == Origin::Mesh).then_some(meta.ttl);
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
            // Novelty is the proxy's own call, made *before* storing and
            // independent of what the store does with the event. Storing is
            // idempotent, so it happens either way; only a first sighting fans
            // out. See `reference/thinning-custom-relay.md` (D2).
            let first_sighting = hub.seen.insert(&event);
            if let Err(e) = hub.store.publish(event.clone()).await {
                return vec![
                    serde_json::json!(["OK", id, false, format!("error: {e}")]).to_string()
                ];
            }
            if first_sighting {
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
            } else {
                // Duplicate: still an accepted outcome per NIP-01.
                vec![serde_json::json!(["OK", id, true, "duplicate:"]).to_string()]
            }
        }
        Some("CLOSE") => {
            if let Some(sub_id) = array.get(1).and_then(|v| v.as_str()) {
                subs.remove(sub_id);
                if origin == Origin::Local {
                    if let Some(gossip) = &hub.gossip {
                        gossip.on_local_unsubscribe(&sub_key(conn_id, sub_id));
                    }
                }
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
            serde_json::json!(["EVENT", msg]).to_string(),
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

    /// An event the store no longer holds must still not be re-flooded.
    ///
    /// This is the failure the seen-set exists to prevent. When novelty came from
    /// the store, an id GC'd at NIP-40 expiry — or any backend that forgot it —
    /// read as new on the next pull, so a fresh wave went out for an old message
    /// every time someone came into range. Novelty belongs to this node's
    /// history, not to what storage currently holds
    /// (`reference/thinning-custom-relay.md` D2, `event-gossip.md` §4).
    #[tokio::test]
    async fn a_forgotten_event_is_not_re_flooded() {
        use std::sync::Mutex;

        struct Count(Mutex<usize>);
        #[async_trait]
        impl Gossiper for Count {
            async fn on_event(&self, _event: Event, _inbound: Inbound) {
                *self.0.lock().unwrap() += 1;
            }
        }

        let store = Arc::new(RelayStore::in_memory());
        let count = Arc::new(Count(Mutex::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on_with(store.clone(), listener, Some(count.clone())));

        let keys = Keys::generate();
        let msg = chat_event(&keys, "mesh", "old news");
        let frame = serde_json::json!(["EVENT", msg]).to_string();

        let publish = |frame: String| async move {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
                .await
                .unwrap();
            ws.send(WsMessage::Text(frame)).await.unwrap();
            let _ = ws.next().await; // await the OK
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };

        publish(frame.clone()).await;
        assert_eq!(*count.0.lock().unwrap(), 1, "first sighting fans out");

        // Empty the store, standing in for expiry GC or a backend that forgot.
        store.wipe().await.unwrap();
        assert_eq!(store.count(), 0);

        publish(frame).await;
        assert_eq!(
            *count.0.lock().unwrap(),
            1,
            "the store forgot it, but we have not: no second wave"
        );
        assert_eq!(
            store.count(),
            1,
            "still stored again — publishing is idempotent, only the fan-out is suppressed"
        );
    }

    /// A wave crosses hops, decrements, and dies — and a hostile budget is clamped.
    ///
    /// Walks A→B→C by feeding each node the frame the previous one emitted, which
    /// is the part that has to hold: the hop count has to survive the envelope, not
    /// the event, and the event has to arrive canonical at every hop.
    #[tokio::test]
    async fn a_push_wave_decrements_and_terminates() {
        use std::sync::Mutex;

        struct Capture(Mutex<Vec<Option<u8>>>);
        #[async_trait]
        impl Gossiper for Capture {
            async fn on_event(&self, _event: Event, inbound: Inbound) {
                self.0.lock().unwrap().push(inbound.event_ttl);
            }
        }

        // One node, fed a frame as if from a mesh peer; returns the ttl its
        // gossiper saw, which is what decides how much further the wave goes.
        async fn hop(frame: &str) -> Option<u8> {
            let cap = Arc::new(Capture(Mutex::new(Vec::new())));
            let hub = RelayHub::new(Arc::new(RelayStore::in_memory()), Some(cap.clone()));
            let mut subs = HashMap::new();
            let peer: IpAddr = "fd00::9".parse().unwrap();
            handle_client_frame(frame, &hub, Origin::Mesh, peer, 0, &mut subs).await;
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            let seen = cap.0.lock().unwrap().clone();
            assert_eq!(seen.len(), 1, "the event was accepted exactly once");
            seen[0]
        }

        let keys = Keys::generate();
        let msg = chat_event(&keys, "mesh", "ripple");
        let event = serde_json::to_value(&msg).unwrap();

        // A originates at ttl 2; B sees 2 and forwards 1; C sees 1 and forwards 0.
        let at_b = hop(&crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::push(2),
            serde_json::json!(["EVENT", event.clone()]),
        ))
        .await;
        assert_eq!(at_b, Some(2));

        let at_c = hop(&crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::push(1),
            serde_json::json!(["EVENT", event.clone()]),
        ))
        .await;
        assert_eq!(at_c, Some(1));

        // The last hop has nothing left to spend.
        let at_d = hop(&crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::push(0),
            serde_json::json!(["EVENT", event.clone()]),
        ))
        .await;
        assert_eq!(at_d, Some(0), "stored, but the wave stops here");

        // A plain frame from a peer carries no budget and is equally terminal.
        let plain = hop(&serde_json::json!(["EVENT", event]).to_string()).await;
        assert_eq!(plain, Some(0));
    }

    /// One query id is served once per node, however many paths it arrives by.
    ///
    /// A circle is a graph, so the same pull reaches a node from several peers.
    /// Without this the fan-out multiplies — at ttl 2 across a circle of ten, on
    /// the order of a hundred requests over BLE for a single query
    /// (`reference/thinning-custom-relay.md`, D8).
    #[tokio::test]
    async fn a_repeated_query_id_is_not_re_fanned() {
        use std::sync::Mutex;

        struct CountReq(Mutex<usize>);
        #[async_trait]
        impl Gossiper for CountReq {
            async fn on_event(&self, _event: Event, _inbound: Inbound) {}
            async fn on_req(
                &self,
                _filters: Vec<serde_json::Value>,
                _meta: crate::mesh_wire::MeshMeta,
                _exclude: Option<IpAddr>,
            ) -> Vec<Event> {
                *self.0.lock().unwrap() += 1;
                Vec::new()
            }
        }

        let counter = Arc::new(CountReq(Mutex::new(0)));
        let hub = RelayHub::new(Arc::new(RelayStore::in_memory()), Some(counter.clone()));
        let query = crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::pull(2, "q-loop", 10_000),
            serde_json::json!(["REQ", "s1", { "kinds": [9] }]),
        );

        // The same query arriving from three different peers, as it would in a
        // circle wired A–B, B–C, C–A.
        for (i, peer) in ["fd00::1", "fd00::2", "fd00::3"].iter().enumerate() {
            let mut subs = HashMap::new();
            let out = handle_client_frame(
                &query,
                &hub,
                Origin::Mesh,
                peer.parse().unwrap(),
                i as u64,
                &mut subs,
            )
            .await;
            assert!(
                out.iter().any(|f| f.contains("EOSE")),
                "every arrival is still answered from our own store"
            );
        }

        assert_eq!(
            *counter.0.lock().unwrap(),
            1,
            "the query is fanned out once, not once per path"
        );
    }

    /// The two boundaries, asserted rather than left as a convention.
    ///
    /// Everything in this plan rests on two rules: the nsite link speaks plain
    /// NIP-01, and so does the link to whatever relay sits behind us. If either
    /// ever carries a `MESH` verb or a ttl key, the relay has stopped being
    /// swappable and the design has quietly reversed itself. That is worth a test
    /// rather than a comment (`reference/thinning-custom-relay.md`).
    #[tokio::test]
    async fn no_mesh_framing_crosses_the_client_or_backend_links() {
        let store = Arc::new(RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(store.clone(), listener));

        let keys = Keys::generate();
        let msg = chat_event(&keys, "mesh", "hello");

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        ws.send(WsMessage::Text(
            serde_json::json!(["REQ", "s1", { "kinds": [9] }]).to_string(),
        ))
        .await
        .unwrap();
        ws.send(WsMessage::Text(
            serde_json::json!(["EVENT", msg]).to_string(),
        ))
        .await
        .unwrap();

        // Collect everything the client link carries back.
        let mut seen = Vec::new();
        for _ in 0..4 {
            match tokio::time::timeout(std::time::Duration::from_millis(300), ws.next()).await {
                Ok(Some(Ok(WsMessage::Text(txt)))) => seen.push(txt.to_string()),
                _ => break,
            }
        }
        assert!(!seen.is_empty(), "the client link carried something");
        for frame in &seen {
            assert!(
                !frame.contains(crate::mesh_wire::MESH)
                    && !frame.contains("event-ttl")
                    && !frame.contains("req-ttl"),
                "boundary 1 must stay plain NIP-01, got: {frame}"
            );
        }

        // The backend link: what actually landed in the store is a canonical
        // event, with no mesh state smuggled into it.
        let stored = store
            .query(&ManifestFilter {
                kinds: vec![9],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        let json = serde_json::to_string(&stored[0]).unwrap();
        assert!(
            !json.contains("event-ttl") && !json.contains(crate::mesh_wire::MESH),
            "boundary 2 must store a canonical NIP-01 event, got: {json}"
        );
        assert_eq!(stored[0].id, msg.id, "and it is the same event");
    }

    /// An unpaired peer never gets a socket at all.
    ///
    /// Refusing per frame would still hand a stranger a WebSocket upgrade and a
    /// round of frames, which over BLE is real cost for a connection that can
    /// never do anything. Admission is a membership check before the upgrade, so
    /// the handshake itself fails (`reference/thinning-custom-relay.md`, D6).
    #[tokio::test]
    async fn an_unpaired_peer_is_refused_before_the_upgrade() {
        struct NoOne;
        impl PeerGate for NoOne {
            fn may_connect(&self, _ip: IpAddr) -> bool {
                false
            }
            fn may_read(&self, _ip: IpAddr) -> bool {
                false
            }
            fn may_publish(&self, _ip: IpAddr, _kind: u16) -> bool {
                false
            }
        }

        use tower::ServiceExt;

        let store = Arc::new(RelayStore::in_memory());
        let hub = RelayHub::with_gate(store, None, Some(Arc::new(NoOne)));
        let app = Router::new()
            .route("/", get(root))
            .layer(axum::middleware::from_fn_with_state(hub.clone(), admit))
            .with_state(hub);

        // A real socket here could only ever be loopback, which bypasses the gate
        // by design, so drive the route with a synthetic mesh address instead.
        let mesh_peer: SocketAddr = "[fd00::1]:9999".parse().unwrap();
        let mut req = axum::http::Request::builder()
            .uri("/")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(mesh_peer));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::FORBIDDEN,
            "an unpaired peer must be refused at the door, not upgraded and then \
             told no per frame"
        );
    }

    /// Revocation reaches a connection that is already open.
    ///
    /// Admission is checked once, at the upgrade, so a peer removed from the
    /// circle mid-session would otherwise keep receiving events on a `REQ` it
    /// opened while still paired. `axum`'s `WebSocket` cannot be built from a raw
    /// socket, and a real socket here could only be loopback (which is exempt by
    /// design), so this covers the decision the live-event branch makes rather
    /// than the socket teardown around it
    /// (`reference/thinning-custom-relay.md`, D6).
    #[test]
    fn a_revoked_peer_stops_being_fed() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Revocable(Arc<AtomicBool>);
        impl PeerGate for Revocable {
            fn may_connect(&self, _ip: IpAddr) -> bool {
                self.0.load(Ordering::Relaxed)
            }
            fn may_read(&self, _ip: IpAddr) -> bool {
                true
            }
            fn may_publish(&self, _ip: IpAddr, _kind: u16) -> bool {
                true
            }
        }

        let paired = Arc::new(AtomicBool::new(true));
        let hub = RelayHub::with_gate(
            Arc::new(RelayStore::in_memory()),
            None,
            Some(Arc::new(Revocable(paired.clone()))),
        );
        let mesh_peer: IpAddr = "fd00::1".parse().unwrap();
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();

        assert!(
            !revoked(&hub, Origin::Mesh, mesh_peer),
            "paired peer is fed"
        );

        paired.store(false, Ordering::Relaxed);
        assert!(
            revoked(&hub, Origin::Mesh, mesh_peer),
            "unpairing must close a connection that is already open"
        );
        assert!(
            !revoked(&hub, Origin::Local, loopback),
            "the in-app WebView is never gated"
        );

        // A hub with no gate is open, which is what the tests and local runs use.
        let open = RelayHub::new(Arc::new(RelayStore::in_memory()), None);
        assert!(!revoked(&open, Origin::Mesh, mesh_peer));
    }

    /// An nsite cannot start a circle-wide pull, and its `EOSE` never waits on a
    /// peer.
    ///
    /// Two halves. A plain `REQ` — what every client actually sends — is answered
    /// from the local store without touching the fan-out. And a hand-built `MESH`
    /// frame, the only way a client could ask for hops, is refused outright:
    /// boundary 1 says the loopback socket speaks plain NIP-01, so mesh framing
    /// has no meaning there (`reference/thinning-custom-relay.md`, D1 and D8).
    ///
    /// The gossiper here hangs for a minute if consulted, so a prompt `EOSE` is
    /// the assertion.
    #[tokio::test]
    async fn a_client_cannot_reach_the_fan_out() {
        use std::sync::Mutex;

        struct Hang(Mutex<usize>);
        #[async_trait]
        impl Gossiper for Hang {
            async fn on_event(&self, _event: Event, _inbound: Inbound) {}
            async fn on_req(
                &self,
                _filters: Vec<serde_json::Value>,
                _meta: crate::mesh_wire::MeshMeta,
                _exclude: Option<IpAddr>,
            ) -> Vec<Event> {
                *self.0.lock().unwrap() += 1;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Vec::new()
            }
        }

        let store = Arc::new(RelayStore::in_memory());
        let hang = Arc::new(Hang(Mutex::new(0)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on_with(store, listener, Some(hang.clone())));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();

        // The ordinary client path: a plain REQ, answered locally.
        ws.send(WsMessage::Text(
            serde_json::json!(["REQ", "s1", { "kinds": [9] }]).to_string(),
        ))
        .await
        .unwrap();
        let eose = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(Ok(WsMessage::Text(txt))) = ws.next().await {
                let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
                if v.get(0).and_then(|x| x.as_str()) == Some("EOSE") {
                    return true;
                }
            }
            false
        })
        .await;
        assert_eq!(
            eose,
            Ok(true),
            "EOSE must arrive at local-store speed, not after a peer round trip"
        );

        // The only way to ask for hops is mesh framing, which does not belong on
        // this socket.
        ws.send(WsMessage::Text(crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::pull(2, "q1", 10_000),
            serde_json::json!(["REQ", "s2", { "kinds": [9] }]),
        )))
        .await
        .unwrap();
        let notice = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("a prompt answer")
            .expect("a frame")
            .expect("text");
        let WsMessage::Text(txt) = notice else {
            panic!("expected a NOTICE")
        };
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v.get(0).and_then(|x| x.as_str()), Some("NOTICE"));

        assert_eq!(
            *hang.0.lock().unwrap(),
            0,
            "a client must not reach the peer fan-out by either route"
        );
    }

    /// A loopback client's REQ is reported to the gossiper as a local subscription
    /// (raw filters passed verbatim), and its CLOSE as an unsubscribe — the seam the
    /// core uses to recreate subscriptions against reappearing peers, filter-blind.
    #[tokio::test]
    async fn local_req_reports_subscription_to_gossiper() {
        use std::sync::Mutex;

        struct SubCap {
            subs: Mutex<Vec<(String, Vec<serde_json::Value>)>>,
            unsubs: Mutex<Vec<String>>,
        }
        #[async_trait]
        impl Gossiper for SubCap {
            async fn on_event(&self, _event: Event, _inbound: Inbound) {}
            fn on_local_subscribe(&self, key: &str, filters: Vec<serde_json::Value>) {
                self.subs.lock().unwrap().push((key.to_string(), filters));
            }
            fn on_local_unsubscribe(&self, key: &str) {
                self.unsubs.lock().unwrap().push(key.to_string());
            }
        }

        let store = Arc::new(RelayStore::in_memory());
        let cap = Arc::new(SubCap {
            subs: Mutex::new(Vec::new()),
            unsubs: Mutex::new(Vec::new()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on_with(store, listener, Some(cap.clone())));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let filter = serde_json::json!({ "kinds": [9], "#d": ["mesh"] });
        ws.send(WsMessage::Text(
            serde_json::json!(["REQ", "s1", filter]).to_string(),
        ))
        .await
        .unwrap();
        // Drain to EOSE so the REQ has been handled.
        while let Some(Ok(WsMessage::Text(t))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap();
            if v[0] == "EOSE" {
                break;
            }
        }

        let subs = cap.subs.lock().unwrap().clone();
        assert_eq!(
            subs.len(),
            1,
            "loopback REQ reported as a local subscription"
        );
        assert_eq!(
            subs[0].1,
            vec![serde_json::json!({ "kinds": [9], "#d": ["mesh"] })],
            "raw filters passed through verbatim (core never interprets them)"
        );

        ws.send(WsMessage::Text(
            serde_json::json!(["CLOSE", "s1"]).to_string(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let unsubs = cap.unsubs.lock().unwrap().clone();
        assert_eq!(
            unsubs,
            vec![subs[0].0.clone()],
            "CLOSE reports the same key"
        );
    }
}
