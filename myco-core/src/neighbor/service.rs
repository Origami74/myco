//! The link service: the neighborhood protocol over HTTP, under `/nh` on the
//! auth plane (`:4873`).
//!
//! ```text
//! POST /nh/{pk}/setup          SetupRequest -> SetupResponse      (the one-time secret)
//! GET  /nh/{pk}/offer          -> our offer for this link         (NIP-98, link key)
//! POST /nh/{pk}/neighborhoods  {add, remove} -> {n, pending}      (NIP-98, link key)
//! POST /nh/{pk}/end            -> 204                             (NIP-98, link key)
//! POST /nh/nearby              NearbyRequest -> NearbyResponse    (NIP-98; direct peers only)
//! ```
//!
//! `/nh/nearby` sets up an ephemeral link with no secret, for a device
//! directly in reach; see [`super::nearby`].
//!
//! `{pk}` is this side's link public key for the link (or invitation) in
//! question, so each link has its own URL and the URL a proof or signature
//! names is exactly one of ours. A device can be reached by several bases
//! (its FIPS address; later a Tor or public one): a request is checked
//! against each.
//!
//! Every refusal says only "refused": a stranger learns nothing about which
//! check failed.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use super::id::NeighborhoodId;
use super::link::{
    check_http_auth, http_auth, Link, LinkOffer, Links, OfferContent, Service, SetupRequest,
    SetupResponse, Shared,
};

/// What the routes need.
#[derive(Clone)]
pub struct LinkService {
    pub links: Arc<Links>,
    /// The device's neighborhoods, once the napplet host has built them.
    /// Until then the link relay answers 503.
    pub neighbors: Arc<std::sync::OnceLock<Arc<super::Neighbors>>>,
    /// This device's bases, e.g. `http://<npub>.fips:4873`. The first is the
    /// one invitations name first.
    pub bases: Vec<String>,
    /// Nearby: who is in reach, and the name nearby offers carry.
    pub nearby: Arc<super::nearby::Nearby>,
}

impl LinkService {
    /// The link URL for one of our link keys, under each base.
    fn link_urls(&self, pk: &str) -> Vec<String> {
        self.bases.iter().map(|b| format!("{b}/nh/{pk}")).collect()
    }

    /// What we offer on a link. Every link gets our endpoints; what a peer may
    /// actually do is decided per request by the gates.
    pub fn offer(&self, pk: &str) -> OfferContent {
        let mut services: Vec<Service> = self
            .link_urls(pk)
            .into_iter()
            .map(|url| Service {
                kind: "link".into(),
                url,
                read: false,
                write: false,
                own: false,
            })
            .collect();
        // The link relay: this link's own door, scoped by `?n=`.
        services.extend(self.bases.iter().map(|b| Service {
            kind: "relay".into(),
            url: format!("{}/nh/{pk}/relay", b.replacen("http", "ws", 1)),
            read: true,
            write: true,
            own: false,
        }));
        OfferContent { services }
    }

    /// Start a link carrying `neighborhoods`, for a QR or link.
    pub fn invite(&self, neighborhoods: Vec<Shared>, ttl_secs: u64) -> LinkOffer {
        self.invite_from(neighborhoods, ttl_secs, None)
    }

    /// As [`LinkService::invite`], saying who it is from.
    pub fn invite_from(
        &self,
        neighborhoods: Vec<Shared>,
        ttl_secs: u64,
        from: Option<String>,
    ) -> LinkOffer {
        // The URLs depend on the new key, so invite with none and fill them in.
        let mut offer = self
            .links
            .invite_from(Vec::new(), neighborhoods, ttl_secs, from);
        offer.link = self.link_urls(&offer.pk);
        offer
    }

