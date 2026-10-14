//! Every HTTP client `myco-core` builds, and where each may connect.
//!
//! A URL is not trusted for where it came from: a server is named by a
//! manifest, a napplet, an author's list, a peer's offer, and the code that
//! carries it can always carry the wrong one. So the guard is not on the
//! inputs but on the client: an [`HttpClient`] is built for one [`Reach`],
//! and refuses, **at the dial**, anything outside it.
//!
//! - [`Reach::Internet`] — the public internet only. The default.
//! - [`Reach::Mesh`] — a Circle peer over FIPS: `.fips` names and the mesh's
//!   `fd00::/8` addresses, nothing else.
//! - [`Reach::Configured`] — a server the user typed in themselves (a custom
//!   Blossom store), which may well be on their own network. The one
//!   exception, and only ever for that.
//!
//! What the guard covers, for `Internet` and `Mesh`:
//!
//! - **Address literals**, judged when the request is made
//!   ([`HttpClient::request`]): a literal never reaches a resolver.
//! - **Names**, judged by the client's resolver on the very addresses it then
//!   connects to — no second lookup for a rebinding name to answer
//!   differently.
//! - **Redirects**, every hop judged as the first URL was, at most
//!   [`MAX_REDIRECTS`].
//! - **No proxy**, which would resolve names out of the resolver's sight.
//!
//! "Private" for `Internet` is [`is_private_ip`] — loopback, RFC 1918,
//! link-local, unique-local (the mesh), CGNAT, NAT64-wrapped private — and
//! this device's own network: any address on one of its interfaces, and any
//! IPv6 address in the `/64` of one of its global addresses (the LAN
//! neighbours of a SLAAC network).
//!
//! A client that cannot be built fails closed: every request is refused.
//! Nothing outside this module builds a `reqwest::Client`; a test holds the
//! crate to that.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use myco_napplet_runtime::nap::outbox::is_private_ip;

/// The most redirects one request follows.
pub(crate) const MAX_REDIRECTS: usize = 5;
/// How long a read of this device's interface addresses is reused.
const OWN_ADDRESSES_FOR: Duration = Duration::from_secs(30);

/// Where a client may connect. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
    Internet,
    Mesh,
    Configured,
}

/// How a client treats a redirect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Redirects {
    /// Followed, at most [`MAX_REDIRECTS`], each hop judged by the reach.
    Judged,
    /// Not followed: the caller follows by hand and judges each hop itself.
    None,
}

/// A request this device will not make, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused: {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// An HTTP client held to one [`Reach`]. The only way `myco-core` makes
/// requests.
#[derive(Clone)]
pub(crate) struct HttpClient {
    /// `None` when it could not be built: every request is refused.
    inner: Option<reqwest::Client>,
    reach: Reach,
    /// Loopback counts as allowed. Only ever true in host tests, whose
    /// servers are on `127.0.0.1`; see [`HttpClient::strict`].
    allow_loopback: bool,
}

impl HttpClient {
    /// The public internet only, redirects judged.
    pub(crate) fn internet(builder: reqwest::ClientBuilder) -> Self {
        Self::build(Reach::Internet, Redirects::Judged, builder, cfg!(test))
    }

    /// The public internet only, redirects not followed — for a caller that
    /// follows them by hand under a stricter rule of its own.
    pub(crate) fn internet_unredirected(builder: reqwest::ClientBuilder) -> Self {
        Self::build(Reach::Internet, Redirects::None, builder, cfg!(test))
    }

    /// A Circle peer over the mesh only.
    pub(crate) fn mesh(builder: reqwest::ClientBuilder) -> Self {
        Self::build(Reach::Mesh, Redirects::Judged, builder, cfg!(test))
    }

    /// A Circle peer over the mesh only, redirects not followed.
    pub(crate) fn mesh_unredirected(builder: reqwest::ClientBuilder) -> Self {
        Self::build(Reach::Mesh, Redirects::None, builder, cfg!(test))
    }

