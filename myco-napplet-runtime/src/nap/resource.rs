//! NAP-RESOURCE — byte resources through the runtime. `blossom:` only, for now.
//!
//! A sandboxed napplet has no network; it names a resource and the runtime
//! fetches, checks and classifies it. What this build offers is the scheme
//! Myco already speaks everywhere else: `blossom:sha256:<hex>`, content
//! addressed, verified by hash before it is delivered.
//!
//! ## Local first, and everything fetched is kept
//!
//! Every ask goes to this device's Blossom store before anywhere else. A miss
//! goes through the [`BlobFetcher`](crate::seams::BlobFetcher) — the Circle's
//! stores over the mesh, the public servers when reachable — and what comes
//! back is **stored** before it is handed over. The second ask, from this or
//! any napplet, is local; and a picture one phone fetched is a picture the
//! whole room can now get over the mesh.
//!
//! That last sentence is also a privacy question, and an open one: the ask
//! tells every Circle member what you are looking at, and the keep makes you
//! a host of it. See `docs/design/napplet/napplet-runtime.md` §7.11 before
//! changing the fetch order or the keep rule.
//!
//! ## Bytes on this wire
//!
//! The shell ↔ Rust channel is JSON, so `blob` travels as base64. The shell
//! (`assets/shell.html`) turns it into a `Blob` typed by `mime` before the
//! message reaches the napplet, which is what the vendored shim resolves
//! `resource.bytes()` with. `mime` is sniffed from the bytes here, never
//! taken from anyone's header — and raw SVG is refused rather than delivered,
//! since this runtime has no sandboxed rasterizer to make it safe.

use base64::Engine;
use nsite_deck::sync::sha256_hex;

use crate::dispatch::NapContext;
use crate::seams::Envelope;

/// The spec's recommended response cap.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
/// The spec's recommended bulk cap.
pub const MAX_URLS: usize = 100;

/// Handle an inbound `resource.*` message.
pub async fn handle(ctx: &NapContext, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "info" => vec![info(message)],
        "bytes" => vec![bytes(ctx, message).await],
        "bytesMany" => vec![bytes_many(ctx, message).await],
        // A fetch here is bounded and has no partial state to abandon; the
        // shim drops a late result for a cancelled id on its own.
        "cancel" => Vec::new(),
        _ => Vec::new(),
    }
}

/// `resource.info` — what this build will say about itself. Advisory: a
/// napplet that skips it and asks for `https:` gets `unsupported-scheme` on
/// that request, as the spec requires.
fn info(message: &Envelope) -> Envelope {
    message.to_result().with_field(
        "info",
        serde_json::json!({
            "schemes": [
                { "scheme": "blossom", "enabled": true },
                { "scheme": "data", "enabled": false },
                { "scheme": "https", "enabled": false },
                { "scheme": "htree", "enabled": false },
                { "scheme": "nostr", "enabled": false },
            ],
            "maxBytes": MAX_BYTES,
            "maxUrls": MAX_URLS,
        }),
    )
}

/// `resource.bytes` — one resource, or one error.
async fn bytes(ctx: &NapContext, message: &Envelope) -> Envelope {
    let Some(url) = message.field("url").and_then(|v| v.as_str()) else {
        return error_for(message, "invalid-request", Some("bytes needs a url"));
    };
    match fetch(ctx, url).await {
        Ok(Fetched { blob, mime }) => message
            .to_result()
            .with_field("blob", blob)
            .with_field("mime", mime),
        Err(e) => error_for(message, e.code, e.message.as_deref()),
    }
}

/// `resource.bytesMany` — each URL as if it were its own `bytes`, in order;
/// one failure never discards its siblings.
async fn bytes_many(ctx: &NapContext, message: &Envelope) -> Envelope {
    let urls: Vec<String> = match message.field("urls").and_then(|v| v.as_array()) {
        Some(items) if !items.is_empty() => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(url) => out.push(url.to_string()),
                    None => {
                        return error_for(message, "invalid-request", Some("urls must be strings"))
                    }
                }
            }
            out
        }
        _ => return error_for(message, "invalid-request", Some("bytesMany needs urls")),
    };
    if urls.len() > MAX_URLS {
        return error_for(
            message,
            "too-large",
            Some(&format!("at most {MAX_URLS} urls per request")),
        );
    }

    let mut items = Vec::with_capacity(urls.len());
    for url in urls {
        let item = match fetch(ctx, &url).await {
            Ok(Fetched { blob, mime }) => {
                serde_json::json!({ "url": url, "ok": true, "blob": blob, "mime": mime })
            }
            Err(e) => {
                let mut item = serde_json::json!({ "url": url, "ok": false, "error": e.code });
                if let Some(m) = e.message {
                    item["message"] = serde_json::Value::String(m);
                }
                item
            }
        };
        items.push(item);
    }
    message.to_result().with_field("items", items)
}