    /// The link a NIP-98 header proves, for one of our link keys and a
    /// request at `/nh/{pk}/{rest}` with `body`.
    fn authorized(
        &self,
        headers: &HeaderMap,
        method: &str,
        pk: &str,
        rest: &str,
        body: &[u8],
    ) -> Option<Link> {
        let header = headers.get("authorization")?.to_str().ok()?;
        let signer = self
            .link_urls(pk)
            .iter()
            .find_map(|url| check_http_auth(header, method, &format!("{url}/{rest}"), body))?;
        let link = self.links.by_peer(&signer).filter(|l| l.own_pk() == pk)?;
        self.links.mark_seen(&link.id);
        Some(link)
    }
}

pub fn routes(service: LinkService) -> Router {
    Router::new()
        .route("/nh/nearby", post(nearby))
        .route("/nh/{pk}/setup", post(setup))
        .route("/nh/{pk}/offer", get(offer))
        .route("/nh/{pk}/neighborhoods", post(neighborhoods))
        .route("/nh/{pk}/end", post(end))
        .route("/nh/{pk}/relay", get(relay))
        .route("/nh/{pk}/blob/{sha256}", get(blob).head(blob_head))
        .with_state(service)
}

fn refused() -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": "refused" })),
    )
        .into_response()
}

/// Marks a listener whose connections' source addresses are not vouched for
/// by FIPS (a kernel socket beside the mesh stack). See
/// `auth_service::serve`.
#[derive(Debug, Clone, Copy)]
pub struct SourceUntrusted;

/// `POST /nh/nearby`: an ephemeral link with a device directly in reach.
/// The source address is the check, so it is read from the connection
/// (present when served with connect info, as the auth plane is), and only
/// where FIPS vouches for it.
async fn nearby(State(svc): State<LinkService>, request: axum::extract::Request) -> Response {
    let source = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip())
        .filter(|_| request.extensions().get::<SourceUntrusted>().is_none());
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    let Ok(body) = axum::body::to_bytes(request.into_body(), 1 << 16).await else {
        return refused();
    };
    match super::nearby::answer(
        &svc,
        source,
        auth.as_deref(),
        &body,
        std::time::Instant::now(),
    ) {
        Ok(response) => axum::Json(response).into_response(),
        Err(super::nearby::Refusal::Used(proof)) => (
            axum::http::StatusCode::CONFLICT,
            axum::Json(super::nearby::UsedSecret {
                error: "used".into(),
                proof,
            }),
        )
            .into_response(),
        Err(super::nearby::Refusal::Refused) => refused(),
        Err(super::nearby::Refusal::Busy) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({ "error": "busy" })),
        )
            .into_response(),
    }
}

async fn setup(State(svc): State<LinkService>, Path(pk): Path<String>, body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<SetupRequest>(&body) else {
        return refused();
    };
    let ours = svc.offer(&pk);
    // The proof names the URL the joiner used: one of ours.
    for url in svc.link_urls(&pk) {
        match svc.links.accept(&pk, &url, &request, &ours, None) {
            Ok((link, response)) => {
                tracing::info!(link = %link.id, n = link.neighborhoods.len(), "link set up");
                return axum::Json(response).into_response();
            }
            Err(super::link::Refused::BadProof) => continue,
            Err(why) => {
                tracing::info!(?why, "link setup refused");
                return refused();
            }
        }
    }
    refused()
}