    /// A server the user configured themselves. Not guarded: it may be on
    /// their own network, and it is theirs to name. Never for a URL anyone
    /// else supplied.
    pub(crate) fn configured(builder: reqwest::ClientBuilder) -> Self {
        Self::build(Reach::Configured, Redirects::Judged, builder, false)
    }

    /// As the production client of `reach`, with loopback refused too — for
    /// the tests of the guard itself.
    #[cfg(test)]
    pub(crate) fn strict(
        reach: Reach,
        redirects: Redirects,
        builder: reqwest::ClientBuilder,
    ) -> Self {
        Self::build(reach, redirects, builder, false)
    }

    fn build(
        reach: Reach,
        redirects: Redirects,
        builder: reqwest::ClientBuilder,
        allow_loopback: bool,
    ) -> Self {
        let builder = match reach {
            Reach::Configured => builder,
            Reach::Internet | Reach::Mesh => {
                let builder = builder.no_proxy().dns_resolver(Arc::new(GuardedResolver {
                    reach,
                    allow_loopback,
                }));
                match redirects {
                    Redirects::None => builder.redirect(reqwest::redirect::Policy::none()),
                    Redirects::Judged => {
                        builder.redirect(reqwest::redirect::Policy::custom(move |attempt| {
                            if attempt.previous().len() > MAX_REDIRECTS {
                                return attempt
                                    .error(format!("more than {MAX_REDIRECTS} redirects"));
                            }
                            match may_dial(reach, attempt.url(), allow_loopback) {
                                Ok(()) => attempt.follow(),
                                Err(why) => attempt.error(Refused(format!("redirect: {why}"))),
                            }
                        }))
                    }
                }
            }
        };
        let inner = builder
            .build()
            .inspect_err(
                |e| tracing::warn!(?reach, error = %e, "no HTTP client; its requests are refused"),
            )
            .ok();
        Self {
            inner,
            reach,
            allow_loopback,
        }
    }

    /// A request to `url`, or why it will not be made: a URL this client's
    /// reach does not cover, judged here for an address literal and at the
    /// dial for a name.
    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, Refused> {
        let parsed = reqwest::Url::parse(url).map_err(|e| Refused(format!("not a URL ({e})")))?;
        may_dial(self.reach, &parsed, self.allow_loopback).map_err(Refused)?;
        let client = self
            .inner
            .as_ref()
            .ok_or_else(|| Refused("this device has no HTTP client".into()))?;
        Ok(client.request(method, parsed))
    }

    pub(crate) fn get(&self, url: &str) -> Result<reqwest::RequestBuilder, Refused> {
        self.request(reqwest::Method::GET, url)
    }

    pub(crate) fn head(&self, url: &str) -> Result<reqwest::RequestBuilder, Refused> {
        self.request(reqwest::Method::HEAD, url)
    }

    pub(crate) fn post(&self, url: &str) -> Result<reqwest::RequestBuilder, Refused> {
        self.request(reqwest::Method::POST, url)
    }

    pub(crate) fn put(&self, url: &str) -> Result<reqwest::RequestBuilder, Refused> {
        self.request(reqwest::Method::PUT, url)
    }
}

/// One internet client for the process, built on first use: a source is
/// made per fetch, and a client per source would read the TLS roots each
/// time and share no connections.
pub(crate) fn shared_internet() -> HttpClient {
    static CLIENT: OnceLock<HttpClient> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            HttpClient::internet(
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    // Generous, as for any blob: MBs on a slow mobile link.
                    .timeout(Duration::from_secs(60)),
            )
        })
        .clone()
}

/// One mesh client for the process, as [`shared_internet`].
pub(crate) fn shared_mesh() -> HttpClient {
    static CLIENT: OnceLock<HttpClient> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            HttpClient::mesh(
                reqwest::Client::builder()
                    // Generous: a blob can be MBs over a slow BLE mesh link.
                    .timeout(Duration::from_secs(60)),
            )
        })
        .clone()
}

