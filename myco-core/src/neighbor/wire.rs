//! Neighborhood traffic over links: a NIP-01 relay per link and neighborhood.
//!
//! ```text
//! ws://<base>/nh/{pk}/relay?n=<neighborhood id>&ttl=<hops>
//!   Authorization: NIP-98 by the peer's link key, `u` = the URL in http form
//! ```
//!
//! The server side admits only a link that carries `n`, then speaks plain
//! NIP-01 scoped to that neighborhood: `REQ` answers from its view, `EVENT`
//! is accepted into it and passed on to the other links carrying it while
//! `ttl` lasts. A `REQ` stays open after `EOSE`: new events in the
//! neighborhood that match it follow until `CLOSE` or the socket ends. The
//! client side is the same thing dialled: send one event, pull a backlog
//! until `EOSE`, or follow a subscription for as long as it is wanted.
//!
//! One connection per exchange for now. Links carry little until they carry
//! a lot; a pool per link can come when they do.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use nostr::{Event, Filter};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::Message;

use super::id::NeighborhoodId;
use super::link::{http_auth, Link};

/// How long one send or pull may take end to end.
pub const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(20);
/// The largest message or frame read from a peer's relay.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// The most events one pull takes from a peer.
pub const MAX_PULL_EVENTS: usize = 2000;

/// The relay URLs a link's peer offers, most preferred first: FIPS first,
/// as it needs no internet. Only ones this device may dial: a mesh peer or
/// a public host, never its own network (see [`crate::http::may_dial_peer`]).
pub fn relay_urls(link: &Link) -> Vec<String> {
    let mut urls: Vec<String> = link
        .services()
        .into_iter()
        .filter(|s| s.kind == "relay")
        .map(|s| s.url)
        .filter(|u| dialable(u))
        .collect();
    urls.sort_by_key(|u| !u.contains(".fips"));
    urls.truncate(super::service::MAX_LINK_URLS);
    urls
}

/// Whether a peer-supplied relay URL may be dialled, judged in http form.
fn dialable(url: &str) -> bool {
    reqwest::Url::parse(&signed_form(url))
        .map_err(|e| e.to_string())
        .and_then(|u| crate::http::may_dial_peer(&u))
        .inspect_err(|why| tracing::debug!(url, why, "peer relay URL not dialled"))
        .is_ok()
}

/// The URL for neighborhood `n` with `ttl` hops on a relay URL.
pub fn scoped(url: &str, n: NeighborhoodId, ttl: u8) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}n={}&ttl={ttl}", n.to_hex())
}

/// The URL NIP-98 signs for a WebSocket: the same URL in http form.
pub fn signed_form(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

type Socket = tokio_tungstenite::WebSocketStream<crate::mesh_net::MeshStream>;

/// Open a link-signed WebSocket to `url`.
pub(super) async fn open(link: &Link, url: &str) -> anyhow::Result<Socket> {
    if !dialable(url) {
        anyhow::bail!("{url}: not dialled");
    }
    let auth = http_auth(&link.keys(), "GET", &signed_form(url), &[]);
    let target = if url.contains(".fips") {
        crate::dns_intercept::mesh_literal_url(url)
            .await
            .map_err(|_| anyhow::anyhow!("the mesh is not up yet"))?
    } else {
        url.to_string()
    };
    let parsed = reqwest::Url::parse(&target)?;
    let host = parsed
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string())
        .ok_or_else(|| anyhow::anyhow!("no host in {url}"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("no port in {url}"))?;
    let addr = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, port),
        Err(_) => {
            // Judged on what the name resolves to, the address then dialled:
            // a name that rebinds to this device's network is refused.
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
                .await?
                .collect();
            if addrs.iter().any(|a| !crate::http::may_connect_peer(a.ip())) {
                anyhow::bail!("{host}: resolves outside the mesh and the public internet");
            }
            addrs
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("{host} does not resolve"))?
        }
    };
    if !crate::http::may_connect_peer(addr.ip()) {
        anyhow::bail!("{url}: not dialled");
    }
    let stream = crate::mesh_net::dial(addr).await?;
    let mut request = target.into_client_request()?;
    request.headers_mut().insert("authorization", auth.parse()?);
    let (ws, _) =
        tokio_tungstenite::client_async_with_config(request, stream, Some(client_config())).await?;
    Ok(ws)
}

