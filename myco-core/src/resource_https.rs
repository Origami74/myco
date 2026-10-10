//! NAP-RESOURCE's `https:` scheme: the [`HttpsFetcher`] Myco wires into the
//! napplet runtime.
//!
//! The runtime has already judged the URL (`validate_https_url`: https only,
//! no userinfo, no private literal, no local or `.fips` name) by the time it
//! gets here. This is the half of the policy only the dialler can enforce:
//!
//! - **What a name resolves to.** The client's resolver is
//!   [`PublicOnlyResolver`], which refuses a name if **any** of its answers is
//!   [`is_private_ip`] — the same predicate and the same any-answer rule as
//!   `outbox::dials_public`, but applied by the client itself, on the
//!   addresses it then connects to. `dials_public` resolves once to check and
//!   lets the client resolve again to dial (its documented TOCTOU gap); a
//!   rebinding name answers the two differently. Here there is one
//!   resolution, and it is the one dialled.
//! - **Every redirect hop.** Redirects are never followed by the client: each
//!   `Location` is joined to the URL it came from, judged by
//!   `validate_https_url` again, and dialled through the same resolver — at
//!   most [`MAX_REDIRECTS`] of them.
//! - **GET, and nothing of the user's.** No cookies (reqwest's cookie store
//!   is not built), no `Authorization`, no `Referer` (only set by a
//!   client-followed redirect), no proxy from the environment, and a plain
//!   User-Agent.
//! - **Size while downloading**: a `Content-Length` over the cap is refused
//!   before the body is read, and a body that runs past it is cut off there.
//! - **Time**: [`FETCH_TIMEOUT`] for the whole thing, waiting for a slot
//!   included.
//! - **Offline-only refuses** with `blocked-by-policy`. A tripped internet
//!   breaker (`Content::internet_looks_down`) is a `network-error` at once,
//!   as for the `blossom:` fetcher's internet half. Neither is *fed* from
//!   here: one napplet's dead link is that server's problem, not evidence
//!   the internet is down.
//! - **At most [`MAX_IN_FLIGHT`] at once** on the device, and one fetch per
//!   URL however many ask for it at the same moment (single-flight).
//!
//! What this hands back is unjudged bytes: the runtime hashes, sniffs and
//! stores them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt as _, Shared};
use myco_napplet_runtime::nap::outbox::is_private_ip;
use myco_napplet_runtime::nap::resource::validate_https_url;
use myco_napplet_runtime::seams::{HttpsError, HttpsErrorCode, HttpsFetcher};

/// The most redirects one fetch follows — the spec's recommended cap.
pub const MAX_REDIRECTS: usize = 5;
/// The whole fetch, from asking for a slot to the last byte — the spec's
/// recommended per-URL timeout.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one server gets to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The most `https:` fetches running at once on this device — the spec's
/// recommended in-flight limit. Past it a fetch waits for a slot, inside its
/// own [`FETCH_TIMEOUT`].
pub const MAX_IN_FLIGHT: usize = 10;
/// What the request says it is. Nothing about the user or the napplet.
const USER_AGENT: &str = concat!("Myco/", env!("CARGO_PKG_VERSION"));

/// A yes/no asked per call — offline-only, the internet breaker.
type Gate = Arc<dyn Fn() -> bool + Send + Sync>;

/// One fetch, shared by everyone who asked for the same URL while it ran.
type Flight = Shared<BoxFuture<'static, Result<Arc<Vec<u8>>, HttpsError>>>;

/// The [`HttpsFetcher`] behind NAP-RESOURCE's `https:` on a device.
pub struct NappletHttps {
    inner: Arc<Inner>,
    /// Fetches running now, by URL and cap.
    flights: Arc<Mutex<HashMap<(String, usize), Flight>>>,
}

struct Inner {
    http: reqwest::Client,
    /// The user switched the internet off (`Content::is_offline_only`).
    offline_only: Gate,
    /// The internet looks down right now (`Content::internet_looks_down`).
    internet_down: Gate,
    slots: tokio::sync::Semaphore,
    /// Judge every hop as the runtime judged the first. Off only in tests,
    /// whose "internet" is plain HTTP on `127.0.0.1`.
    guard: bool,
}