/// One delivered resource: base64 bytes and the sniffed type.
struct Fetched {
    blob: String,
    mime: String,
}

/// A per-resource failure, in the spec's vocabulary.
struct Failure {
    code: &'static str,
    message: Option<String>,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Some(message.into()),
        }
    }
}

/// Resolve one URL: parse, local store, fetcher, verify, store, classify.
async fn fetch(ctx: &NapContext, url: &str) -> Result<Fetched, Failure> {
    let sha = parse_blossom_url(url)?;

    let stored = ctx
        .blobs
        .get(&sha)
        .await
        .map_err(|e| Failure::new("network-error", format!("local store: {e}")))?;
    let raw = match stored {
        Some(raw) => raw,
        None => {
            let fetched = ctx
                .fetcher
                .fetch(&sha)
                .await
                .map_err(|e| Failure::new("network-error", e.to_string()))?
                .ok_or(Failure {
                    code: "not-found",
                    message: None,
                })?;
            // Verified here whatever the fetcher did, then kept: the spec's
            // hash check, and the "anything queried is saved" rule, in the
            // one place every miss passes through.
            if sha256_hex(&fetched) != sha {
                return Err(Failure::new("decode-failed", "sha256 mismatch"));
            }
            if let Err(e) = ctx.blobs.put(&fetched).await {
                tracing::warn!(sha = %sha, error = %e, "resource: could not store a fetched blob");
            }
            fetched
        }
    };

    if raw.len() > MAX_BYTES {
        return Err(Failure::new(
            "too-large",
            format!("{} bytes, cap is {MAX_BYTES}", raw.len()),
        ));
    }
    let mime = sniff_mime(&raw);
    if mime == "image/svg+xml" {
        // Raw SVG is an active XML surface; without a sandboxed rasterizer
        // the only safe delivery is none.
        return Err(Failure::new(
            "blocked-by-policy",
            "SVG is not delivered raw by this runtime",
        ));
    }
    Ok(Fetched {
        blob: base64::engine::general_purpose::STANDARD.encode(&raw),
        mime: mime.to_string(),
    })
}

/// The sha256 named by a `blossom:` URL. The canonical form is
/// `blossom:sha256:<hex>`; the bare `blossom:<hex>` the shim's examples use
/// is accepted too. Anything else is the spec's `unsupported-scheme`, with
/// a malformed blossom URL as `invalid-request`.
fn parse_blossom_url(url: &str) -> Result<String, Failure> {
    let Some(rest) = url.strip_prefix("blossom:") else {
        return Err(Failure::new(
            "unsupported-scheme",
            format!("only blossom: is supported, not {}", scheme_of(url)),
        ));
    };
    let hex = rest.strip_prefix("sha256:").unwrap_or(rest);
    let hex = hex.split(['/', '?', '#']).next().unwrap_or("");
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(hex.to_ascii_lowercase())
    } else {
        Err(Failure::new(
            "invalid-request",
            "blossom URL must name a sha256 hex",
        ))
    }
}

fn scheme_of(url: &str) -> &str {
    url.split(':').next().unwrap_or("")
}