async fn offer(
    State(svc): State<LinkService>,
    Path(pk): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(link) = svc.authorized(&headers, "GET", &pk, "offer", &[]) else {
        return refused();
    };
    // No name in any offer: a nearby peer gets none at all, other links got
    // it in the invitation.
    axum::Json(super::link::sign_offer(&link.keys(), &svc.offer(&pk), None)).into_response()
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Change {
    #[serde(default)]
    pub add: Vec<Shared>,
    #[serde(default)]
    pub remove: Vec<NeighborhoodId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Changed {
    /// What the link carries now.
    pub n: Vec<NeighborhoodId>,
    /// Additions waiting for this side's user.
    pub pending: Vec<NeighborhoodId>,
}

/// Removals take effect at once. Additions need this side's user — a
/// neighborhood with a random id must never be added unasked — so they are
/// held as pending until the shell can ask.
async fn neighborhoods(
    State(svc): State<LinkService>,
    Path(pk): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(link) = svc.authorized(&headers, "POST", &pk, "neighborhoods", &body) else {
        return refused();
    };
    let Ok(change) = serde_json::from_slice::<Change>(&body) else {
        return refused();
    };
    // An ephemeral link carries auto-accepted neighborhoods only, and those
    // need no one's yes: what both carry is added at once, the rest ignored.
    if link.ephemeral.is_some() {
        let ours: Vec<NeighborhoodId> = svc
            .neighbors
            .get()
            .map(|n| n.auto_accepted().into_iter().map(|s| s.id).collect())
            .unwrap_or_default();
        let add: Vec<NeighborhoodId> = change
            .add
            .iter()
            .map(|s| s.id)
            .filter(|id| ours.contains(id))
            .collect();
        let now = svc
            .links
            .set_neighborhoods(&link.id, &add, &change.remove)
            .unwrap_or_default();
        if now.is_empty() {
            svc.links.end(&link.id);
        }
        return axum::Json(Changed {
            n: now.into_iter().collect(),
            pending: Vec::new(),
        })
        .into_response();
    }
    let now = svc
        .links
        .set_neighborhoods(&link.id, &[], &change.remove)
        .unwrap_or_default();
    let pending = svc.links.hold_pending(&link.id, change.add);
    // They took away the last thing we shared: the connection is over.
    if now.is_empty() && pending.is_empty() {
        svc.links.end(&link.id);
        tracing::info!(link = %link.id, "link ended: nothing left to share");
    }
    axum::Json(Changed {
        n: now.into_iter().collect(),
        pending,
    })
    .into_response()
}

async fn end(
    State(svc): State<LinkService>,
    Path(pk): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(link) = svc.authorized(&headers, "POST", &pk, "end", &body) else {
        return refused();
    };
    svc.links.end(&link.id);
    tracing::info!(link = %link.id, "link ended by the peer");
    StatusCode::NO_CONTENT.into_response()
}

// --- the link relay -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Scope {
    n: String,
    #[serde(default)]
    ttl: u8,
}

/// `GET /nh/{pk}/relay?n=…&ttl=…`: a NIP-01 WebSocket scoped to one
/// neighborhood the link carries. Signed (NIP-98) over the URL in http form.
async fn relay(
    State(svc): State<LinkService>,
    Path(pk): Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    axum::extract::Query(scope): axum::extract::Query<Scope>,
    headers: HeaderMap,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let rest = format!("relay?{}", query.unwrap_or_default());
    let Some(link) = svc.authorized(&headers, "GET", &pk, &rest, &[]) else {
        return refused();
    };
    let Some(slot) = svc.links.relay_slot(&link.id) else {
        return (StatusCode::TOO_MANY_REQUESTS, "too many sockets").into_response();
    };
    let ws = ws
        .max_message_size(MAX_RELAY_MESSAGE)
        .max_frame_size(MAX_RELAY_MESSAGE);
    let Some(n) = NeighborhoodId::from_hex(&scope.n) else {
        return refused();
    };
    let Some(neighbors) = svc.neighbors.get().cloned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !link.neighborhoods.contains(&n) || !neighbors.carries(n) {
        return refused();
    }
    let guard = SocketGuard::new(svc.links.clone(), slot);
    ws.on_upgrade(move |socket| relay_session(socket, neighbors, link, n, scope.ttl, guard))
}

async fn relay_session(
    mut socket: axum::extract::ws::WebSocket,
    neighbors: Arc<super::Neighbors>,
    link: Link,
    n: NeighborhoodId,
    ttl: u8,
    mut guard: SocketGuard,
) {
    use axum::extract::ws::Message;
    use tokio::sync::broadcast::error::RecvError;
    // Open subscriptions: new events in `n` matching one follow it.
    let mut open: BTreeMap<String, Vec<nostr::Filter>> = BTreeMap::new();
    let mut live = neighbors.live_events();
    loop {
        let idle_at = guard.idle_at();
        let replies: Vec<serde_json::Value> = tokio::select! {
            // The link ended or stopped carrying `n`: so does this socket.
            still = guard.changed(&link.id, n) => {
                if !still {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                continue;
            }
            _ = tokio::time::sleep_until(idle_at), if open.is_empty() => {
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
            frame = socket.recv() => {
                let Some(Ok(frame)) = frame else { return };
                if !guard.on_frame(&link.id, n) {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                let Message::Text(text) = frame else {
                    continue;
                };
                let Ok(serde_json::Value::Array(msg)) =
                    serde_json::from_str::<serde_json::Value>(&text)
                else {
                    continue;
                };
                match msg.first().and_then(|v| v.as_str()) {
                    Some("EVENT") => {
                        let Some(event) = msg
                            .get(1)
                            .and_then(|v| serde_json::from_value::<nostr::Event>(v.clone()).ok())
                        else {
                            continue;
                        };
                        let id = event.id.to_hex();
                        // Only where this side offers relay writes.
                        if !neighbors.offer(n).is_some_and(|o| o.relay_write) {
                            vec![serde_json::json!([
                                "OK", id, false, "restricted: no relay writes here"
                            ])]
                        } else {
                            let took = neighbors.receive(n, event, &link.id, ttl).await;
                            vec![serde_json::json!(["OK", id, took, ""])]
                        }
                    }
                    Some("REQ") if !neighbors.offer(n).is_some_and(|o| o.relay_read) => {
                        // Only where this side offers relay reads.
                        let sub = msg.get(1).cloned().unwrap_or_default();
                        if let Some(sub) = sub.as_str() {
                            open.remove(sub);
                        }
                        vec![serde_json::json!([
                            "CLOSED", sub, "restricted: no relay reads here"
                        ])]
                    }
                    Some("REQ") => {
                        let sub = msg.get(1).cloned().unwrap_or_default();
                        let filters: Vec<nostr::Filter> = msg
                            .iter()
                            .skip(2)
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect();
                        let mut out: Vec<serde_json::Value> = neighbors
                            .link_backlog(n, &filters)
                            .await
                            .into_iter()
                            .map(|e| serde_json::json!(["EVENT", sub, e]))
                            .collect();
                        out.push(serde_json::json!(["EOSE", sub]));
                        if let Some(sub) = sub.as_str() {
                            if open.len() < MAX_OPEN_REQS || open.contains_key(sub) {
                                open.insert(sub.to_string(), filters);
                            }
                        }
                        out
                    }
                    Some("CLOSE") => {
                        if let Some(sub) = msg.get(1).and_then(|v| v.as_str()) {
                            open.remove(sub);
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            kept = live.recv(), if !open.is_empty() => match kept {
                // Only what may be passed on: a push like a forward, which
                // the peer keeps as pulled. And only while reads are offered.
                Ok(kept)
                    if neighbors.link_may_see(&link.id, n, &kept)
                        && neighbors.offer(n).is_some_and(|o| o.relay_read) =>
                {
                    let any = nostr::filter::MatchEventOptions::new();
                    open.iter()
                        .filter(|(_, fs)| fs.iter().any(|f| f.match_event(&kept.event, any)))
                        .map(|(sub, _)| serde_json::json!(["EVENT", sub, kept.event]))
                        .collect()
                }
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
        };
        for reply in replies {
            if socket.send(Message::text(reply.to_string())).await.is_err() {
                return;
            }
        }
    }
}

/// Most subscriptions one link relay connection keeps open.
const MAX_OPEN_REQS: usize = 16;

/// The largest message or frame a link relay socket reads.
pub const MAX_RELAY_MESSAGE: usize = 64 * 1024;
/// How long a link relay socket with no open subscription may sit with
/// nothing from the peer before it is closed.
pub const RELAY_IDLE: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// Sustained frames per second a peer may send on one relay socket, and the
/// burst it may spend at once. Past it the socket is closed.
const FRAMES_PER_SEC: f64 = 20.0;
const FRAME_BURST: f64 = 100.0;

/// What keeps one link relay socket honest: it holds one of its link's
/// sockets ([`super::link::RelaySlot`]), lives only while the link carries
/// its neighborhood, closes when idle, and paces the peer's frames.
pub(crate) struct SocketGuard {
    links: Arc<Links>,
    changes: tokio::sync::watch::Receiver<u64>,
    _slot: super::link::RelaySlot,
    last_frame: tokio::time::Instant,
    tokens: f64,
    refilled: tokio::time::Instant,
}

impl SocketGuard {
    fn new(links: Arc<Links>, slot: super::link::RelaySlot) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            changes: links.watch(),
            links,
            _slot: slot,
            last_frame: now,
            tokens: FRAME_BURST,
            refilled: now,
        }
    }

    /// Waits for the links to change, then whether `link_id` still carries
    /// `n`. `false` too when the links are gone.
    async fn changed(&mut self, link_id: &str, n: NeighborhoodId) -> bool {
        if self.changes.changed().await.is_err() {
            return false;
        }
        self.links.carries(link_id, n)
    }

    /// When this socket counts as idle.
    fn idle_at(&self) -> tokio::time::Instant {
        self.last_frame + RELAY_IDLE
    }

    /// A frame came from the peer: whether to go on reading. Not when the
    /// link no longer carries `n`, nor when the peer is over its pace.
    fn on_frame(&mut self, link_id: &str, n: NeighborhoodId) -> bool {
        let now = tokio::time::Instant::now();
        self.last_frame = now;
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = (self.tokens + elapsed * FRAMES_PER_SEC).min(FRAME_BURST);
        self.refilled = now;
        if self.tokens < 1.0 {
            tracing::debug!(link = %link_id, "link relay socket over its pace; closed");
            return false;
        }
        self.tokens -= 1.0;
        self.links.carries(link_id, n)
    }
}

// --- link blobs -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BlobScope {
    n: String,
}

/// `GET /nh/{pk}/blob/{sha256}?n=…`: a blob kept here in a neighborhood the
/// link carries, if this side offers blob reads there. Nothing outside that
/// neighborhood's scope is served. Signed (NIP-98) over the full URL.
async fn blob(
    State(svc): State<LinkService>,
    Path((pk, sha256)): Path<(String, String)>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    axum::extract::Query(scope): axum::extract::Query<BlobScope>,
    headers: HeaderMap,
) -> Response {
    let rest = format!("blob/{sha256}?{}", query.unwrap_or_default());
    let Some(link) = svc.authorized(&headers, "GET", &pk, &rest, &[]) else {
        return refused();
    };
    let Some(n) = NeighborhoodId::from_hex(&scope.n) else {
        return refused();
    };
    let Some(neighbors) = svc.neighbors.get().cloned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !link.neighborhoods.contains(&n) {
        return refused();
    }
    match neighbors.link_blob(n, &sha256).await {
        Some(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `HEAD /nh/{pk}/blob/{sha256}?n=…`: whether [`blob`] would serve it, with
/// no body. Signed (NIP-98, method `HEAD`) over the full URL.
async fn blob_head(
    State(svc): State<LinkService>,
    Path((pk, sha256)): Path<(String, String)>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    axum::extract::Query(scope): axum::extract::Query<BlobScope>,
    headers: HeaderMap,
) -> Response {
    let rest = format!("blob/{sha256}?{}", query.unwrap_or_default());
    let Some(link) = svc.authorized(&headers, "HEAD", &pk, &rest, &[]) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Some(n) = NeighborhoodId::from_hex(&scope.n) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Some(neighbors) = svc.neighbors.get().cloned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !link.neighborhoods.contains(&n) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if neighbors.link_has(n, &sha256).await {
        StatusCode::OK.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

// --- presence -------------------------------------------------------------------

/// How often each link is checked on.
pub const PRESENCE_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Keep links' presence current: every [`PRESENCE_EVERY`], ask each peer for
/// its offer. An answer means they're here now (see `Links::reachable`), and
/// it refreshes their name and endpoints. Runs for the life of the runtime.
pub async fn keep_in_touch(links: Arc<Links>) {
    let mut tick = tokio::time::interval(PRESENCE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        // Only connections that still share something are anyone's business.
        for link in links
            .all()
            .into_iter()
            .filter(|l| !l.neighborhoods.is_empty())
        {
            let links = links.clone();
            tokio::spawn(async move {
                let Ok(resp) = call(&link, reqwest::Method::GET, "offer", None).await else {
                    return;
                };
                if !resp.status().is_success() {
                    return;
                }
                links.mark_seen(&link.id);
                if let Ok(bytes) = read_capped(resp, MAX_ANSWER).await {
                    if let Ok(offer) = serde_json::from_slice::<nostr::Event>(&bytes) {
                        links.update_offer(&link.id, offer);
                    }
                }
            });
        }
    }
}

/// The largest answer read from a peer's link service: an offer or a setup
/// answer is a few signed events.
pub const MAX_ANSWER: usize = 64 * 1024;

/// A response's body, refused once it passes `limit` bytes: read as it
/// streams, so a peer cannot make us hold more.
pub(super) async fn read_capped(
    mut resp: reqwest::Response,
    limit: usize,
) -> anyhow::Result<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > limit as u64) {
        anyhow::bail!("answer larger than {limit} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if body.len() + chunk.len() > limit {
            anyhow::bail!("answer larger than {limit} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

// --- the joining side ----------------------------------------------------------

/// The client a URL is reached with: the mesh for `.fips` and mesh
/// addresses, the internet otherwise. Either refuses what its reach does not
/// cover (see [`crate::http`]).
pub(super) fn client_for(url: &str) -> crate::http::HttpClient {
    let parsed = reqwest::Url::parse(url).ok();
    let mesh = match parsed.as_ref().and_then(|u| u.host()) {
        Some(nostr::types::url::Host::Domain(d)) => d.trim_end_matches('.').ends_with(".fips"),
        Some(nostr::types::url::Host::Ipv6(v6)) => {
            crate::http::is_mesh_address(std::net::IpAddr::V6(v6))
        }
        _ => false,
    };
    if mesh {
        crate::http::shared_mesh()
    } else {
        crate::http::shared_internet()
    }
}

/// Whether a link URL a peer handed us may be dialled at all: a mesh peer or
/// a public host (see [`crate::http::may_dial_peer`]).
fn dialable(url: &str) -> bool {
    reqwest::Url::parse(url)
        .map_err(|e| e.to_string())
        .and_then(|u| crate::http::may_dial_peer(&u))
        .is_ok()
}

/// Join a link from a scanned [`LinkOffer`], accepting `accept` of its
/// neighborhoods: try each of the initiator's URLs until one answers.
pub async fn join(
    links: &Links,
    offer: &LinkOffer,
    accept: Vec<NeighborhoodId>,
    ours: &(dyn Fn(&str) -> OfferContent + Sync),
    our_name: Option<&str>,
) -> anyhow::Result<Link> {
    let mut last = anyhow::anyhow!("the invitation names no link URL");
    for url in &offer.link {
        let Some((request, joining)) = Links::join(offer, url, accept.clone(), ours, our_name)
        else {
            anyhow::bail!("the invitation has expired");
        };
        if !dialable(url) {
            last = anyhow::anyhow!("{url}: not dialled");
            continue;
        }
        let sent = match client_for(url).post(&format!("{url}/setup")) {
            Ok(req) => {
                req.header("content-type", "application/json")
                    .body(serde_json::to_vec(&request)?)
                    .send()
                    .await
            }
            Err(e) => {
                last = anyhow::anyhow!("{url}: {}", e.0);
                continue;
            }
        };
        match sent {
            Ok(resp) if resp.status().is_success() => {
                let response: SetupResponse =
                    serde_json::from_slice(&read_capped(resp, MAX_ANSWER).await?)?;
                return links
                    .finish(joining, &response)
                    .map_err(|why| anyhow::anyhow!("their answer was not valid: {why:?}"));
            }
            Ok(resp) => last = anyhow::anyhow!("{url}: refused ({})", resp.status()),
            Err(e) => last = anyhow::anyhow!("{url}: {e}"),
        }
    }
    Err(last)
}

/// A signed request to the peer's link service, at the first link URL in
/// their offer that answers.
pub async fn call(
    link: &Link,
    method: reqwest::Method,
    rest: &str,
    body: Option<serde_json::Value>,
) -> anyhow::Result<reqwest::Response> {
    let keys = link.keys();
    let urls: Vec<String> = link
        .services()
        .into_iter()
        .filter(|s| s.kind == "link")
        .map(|s| s.url)
        .collect();
    let body = body.map(|b| serde_json::to_vec(&b)).transpose()?;
    let mut last = anyhow::anyhow!("the peer offers no link URL");
    for base in urls {
        let url = format!("{base}/{rest}");
        if !dialable(&url) {
            last = anyhow::anyhow!("{url}: not dialled");
            continue;
        }
        let req = match client_for(&url).request(method.clone(), &url) {
            Ok(req) => req,
            Err(e) => {
                last = anyhow::anyhow!("{url}: {}", e.0);
                continue;
            }
        };
        let signed = body.as_deref().unwrap_or_default();
        let req = req.header(
            "authorization",
            http_auth(&keys, method.as_str(), &url, signed),
        );
        let req = match &body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(b.clone()),
            None => req,
        };
        match req.send().await {
            Ok(resp) => return Ok(resp),
            Err(e) => last = anyhow::anyhow!("{url}: {e}"),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    const BASE: &str = "http://alice.fips:4873";

    fn service() -> LinkService {
        let d = std::env::temp_dir().join(format!("myco-linksvc-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        LinkService {
            links: Arc::new(Links::open(&d)),
            neighbors: Arc::default(),
            bases: vec![BASE.into()],
            nearby: Arc::default(),
        }
    }

    async fn send(svc: &LinkService, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let resp = routes(svc.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn post_json(path: &str, body: &impl Serialize, auth: Option<String>) -> Request<Body> {
        let mut b = Request::post(path).header("content-type", "application/json");
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    /// Alice invites, Bob joins over the routes; returns Bob's side.
    async fn linked(svc: &LinkService, home: &Shared) -> Link {
        let offer = svc.invite(vec![home.clone()], 600);
        assert_eq!(offer.link, [format!("{BASE}/nh/{}", offer.pk)]);
        let bob = Links::open(
            &std::env::temp_dir().join(format!("myco-linksvc-bob-{}", super::super::id::token())),
        );
        let url = &offer.link[0];
        let (request, joining) = Links::join(
            &offer,
            url,
            vec![home.id],
            &|_| OfferContent::default(),
            None,
        )
        .unwrap();
        let (status, body) = send(
            svc,
            post_json(&format!("/nh/{}/setup", offer.pk), &request, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let response: SetupResponse = serde_json::from_value(body).unwrap();
        bob.finish(joining, &response).unwrap()
    }

    fn home() -> Shared {
        Shared {
            id: NeighborhoodId::random(),
            name: "Home".into(),
        }
    }

    #[tokio::test]
    async fn a_link_is_set_up_over_the_routes_and_offers_our_endpoints() {
        let svc = service();
        let home = home();
        let bob = linked(&svc, &home).await;
        assert!(bob.neighborhoods.contains(&home.id));
        let kinds: Vec<_> = bob.services().into_iter().map(|s| s.kind).collect();
        assert_eq!(kinds, ["link", "relay"]);
        let relay = bob
            .services()
            .into_iter()
            .find(|s| s.kind == "relay")
            .unwrap();
        assert_eq!(
            relay.url,
            format!("ws://alice.fips:4873/nh/{}/relay", bob.peer)
        );
        assert_eq!(svc.links.all().len(), 1);
    }

    #[tokio::test]
    async fn a_bad_setup_says_only_refused() {
        let svc = service();
        let offer = svc.invite(vec![], 600);
        let (request, _) = Links::join(
            &offer,
            "http://elsewhere.example/nh/x",
            vec![],
            &|_| OfferContent::default(),
            None,
        )
        .unwrap();
        let (status, body) = send(
            &svc,
            post_json(&format!("/nh/{}/setup", offer.pk), &request, None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, serde_json::json!({"error": "refused"}));
    }

    #[tokio::test]
    async fn the_offer_needs_the_peers_link_key() {
        let svc = service();
        let bob = linked(&svc, &home()).await;
        let path = format!("/nh/{}/offer", bob.peer);
        let url = format!("{BASE}{path}");
        let signed = |keys: &nostr::Keys| {
            Request::get(&path)
                .header("authorization", http_auth(keys, "GET", &url, b""))
                .body(Body::empty())
                .unwrap()
        };
        let (status, body) = send(&svc, signed(&bob.keys())).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            super::super::link::read_offer(&serde_json::from_value(body).unwrap(), &bob.peer)
                .is_some()
        );
        let (status, _) = send(&svc, signed(&nostr::Keys::generate())).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn removals_apply_and_additions_wait_for_the_user() {
        let svc = service();
        let home = home();
        let bob = linked(&svc, &home).await;
        let path = format!("/nh/{}/neighborhoods", bob.peer);
        let url = format!("{BASE}{path}");
        let club = Shared {
            id: NeighborhoodId::random(),
            name: "Club".into(),
        };
        let change = Change {
            add: vec![club.clone()],
            remove: vec![home.id],
        };
        let auth = http_auth(
            &bob.keys(),
            "POST",
            &url,
            &serde_json::to_vec(&change).unwrap(),
        );
        let (status, body) = send(&svc, post_json(&path, &change, Some(auth))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let changed: Changed = serde_json::from_value(body).unwrap();
        assert!(changed.n.is_empty(), "home removed");
        assert_eq!(changed.pending, [club.id], "club waits for the user");
        assert!(svc.links.all()[0].neighborhoods.is_empty());
    }

    #[tokio::test]
    async fn removing_the_last_neighborhood_ends_the_link() {
        let svc = service();
        let home = home();
        let bob = linked(&svc, &home).await;
        let path = format!("/nh/{}/neighborhoods", bob.peer);
        let change = Change {
            add: Vec::new(),
            remove: vec![home.id],
        };
        let body = serde_json::to_vec(&change).unwrap();
        let auth = http_auth(&bob.keys(), "POST", &format!("{BASE}{path}"), &body);
        let (status, _) = send(&svc, post_json(&path, &change, Some(auth))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(svc.links.all().is_empty(), "nothing shared, so no link");
    }

    #[tokio::test]
    async fn a_peer_can_end_the_link() {
        let svc = service();
        let bob = linked(&svc, &home()).await;
        let path = format!("/nh/{}/end", bob.peer);
        let auth = http_auth(&bob.keys(), "POST", &format!("{BASE}{path}"), b"");
        let req = Request::post(&path)
            .header("authorization", auth)
            .body(Body::empty())
            .unwrap();
        let resp = routes(svc.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(svc.links.all().is_empty());
    }
}