impl NappletHttps {
    /// Over the content layer: its offline-only setting and its internet
    /// breaker, read per call.
    pub fn new(content: Arc<crate::content::Content>) -> Self {
        let (offline, down) = (content.clone(), content);
        Self::with_gates(
            Arc::new(move || offline.is_offline_only()),
            Arc::new(move || down.internet_looks_down()),
            true,
        )
    }

    fn with_gates(offline_only: Gate, internet_down: Gate, guard: bool) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(FETCH_TIMEOUT)
            // Followed by hand, so every hop is judged; see the module docs.
            .redirect(reqwest::redirect::Policy::none())
            // A proxy would resolve the name itself, out of the resolver's
            // sight.
            .no_proxy()
            .dns_resolver(Arc::new(PublicOnlyResolver { guard }))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                http,
                offline_only,
                internet_down,
                slots: tokio::sync::Semaphore::new(MAX_IN_FLIGHT),
                guard,
            }),
            flights: Arc::default(),
        }
    }

    /// Plain HTTP to loopback, with the gates given — for tests, which must
    /// never reach the internet.
    #[cfg(test)]
    fn for_test(offline_only: Gate) -> Self {
        Self::with_gates(offline_only, Arc::new(|| false), false)
    }
}

#[async_trait::async_trait]
impl HttpsFetcher for NappletHttps {
    async fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, HttpsError> {
        // Asked before the flight is joined, so a switch to offline-only is
        // obeyed by the very next call even while an older fetch finishes.
        if (self.inner.offline_only)() {
            return Err(HttpsError::new(
                HttpsErrorCode::BlockedByPolicy,
                "offline-only is on in Settings, so nothing is fetched from the internet",
            ));
        }
        if (self.inner.internet_down)() {
            return Err(HttpsError::new(
                HttpsErrorCode::NetworkError,
                "no internet right now",
            ));
        }
        let key = (url.to_string(), max_bytes);
        let flight = {
            let mut flights = self.flights.lock().unwrap();
            if let Some(flight) = flights.get(&key) {
                flight.clone()
            } else {
                let inner = self.inner.clone();
                let url = url.to_string();
                let flight = async move { inner.fetch(url, max_bytes).await.map(Arc::new) }
                    .boxed()
                    .shared();
                flights.insert(key.clone(), flight.clone());
                // Driven by its own task, not by whoever asked: an asker
                // dropped mid-fetch would otherwise leave the flight in the
                // map unpolled, holding its slot with its timeout stopped.
                // The task clears it when done — only if it is still this
                // flight, not a newer one for the same URL.
                let driver = flight.clone();
                let flights = self.flights.clone();
                tokio::spawn(async move {
                    let _ = driver.clone().await;
                    let mut flights = flights.lock().unwrap();
                    if flights
                        .get(&key)
                        .is_some_and(|f| Shared::ptr_eq(f, &driver))
                    {
                        flights.remove(&key);
                    }
                });
                flight
            }
        };
        flight
            .await
            .map(|bytes| Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()))
    }
}

impl Inner {
    /// One fetch, bounded in time, waiting for a slot included.
    async fn fetch(&self, url: String, max_bytes: usize) -> Result<Vec<u8>, HttpsError> {
        match tokio::time::timeout(FETCH_TIMEOUT, self.fetch_unbounded(&url, max_bytes)).await {
            Ok(result) => result,
            Err(_) => Err(HttpsError::new(
                HttpsErrorCode::Timeout,
                format!("no answer within {}s", FETCH_TIMEOUT.as_secs()),
            )),
        }
    }

    async fn fetch_unbounded(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, HttpsError> {
        let _slot = self.slots.acquire().await.map_err(|_| {
            HttpsError::new(HttpsErrorCode::NetworkError, "the fetcher is shutting down")
        })?;
        let mut current = first_hop(url, self.guard)?;
        let mut hops = 0usize;
        loop {
            let response = self
                .http
                .get(current.clone())
                .send()
                .await
                .map_err(|e| classify(&e))?;
            let status = response.status();
            if status.is_redirection() {
                let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|l| l.to_str().ok())
                else {
                    return Err(HttpsError::new(
                        HttpsErrorCode::NetworkError,
                        format!("HTTP {status} without a Location"),
                    ));
                };
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(HttpsError::new(
                        HttpsErrorCode::BlockedByPolicy,
                        format!("more than {MAX_REDIRECTS} redirects"),
                    ));
                }
                current = next_hop(&current, location, self.guard)?;
                continue;
            }
            if matches!(status.as_u16(), 404 | 410) {
                return Err(HttpsError::new(
                    HttpsErrorCode::NotFound,
                    format!("HTTP {status}"),
                ));
            }
            if !status.is_success() {
                return Err(HttpsError::new(
                    HttpsErrorCode::NetworkError,
                    format!("HTTP {status}"),
                ));
            }
            return read_capped(response, max_bytes).await;
        }
    }
}

