//! The BUD-01 Blossom HTTP server over any [`BlobStore`] (the embedded
//! [`FsBlobStore`], or a view that also reads a cache), so the node can serve its
//! blobs to mesh peers at `http://[fd00::self]:24243`. Blobs are
//! self-authenticating, so there is no per-blob auth; access is decided per
//! request by an [`AccessFn`] taking the [`BlobOp`], which is how `myco-core`
//! grants reads to paired peers while withholding uploads. Routes:
//!
//! - `GET  /<sha256>` → blob bytes (`application/octet-stream`; the gateway infers
//!   the real content-type from the manifest path, not from Blossom). A client
//!   sending `Accept-Encoding: zstd` gets the precompressed variant instead
//!   when one is stored ([`FsBlobStore::store_zstd`]), as
//!   `Content-Encoding: zstd`; the bytes it decodes to are the blob.
//! - `HEAD /<sha256>` → 200 if present, 404 if not (the "all blobs present?" probe).
//! - `PUT  /upload`    → store the body keyed by `sha256(body)`, return a descriptor.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use nsite_deck::seams::BlobStore;

#[cfg(doc)]
use crate::FsBlobStore;

/// What a request wants to do, so a gate can allow reads while refusing writes.
/// Uploads cost us disk and nothing in normal operation needs them (propagation
/// is pull-based), so the two are worth separating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobOp {
    /// `GET` / `HEAD` a blob.
    Read,
    /// `PUT /upload`.
    Write,
}

/// Decides whether a non-loopback (mesh) source may perform `op` on our Blossom.
/// `myco-core` backs this with the Circle and its per-peer permissions, so only
/// paired peers can pull, and only peers granted upload can push; loopback (the
/// in-app gateway) always bypasses it. Behind an `Arc` so the **live** Circle is
/// consulted per request — membership and permissions change at runtime.
pub type AccessFn = Arc<dyn Fn(IpAddr, BlobOp) -> bool + Send + Sync>;

/// Shared handler state: the blob store plus an optional mesh access gate.
#[derive(Clone)]
struct BlossomState {
    store: Arc<dyn BlobStore>,
    access: Option<AccessFn>,
    /// Where zstd variants are looked for ([`FsBlobStore::zstd_dir`]).
    precompressed: Option<PathBuf>,
}

impl BlossomState {
    /// Loopback is always allowed; a mesh source must pass the gate (if one is
    /// set) for the operation it is attempting.
    fn allows(&self, addr: SocketAddr, op: BlobOp) -> bool {
        let ip = addr.ip();
        ip.is_loopback() || self.access.as_ref().is_none_or(|a| a(ip, op))
    }
}

/// Serve Blossom on `addr` until the future is dropped/aborted.
pub async fn serve(store: Arc<dyn BlobStore>, addr: SocketAddr) -> anyhow::Result<()> {
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

/// Serve on an already-bound listener with **no** access gate — every source is
/// served (the local/test path; an ephemeral port the caller picked).
pub async fn serve_on(
    store: Arc<dyn BlobStore>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    serve_state(
        BlossomState {
            store,
            access: None,
            precompressed: None,
        },
        listener,
    )
    .await
}

/// Serve on an already-bound listener, restricting **mesh** sources to those that
/// pass `access` (loopback is always allowed). The runtime uses this so only
/// paired (Circle) peers can pull/push blobs. `precompressed` is the store's
/// [`FsBlobStore::zstd_dir`], when it keeps zstd variants.
///
/// Any listener that names its peers by socket address will do — a kernel
/// socket, or an embedder's userspace stack.
pub async fn serve_on_guarded<L>(
    store: Arc<dyn BlobStore>,
    listener: L,
    access: AccessFn,
    precompressed: Option<PathBuf>,
) -> anyhow::Result<()>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    serve_state(
        BlossomState {
            store,
            access: Some(access),
            precompressed,
        },
        listener,
    )
    .await
}