/// Classify bytes by what they are, never by what anyone said they were.
/// Enough of the magic numbers for what napplets actually load — pictures,
/// sound, documents, text — with `application/octet-stream` for the rest.
pub fn sniff_mime(bytes: &[u8]) -> &'static str {
    const fn starts(bytes: &[u8], magic: &[u8]) -> bool {
        bytes.len() >= magic.len() && {
            let mut i = 0;
            while i < magic.len() {
                if bytes[i] != magic[i] {
                    return false;
                }
                i += 1;
            }
            true
        }
    }
    if starts(bytes, b"\x89PNG\r\n\x1a\n") {
        return "image/png";
    }
    if starts(bytes, b"\xff\xd8\xff") {
        return "image/jpeg";
    }
    if starts(bytes, b"GIF87a") || starts(bytes, b"GIF89a") {
        return "image/gif";
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return "audio/wav";
    }
    if starts(bytes, b"BM") {
        return "image/bmp";
    }
    if starts(bytes, b"\x00\x00\x01\x00") {
        return "image/x-icon";
    }
    if starts(bytes, b"%PDF-") {
        return "application/pdf";
    }
    if starts(bytes, b"ID3") || starts(bytes, b"\xff\xfb") || starts(bytes, b"\xff\xf3") {
        return "audio/mpeg";
    }
    if starts(bytes, b"OggS") {
        return "audio/ogg";
    }
    if starts(bytes, b"fLaC") {
        return "audio/flac";
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return "video/mp4";
    }
    if starts(bytes, b"\x1a\x45\xdf\xa3") {
        return "video/webm";
    }
    if starts(bytes, b"PK\x03\x04") {
        return "application/zip";
    }
    if starts(bytes, b"\x1f\x8b") {
        return "application/gzip";
    }
    if starts(bytes, b"wOFF") {
        return "font/woff";
    }
    if starts(bytes, b"wOF2") {
        return "font/woff2";
    }
    if starts(bytes, b"\x00\x01\x00\x00") {
        return "font/ttf";
    }
    if starts(bytes, b"OTTO") {
        return "font/otf";
    }

    // Text. Look for SVG before anything else claims it: an SVG is XML, and
    // XML is text, and text would be delivered.
    let head = &bytes[..bytes.len().min(1024)];
    if let Ok(text) = std::str::from_utf8(head) {
        let lower = text.to_ascii_lowercase();
        if lower.contains("<svg") {
            return "image/svg+xml";
        }
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let trimmed = text.trim_start();
        if trimmed.starts_with("<?xml") {
            return "application/xml";
        }
        if (trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<serde_json::Value>(text).is_ok()
        {
            return "application/json";
        }
        if trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html") {
            return "text/html";
        }
        return "text/plain";
    }
    "application/octet-stream"
}