/// Whether `url` is one a client of `reach` may dial, by what it says
/// before any lookup: the scheme, userinfo, and the host — an address
/// literal judged in full, a name judged by its shape (its addresses are the
/// resolver's to judge).
pub(crate) fn may_dial(
    reach: Reach,
    url: &reqwest::Url,
    allow_loopback: bool,
) -> Result<(), String> {
    use reqwest::Url;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{}: is not fetched", url.scheme()));
    }
    let loopback_ok = |url: &Url| {
        allow_loopback
            && match url.host() {
                Some(nostr::types::url::Host::Ipv4(v4)) => v4.is_loopback(),
                Some(nostr::types::url::Host::Ipv6(v6)) => v6.is_loopback(),
                Some(nostr::types::url::Host::Domain(d)) => d.trim_end_matches('.') == "localhost",
                None => false,
            }
    };
    match reach {
        Reach::Configured => Ok(()),
        _ if loopback_ok(url) => Ok(()),
        Reach::Internet => {
            myco_napplet_runtime::nap::resource::judge_public_host(url)
                .map_err(|r| r.reason().to_string())?;
            let literal = match url.host() {
                Some(nostr::types::url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
                Some(nostr::types::url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
                _ => None,
            };
            match literal {
                Some(ip) if is_own_network(ip) => {
                    Err("this device's own network is not fetched".into())
                }
                _ => Ok(()),
            }
        }
        Reach::Mesh => {
            if !url.username().is_empty() || url.password().is_some() {
                return Err("a URL with userinfo is not fetched".into());
            }
            match url.host() {
                Some(nostr::types::url::Host::Domain(d)) if is_mesh_name(d) => Ok(()),
                Some(nostr::types::url::Host::Ipv6(v6)) if is_mesh_address(IpAddr::V6(v6)) => {
                    Ok(())
                }
                _ => Err("only a mesh peer is reached this way".into()),
            }
        }
    }
}

/// A FIPS mesh name: `<npub>.fips`.
fn is_mesh_name(name: &str) -> bool {
    name.trim_end_matches('.')
        .to_ascii_lowercase()
        .ends_with(".fips")
}

/// A FIPS mesh address: `fd00::/8`.
fn is_mesh_address(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xff00 == 0xfd00)
}

/// Whether `ip` is this device or its LAN as seen through its own
/// interfaces: an address one of them holds, or an IPv6 address in the `/64`
/// of one of their global addresses. Loopback interfaces are left to
/// [`is_private_ip`].
fn is_own_network(ip: IpAddr) -> bool {
    let own = own_addresses();
    own.iter().any(|mine| {
        *mine == ip
            || match (mine, ip) {
                (IpAddr::V6(mine), IpAddr::V6(theirs)) => {
                    !is_private_ip(IpAddr::V6(*mine))
                        && mine.segments()[..4] == theirs.segments()[..4]
                }
                _ => false,
            }
    })
}

/// This device's interface addresses, loopback left out, re-read at most
/// every [`OWN_ADDRESSES_FOR`] (a phone changes networks).
fn own_addresses() -> Vec<IpAddr> {
    type Cached = Mutex<Option<(Instant, Vec<IpAddr>)>>;
    static CACHE: OnceLock<Cached> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if let Some((at, addrs)) = cache.as_ref() {
        if at.elapsed() < OWN_ADDRESSES_FOR {
            return addrs.clone();
        }
    }
    let addrs: Vec<IpAddr> = if_addrs::get_if_addrs()
        .map(|ifs| {
            ifs.into_iter()
                .filter(|i| !i.is_loopback())
                .map(|i| i.ip())
                .collect()
        })
        .unwrap_or_default();
    *cache = Some((Instant::now(), addrs.clone()));
    addrs
}

/// Whether a request failed because the guard refused it — the device's
/// answer, not the server's health, so not to be held against the server.
pub(crate) fn refused_by_guard(e: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = source {
        if err.downcast_ref::<Refused>().is_some() {
            return true;
        }
        source = err.source();
    }
    false
}

