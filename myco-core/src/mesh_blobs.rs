//! `mesh.blobs`: which blobs the Circle's Blossom stores hold, asked over the
//! mesh with a `HEAD` and nothing more.
//!
//! A napplet asks "can the phones around me serve these files?" — an app
//! store hiding apps whose `index.html` nobody nearby has. The answer is a
//! count per blob, built from one BUD-01 `HEAD /<sha256>` per reachable
//! Circle member, at `http://<npub>.fips:24243`, the address
//! [`BlossomFetcher`](crate::napplet::BlossomFetcher) downloads from.
//!
//! ## What it never does
//!
//! - **Ask the internet.** The only URL it builds is a peer's `.fips` name,
//!   from an npub that parses, and a redirect is not followed — a peer cannot
//!   point the probe off the mesh.
//! - **Download a body.** `HEAD` only; the response is read for its status.
//!
//! ## How it is bounded
//!
//! A napplet may call this on a timer, for up to
//! [`MAX_BLOB_HASHES`](myco_napplet_runtime::MAX_BLOB_HASHES) blobs, with
//! every blob costing one request per peer — over BLE as often as not. So:
//!
//! - **An answer is remembered** per (peer, blob): a hit for
//!   [`HIT_REMEMBERED`], a miss for the shorter [`MISS_REMEMBERED`] (a peer
//!   may be fetching it right now). A repeat call inside that costs nothing.
//! - **A few at a time** ([`CONCURRENCY`]), each given [`HEAD_TIMEOUT`].
//! - **A peer that fails once is skipped** for the rest of the call: a peer
//!   that is gone must not cost a timeout per blob. Its blobs are remembered
//!   as misses.
//! - **One deadline per call** ([`CALL_DEADLINE`]). What has not answered by
//!   then counts as not held and is not remembered, so the next call asks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use myco_napplet_runtime::BlobReach;

/// How long a peer holding a blob is believed without asking again.
pub const HIT_REMEMBERED: Duration = Duration::from_secs(60);
/// How long a peer not holding a blob is believed. Shorter: it may be
/// syncing that very app.
pub const MISS_REMEMBERED: Duration = Duration::from_secs(20);
/// How many `HEAD`s are in flight at once, across all peers.
pub const CONCURRENCY: usize = 6;
/// How long one `HEAD` gets.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(4);
/// How long one `mesh.blobs` call gets, all together. Well inside the
/// prelude's 30 s request timeout.
pub const CALL_DEADLINE: Duration = Duration::from_secs(12);
/// The most (peer, blob) answers remembered; past it the expired go, then
/// all of them.
const MAX_REMEMBERED: usize = 8192;

/// The two things the probe needs from the world. A trait so the bounds can
/// be tested with no mesh; [`HttpProbe`] is the real one.
#[async_trait::async_trait]
pub trait Probe: Send + Sync {
    /// The npubs of the Circle members reachable right now.
    fn peers(&self) -> Vec<String>;
    /// Whether `npub`'s Blossom holds `sha256_hex`: `Ok(false)` for a clear
    /// "no", `Err` for no answer at all.
    async fn head(&self, npub: &str, sha256_hex: &str) -> anyhow::Result<bool>;
}

/// The bounds, separate so tests can shrink the timers.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub hit_remembered: Duration,
    pub miss_remembered: Duration,
    pub concurrency: usize,
    pub head_timeout: Duration,
    pub call_deadline: Duration,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            hit_remembered: HIT_REMEMBERED,
            miss_remembered: MISS_REMEMBERED,
            concurrency: CONCURRENCY,
            head_timeout: HEAD_TIMEOUT,
            call_deadline: CALL_DEADLINE,
        }
    }
}

/// The bounded, remembering asker behind `mesh.blobs`.
pub struct MeshBlobs {
    probe: Arc<dyn Probe>,
    bounds: Bounds,
    /// (npub, sha256) → (held, when that was learned).
    remembered: Mutex<HashMap<(String, String), (bool, Instant)>>,
}

impl MeshBlobs {
    pub fn new(probe: Arc<dyn Probe>) -> Self {
        Self::with_bounds(probe, Bounds::default())
    }

    pub fn with_bounds(probe: Arc<dyn Probe>, bounds: Bounds) -> Self {
        Self {
            probe,
            bounds,
            remembered: Mutex::new(HashMap::new()),
        }
    }

