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
//! `ttl` lasts. The client side is the same thing dialled: send one event, or
//! pull a backlog until `EOSE`.
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

/// The relay URLs a link's peer offers, most preferred first: FIPS first,
/// as it needs no internet.
pub fn relay_urls(link: &Link) -> Vec<String> {
    let mut urls: Vec<String> = link
        .services()
        .into_iter()
        .filter(|s| s.kind == "relay")
        .map(|s| s.url)
        .collect();
    urls.sort_by_key(|u| !u.contains(".fips"));
    urls
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
async fn open(link: &Link, url: &str) -> anyhow::Result<Socket> {
    let auth = http_auth(&link.keys(), "GET", &signed_form(url));
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
        Err(_) => tokio::net::lookup_host((host.as_str(), port))
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("{host} does not resolve"))?,
    };
    let stream = crate::mesh_net::dial(addr).await?;
    let mut request = target.into_client_request()?;
    request.headers_mut().insert("authorization", auth.parse()?);
    let (ws, _) = tokio_tungstenite::client_async(request, stream).await?;
    Ok(ws)
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
            let mut events = Vec::new();
            while let Some(frame) = ws.next().await {
                let Message::Text(text) = frame? else {
                    continue;
                };
                let v: serde_json::Value = serde_json::from_str(&text)?;
                match v[0].as_str() {
                    Some("EVENT") => {
                        if let Ok(e) = serde_json::from_value::<Event>(v[2].clone()) {
                            if e.verify().is_ok() {
                                events.push(e);
                            }
                        }
                    }
                    Some("EOSE") | Some("CLOSED") => break,
                    _ => {}
                }
            }
            let _ = ws.close(None).await;
            anyhow::Ok(events)
        })
        .await;
        match attempt {
            Ok(Ok(events)) => return Ok(events),
            Ok(Err(e)) => last = e,
            Err(_) => last = anyhow::anyhow!("{url}: timed out"),
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
}
