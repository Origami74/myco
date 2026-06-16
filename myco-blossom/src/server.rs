//! The BUD-01 Blossom HTTP server over [`FsBlobStore`], so the node can serve its
//! blobs to mesh peers at `http://[fd00::self]:24242`. No auth on the mesh-local
//! server (the blob hash is self-authenticating). Routes:
//!
//! - `GET  /<sha256>` → blob bytes (`application/octet-stream`; the gateway infers
//!   the real content-type from the manifest path, not from Blossom).
//! - `HEAD /<sha256>` → 200 if present, 404 if not (the "all blobs present?" probe).
//! - `PUT  /upload`    → store the body keyed by `sha256(body)`, return a descriptor.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use nsite_deck::seams::BlobStore;

use crate::FsBlobStore;

/// Serve Blossom on `addr` until the future is dropped/aborted.
pub async fn serve(store: Arc<FsBlobStore>, addr: SocketAddr) -> anyhow::Result<()> {
    serve_on(store, bind(addr)?).await
}

/// Bind a listener for Blossom. IPv6 binds are **`IPV6_V6ONLY`** so `[::]:port`
/// does not collide with a `127.0.0.1:port` squatter (the mesh is IPv6-only).
/// Returns the bind error so the caller can warn the user. Call within a runtime.
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

/// Serve on an already-bound listener (lets the caller pick an ephemeral port).
pub async fn serve_on(
    store: Arc<FsBlobStore>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/{sha256}", get(get_blob).head(head_blob))
        .route("/upload", put(upload))
        .with_state(store);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Strip an optional extension (`<sha256>.png`) — some clients append one.
fn blob_hash(raw: &str) -> String {
    raw.split('.').next().unwrap_or(raw).to_ascii_lowercase()
}

async fn get_blob(Path(sha256): Path<String>, State(store): State<Arc<FsBlobStore>>) -> Response {
    match store.get(&blob_hash(&sha256)).await {
        Ok(Some(bytes)) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn head_blob(Path(sha256): Path<String>, State(store): State<Arc<FsBlobStore>>) -> StatusCode {
    if store.has(&blob_hash(&sha256)).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn upload(State(store): State<Arc<FsBlobStore>>, body: Bytes) -> Response {
    match store.put(&body).await {
        Ok(sha256) => {
            let descriptor = serde_json::json!({ "sha256": sha256, "size": body.len() });
            (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                descriptor.to_string(),
            )
                .into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn http_blossom_get_head_upload() {
        let dir =
            std::env::temp_dir().join(format!("myco-blossom-srv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(FsBlobStore::open(&dir).unwrap());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(store.clone(), listener));

        let base = format!("http://{addr}");
        let client = reqwest::Client::new();

        // Upload, then GET it back by the returned hash.
        let body = b"hello mesh blossom".to_vec();
        let resp = client
            .put(format!("{base}/upload"))
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let descriptor: serde_json::Value =
            serde_json::from_str(&resp.text().await.unwrap()).unwrap();
        let hash = descriptor["sha256"].as_str().unwrap().to_string();

        let got = client.get(format!("{base}/{hash}")).send().await.unwrap();
        assert_eq!(got.status(), 200);
        assert_eq!(got.bytes().await.unwrap().as_ref(), body.as_slice());

        // HEAD present vs absent.
        let head_ok = client.head(format!("{base}/{hash}")).send().await.unwrap();
        assert_eq!(head_ok.status(), 200);
        let head_miss = client
            .head(format!("{base}/{}", "00".repeat(32)))
            .send()
            .await
            .unwrap();
        assert_eq!(head_miss.status(), 404);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