    /// How many reachable peers hold each of `hashes`.
    pub async fn ask(&self, hashes: &[String]) -> BlobReach {
        let peers = self.probe.peers();
        let mut holders: BTreeMap<String, usize> = hashes.iter().map(|h| (h.clone(), 0)).collect();

        // Blob-major, so the first round touches every peer once: a dead
        // peer is found out on its first blob, not its last.
        let mut todo = Vec::new();
        for hash in hashes {
            for npub in &peers {
                match self.recall(npub, hash) {
                    Some(true) => *holders.get_mut(hash).unwrap() += 1,
                    Some(false) => {}
                    None => todo.push((npub.clone(), hash.clone())),
                }
            }
        }

        if !todo.is_empty() {
            let failed: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
            let failed = &failed;
            let mut asks = futures_util::stream::iter(todo)
                .map(|(npub, hash)| async move {
                    if failed.lock().unwrap().contains(&npub) {
                        self.remember(&npub, &hash, false);
                        return None;
                    }
                    let answer = tokio::time::timeout(
                        self.bounds.head_timeout,
                        self.probe.head(&npub, &hash),
                    )
                    .await;
                    match answer {
                        Ok(Ok(held)) => {
                            self.remember(&npub, &hash, held);
                            held.then_some(hash)
                        }
                        _ => {
                            failed.lock().unwrap().insert(npub.clone());
                            self.remember(&npub, &hash, false);
                            None
                        }
                    }
                })
                .buffer_unordered(self.bounds.concurrency.max(1));
            let deadline = tokio::time::sleep(self.bounds.call_deadline);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    next = asks.next() => match next {
                        Some(Some(hash)) => *holders.get_mut(&hash).unwrap() += 1,
                        Some(None) => {}
                        None => break,
                    },
                    _ = &mut deadline => {
                        tracing::debug!("mesh.blobs: call deadline reached");
                        break;
                    }
                }
            }
        }

        BlobReach {
            peers: peers.len(),
            holders,
        }
    }

    fn recall(&self, npub: &str, hash: &str) -> Option<bool> {
        let mut remembered = self.remembered.lock().unwrap();
        let key = (npub.to_string(), hash.to_string());
        let (held, at) = *remembered.get(&key)?;
        let fresh = if held {
            self.bounds.hit_remembered
        } else {
            self.bounds.miss_remembered
        };
        if at.elapsed() < fresh {
            Some(held)
        } else {
            remembered.remove(&key);
            None
        }
    }

    fn remember(&self, npub: &str, hash: &str, held: bool) {
        let mut remembered = self.remembered.lock().unwrap();
        if remembered.len() >= MAX_REMEMBERED {
            let longest = self.bounds.hit_remembered.max(self.bounds.miss_remembered);
            remembered.retain(|_, (_, at)| at.elapsed() < longest);
            if remembered.len() >= MAX_REMEMBERED {
                remembered.clear();
            }
        }
        remembered.insert((npub.to_string(), hash.to_string()), (held, Instant::now()));
    }
}

/// A peer's Blossom `HEAD` URL — always a `.fips` name, never anything else.
pub fn head_url(npub: &str, sha256_hex: &str) -> anyhow::Result<String> {
    fips::PeerIdentity::from_npub(npub).map_err(|e| anyhow::anyhow!("invalid npub {npub}: {e}"))?;
    anyhow::ensure!(
        sha256_hex.len() == 64 && sha256_hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "not a sha256 hash"
    );
    Ok(format!(
        "{}/{}",
        crate::ip_source::mesh_blossom_url(npub),
        sha256_hex
    ))
}

/// The real probe: the Circle members in reach, asked over the mesh.
pub struct HttpProbe {
    content: Arc<crate::content::Content>,
    http: reqwest::Client,
}

impl HttpProbe {
    pub fn new(content: Arc<crate::content::Content>) -> Self {
        Self {
            content,
            http: probe_client(),
        }
    }
}