/// What a peer's relay may send us: messages and frames of at most
/// [`MAX_MESSAGE`].
fn client_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE),
        max_frame_size: Some(MAX_MESSAGE),
        ..Default::default()
    }
}

/// Send one event into `n` over `link`, with `ttl` hops left for the peer to
/// pass it on. `Ok(true)` when the peer took it.
pub async fn send(link: &Link, n: NeighborhoodId, ttl: u8, event: &Event) -> anyhow::Result<bool> {
    let mut last = anyhow::anyhow!("the peer offers no relay");
    for url in relay_urls(link) {
        let attempt = tokio::time::timeout(EXCHANGE_TIMEOUT, async {
            let mut ws = open(link, &scoped(&url, n, ttl)).await?;
            ws.send(Message::text(
                serde_json::json!(["EVENT", event]).to_string(),
            ))
            .await?;
            while let Some(frame) = ws.next().await {
                let Message::Text(text) = frame? else {
                    continue;
                };
                let v: serde_json::Value = serde_json::from_str(&text)?;
                if v[0] == "OK" && v[1] == event.id.to_hex() {
                    let _ = ws.close(None).await;
                    return Ok(v[2].as_bool().unwrap_or(false));
                }
            }
            anyhow::Ok(false)
        })
        .await;
        match attempt {
            Ok(Ok(took)) => return Ok(took),
            Ok(Err(e)) => last = e,
            Err(_) => last = anyhow::anyhow!("{url}: timed out"),
        }
    }
    Err(last)
}

/// Pull what the peer holds in `n` matching `filters`, until `EOSE`.
pub async fn pull(
    link: &Link,
    n: NeighborhoodId,
    filters: &[Filter],
) -> anyhow::Result<Vec<Event>> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    pull_into(link, n, filters, tx).await?;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    Ok(events)
}