async fn serve_state<L>(state: BlossomState, listener: L) -> anyhow::Result<()>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    // Raise the request-body cap from axum's 2 MB default: nsite assets and the
    // adaptive dev speedtest (up to 16 MiB) exceed it. 64 MiB is a generous cap
    // that still bounds a single upload.
    const MAX_UPLOAD: usize = 64 * 1024 * 1024;
    let app = Router::new()
        .route("/{sha256}", get(get_blob).head(head_blob))
        .route(
            "/upload",
            put(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD)),
        )
        .with_state(state);
    // Connect-info gives each request's source address, so the gate can tell a
    // loopback (in-app gateway) request from a mesh peer's. `tap_io` is what
    // lets axum read it off any listener, not only its own `TcpListener`.
    axum::serve(
        axum::serve::ListenerExt::tap_io(listener, |_: &mut L::Io| {}),
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Strip an optional extension (`<sha256>.png`) — some clients append one.
fn blob_hash(raw: &str) -> String {
    raw.split('.').next().unwrap_or(raw).to_ascii_lowercase()
}

async fn get_blob(
    Path(sha256): Path<String>,
    State(st): State<BlossomState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !st.allows(addr, BlobOp::Read) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let hash = blob_hash(&sha256);
    if let Some(variant) = st
        .precompressed
        .as_ref()
        .filter(|_| accepts_zstd(&headers) && is_hash(&hash))
        .map(|dir| dir.join(&hash))
    {
        if let Ok(bytes) = tokio::fs::read(&variant).await {
            return (
                [
                    (header::CONTENT_TYPE, "application/octet-stream"),
                    (header::CONTENT_ENCODING, "zstd"),
                    (header::VARY, "Accept-Encoding"),
                ],
                bytes,
            )
                .into_response();
        }
    }
    match st.store.get(&hash).await {
        Ok(Some(bytes)) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Whether the request lists `zstd` in `Accept-Encoding` (with a non-zero
/// q-value, if any).
fn accepts_zstd(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            parts.next().is_some_and(|c| c.eq_ignore_ascii_case("zstd"))
                && parts.all(|p| p.replace(' ', "") != "q=0")
        })
}

/// A 64-hex name: the only kind joined onto the variants directory.
fn is_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn head_blob(
    Path(sha256): Path<String>,
    State(st): State<BlossomState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> StatusCode {
    if !st.allows(addr, BlobOp::Read) {
        return StatusCode::FORBIDDEN;
    }
    if st.store.has(&blob_hash(&sha256)).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn upload(
    State(st): State<BlossomState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> Response {
    // Uploads are the one operation a paired peer is not granted by default:
    // nothing in normal operation pushes blobs to a peer, and it costs us disk.
    if !st.allows(addr, BlobOp::Write) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match st.store.put(&body).await {
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
    use crate::FsBlobStore;

    #[tokio::test]
    async fn http_blossom_get_head_upload() {
        let dir = std::env::temp_dir().join(format!("myco-blossom-srv-{}", std::process::id()));
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

    #[tokio::test]
    async fn zstd_variant_only_for_a_client_that_asks() {
        let dir = std::env::temp_dir().join(format!("myco-blossom-zstd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(FsBlobStore::open(&dir).unwrap());
        let body = b"compress me ".repeat(4096);
        let hash = store.put(&body).await.unwrap();
        let encoded = store.store_zstd(&hash, 3).unwrap().unwrap();
        assert!(encoded < body.len() as u64);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let open: AccessFn = Arc::new(|_, _| true);
        tokio::spawn(serve_on_guarded(
            store.clone(),
            listener,
            open,
            Some(store.zstd_dir()),
        ));
        let url = format!("http://{addr}/{hash}");
        let client = reqwest::Client::new();

        // Asked: the variant, labelled, decoding to the blob.
        let got = client
            .get(&url)
            .header("accept-encoding", "gzip, zstd")
            .send()
            .await
            .unwrap();
        assert_eq!(got.headers()["content-encoding"], "zstd");
        let bytes = got.bytes().await.unwrap();
        assert_eq!(bytes.len() as u64, encoded);
        assert_eq!(
            crate::decode_zstd_bounded(&bytes, body.len()).unwrap(),
            body
        );
        // A bound below the decoded size refuses it.
        assert!(crate::decode_zstd_bounded(&bytes, body.len() - 1).is_none());

        // Not asked (or refused with q=0): the plain blob.
        for accept in [None, Some("zstd;q=0")] {
            let mut req = client.get(&url);
            if let Some(accept) = accept {
                req = req.header("accept-encoding", accept);
            }
            let got = req.send().await.unwrap();
            assert!(got.headers().get("content-encoding").is_none());
            assert_eq!(got.bytes().await.unwrap().as_ref(), body.as_slice());
        }

        // A frame asking for a 2^27 window (zstd's default limit) is refused.
        let mut big_window = Vec::new();
        {
            let mut encoder = zstd::stream::write::Encoder::new(&mut big_window, 1).unwrap();
            encoder.window_log(27).unwrap();
            encoder.include_contentsize(false).unwrap();
            std::io::Write::write_all(&mut encoder, b"tiny").unwrap();
            encoder.finish().unwrap();
        }
        assert!(crate::decode_zstd_bounded(&big_window, 1024).is_none());

        // Removing the blob takes its variant with it.
        assert!(store.remove(&hash));
        assert!(!store.has_zstd(&hash));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
