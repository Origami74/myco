//! The link service: the neighborhood protocol over HTTP, under `/nh` on the
//! auth plane (`:4873`).
//!
//! ```text
//! POST /nh/{pk}/setup          SetupRequest -> SetupResponse      (the one-time secret)
//! GET  /nh/{pk}/offer          -> our offer for this link         (NIP-98, link key)
//! POST /nh/{pk}/neighborhoods  {add, remove} -> {n, pending}      (NIP-98, link key)
//! POST /nh/{pk}/end            -> 204                             (NIP-98, link key)
//! ```
//!
//! `{pk}` is this side's link public key for the link (or invitation) in
//! question, so each link has its own URL and the URL a proof or signature
//! names is exactly one of ours. A device can be reached by several bases
//! (its FIPS address; later a Tor or public one): a request is checked
//! against each.
//!
//! Every refusal says only "refused": a stranger learns nothing about which
//! check failed.

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
    /// This device's own endpoints, offered on every link.
    pub relay_url: String,
    pub blossom_url: String,
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
        services.push(Service {
            kind: "blossom".into(),
            url: self.blossom_url.clone(),
            read: true,
            write: false,
            own: false,
        });
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
        let mut offer = self.links.invite(Vec::new(), neighborhoods, ttl_secs);
        offer.link = self.link_urls(&offer.pk);
        offer.from = from.filter(|f| !f.trim().is_empty());
        offer
    }

    /// The link a NIP-98 header proves, for one of our link keys and a
    /// request at `/nh/{pk}/{rest}`.
    fn authorized(&self, headers: &HeaderMap, method: &str, pk: &str, rest: &str) -> Option<Link> {
        let header = headers.get("authorization")?.to_str().ok()?;
        let signer = self
            .link_urls(pk)
            .iter()
            .find_map(|url| check_http_auth(header, method, &format!("{url}/{rest}")))?;
        let link = self.links.by_peer(&signer).filter(|l| l.own_pk() == pk)?;
        self.links.mark_seen(&link.id);
        Some(link)
    }
}

pub fn routes(service: LinkService) -> Router {
    Router::new()
        .route("/nh/{pk}/setup", post(setup))
        .route("/nh/{pk}/offer", get(offer))
        .route("/nh/{pk}/neighborhoods", post(neighborhoods))
        .route("/nh/{pk}/end", post(end))
        .route("/nh/{pk}/relay", get(relay))
        .with_state(service)
}

fn refused() -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": "refused" })),
    )
        .into_response()
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
    let Some(link) = svc.authorized(&headers, "GET", &pk, "offer") else {
        return refused();
    };
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
    let Some(link) = svc.authorized(&headers, "POST", &pk, "neighborhoods") else {
        return refused();
    };
    let Ok(change) = serde_json::from_slice::<Change>(&body) else {
        return refused();
    };
    let now = svc
        .links
        .set_neighborhoods(&link.id, &[], &change.remove)
        .unwrap_or_default();
    let pending = svc.links.hold_pending(&link.id, change.add);
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
) -> Response {
    let Some(link) = svc.authorized(&headers, "POST", &pk, "end") else {
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
    let Some(link) = svc.authorized(&headers, "GET", &pk, &rest) else {
        return refused();
    };
    let Some(n) = NeighborhoodId::from_hex(&scope.n) else {
        return refused();
    };
    let Some(neighbors) = svc.neighbors.get().cloned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !link.neighborhoods.contains(&n) || !neighbors.carries(n) {
        return refused();
    }
    ws.on_upgrade(move |socket| relay_session(socket, neighbors, link, n, scope.ttl))
}

async fn relay_session(
    mut socket: axum::extract::ws::WebSocket,
    neighbors: Arc<super::Neighbors>,
    link: Link,
    n: NeighborhoodId,
    ttl: u8,
) {
    use axum::extract::ws::Message;
    while let Some(Ok(frame)) = socket.recv().await {
        let Message::Text(text) = frame else {
            continue;
        };
        let Ok(serde_json::Value::Array(msg)) = serde_json::from_str::<serde_json::Value>(&text)
        else {
            continue;
        };
        let replies: Vec<serde_json::Value> = match msg.first().and_then(|v| v.as_str()) {
            Some("EVENT") => {
                let Some(event) = msg
                    .get(1)
                    .and_then(|v| serde_json::from_value::<nostr::Event>(v.clone()).ok())
                else {
                    continue;
                };
                let id = event.id.to_hex();
                let took = neighbors.receive(n, event, &link.id, ttl).await;
                vec![serde_json::json!(["OK", id, took, ""])]
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
                out
            }
            _ => Vec::new(),
        };
        for reply in replies {
            if socket.send(Message::text(reply.to_string())).await.is_err() {
                return;
            }
        }
    }
}

// --- the joining side ----------------------------------------------------------

/// The client a URL is reached with: the mesh for `.fips`, the internet
/// otherwise.
fn client_for(url: &str) -> crate::http::HttpClient {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    if host.ends_with(".fips") {
        crate::http::shared_mesh()
    } else {
        crate::http::shared_internet()
    }
}

/// Join a link from a scanned [`LinkOffer`], accepting `accept` of its
/// neighborhoods: try each of the initiator's URLs until one answers.
pub async fn join(
    links: &Links,
    offer: &LinkOffer,
    accept: Vec<NeighborhoodId>,
    ours: &(dyn Fn(&str) -> OfferContent + Sync),
) -> anyhow::Result<Link> {
    let mut last = anyhow::anyhow!("the invitation names no link URL");
    for url in &offer.link {
        let Some((request, joining)) = Links::join(offer, url, accept.clone(), ours, None) else {
            anyhow::bail!("the invitation has expired");
        };
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
                let response: SetupResponse = serde_json::from_slice(&resp.bytes().await?)?;
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
    let mut last = anyhow::anyhow!("the peer offers no link URL");
    for base in urls {
        let url = format!("{base}/{rest}");
        let req = match client_for(&url).request(method.clone(), &url) {
            Ok(req) => req,
            Err(e) => {
                last = anyhow::anyhow!("{url}: {}", e.0);
                continue;
            }
        };
        let req = req.header("authorization", http_auth(&keys, method.as_str(), &url));
        let req = match &body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(serde_json::to_vec(b)?),
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
            relay_url: "ws://alice.fips:4870".into(),
            blossom_url: "http://alice.fips:24243".into(),
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
        assert_eq!(kinds, ["link", "relay", "blossom"]);
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
                .header("authorization", http_auth(keys, "GET", &url))
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
        let auth = http_auth(&bob.keys(), "POST", &url);
        let (status, body) = send(&svc, post_json(&path, &change, Some(auth))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let changed: Changed = serde_json::from_value(body).unwrap();
        assert!(changed.n.is_empty(), "home removed");
        assert_eq!(changed.pending, [club.id], "club waits for the user");
        assert!(svc.links.all()[0].neighborhoods.is_empty());
    }

    #[tokio::test]
    async fn a_peer_can_end_the_link() {
        let svc = service();
        let bob = linked(&svc, &home()).await;
        let path = format!("/nh/{}/end", bob.peer);
        let auth = http_auth(&bob.keys(), "POST", &format!("{BASE}{path}"));
        let req = Request::post(&path)
            .header("authorization", auth)
            .body(Body::empty())
            .unwrap();
        let resp = routes(svc.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(svc.links.all().is_empty());
    }
}
