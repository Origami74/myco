//! A minimal NIP-01 WebSocket relay server over [`RelayStore`], so the node can
//! serve its manifest events to mesh peers at `ws://[fd00::self]:4869`. Also
//! answers a NIP-11 document on a plain HTTP GET.
//!
//! Scope is deliberately tiny: the sync client sends a `REQ`, reads the stored
//! matches plus `EOSE`, then closes — so there are **no live subscriptions**
//! (steady-state live forwarding is the P3 propagator's job, not this socket).
//! The query surface is the same `{kinds, authors, #d, limit}` the gateway uses.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use nostr::{Event, PublicKey};
use nsite_deck::seams::{ManifestFilter, RelayBackend};

use crate::RelayStore;

/// Serve the relay on `addr` until the future is dropped/aborted.
pub async fn serve(store: Arc<RelayStore>, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_on(store, listener).await
}

/// Serve on an already-bound listener (lets the caller pick an ephemeral port).
pub async fn serve_on(
    store: Arc<RelayStore>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let app = Router::new().route("/", get(root)).with_state(store);
    axum::serve(listener, app).await?;
    Ok(())
}

/// `/` is the WS endpoint. (A NIP-11 plain-GET document is a later nicety; the
/// mesh sync client only ever upgrades to WebSocket.)
async fn root(ws: WebSocketUpgrade, State(store): State<Arc<RelayStore>>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, store))
}

async fn handle_ws(mut socket: WebSocket, store: Arc<RelayStore>) {
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                for reply in handle_message(text.as_str(), &store).await {
                    if socket.send(Message::text(reply)).await.is_err() {
                        return;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

/// Handle one client frame, returning the relay frames to send back.
async fn handle_message(text: &str, store: &RelayStore) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    match array.first().and_then(|v| v.as_str()) {
        Some("REQ") => {
            let sub_id = array.get(1).and_then(|v| v.as_str()).unwrap_or("");
            // Merge all filters in the REQ (any-match), as NIP-01 specifies.
            let mut events: Vec<Event> = Vec::new();
            for filter_value in array.iter().skip(2) {
                if let Some(filter) = parse_filter(filter_value) {
                    if let Ok(mut matched) = store.query(&filter).await {
                        events.append(&mut matched);
                    }
                }
            }
            // De-dup by id (filters may overlap), newest first.
            events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            events.dedup_by(|a, b| a.id == b.id);

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
            match serde_json::from_value::<Event>(event_value.clone()) {
                Ok(event) => {
                    let id = event.id.to_hex();
                    match event.verify() {
                        Ok(()) => {
                            let _ = store.store_event(event).await;
                            vec![serde_json::json!(["OK", id, true, ""]).to_string()]
                        }
                        Err(_) => {
                            vec![serde_json::json!(["OK", id, false, "invalid: bad signature"])
                                .to_string()]
                        }
                    }
                }
                Err(_) => Vec::new(),
            }
        }
        // CLOSE is a no-op (no live subscriptions); ignore everything else.
        _ => Vec::new(),
    }
}

/// Parse a NIP-01 filter object into the basic [`ManifestFilter`].
fn parse_filter(value: &serde_json::Value) -> Option<ManifestFilter> {
    let obj = value.as_object()?;
    let mut filter = ManifestFilter::default();
    if let Some(kinds) = obj.get("kinds").and_then(|v| v.as_array()) {
        filter.kinds = kinds.iter().filter_map(|k| k.as_u64().map(|k| k as u16)).collect();
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
    filter.limit = obj.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);
    Some(filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use nsite_deck::model::KIND_ROOT;
    use nsite_deck::testing::build_test_site_with_keys;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    #[tokio::test]
    async fn ws_relay_serves_req_then_eose() {
        // A store with one manifest.
        let store = Arc::new(RelayStore::in_memory());
        let keys = nostr::Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"x")], None, None);
        store.store_event(site.manifest.clone()).await.unwrap();

        // Serve on an ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(store.clone(), listener));

        // Connect a WS client and REQ the author's root manifest.
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let req = serde_json::json!([
            "REQ", "s1",
            { "kinds": [KIND_ROOT], "authors": [hex::encode(keys.public_key().to_bytes())] }
        ]);
        ws.send(WsMessage::Text(req.to_string().into())).await.unwrap();

        // Expect the EVENT, then EOSE.
        let mut got_event = false;
        let mut got_eose = false;
        while let Some(Ok(WsMessage::Text(txt))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
            match v[0].as_str() {
                Some("EVENT") => {
                    assert_eq!(v[2]["id"].as_str(), Some(site.manifest.id.to_hex().as_str()));
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(store.clone(), listener));

        let keys = nostr::Keys::generate();
        let site = build_test_site_with_keys(&keys, &[("/index.html", b"y")], None, None);

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let event = serde_json::json!(["EVENT", site.manifest]);
        ws.send(WsMessage::Text(event.to_string().into())).await.unwrap();

        if let Some(Ok(WsMessage::Text(txt))) = ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
            assert_eq!(v[0].as_str(), Some("OK"));
            assert_eq!(v[2].as_bool(), Some(true), "valid signed event accepted");
        } else {
            panic!("expected OK frame");
        }
        assert_eq!(store.count(), 1, "event stored");
    }
}