/// The client's resolver: the system's answers, unless **any** of them is
/// outside the reach, in which case the name is refused and nothing is
/// dialled. The addresses returned are the ones connected to.
struct GuardedResolver {
    reach: Reach,
    allow_loopback: bool,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let (reach, allow_loopback) = (self.reach, self.allow_loopback);
        let host = name.as_str().to_string();
        Box::pin(async move {
            let refuse = |why: &str| {
                Err(Box::new(Refused(format!("{host}: {why}")))
                    as Box<dyn std::error::Error + Send + Sync>)
            };
            let loopback_name = allow_loopback && host.trim_end_matches('.') == "localhost";
            if reach == Reach::Mesh && !is_mesh_name(&host) && !loopback_name {
                return refuse("not a mesh name");
            }
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let allowed = |ip: IpAddr| {
                (allow_loopback && ip.is_loopback())
                    || match reach {
                        Reach::Internet => !is_private_ip(ip) && !is_own_network(ip),
                        Reach::Mesh => is_mesh_address(ip),
                        Reach::Configured => true,
                    }
            };
            if addrs.iter().any(|a| !allowed(a.ip())) {
                return refuse(match reach {
                    Reach::Mesh => "resolves outside the mesh",
                    _ => "resolves to a private address",
                });
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn url(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).unwrap()
    }

    /// A server counting the requests it was sent, with `/to-private`
    /// redirecting into a private network.
    async fn counting_server(hits: Arc<AtomicUsize>) -> (String, u16) {
        use axum::response::{IntoResponse as _, Redirect};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new()
            .route(
                "/to-private",
                axum::routing::get(|| async {
                    Redirect::temporary("http://10.255.255.1:1/x").into_response()
                }),
            )
            .fallback(move || {
                hits.fetch_add(1, Ordering::Relaxed);
                async { "secret" }
            });
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://127.0.0.1:{port}"), port)
    }

    /// What each reach will dial, judged before any lookup.
    #[test]
    fn each_reach_judges_the_url_before_the_dial() {
        for ok in [
            "https://example.com/x",
            "http://93.184.215.14/x",
            "https://[2606:4700::1]/x",
        ] {
            assert!(may_dial(Reach::Internet, &url(ok), false).is_ok(), "{ok}");
            assert!(
                may_dial(Reach::Mesh, &url(ok), false).is_err(),
                "mesh took {ok}"
            );
        }
        for refused in [
            "http://127.0.0.1/x",
            "http://localhost:24243/x",
            "http://10.0.0.1/x",
            "http://[fd00::1]:24243/x",
            "http://npub1peer.fips:24243/x",
            "http://[64:ff9b::c0a8:101]/x",
            "https://user@example.com/x",
            "file:///etc/passwd",
        ] {
            assert!(
                may_dial(Reach::Internet, &url(refused), false).is_err(),
                "{refused}"
            );
        }
        for ok in ["http://npub1peer.fips:24243/x", "http://[fd12::3]:24243/x"] {
            assert!(may_dial(Reach::Mesh, &url(ok), false).is_ok(), "{ok}");
        }
        for refused in [
            "http://192.168.1.1/x",
            "http://127.0.0.1/x",
            "http://[fe80::1]/x",
            "http://example.com/x",
            "http://user@npub1peer.fips/x",
        ] {
            assert!(
                may_dial(Reach::Mesh, &url(refused), false).is_err(),
                "{refused}"
            );
        }
        assert!(may_dial(Reach::Configured, &url("http://192.168.1.10:3000/"), false).is_ok());
        // Loopback only where a test says so.
        assert!(may_dial(Reach::Internet, &url("http://127.0.0.1:1/"), true).is_ok());
        assert!(may_dial(Reach::Mesh, &url("http://localhost:1/"), true).is_ok());
    }

    /// One of this device's own addresses is refused, and so is a neighbour
    /// in the `/64` of one of its global IPv6 addresses.
    #[test]
    fn this_devices_own_network_is_refused() {
        for mine in own_addresses() {
            assert!(is_own_network(mine), "{mine}");
            if let IpAddr::V6(v6) = mine {
                if !is_private_ip(mine) {
                    let mut s = v6.segments();
                    s[7] ^= 0x1;
                    assert!(is_own_network(IpAddr::V6(std::net::Ipv6Addr::from(s))));
                }
            }
        }
        assert!(!is_own_network("93.184.215.14".parse().unwrap()));
    }

    /// The guard is in the client, not the caller: a refused URL never
    /// reaches the server.
    #[tokio::test]
    async fn a_refused_url_is_never_sent() {
        let hits = Arc::new(AtomicUsize::new(0));
        let (base, _) = counting_server(hits.clone()).await;
        let strict = HttpClient::strict(
            Reach::Internet,
            Redirects::Judged,
            reqwest::Client::builder(),
        );
        assert!(strict.get(&format!("{base}/x")).is_err());
        let mesh = HttpClient::strict(Reach::Mesh, Redirects::Judged, reqwest::Client::builder());
        assert!(mesh.get(&format!("{base}/x")).is_err());
        assert_eq!(hits.load(Ordering::Relaxed), 0);
    }

    /// A name resolving to a private address is refused by the resolver,
    /// for the address it would have dialled — `localhost` standing in for a
    /// public name rebound to loopback. Asked of the inner client, past the
    /// request-time check, so it is the resolver that answers.
    #[tokio::test]
    async fn a_name_resolving_to_a_private_address_is_refused_at_the_dial() {
        let hits = Arc::new(AtomicUsize::new(0));
        let (_, port) = counting_server(hits.clone()).await;
        for reach in [Reach::Internet, Reach::Mesh] {
            let strict = HttpClient::strict(reach, Redirects::Judged, reqwest::Client::builder());
            let e = strict
                .inner
                .as_ref()
                .unwrap()
                .get(format!("http://localhost:{port}/x"))
                .send()
                .await
                .unwrap_err();
            assert!(refused_by_guard(&e), "{reach:?}: {e}");
        }
        assert_eq!(hits.load(Ordering::Relaxed), 0);
    }

    /// A redirect is judged before it is followed: by literal and by name.
    /// The first hop is on loopback, so the client allows loopback — and the
    /// literal hop goes to a private address it would otherwise try.
    #[tokio::test]
    async fn a_redirect_into_a_private_network_is_refused() {
        let hits = Arc::new(AtomicUsize::new(0));
        let (base, _) = counting_server(hits.clone()).await;
        let client = HttpClient::build(
            Reach::Internet,
            Redirects::Judged,
            reqwest::Client::builder().connect_timeout(Duration::from_secs(1)),
            true,
        );
        let e = client
            .get(&format!("{base}/to-private"))
            .unwrap()
            .send()
            .await
            .unwrap_err();
        assert!(e.is_redirect() && refused_by_guard(&e), "{e}");
        let mesh = HttpClient::build(
            Reach::Mesh,
            Redirects::Judged,
            reqwest::Client::builder(),
            true,
        );
        // Mesh, loopback allowed for the first hop: a hop off the mesh is
        // refused all the same.
        let e = mesh
            .get(&format!("{base}/to-private"))
            .unwrap()
            .send()
            .await
            .unwrap_err();
        assert!(e.is_redirect(), "{e}");
        assert_eq!(hits.load(Ordering::Relaxed), 0);
    }

    /// A client that could not be built refuses everything.
    #[test]
    fn a_client_that_failed_to_build_refuses() {
        let broken = HttpClient {
            inner: None,
            reach: Reach::Internet,
            allow_loopback: false,
        };
        assert!(broken.get("https://example.com/").is_err());
    }

    /// Nothing outside this module holds a `reqwest::Client`: every request
    /// goes through a reach. A builder handed to one of [`HttpClient`]'s
    /// constructors is fine; naming the client type, or `reqwest::get`, is
    /// not. A heuristic, not a proof: it catches a client held in a field or
    /// passed around, which is how one would be used. Test code is exempt.
    #[test]
    fn no_client_is_held_outside_this_module() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src.clone()];
        let mut offenders = Vec::new();
        while let Some(dir) = files.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("http.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let shipped = text.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
                let named = shipped.match_indices("reqwest::Client").any(|(at, _)| {
                    let rest = &shipped[at..];
                    !rest.starts_with("reqwest::Client::builder()")
                        && !rest.starts_with("reqwest::ClientBuilder")
                });
                if named || shipped.contains("reqwest::get(") {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }
}