/// The URL as the runtime handed it, parsed — and judged again when
/// guarded, since a seam should not trust that it was.
fn first_hop(url: &str, guard: bool) -> Result<reqwest::Url, HttpsError> {
    if guard {
        return validate_https_url(url)
            .map_err(|r| HttpsError::new(HttpsErrorCode::BlockedByPolicy, r.to_string()));
    }
    reqwest::Url::parse(url)
        .map_err(|e| HttpsError::new(HttpsErrorCode::NetworkError, e.to_string()))
}

/// Where a redirect from `from` to `location` goes, if it may be followed:
/// joined (a relative `Location` is the norm) and judged by the runtime's own
/// `validate_https_url` when guarded — so a public server cannot bounce the
/// fetch to `http:`, loopback, the LAN, the metadata address or a mesh name.
/// What the new host's name resolves to is checked again at its own dial.
fn next_hop(from: &reqwest::Url, location: &str, guard: bool) -> Result<reqwest::Url, HttpsError> {
    let blocked = |why: String| HttpsError::new(HttpsErrorCode::BlockedByPolicy, why);
    let mut next = from
        .join(location)
        .map_err(|e| blocked(format!("redirect to an unparseable Location ({e})")))?;
    next.set_fragment(None);
    if guard {
        validate_https_url(next.as_str())
            .map_err(|r| blocked(format!("redirect refused: {}", r.reason())))
    } else if matches!(next.scheme(), "http" | "https") {
        Ok(next)
    } else {
        Err(blocked(format!("redirect to {}:", next.scheme())))
    }
}

/// The body, refused the moment it is known to be over `max_bytes`: from the
/// `Content-Length` before a byte is read, else from the running count.
async fn read_capped(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, HttpsError> {
    let too_large = |n: u64| {
        HttpsError::new(
            HttpsErrorCode::TooLarge,
            format!("{n} bytes, cap is {max_bytes}"),
        )
    };
    if let Some(len) = response.content_length() {
        if len > max_bytes as u64 {
            return Err(too_large(len));
        }
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .map_or(0, |len| len as usize)
            .min(max_bytes),
    );
    while let Some(chunk) = response.chunk().await.map_err(|e| classify(&e))? {
        if body.len() + chunk.len() > max_bytes {
            return Err(too_large((body.len() + chunk.len()) as u64));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A reqwest failure as the spec's code: a name refused by
/// [`PublicOnlyResolver`] is the policy's, a timeout is `timeout`, and the
/// rest — DNS, TCP, TLS — `network-error`.
fn classify(e: &reqwest::Error) -> HttpsError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = source {
        if err.downcast_ref::<PrivateAddress>().is_some() {
            return HttpsError::new(HttpsErrorCode::BlockedByPolicy, err.to_string());
        }
        source = err.source();
    }
    if e.is_timeout() {
        return HttpsError::new(HttpsErrorCode::Timeout, e.to_string());
    }
    HttpsError::new(HttpsErrorCode::NetworkError, e.to_string())
}

/// A name refused because it resolved to a private address.
#[derive(Debug)]
struct PrivateAddress(String);

impl std::fmt::Display for PrivateAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} resolves to a private address", self.0)
    }
}

impl std::error::Error for PrivateAddress {}