/// The domain's error envelope: `<type>.error` with `error` and, when there is
/// something to say, `message` — a distinct type from `.result`, as this NAP
/// has it.
fn error_for(message: &Envelope, code: &str, detail: Option<&str>) -> Envelope {
    let mut out = Envelope::new(format!("{}.error", message.msg_type)).with_field("error", code);
    out.id = message.id.clone();
    if let Some(detail) = detail {
        out = out.with_field("message", detail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::{NappletIdentity, Session};
    use crate::testing::test_context_with_fetcher;
    use serde_json::json;

    fn granted() -> Session {
        let mut s = Session::new(NappletIdentity::new("pics", "aggregate"), ["resource"]);
        s.on_ready();
        s
    }

    async fn call(ctx: &NapContext, e: Envelope) -> serde_json::Value {
        let out = dispatch(ctx, &mut granted(), &e).await.envelopes().to_vec();
        assert_eq!(out.len(), 1);
        serde_json::to_value(&out[0]).unwrap()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

    /// Local first: a blob the store holds is delivered without the fetcher
    /// being asked at all.
    #[tokio::test]
    async fn a_stored_blob_is_delivered_without_fetching() {
        let (ctx, fetcher) = test_context_with_fetcher();
        let sha = ctx.blobs.put(PNG).await.unwrap();
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{sha}")),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.result");
        assert_eq!(r["id"], "b1");
        assert_eq!(r["mime"], "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(r["blob"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, PNG);
        assert!(
            fetcher.asked().is_empty(),
            "the fetcher was asked for a stored blob"
        );
    }

    /// A miss is fetched, verified, **stored**, and delivered; the next ask
    /// is local.
    #[tokio::test]
    async fn a_missing_blob_is_fetched_verified_and_kept() {
        let (ctx, fetcher) = test_context_with_fetcher();
        let sha = sha256_hex(PNG);
        fetcher.hold(PNG);
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:{sha}")),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.result");
        assert_eq!(r["mime"], "image/png");
        assert_eq!(fetcher.asked(), vec![sha.clone()]);
        assert!(ctx.blobs.has(&sha).await, "the fetched blob was not kept");

        let _ = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b2")
                .with_field("url", format!("blossom:sha256:{sha}")),
        )
        .await;
        assert_eq!(
            fetcher.asked().len(),
            1,
            "the second ask went past the store"
        );
    }

    /// Bytes that do not hash to the name are never delivered or kept.
    #[tokio::test]
    async fn a_hash_mismatch_is_decode_failed_and_not_stored() {
        let (ctx, fetcher) = test_context_with_fetcher();
        let sha = sha256_hex(PNG);
        fetcher.lie(&sha, b"not the png");
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{sha}")),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.error");
        assert_eq!(r["error"], "decode-failed");
        assert!(r.get("blob").is_none());
        assert!(!ctx.blobs.has(&sha).await);
    }

    #[tokio::test]
    async fn nobody_has_it_is_not_found() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{}", "ab".repeat(32))),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.error");
        assert_eq!(r["error"], "not-found");
    }

    /// Other schemes fail per request, whether or not `info` was consulted.
    #[tokio::test]
    async fn other_schemes_are_unsupported_and_bad_urls_invalid() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        for (url, code) in [
            ("https://example.com/a.png", "unsupported-scheme"),
            ("nostr:npub1abc", "unsupported-scheme"),
            ("htree://x", "unsupported-scheme"),
            ("blossom:sha256:nothex", "invalid-request"),
            ("blossom:", "invalid-request"),
        ] {
            let r = call(
                &ctx,
                Envelope::new("resource.bytes")
                    .with_id("b1")
                    .with_field("url", url),
            )
            .await;
            assert_eq!(r["type"], "resource.bytes.error", "{url}");
            assert_eq!(r["error"], code, "{url}");
        }
    }

    /// Raw SVG is refused: the sniff finds it whatever it was named.
    #[tokio::test]
    async fn raw_svg_is_blocked_by_policy() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let svg = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        let sha = ctx.blobs.put(svg).await.unwrap();
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{sha}")),
        )
        .await;
        assert_eq!(r["error"], "blocked-by-policy");
    }

    /// Bulk: order and length preserved, one failure beside its successful
    /// siblings; an empty list or too many is a top-level error.
    #[tokio::test]
    async fn bytes_many_keeps_order_and_isolates_failures() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let png = ctx.blobs.put(PNG).await.unwrap();
        let text = ctx.blobs.put(b"hello").await.unwrap();
        let missing = "cd".repeat(32);
        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m1")
                .with_field(
                    "urls",
                    json!([
                        format!("blossom:sha256:{png}"),
                        format!("blossom:sha256:{missing}"),
                        "https://example.com/x",
                        format!("blossom:sha256:{text}"),
                    ]),
                ),
        )
        .await;
        assert_eq!(r["type"], "resource.bytesMany.result");
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0]["ok"], true);
        assert_eq!(items[0]["mime"], "image/png");
        assert_eq!(items[1]["ok"], false);
        assert_eq!(items[1]["error"], "not-found");
        assert!(items[1].get("blob").is_none());
        assert_eq!(items[2]["error"], "unsupported-scheme");
        assert_eq!(items[3]["ok"], true);
        assert_eq!(items[3]["mime"], "text/plain");

        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m2")
                .with_field("urls", json!([])),
        )
        .await;
        assert_eq!(r["type"], "resource.bytesMany.error");
        assert_eq!(r["error"], "invalid-request");

        let many: Vec<String> = (0..MAX_URLS + 1)
            .map(|_| format!("blossom:sha256:{png}"))
            .collect();
        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m3")
                .with_field("urls", many),
        )
        .await;
        assert_eq!(r["error"], "too-large");
    }

    #[tokio::test]
    async fn info_says_blossom_only() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let r = call(&ctx, Envelope::new("resource.info").with_id("i1")).await;
        assert_eq!(r["type"], "resource.info.result");
        let schemes = r["info"]["schemes"].as_array().unwrap();
        let enabled: Vec<&str> = schemes
            .iter()
            .filter(|s| s["enabled"] == true)
            .map(|s| s["scheme"].as_str().unwrap())
            .collect();
        assert_eq!(enabled, vec!["blossom"]);
        assert_eq!(r["info"]["maxBytes"], MAX_BYTES);
    }

    #[test]
    fn sniffing_is_by_bytes() {
        assert_eq!(sniff_mime(PNG), "image/png");
        assert_eq!(sniff_mime(b"\xff\xd8\xff\xe0JFIF"), "image/jpeg");
        assert_eq!(sniff_mime(b"GIF89a"), "image/gif");
        assert_eq!(sniff_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "image/webp");
        assert_eq!(sniff_mime(b"%PDF-1.7"), "application/pdf");
        assert_eq!(sniff_mime(b"{\"a\": 1}"), "application/json");
        assert_eq!(sniff_mime(b"just words"), "text/plain");
        assert_eq!(sniff_mime(b"<svg xmlns='x'/>"), "image/svg+xml");
        assert_eq!(sniff_mime(b"\x00\xff\xfe\x01"), "application/octet-stream");
        assert_eq!(sniff_mime(b""), "text/plain");
    }

    #[tokio::test]
    async fn an_ungranted_napplet_is_refused() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let mut s = Session::new(NappletIdentity::new("pics", "aggregate"), ["relay"]);
        s.on_ready();
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", "blossom:sha256:00"),
        )
        .await
        .envelopes()
        .to_vec();
        let r = serde_json::to_value(&out[0]).unwrap();
        assert!(r["error"].as_str().unwrap().contains("not granted"));
    }
}