/// The client every probe goes out on: no redirects (a peer cannot send the
/// probe off the mesh), no proxy, and a short timeout of its own.
fn probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(HEAD_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// One `HEAD`: 2xx is held; 404, and the 401/403 a peer gives a stranger,
/// are a clear "no"; anything else is no answer.
async fn head_status(http: &reqwest::Client, url: &str) -> anyhow::Result<bool> {
    let status = http.head(url).send().await?.status();
    if status.is_success() {
        return Ok(true);
    }
    match status.as_u16() {
        401 | 403 | 404 => Ok(false),
        other => anyhow::bail!("HEAD {url}: {other}"),
    }
}

#[async_trait::async_trait]
impl Probe for HttpProbe {
    fn peers(&self) -> Vec<String> {
        self.content.reachable_npubs()
    }

    async fn head(&self, npub: &str, sha256_hex: &str) -> anyhow::Result<bool> {
        head_status(&self.http, &head_url(npub, sha256_hex)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const A: &str = "b1674191a88ec5cdd733e4240a81803105dc412d6c6708d53ab94fc248f4f553";
    const B: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    /// Peers by name, what each holds, which never answer, and every ask.
    #[derive(Default)]
    struct FakeProbe {
        peers: Vec<String>,
        held: HashSet<(String, String)>,
        silent: HashSet<String>,
        asks: Mutex<Vec<(String, String)>>,
        in_flight: AtomicUsize,
        most_in_flight: AtomicUsize,
    }

    impl FakeProbe {
        fn new(peers: &[&str]) -> Self {
            Self {
                peers: peers.iter().map(|p| p.to_string()).collect(),
                ..Default::default()
            }
        }
        fn holding(mut self, peer: &str, hash: &str) -> Self {
            self.held.insert((peer.to_string(), hash.to_string()));
            self
        }
        fn silent(mut self, peer: &str) -> Self {
            self.silent.insert(peer.to_string());
            self
        }
        fn asks(&self) -> Vec<(String, String)> {
            self.asks.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Probe for FakeProbe {
        fn peers(&self) -> Vec<String> {
            self.peers.clone()
        }
        async fn head(&self, npub: &str, hash: &str) -> anyhow::Result<bool> {
            self.asks
                .lock()
                .unwrap()
                .push((npub.to_string(), hash.to_string()));
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_in_flight.fetch_max(now, Ordering::SeqCst);
            if self.silent.contains(npub) {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(self.held.contains(&(npub.to_string(), hash.to_string())))
        }
    }

    fn quick() -> Bounds {
        Bounds {
            head_timeout: Duration::from_millis(100),
            call_deadline: Duration::from_secs(2),
            ..Bounds::default()
        }
    }

    fn hashes(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| h.to_string()).collect()
    }

    #[tokio::test]
    async fn counts_hits_and_misses_per_blob() {
        let probe = Arc::new(
            FakeProbe::new(&["p1", "p2", "p3"])
                .holding("p1", A)
                .holding("p3", A),
        );
        let blobs = MeshBlobs::with_bounds(probe.clone(), quick());
        let reach = blobs.ask(&hashes(&[A, B])).await;
        assert_eq!(reach.peers, 3);
        assert_eq!(reach.holders[A], 2);
        assert_eq!(reach.holders[B], 0);
        assert_eq!(probe.asks().len(), 6);
    }

    #[tokio::test]
    async fn no_peers_is_zero_everywhere_and_asks_nobody() {
        let probe = Arc::new(FakeProbe::new(&[]));
        let blobs = MeshBlobs::with_bounds(probe.clone(), quick());
        let reach = blobs.ask(&hashes(&[A])).await;
        assert_eq!(reach.peers, 0);
        assert_eq!(reach.holders[A], 0);
        assert!(probe.asks().is_empty());
    }

    /// A repeat call inside the memory asks nobody, hits and misses alike.
    #[tokio::test]
    async fn answers_are_remembered() {
        let probe = Arc::new(FakeProbe::new(&["p1", "p2"]).holding("p1", A));
        let blobs = MeshBlobs::with_bounds(probe.clone(), quick());
        blobs.ask(&hashes(&[A, B])).await;
        let reach = blobs.ask(&hashes(&[A, B])).await;
        assert_eq!(reach.holders[A], 1);
        assert_eq!(probe.asks().len(), 4, "the second call asked again");
    }

    /// A miss is forgotten sooner than a hit: the peer may be syncing it.
    #[tokio::test]
    async fn a_miss_is_forgotten_before_a_hit() {
        let probe = Arc::new(FakeProbe::new(&["p1"]).holding("p1", A));
        let bounds = Bounds {
            miss_remembered: Duration::from_millis(30),
            ..quick()
        };
        let blobs = MeshBlobs::with_bounds(probe.clone(), bounds);
        blobs.ask(&hashes(&[A, B])).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        blobs.ask(&hashes(&[A, B])).await;
        let asks = probe.asks();
        assert_eq!(asks.iter().filter(|(_, h)| h == A).count(), 1);
        assert_eq!(asks.iter().filter(|(_, h)| h == B).count(), 2);
    }

    /// A peer that does not answer costs one timeout, not one per blob.
    #[tokio::test]
    async fn a_silent_peer_is_skipped_after_its_first_timeout() {
        let probe = Arc::new(
            FakeProbe::new(&["gone", "here"])
                .holding("here", A)
                .silent("gone"),
        );
        let bounds = Bounds {
            concurrency: 1,
            ..quick()
        };
        let blobs = MeshBlobs::with_bounds(probe.clone(), bounds);
        let many: Vec<String> = (0..10)
            .map(|i| format!("{i:064x}"))
            .chain([A.to_string()])
            .collect();
        let started = Instant::now();
        let reach = blobs.ask(&many).await;
        assert_eq!(reach.holders[A], 1);
        let gone = probe.asks().iter().filter(|(p, _)| p == "gone").count();
        assert_eq!(gone, 1, "the silent peer was asked {gone} times");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Never more than the concurrency bound in flight.
    #[tokio::test]
    async fn asks_a_few_at_a_time() {
        let peers: Vec<String> = (0..5).map(|i| format!("p{i}")).collect();
        let peer_refs: Vec<&str> = peers.iter().map(String::as_str).collect();
        let probe = Arc::new(FakeProbe::new(&peer_refs));
        let blobs = MeshBlobs::with_bounds(probe.clone(), quick());
        let many: Vec<String> = (0..20).map(|i| format!("{i:064x}")).collect();
        blobs.ask(&many).await;
        assert_eq!(probe.asks().len(), 100);
        assert!(probe.most_in_flight.load(Ordering::SeqCst) <= CONCURRENCY);
    }

    /// The call ends at its deadline whatever is still out; what did not
    /// answer is not remembered, so the next call asks it.
    #[tokio::test]
    async fn the_call_deadline_holds() {
        let probe = Arc::new(FakeProbe::new(&["gone"]).silent("gone"));
        let bounds = Bounds {
            head_timeout: Duration::from_secs(3600),
            call_deadline: Duration::from_millis(50),
            ..Bounds::default()
        };
        let blobs = MeshBlobs::with_bounds(probe.clone(), bounds);
        let started = Instant::now();
        let reach = blobs.ask(&hashes(&[A])).await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(reach.holders[A], 0);
        blobs.ask(&hashes(&[A])).await;
        assert_eq!(probe.asks().len(), 2);
    }

    /// The only URL built is a peer's `.fips` Blossom, from a real npub.
    #[test]
    fn the_url_is_always_on_the_mesh() {
        let npub =
            nostr::nips::nip19::ToBech32::to_bech32(&nostr::Keys::generate().public_key()).unwrap();
        assert_eq!(
            head_url(&npub, A).unwrap(),
            format!("http://{npub}.fips:24243/{A}")
        );
        assert!(head_url("blossom.primal.net", A).is_err());
        assert!(head_url("npub1nope", A).is_err());
        assert!(head_url(&npub, "../upload").is_err());
    }

    /// The probe sends `HEAD` (no body is asked for), reads only the
    /// status, and does not follow a redirect off the mesh.
    #[tokio::test]
    async fn the_http_probe_sends_head_and_follows_no_redirect() {
        use axum::{http::StatusCode, routing::any, Router};
        let methods = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = methods.clone();
        let app = Router::new()
            .route(
                "/held",
                any(move |method: axum::http::Method| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(method.to_string());
                        (StatusCode::OK, vec![0u8; 1 << 20])
                    }
                }),
            )
            .route("/missing", any(|| async { StatusCode::NOT_FOUND }))
            .route("/stranger", any(|| async { StatusCode::FORBIDDEN }))
            .route(
                "/broken",
                any(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route(
                "/away",
                any(|| async {
                    (
                        StatusCode::FOUND,
                        [("location", "https://blossom.example/held")],
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let http = probe_client();
        assert!(head_status(&http, &format!("{base}/held")).await.unwrap());
        assert!(!head_status(&http, &format!("{base}/missing"))
            .await
            .unwrap());
        assert!(!head_status(&http, &format!("{base}/stranger"))
            .await
            .unwrap());
        assert!(head_status(&http, &format!("{base}/broken")).await.is_err());
        assert!(head_status(&http, &format!("{base}/away")).await.is_err());
        assert_eq!(*methods.lock().unwrap(), vec!["HEAD".to_string()]);
    }
}
