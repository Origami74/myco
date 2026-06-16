//! The gateway **engine**: `resolve host → manifest → path→sha256 → verified
//! blob → response`. This is the single serving path; both front doors — the
//! in-app WebView `shouldInterceptRequest` (via the `gatewayGet` JNI) and the
//! localhost `:80` HTTP handler for external browsers (P3) — call [`serve`].
//!
//! v0 serves **direct** from the local relay + Blossom: no version dirs, no
//! atomic swap, no htdocs cache (`docs/design/nsite-layer.md` §4). A site is only
//! served once **all** its referenced blobs are present; until then the gateway
//! returns a small loading page (HTTP 503) so a half-synced site never renders.

use std::collections::HashSet;

use crate::content_type;
use crate::host::{self, SiteAddr};
use crate::model::{kind_for, Manifest};
use crate::seams::{BlobStore, RelayBackend};

/// A fully-resolved gateway response, ready to hand to a `WebResourceResponse`
/// (in-app) or an HTTP response (external).
#[derive(Debug, Clone)]
pub struct GatewayResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// Extra headers beyond Content-Type/Content-Length (e.g. `Content-Range`,
    /// `Accept-Ranges`).
    pub headers: Vec<(String, String)>,
}

impl GatewayResponse {
    fn html(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8".to_string(),
            body: body.into().into_bytes(),
            headers: Vec::new(),
        }
    }
}

/// Whether a site can be served from the local stores right now.
pub enum Readiness {
    /// Manifest present and every referenced blob is in the local Blossom.
    Ready(Manifest),
    /// No manifest for this site in the local relay yet.
    ManifestMissing,
    /// Manifest present but some referenced blobs are missing (still syncing).
    Incomplete {
        manifest: Manifest,
        present: usize,
        total: usize,
    },
}

/// Compute a site's readiness from the local stores — shared by [`serve`] and the
/// FFI `siteStatus` reporting.
pub async fn readiness(
    relay: &dyn RelayBackend,
    blobs: &dyn BlobStore,
    addr: &SiteAddr,
) -> anyhow::Result<Readiness> {
    let kind = kind_for(addr.d_tag.as_deref());
    let event = relay
        .get_manifest(kind, &addr.author, addr.d_tag.as_deref())
        .await?;
    let Some(event) = event else {
        return Ok(Readiness::ManifestMissing);
    };
    let manifest = Manifest::from_event(event)?;

    let unique: HashSet<&str> = manifest.blob_hashes().collect();
    let total = unique.len();
    let mut present = 0usize;
    for hash in &unique {
        if blobs.has(hash).await {
            present += 1;
        }
    }
    if present == total {
        Ok(Readiness::Ready(manifest))
    } else {
        Ok(Readiness::Incomplete {
            manifest,
            present,
            total,
        })
    }
}

/// Serve one request for `http://<host>.nsite/<path>` direct from the local
/// stores. Pure read — never triggers a sync (the host app drives sync via
/// `OpenNsite`); an absent/incomplete site yields a 503 loading page.
pub async fn serve(
    relay: &dyn RelayBackend,
    blobs: &dyn BlobStore,
    host: &str,
    path: &str,
    range: Option<&str>,
) -> GatewayResponse {
    let Some(addr) = host::resolve_host(host) else {
        return GatewayResponse::html(404, not_in_library_page(host));
    };

    let manifest = match readiness(relay, blobs, &addr).await {
        Ok(Readiness::Ready(m)) => m,
        Ok(Readiness::ManifestMissing) => {
            return GatewayResponse::html(503, loading_page("Loading…"));
        }
        Ok(Readiness::Incomplete { present, total, .. }) => {
            return GatewayResponse::html(
                503,
                loading_page(&format!("Loading… {present}/{total} files")),
            );
        }
        Err(e) => return GatewayResponse::html(500, format!("<h1>500</h1><pre>{e}</pre>")),
    };

    let normalized = normalize_path(path);

    // Map path -> sha256, falling back to /404.html, then a gateway 404.
    let (hash, status) = match manifest.hash_for(&normalized) {
        Some(h) => (h.to_string(), 200u16),
        None => match manifest.hash_for("/404.html") {
            Some(h) => (h.to_string(), 404),
            None => return GatewayResponse::html(404, gateway_404_page(&normalized)),
        },
    };

    match blobs.get(&hash).await {
        Ok(Some(bytes)) => {
            let ctype = if status == 404 {
                "text/html; charset=utf-8"
            } else {
                content_type::from_path(&normalized)
            }
            .to_string();
            build_body_response(status, ctype, bytes, range)
        }
        // Present in the manifest + passed the all-present check, but the blob
        // read failed (corruption / race) — treat as still incomplete.
        Ok(None) => GatewayResponse::html(503, loading_page("Loading…")),
        Err(e) => GatewayResponse::html(500, format!("<h1>500</h1><pre>{e}</pre>")),
    }
}

