//! `IpPeerSource` — the **online fallback** [`PeerSource`]: fetch an externally-
//! authored nsite from **public** relays + Blossom over normal IP. This is the
//! tier-3 source in `docs/design/nsite-layer.md` §5 and, in P2, the way content
//! enters the device: a user pastes `<npub>.nsite.lol` (or a bare npub) and Myco
//! downloads the signed manifest + blobs, verifies, and mirrors them locally so
//! the site then serves offline forever. The FIPS-peer source (P3) implements the
//! same trait; the sync engine doesn't care which.
//!
//! Gated by `sync.offline_only`: when set, no IP source is installed and Myco
//! never reaches the internet (`docs/reference/config.md`).

use std::time::Duration;

use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::{SinkExt, StreamExt};
use nostr::{Event, PublicKey};
use nsite_deck::seams::PeerSource;
use nsite_deck::{kind_for, sha256_hex};
use tokio_tungstenite::tungstenite::Message;

/// A small, sensible default set of public relays that carry nsite manifests.
pub fn default_relays() -> Vec<String> {
    [
        "wss://relay.damus.io",
        "wss://nos.lol",
        "wss://relay.nostr.band",
        "wss://relay.primal.net",
        "wss://purplepag.es",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Default public Blossom servers, tried after a manifest's own `["server",…]`
/// hints.
pub fn default_blossom_servers() -> Vec<String> {
    [
        "https://blossom.primal.net",
        "https://cdn.satellite.earth",
        "https://blossom.band",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Fetches manifests from public relays and blobs from public Blossom over IP.
pub struct IpPeerSource {
    relays: Vec<String>,
    blossom_servers: Vec<String>,
    http: reqwest::Client,
    timeout: Duration,
    /// When true, blob fetches ignore the manifest's `["server", …]` hints and
    /// use only `blossom_servers` — so a **mesh** source never reaches out to
    /// public Blossom over the internet (it stays on the peer's `[fd00::]:24242`).
    ignore_manifest_servers: bool,
}

impl IpPeerSource {
    pub fn new(relays: Vec<String>, blossom_servers: Vec<String>) -> Self {
        Self {
            relays,
            blossom_servers,
            http: reqwest::Client::builder()
                // Generous: a blob can be MBs over a slow BLE mesh link.
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            timeout: Duration::from_secs(8),
            ignore_manifest_servers: false,
        }
    }

    /// Fetch blobs only from this source's own servers, never the manifest's
    /// public `server` hints (used by the mesh source — keep it on the mesh).
    pub fn ignoring_manifest_servers(mut self) -> Self {
        self.ignore_manifest_servers = true;
        self
    }

    /// The defaults (public relays + Blossom).
    pub fn with_defaults() -> Self {
        Self::new(default_relays(), default_blossom_servers())
    }

    /// Override the per-relay timeout (mesh links want a longer one than IP).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// A [`PeerSource`] that pulls from a specific **holder's** embedded relay +
/// Blossom over the FIPS mesh, addressed by the holder's npub: the ULA
/// `fd00:: = fd + node_addr[0..15]` (`PeerIdentity::from_npub`), reached at
/// `ws://[fd00::holder]:4869` / `http://[fd00::holder]:24242`. Requires the
/// app-owned TUN to be up so the IPv6 socket routes over the mesh. A longer
/// timeout than the IP source absorbs BLE latency + first-contact session setup.
pub fn mesh_source_for(holder_npub: &str) -> anyhow::Result<IpPeerSource> {
    let peer = fips::PeerIdentity::from_npub(holder_npub)
        .map_err(|e| anyhow::anyhow!("invalid holder npub {holder_npub}: {e}"))?;
    let ip = peer.address().to_ipv6();
    Ok(IpPeerSource::new(
        vec![format!("ws://[{ip}]:4869")],
        vec![format!("http://[{ip}]:24242")],
    )
    .with_timeout(Duration::from_secs(20))
    .ignoring_manifest_servers())
}

/// Discover the nsite manifests a relay holds — kind 15128 (root) + 35128 (named)
/// — for "nsites around me" (querying a Circle peer's mesh relay). The whole call
/// is hard-bounded by `timeout`; returns signature-verified manifest events. A
/// dead/slow peer relay yields an empty set rather than stalling discovery.
pub async fn discover_manifests(relay_url: &str, timeout: Duration, limit: usize) -> Vec<Event> {
    let filter = serde_json::json!({
        "kinds": [nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
        "limit": limit,
    });
    match tokio::time::timeout(timeout, query_relay(relay_url, filter)).await {
        Ok(Ok(events)) => events.into_iter().filter(|e| e.verify().is_ok()).collect(),
        _ => Vec::new(),
    }
}

/// Publish one signed event to a relay (`["EVENT", …]`), best-effort: connect,
/// send, briefly await the `OK`, close. The whole call is hard-bounded by
/// `timeout` so an unreachable peer relay can't stall the fan-out. Returns whether
/// the event was sent (not whether it was accepted — chat is fire-and-forget).
pub async fn publish_event(
    url: &str,
    event: &nostr::Event,
    event_ttl: u8,
    timeout: Duration,
) -> bool {
    let send = async {
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.ok()?;
        // Carry the hop budget as a transient top-level `event-ttl` field (not a
        // tag; doesn't touch the signature). The peer relay reads it for fan-out
        // and strips it on store. See docs/design/event-gossip.md §2.
        let mut ev_json = serde_json::to_value(event).ok()?;
        if let Some(obj) = ev_json.as_object_mut() {
            obj.insert("event-ttl".to_string(), serde_json::json!(event_ttl));
        }
        let frame = serde_json::json!(["EVENT", ev_json]).to_string();
        ws.send(Message::Text(frame)).await.ok()?;
        // Wait briefly for the OK, but don't depend on it; then close politely.
        let _ = ws.next().await;
        let _ = ws.send(Message::Close(None)).await;
        Some(())
    };
    matches!(tokio::time::timeout(timeout, send).await, Ok(Some(())))
}

/// Query one relay for a single filter, collecting events until EOSE. The whole
/// call (connect + REQ + read) is hard-bounded by a `timeout` at the call site,
/// so a dead relay can't hang the sync on a slow TCP/TLS connect.
pub async fn query_relay(url: &str, filter: serde_json::Value) -> anyhow::Result<Vec<Event>> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await?;
    let req = serde_json::json!(["REQ", "myco", filter]);
    ws.send(Message::Text(req.to_string())).await?;

    let mut events = Vec::new();
    while let Some(msg) = ws.next().await {
        match msg {
            Ok(Message::Text(txt)) => {
                let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) else {
                    continue;
                };
                match val.get(0).and_then(|v| v.as_str()) {
                    Some("EVENT") => {
                        if let Some(ev) = val.get(2) {
                            if let Ok(event) = serde_json::from_value::<Event>(ev.clone()) {
                                events.push(event);
                            }
                        }
                    }
                    Some("EOSE") | Some("CLOSED") => break,
                    _ => {}
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {} // ping/pong/binary
        }
    }
    let _ = ws.send(Message::Close(None)).await;
    Ok(events)
}

#[async_trait]
impl PeerSource for IpPeerSource {
    async fn fetch_manifest(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let kind = kind_for(d_tag);
        let mut filter = serde_json::json!({
            "kinds": [kind],
            "authors": [hex::encode(author.to_bytes())],
            "limit": 1,
        });
        if let Some(d) = d_tag {
            filter["#d"] = serde_json::json!([d]);
        }

        // Each relay is hard-bounded by `self.timeout` (connect + read), so a dead
        // relay can't stall the whole sync on a slow TCP/TLS connect; the rest
        // still answer. A timeout/error yields an empty set for that relay.
        let queries = self.relays.iter().map(|url| async {
            match tokio::time::timeout(self.timeout, query_relay(url, filter.clone())).await {
                Ok(Ok(events)) => events,
                _ => Vec::new(),
            }
        });
        let results = join_all(queries).await;

        // Pick the newest event that verifies and matches the requested slot.
        let mut newest: Option<Event> = None;
        for events in results.into_iter() {
            for ev in events {
                if ev.pubkey != *author || ev.kind.as_u16() != kind {
                    continue;
                }
                if d_tag.is_some() && event_d_tag(&ev).as_deref() != d_tag {
                    continue;
                }
                if ev.verify().is_err() {
                    continue;
                }
                if newest.as_ref().is_none_or(|n| ev.created_at > n.created_at) {
                    newest = Some(ev);
                }
            }
        }
        Ok(newest)
    }

    async fn fetch_blob(
        &self,
        sha256_hex_want: &str,
        servers: &[String],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        // Manifest hints first, then this source's own servers (deduped). A mesh
        // source skips the manifest's public hints entirely (stay on the mesh).
        let mut candidates: Vec<String> =
            if self.ignore_manifest_servers { Vec::new() } else { servers.to_vec() };
        for s in &self.blossom_servers {
            if !candidates.contains(s) {
                candidates.push(s.clone());
            }
        }

        for server in candidates {
            let url = format!("{}/{}", server.trim_end_matches('/'), sha256_hex_want);
            let resp = match self.http.get(&url).send().await {
                Ok(r) if r.status().is_success() => r,
                _ => continue,
            };
            let Ok(bytes) = resp.bytes().await else {
                continue;
            };
            // Self-authenticating: only accept bytes that hash to the wanted name.
            if sha256_hex(&bytes) == sha256_hex_want {
                return Ok(Some(bytes.to_vec()));
            }
        }
        Ok(None)
    }
}

fn event_d_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d"))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A mock relay: accept one WS connection, read the REQ, reply with the given
    /// event then EOSE. Returns the `ws://` URL.
    async fn mock_relay(event_json: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                // Read the REQ (ignore contents; the test filter always matches).
                if let Some(Ok(Message::Text(_req))) = ws.next().await {
                    let event = serde_json::json!(["EVENT", "myco", serde_json::from_str::<serde_json::Value>(&event_json).unwrap()]);
                    ws.send(Message::Text(event.to_string())).await.unwrap();
                    ws.send(Message::Text(serde_json::json!(["EOSE", "myco"]).to_string()))
                        .await
                        .unwrap();
                }
            }
        });
        format!("ws://{addr}")
    }

    /// A mock Blossom: serve `GET /<hash>` from a (hash -> bytes) map. Returns the
    /// `http://` base URL.
    async fn mock_blossom(blobs: Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let map: Arc<std::collections::HashMap<String, Vec<u8>>> =
            Arc::new(blobs.into_iter().collect());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let map = map.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    // "GET /<hash> HTTP/1.1"
                    let hash = req
                        .split_whitespace()
                        .nth(1)
                        .map(|p| p.trim_start_matches('/').to_string())
                        .unwrap_or_default();
                    let resp = match map.get(&hash) {
                        Some(bytes) => {
                            let mut r = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                bytes.len()
                            )
                            .into_bytes();
                            r.extend_from_slice(bytes);
                            r
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = stream.write_all(&resp).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn publish_event_delivers_to_relay() {
        // End-to-end against a real myco-relay: publish a chat event, then confirm
        // it landed in the store.
        let store = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(myco_relay::server::serve_on(store.clone(), listener));

        let keys = nostr::Keys::generate();
        let ev = nostr::EventBuilder::new(nostr::Kind::from(9u16), "hi over mesh")
            .tags([nostr::Tag::identifier("mesh".to_string())])
            .sign_with_keys(&keys)
            .unwrap();

        assert!(publish_event(&format!("ws://{addr}"), &ev, 3, Duration::from_secs(5)).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(store.count(), 1, "published event stored by the relay");
    }

    #[tokio::test]
    async fn fetches_manifest_and_blob_over_ip() {
        let site = nsite_deck::testing::build_test_site(
            &[("/index.html", b"<h1>online</h1>")],
            None,
            Some("Online Site"),
        );
        let event_json = serde_json::to_string(&site.manifest).unwrap();

        let relay_url = mock_relay(event_json).await;
        let blossom_url = mock_blossom(site.blobs.clone()).await;

        let source = IpPeerSource::new(vec![relay_url], vec![blossom_url]);

        // Manifest comes back, verified, matching the author.
        let got = source.fetch_manifest(&site.author, None).await.unwrap();
        assert_eq!(got.map(|e| e.id), Some(site.manifest.id));

        // Blob comes back, hash-verified.
        let (hash, bytes) = &site.blobs[0];
        let blob = source.fetch_blob(hash, &[]).await.unwrap();
        assert_eq!(blob.as_deref(), Some(bytes.as_slice()));

        // A wrong hash yields nothing.
        let miss = source
            .fetch_blob("00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff", &[])
            .await
            .unwrap();
        assert_eq!(miss, None);
    }

    #[tokio::test]
    async fn discover_manifests_reads_kind_filtered_sites() {
        // A signed manifest advertised on a peer's (mock) mesh relay.
        let site = nsite_deck::testing::build_test_site(
            &[("/index.html", b"<h1>around me</h1>")],
            None,
            Some("Nearby Site"),
        );
        let relay_url = mock_relay(serde_json::to_string(&site.manifest).unwrap()).await;

        let events = discover_manifests(&relay_url, Duration::from_secs(5), 50).await;
        assert_eq!(events.len(), 1, "discovery returns the verified manifest");
        let m = nsite_deck::Manifest::from_event(events.into_iter().next().unwrap()).unwrap();
        assert_eq!(m.title.as_deref(), Some("Nearby Site"));
        assert_eq!(m.author, site.author);
    }

    /// Live network check against a real public nsite (the link the user gave).
    /// Ignored by default; run with:
    /// `cargo test -p myco-core fetch_real -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "hits the public internet"]
    async fn fetch_real_nsite_over_ip() {
        let link = "https://npub1apgedl4jczacut0dasn0mszyyhhxzlvjcshjkczms47nt2d4eymsku78ws.nsite.lol/";
        let addr = nsite_deck::parse_link(link).expect("parse link");

        let source = IpPeerSource::with_defaults();
        let manifest = source
            .fetch_manifest(&addr.author, addr.d_tag.as_deref())
            .await
            .expect("fetch ok")
            .expect("manifest found on public relays");
        let m = nsite_deck::Manifest::from_event(manifest).expect("parse manifest");
        println!(
            "manifest: title={:?}, {} paths",
            m.title,
            m.paths.len()
        );
        assert!(!m.paths.is_empty(), "manifest should map at least one path");

        // Pull + verify the index blob (or the first path).
        let (path, hash) = m
            .paths
            .iter()
            .find(|(p, _)| p.as_str() == "/index.html")
            .or_else(|| m.paths.iter().next())
            .expect("at least one path");
        let blob = source
            .fetch_blob(hash, &m.servers)
            .await
            .expect("blob fetch ok")
            .unwrap_or_else(|| panic!("blob for {path} not found on any Blossom"));
        println!("fetched {path} ({} bytes), sha256 verified", blob.len());
    }

    /// Full path: a Content layer with the IP source installed syncs a pasted
    /// link to `ready`, then serves it locally.
    #[tokio::test]
    async fn open_site_syncs_from_ip_source() {
        use crate::content::Content;
        use nostr::nips::nip19::ToBech32;

        let site = nsite_deck::testing::build_test_site(
            &[("/index.html", b"hello from the internet")],
            None,
            None,
        );
        let relay_url = mock_relay(serde_json::to_string(&site.manifest).unwrap()).await;
        let blossom_url = mock_blossom(site.blobs.clone()).await;

        let dir = std::env::temp_dir().join(format!("myco-ip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        content.set_source(Arc::new(IpPeerSource::new(vec![relay_url], vec![blossom_url])));

        let addr = nsite_deck::SiteAddr {
            author: site.author,
            d_tag: None,
        };
        content.clone().open_site(addr, None).await;

        let sites = content.sites_snapshot();
        assert_eq!(sites[0].state, "ready", "site should sync to ready over IP");

        let host = format!("{}.nsite", site.author.to_bech32().unwrap());
        let resp = content.gateway_get(&host, "/", None).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello from the internet");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