/// The client's resolver: the system's answers, unless **any** of them is
/// private, in which case the name is refused and nothing is dialled. The
/// addresses returned are the ones connected to, so there is no second
/// lookup for a rebinding name to answer differently.
///
/// Address literals never reach a resolver; the runtime's
/// `validate_https_url` judges those, on the first URL and on every hop.
struct PublicOnlyResolver {
    /// Off only in tests, whose servers are on loopback.
    guard: bool,
}

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let guard = self.guard;
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if guard && addrs.iter().any(|a| is_private_ip(a.ip())) {
                return Err(
                    Box::new(PrivateAddress(host)) as Box<dyn std::error::Error + Send + Sync>
                );
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A plain-HTTP "internet" on loopback, counting the requests it was
    /// sent.
    async fn server(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        base
    }

    fn open() -> Gate {
        Arc::new(|| false)
    }

    /// Redirect hops are judged as the first URL was: a public server
    /// cannot bounce the fetch to `http:`, loopback, the LAN, the metadata
    /// address, userinfo or a mesh name; a relative `Location` is fine.
    #[test]
    fn a_redirect_is_judged_like_the_first_url() {
        let from = reqwest::Url::parse("https://a.example/p/q.png").unwrap();
        for (location, to) in [
            ("/r.png", "https://a.example/r.png"),
            ("s.png", "https://a.example/p/s.png"),
            ("https://cdn.example/x#f", "https://cdn.example/x"),
            ("//cdn.example/y", "https://cdn.example/y"),
        ] {
            assert_eq!(next_hop(&from, location, true).unwrap().as_str(), to);
        }
        for location in [
            "http://a.example/r.png",
            "https://127.0.0.1/",
            "https://127.1/",
            "https://10.1.2.3/",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]:4870/",
            "https://localhost:24243/",
            "https://user@b.example/",
            "https://npub1peer.fips/",
            "file:///etc/passwd",
        ] {
            let e = next_hop(&from, location, true).unwrap_err();
            assert_eq!(e.code, HttpsErrorCode::BlockedByPolicy, "{location}");
        }
        assert_eq!(
            first_hop("http://example.com/", true).unwrap_err().code,
            HttpsErrorCode::BlockedByPolicy
        );
    }

    /// A name that resolves to a private address is refused at the dial —
    /// the check the runtime cannot make. `localhost` stands in for a public
    /// name rebound to loopback: the guarded client never connects to the
    /// server listening there.
    #[tokio::test]
    async fn a_name_resolving_to_a_private_address_is_refused_at_the_dial() {
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let base = server(axum::Router::new().route(
            "/x",
            axum::routing::get(move || {
                count.fetch_add(1, Ordering::Relaxed);
                async { "secret" }
            }),
        ))
        .await;
        let port = base.rsplit(':').next().unwrap();
        let guarded = NappletHttps::with_gates(open(), open(), true);
        let e = guarded
            .inner
            .http
            .get(format!("http://localhost:{port}/x"))
            .send()
            .await
            .unwrap_err();
        assert_eq!(classify(&e).code, HttpsErrorCode::BlockedByPolicy, "{e}");
        assert_eq!(
            hits.load(Ordering::Relaxed),
            0,
            "the private server was dialled"
        );
    }

    /// The body comes back; a 404 is `not-found`, a 500 `network-error`.
    #[tokio::test]
    async fn statuses_map_to_the_spec_codes() {
        use axum::http::StatusCode;
        let base = server(
            axum::Router::new()
                .route("/ok", axum::routing::get(|| async { "hello" }))
                .route("/gone", axum::routing::get(|| async { StatusCode::GONE }))
                .route(
                    "/broken",
                    axum::routing::get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
                ),
        )
        .await;
        let https = NappletHttps::for_test(open());
        assert_eq!(
            https.get(&format!("{base}/ok"), 1 << 20).await.unwrap(),
            b"hello"
        );
        for (path, code) in [
            ("/missing", HttpsErrorCode::NotFound),
            ("/gone", HttpsErrorCode::NotFound),
            ("/broken", HttpsErrorCode::NetworkError),
        ] {
            let e = https
                .get(&format!("{base}{path}"), 1 << 20)
                .await
                .unwrap_err();
            assert_eq!(e.code, code, "{path}");
        }
    }

    /// Redirects are followed by hand, up to the cap and not past it.
    #[tokio::test]
    async fn redirects_are_followed_up_to_the_cap() {
        use axum::extract::Path;
        use axum::response::Redirect;
        let base = server(axum::Router::new().route(
            "/hop/{n}",
            axum::routing::get(|Path(n): Path<usize>| async move {
                if n == 0 {
                    axum::response::Response::new("landed".into())
                } else {
                    axum::response::IntoResponse::into_response(Redirect::temporary(&format!(
                        "/hop/{}",
                        n - 1
                    )))
                }
            }),
        ))
        .await;
        let https = NappletHttps::for_test(open());
        assert_eq!(
            https
                .get(&format!("{base}/hop/{MAX_REDIRECTS}"), 1 << 20)
                .await
                .unwrap(),
            b"landed"
        );
        let e = https
            .get(&format!("{base}/hop/{}", MAX_REDIRECTS + 1), 1 << 20)
            .await
            .unwrap_err();
        assert_eq!(e.code, HttpsErrorCode::BlockedByPolicy);
    }

    /// The cap holds whether the server says the size up front or streams a
    /// body that runs past it.
    #[tokio::test]
    async fn the_cap_is_enforced_while_downloading() {
        let base = server(
            axum::Router::new().route("/big", axum::routing::get(|| async { vec![7u8; 4096] })),
        )
        .await;
        let https = NappletHttps::for_test(open());
        let e = https.get(&format!("{base}/big"), 4095).await.unwrap_err();
        assert_eq!(e.code, HttpsErrorCode::TooLarge);
        assert_eq!(
            https.get(&format!("{base}/big"), 4096).await.unwrap().len(),
            4096
        );

        // No Content-Length: chunked, written by hand, past the cap.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await;
            for _ in 0..8 {
                let _ = sock.write_all(b"400\r\n").await;
                let _ = sock.write_all(&[1u8; 0x400]).await;
                let _ = sock.write_all(b"\r\n").await;
            }
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });
        let e = https
            .get(&format!("http://{addr}/stream"), 4096)
            .await
            .unwrap_err();
        assert_eq!(e.code, HttpsErrorCode::TooLarge);
    }

    /// Offline-only refuses before anything is dialled.
    #[tokio::test]
    async fn offline_only_is_blocked_by_policy() {
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let base = server(axum::Router::new().route(
            "/x",
            axum::routing::get(move || {
                count.fetch_add(1, Ordering::Relaxed);
                async { "x" }
            }),
        ))
        .await;
        let offline = Arc::new(AtomicBool::new(true));
        let gate = offline.clone();
        let https = NappletHttps::for_test(Arc::new(move || gate.load(Ordering::Relaxed)));
        let e = https.get(&format!("{base}/x"), 1 << 20).await.unwrap_err();
        assert_eq!(e.code, HttpsErrorCode::BlockedByPolicy);
        assert_eq!(hits.load(Ordering::Relaxed), 0);
        offline.store(false, Ordering::Relaxed);
        assert!(https.get(&format!("{base}/x"), 1 << 20).await.is_ok());
    }

    /// Asks for one URL at the same moment share one request.
    #[tokio::test]
    async fn concurrent_asks_for_one_url_share_one_fetch() {
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let base = server(axum::Router::new().route(
            "/slow",
            axum::routing::get(move || {
                count.fetch_add(1, Ordering::Relaxed);
                async {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    "slow"
                }
            }),
        ))
        .await;
        let https = Arc::new(NappletHttps::for_test(open()));
        let url = format!("{base}/slow");
        let asks: Vec<_> = (0..4)
            .map(|_| {
                let https = https.clone();
                let url = url.clone();
                tokio::spawn(async move { https.get(&url, 1 << 20).await })
            })
            .collect();
        for ask in asks {
            assert_eq!(ask.await.unwrap().unwrap(), b"slow");
        }
        assert_eq!(
            hits.load(Ordering::Relaxed),
            1,
            "one URL was fetched more than once"
        );
        settled(&https).await;
    }

    /// An asker dropped mid-fetch does not strand the flight: it still runs
    /// to the end, gives its slot back and leaves the map.
    #[tokio::test]
    async fn a_dropped_asker_does_not_strand_its_flight() {
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let base = server(axum::Router::new().route(
            "/slow",
            axum::routing::get(move || {
                count.fetch_add(1, Ordering::Relaxed);
                async {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    "slow"
                }
            }),
        ))
        .await;
        let https = Arc::new(NappletHttps::for_test(open()));
        let url = format!("{base}/slow");
        let asker = {
            let (https, url) = (https.clone(), url.clone());
            tokio::spawn(async move { https.get(&url, 1 << 20).await })
        };
        while hits.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        asker.abort();
        settled(&https).await;
        assert_eq!(
            https.inner.slots.available_permits(),
            MAX_IN_FLIGHT,
            "a dropped asker kept its slot"
        );
        assert_eq!(https.get(&url, 1 << 20).await.unwrap(), b"slow");
    }

    /// Waits for the flights' own tasks to clear the map.
    async fn settled(https: &NappletHttps) {
        for _ in 0..200 {
            if https.flights.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("a finished flight stayed");
    }
}