/// As [`pull`], streaming: each verified event goes to `tx` the moment it
/// arrives, not at `EOSE`. Returns how many were sent; at most
/// [`MAX_PULL_EVENTS`] are read.
pub async fn pull_into(
    link: &Link,
    n: NeighborhoodId,
    filters: &[Filter],
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> anyhow::Result<usize> {
    let mut last = anyhow::anyhow!("the peer offers no relay");
    for url in relay_urls(link) {
        let attempt = tokio::time::timeout(EXCHANGE_TIMEOUT, async {
            let mut ws = open(link, &scoped(&url, n, 0)).await?;
            let mut req = vec![serde_json::json!("REQ"), serde_json::json!("pull")];
            req.extend(
                filters
                    .iter()
                    .map(|f| serde_json::to_value(f).unwrap_or_default()),
            );
            ws.send(Message::text(serde_json::Value::Array(req).to_string()))
                .await?;
            let mut sent = 0usize;
            let mut read = 0usize;
            while let Some(frame) = ws.next().await {
                let Message::Text(text) = frame? else {
                    continue;
                };
                let v: serde_json::Value = serde_json::from_str(&text)?;
                match v[0].as_str() {
                    Some("EVENT") => {
                        read += 1;
                        if read > MAX_PULL_EVENTS {
                            tracing::debug!(url, "pull cut at {MAX_PULL_EVENTS} events");
                            break;
                        }
                        if let Ok(e) = serde_json::from_value::<Event>(v[2].clone()) {
                            if e.verify().is_ok() && tx.send(e).is_ok() {
                                sent += 1;
                            }
                        }
                    }
                    Some("EOSE") | Some("CLOSED") => break,
                    _ => {}
                }
            }
            let _ = ws.close(None).await;
            anyhow::Ok(sent)
        })
        .await;
        match attempt {
            Ok(Ok(sent)) => return Ok(sent),
            Ok(Err(e)) => last = e,
            Err(_) => last = anyhow::anyhow!("{url}: timed out"),
        }
    }
    Err(last)
}

/// Hold a subscription open on the peer: what it holds in `n` matching
/// `filters`, then whatever it gets later, each verified event to `tx` as it
/// arrives. Returns when the peer or the network ends it, or `tx` is closed.
/// `on_open` is called once the subscription is up.
pub async fn follow(
    link: &Link,
    n: NeighborhoodId,
    filters: &[Filter],
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    on_open: impl FnOnce(),
) -> anyhow::Result<()> {
    let mut last = anyhow::anyhow!("the peer offers no relay");
    for url in relay_urls(link) {
        let opened = tokio::time::timeout(EXCHANGE_TIMEOUT, async {
            let mut ws = open(link, &scoped(&url, n, 0)).await?;
            let mut req = vec![serde_json::json!("REQ"), serde_json::json!("live")];
            req.extend(
                filters
                    .iter()
                    .map(|f| serde_json::to_value(f).unwrap_or_default()),
            );
            ws.send(Message::text(serde_json::Value::Array(req).to_string()))
                .await?;
            anyhow::Ok(ws)
        })
        .await;
        let mut ws = match opened {
            Ok(Ok(ws)) => ws,
            Ok(Err(e)) => {
                last = e;
                continue;
            }
            Err(_) => {
                last = anyhow::anyhow!("{url}: timed out");
                continue;
            }
        };
        on_open();
        loop {
            tokio::select! {
                frame = ws.next() => {
                    let Some(frame) = frame else { return Ok(()) };
                    let Message::Text(text) = frame? else {
                        continue;
                    };
                    let v: serde_json::Value = serde_json::from_str(&text)?;
                    match v[0].as_str() {
                        Some("EVENT") => {
                            if let Ok(e) = serde_json::from_value::<Event>(v[2].clone()) {
                                if e.verify().is_ok() && tx.send(e).is_err() {
                                    let _ = ws.close(None).await;
                                    return Ok(());
                                }
                            }
                        }
                        Some("CLOSED") => return Ok(()),
                        _ => {}
                    }
                }
                _ = tx.closed() => {
                    let _ = ws.close(None).await;
                    return Ok(());
                }
            }
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_scoped_and_signed_in_http_form() {
        let n = NeighborhoodId::from_name("x");
        let u = scoped("ws://a.fips:4873/nh/pk/relay", n, 2);
        assert_eq!(
            u,
            format!("ws://a.fips:4873/nh/pk/relay?n={}&ttl=2", n.to_hex())
        );
        assert_eq!(signed_form(&u), u.replacen("ws://", "http://", 1));
        assert_eq!(signed_form("wss://r.example/x"), "https://r.example/x");
    }

    #[test]
    fn only_mesh_and_public_relay_urls_are_dialled() {
        use super::super::link::{sign_offer, Links, OfferContent, Service};
        let d = std::env::temp_dir().join(format!("myco-wire-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        let links = Links::open(&d);
        let relay = |url: &str| Service {
            kind: "relay".into(),
            url: url.into(),
            read: true,
            write: true,
            own: false,
        };
        let npub = {
            use nostr::nips::nip19::ToBech32;
            nostr::Keys::generate().public_key().to_bech32().unwrap()
        };
        let fips = format!("ws://{npub}.fips:4873/nh/x/relay");
        let offered = [
            "ws://8.8.8.8:4873/nh/x/relay",
            "ws://[fd12::1]:4873/nh/x/relay",
            "wss://relay.example.com/nh/x/relay",
            fips.as_str(),
            // Refused: this device's networks and the reserved ranges.
            "ws://10.0.0.1:4873/",
            "ws://172.16.3.4:4873/",
            "ws://192.168.1.1:4873/",
            "ws://100.64.0.1:4873/",
            "ws://169.254.1.1:4873/",
            "ws://0.0.0.0:4873/",
            "ws://255.255.255.255:4873/",
            "ws://224.0.0.1:4873/",
            "ws://192.0.2.1:4873/",
            "ws://[fe80::1]:4873/",
            "ws://[fc00::1]:4873/",
            "ws://[::]:4873/",
            "ws://[ff02::1]:4873/",
            "ws://[2001:db8::1]:4873/",
            "ws://[::ffff:10.0.0.1]:4873/",
            "ws://[::ffff:192.168.0.1]:4873/",
            "ws://router.lan:4873/",
            "ws://intranet/",
            "ws://user:pw@relay.example.com/",
            "file:///etc/passwd",
        ];
        let content = OfferContent {
            services: offered.iter().map(|u| relay(u)).collect(),
        };
        let peer = nostr::Keys::generate();
        let link = links
            .add_ephemeral(
                &nostr::Keys::generate(),
                sign_offer(&peer, &content, None),
                Default::default(),
                "device",
            )
            .unwrap();
        assert_eq!(
            relay_urls(&link),
            [
                fips.as_str(),
                "ws://8.8.8.8:4873/nh/x/relay",
                "ws://[fd12::1]:4873/nh/x/relay",
                "wss://relay.example.com/nh/x/relay",
            ]
        );
    }
}