/// Normalize a request path to a manifest path: index.html fallback for the
/// root, directory, and extensionless cases (`docs/design/nsite-layer.md` §4.1).
pub fn normalize_path(path: &str) -> String {
    // Drop any query/fragment defensively, ensure a leading slash.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = if path.is_empty() { "/" } else { path };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };

    if path == "/" {
        return "/index.html".to_string();
    }
    if path.ends_with('/') {
        return format!("{path}index.html");
    }
    // Extensionless last segment → treat as a directory.
    let last = path.rsplit('/').next().unwrap_or("");
    if !last.contains('.') {
        return format!("{path}/index.html");
    }
    path
}

/// Build a 200 (or 206 for a satisfiable Range) body response, with
/// `Accept-Ranges: bytes` so media seeking works.
fn build_body_response(
    status: u16,
    content_type: String,
    bytes: Vec<u8>,
    range: Option<&str>,
) -> GatewayResponse {
    let total = bytes.len();
    let mut headers = vec![("Accept-Ranges".to_string(), "bytes".to_string())];

    if status == 200 {
        if let Some(spec) = range.and_then(|r| parse_byte_range(r, total)) {
            let (start, end) = spec; // inclusive
            let slice = bytes[start..=end].to_vec();
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {start}-{end}/{total}"),
            ));
            return GatewayResponse {
                status: 206,
                content_type,
                body: slice,
                headers,
            };
        }
        if range.is_some() {
            // A Range header we couldn't satisfy.
            return GatewayResponse {
                status: 416,
                content_type: "text/plain; charset=utf-8".to_string(),
                body: Vec::new(),
                headers: vec![("Content-Range".to_string(), format!("bytes */{total}"))],
            };
        }
    }

    GatewayResponse {
        status,
        content_type,
        body: bytes,
        headers,
    }
}

/// Parse a single `bytes=start-end` range against a known length → inclusive
/// `(start, end)`. Supports an open end (`bytes=500-`); ignores multi-range and
/// suffix ranges (returns `None` → caller serves the full body / 416).
fn parse_byte_range(header: &str, total: usize) -> Option<(usize, usize)> {
    let spec = header.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None; // multi-range unsupported
    }
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
        return None; // suffix range (bytes=-N) unsupported
    }
    let start: usize = start_s.trim().parse().ok()?;
    let end: usize = if end_s.trim().is_empty() {
        total.checked_sub(1)?
    } else {
        end_s.trim().parse().ok()?
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end.min(total - 1)))
}

fn loading_page(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"refresh\" content=\"1\">\
<title>Loading…</title></head>\
<body style=\"font-family:sans-serif;text-align:center;padding-top:3rem\">\
<p>{}</p></body></html>",
        html_escape(message)
    )
}

fn gateway_404_page(path: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>404</title></head>\
<body style=\"font-family:sans-serif;text-align:center;padding-top:3rem\">\
<h1>404</h1><p>{}</p></body></html>",
        html_escape(path)
    )
}

fn not_in_library_page(host: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Not in your Library</title></head>\
<body style=\"font-family:sans-serif;text-align:center;padding-top:3rem\">\
<h1>Not in your Library</h1><p>{}</p></body></html>",
        html_escape(host)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path(""), "/index.html");
        assert_eq!(normalize_path("/"), "/index.html");
        assert_eq!(normalize_path("/foo/"), "/foo/index.html");
        assert_eq!(normalize_path("/about"), "/about/index.html");
        assert_eq!(normalize_path("/style.css"), "/style.css");
        assert_eq!(normalize_path("index.html"), "/index.html");
        assert_eq!(normalize_path("/a/b.js?v=2"), "/a/b.js");
    }

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_byte_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_byte_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_byte_range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(parse_byte_range("bytes=2000-3000", 1000), None);
        assert_eq!(parse_byte_range("bytes=-100", 1000), None);
        assert_eq!(parse_byte_range("bytes=0-10,20-30", 1000), None);
    }
}
