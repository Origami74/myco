//! NAP-UPLOAD — keep a napplet's file on this device, then put it on the
//! user's Blossom servers.
//!
//! The spec is the upstream draft, napplet/naps PR #33
//! (<https://github.com/napplet/naps/pull/33>, `naps/NAP-UPLOAD.md`). Myco's
//! choices where it leaves room are in `docs/design/napplet/NAP-UPLOAD.md`.
//!
//! The napplet hands over bytes and a description; the shell picks the
//! servers, signs the Blossom (BUD-02) authorization as the user, uploads, and
//! answers with the URL and NIP-94 tags. The napplet never names a server,
//! never sees the authorization, and never opens a socket.
//!
//! **This device first.** Before any server is tried, the file is stripped
//! of its metadata ([`strip`]) and kept in this device's own Blossom store,
//! which nothing evicts and Circle peers read over the mesh. So an upload
//! works with no internet, with offline-only on, and with no server at all:
//! the answer is then `complete` with a `blossom:sha256:<hex>` URL (which
//! NAP-RESOURCE resolves) and `localOnly: true`, a Myco addition, beside
//! `remoteError` saying why no server has it. Only a napplet without the
//! grant, a bad request, or a user who said no to signing gets a refusal.
//!
//! - `upload.info` — advisory: Blossom, the size cap. Never a preflight.
//! - `upload.upload` — answered once finished (`complete`, `failed` or
//!   `cancelled`), which the spec allows. No progress is pushed. Myco's
//!   prelude waits [`crate::SIGNING_TIMEOUT`] for it, since signing with a
//!   signer app is a person approving.
//! - `upload.status` — the last status for an `uploadId` this session made.
//!
//! Errors follow the spec: a top-level `error` when no upload was created
//! (refused, bad request, too large, unsupported rail), and a `result` with
//! `ok: false` once one was. The strings are the spec's where one fits.
//!
//! **The bytes.** `request.data` is a `Blob` or `ArrayBuffer` on the wire,
//! which does not survive the shell's JSON hop to Rust. The shell
//! (`assets/shell.html`, trusted) reads it and forwards it as
//! `request.dataBase64`, with the blob's own type as `request.dataType`; one
//! over [`MAX_BYTES`] is not read at all and arrives as `request.dataSize`
//! alone, refused here. The hop is inside the shell: the napplet sends a
//! Blob, as the spec says. The decoded bytes are still the napplet's: checked
//! against the cap again, hashed here, and sniffed when no type was given.

use base64::Engine as _;
use serde_json::{json, Value};

use crate::dispatch::NapContext;
use crate::seams::{Envelope, UploadBlob, UploadErrorCode};
use crate::session::Session;

/// The largest upload, in bytes. Mirrored in `assets/shell.html`
/// (`UPLOAD_MAX_BYTES`), which declines to read anything bigger; a test pins
/// the two together.
///
/// 16 MiB: a picture, a short clip, a document. The bytes cross the shell's
/// JSON channel as base64 (a third larger) and are copied a few times on the
/// way into Rust, so the cap is also what one upload may cost in memory.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

/// The one rail implemented.
pub const RAIL_BLOSSOM: &str = "blossom";

// The spec's error strings used here (napplet/naps PR #33, Error Handling).
pub(crate) const POLICY_DENIED: &str = "policy denied";
const UNSUPPORTED_RAIL: &str = "unsupported rail";
const FILE_TOO_LARGE: &str = "file too large";
/// Not one of the spec's (its list is "common errors", not closed): the
/// request carried no bytes, or bytes that did not decode.
const INVALID_REQUEST: &str = "invalid request";
/// Not one of the spec's either: a file of a type Myco strips ([`strip`])
/// that does not parse as one. Refused rather than sent with whatever
/// metadata it carries.
const MALFORMED_FILE: &str = "malformed file";

mod strip;

/// Handle an inbound `upload.*` message.
pub async fn handle(ctx: &NapContext, session: &Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "info" => vec![info(ctx, message).await],
        "upload" => vec![upload(ctx, session, message).await],
        "status" => vec![status(session, message)],
        // An unrecognized action is silence, as NIP-5D asks.
        _ => Vec::new(),
    }
}

/// `upload.info` — advisory: Blossom, always enabled (this device's own
/// store takes every upload), and the URLs it may answer with: `blossom:`
/// always, `https` too while the servers can be reached. Every type is
/// accepted, so no `mimeTypes`.
async fn info(ctx: &NapContext, message: &Envelope) -> Envelope {
    let returns = if ctx.uploads.available().await {
        json!(["https", "blossom"])
    } else {
        json!(["blossom"])
    };
    message.to_result().with_field(
        "info",
        json!({
            "rails": [
                {
                    "rail": RAIL_BLOSSOM,
                    "enabled": true,
                    "returns": returns,
                },
            ],
            "maxBytes": MAX_BYTES,
        }),
    )
}

/// A request read off the wire, before anything leaves the device.
#[derive(Debug, PartialEq)]
struct Parsed {
    /// The stripped bytes, their hash and type.
    blob: UploadBlob,
    caption: Option<String>,
    /// The hash of what the napplet sent, when stripping changed it.
    original_sha256: Option<String>,
}

/// Read and check an `upload.upload` request. `Err` is the spec's error
/// string: nothing was uploaded, so it is the top-level `error`.
fn parse(message: &Envelope) -> Result<Parsed, &'static str> {
    let Some(request) = message.field("request").and_then(Value::as_object) else {
        return Err(INVALID_REQUEST);
    };
    match request.get("rail") {
        // Omitted: the shell picks, and Blossom is all there is.
        None | Some(Value::Null) => {}
        Some(Value::String(rail)) if rail == RAIL_BLOSSOM => {}
        Some(_) => return Err(UNSUPPORTED_RAIL),
    }
    let bytes = match request.get("dataBase64") {
        Some(Value::String(b64)) => decode(b64)?,
        // The shell names the size of what it would not carry.
        _ if request.get("dataSize").and_then(Value::as_u64).is_some() => {
            return Err(FILE_TOO_LARGE)
        }
        _ => return Err(INVALID_REQUEST),
    };
    if bytes.is_empty() {
        return Err(INVALID_REQUEST);
    }
    let mime = [request.get("mimeType"), request.get("dataType")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(clean_mime)
        .unwrap_or_else(|| crate::nap::resource::sniff_mime(&bytes).to_string());
    // Stripped before it is hashed, kept or sent: everything after this
    // sees only the clean file.
    let (bytes, original_sha256) = match strip::strip(&bytes, &mime) {
        Ok(Some(clean)) if clean != bytes => (clean, Some(nsite_deck::sync::sha256_hex(&bytes))),
        Ok(_) => (bytes, None),
        Err(e) => {
            tracing::info!(error = %e, "napplet upload refused");
            return Err(MALFORMED_FILE);
        }
    };
    let sha256 = nsite_deck::sync::sha256_hex(&bytes);
    Ok(Parsed {
        blob: UploadBlob {
            bytes,
            sha256,
            mime,
            filename: short_text(request.get("filename"), 200),
        },
        caption: short_text(request.get("caption"), 1000),
        original_sha256,
    })
}

/// Decode the shell's base64, refusing anything over the cap before the
/// work of decoding it.
fn decode(b64: &str) -> Result<Vec<u8>, &'static str> {
    // Four characters per three bytes, padded.
    let longest = MAX_BYTES.div_ceil(3) * 4;
    if b64.len() > longest {
        return Err(FILE_TOO_LARGE);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| INVALID_REQUEST)?;
    if bytes.len() > MAX_BYTES {
        return Err(FILE_TOO_LARGE);
    }
    Ok(bytes)
}

/// A MIME type as `type/subtype`, lowercased and without parameters — or
/// `None` for anything that is not one, so the next source is tried.
fn clean_mime(raw: &str) -> Option<String> {
    let essence = raw.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, sub) = essence.split_once('/')?;
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    (token(kind) && token(sub)).then_some(essence)
}

/// A napplet-supplied string, trimmed, without control characters, and cut
/// to `max` characters. `None` when absent or empty.
fn short_text(value: Option<&Value>, max: usize) -> Option<String> {
    let text: String = value?
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// `upload.upload` — keep it here, upload it, and answer once both are done.
async fn upload(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let Parsed {
        blob,
        caption,
        original_sha256,
    } = match parse(message) {
        Ok(parsed) => parsed,
        Err(e) => return message.to_error(e),
    };
    let upload_id = new_upload_id(&blob.sha256);
    tracing::info!(
        napplet = %session.identity().d_tag,
        size = blob.bytes.len(),
        mime = %blob.mime,
        sha256 = %blob.sha256,
        stripped = original_sha256.is_some(),
        "napplet upload"
    );
    // This device first: kept, so it is never evicted, and readable by
    // Circle peers. Whatever the servers do next, the file exists here.
    let kept = match ctx.kept_blobs.put(&blob.bytes).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "uploaded blob not kept on this device");
            false
        }
    };
    // What the napplet handed over, for `originalSha256`.
    let sent_sha256 = original_sha256.as_deref().unwrap_or(&blob.sha256);
    let dimensions = dimensions(&blob.bytes);
    let result = match ctx.uploads.upload(&blob).await {
        Ok(done) => {
            let mime = done.mime.clone().unwrap_or_else(|| blob.mime.clone());
            let mut result = json!({
                "ok": true,
                "uploadId": upload_id,
                "status": "complete",
                "rail": RAIL_BLOSSOM,
                "url": done.url,
                "fallbackUrls": done.fallback_urls,
                "sha256": done.sha256,
                "size": done.size,
                "mimeType": mime,
                "nip94": nip94(&done, &blob.sha256, &mime, dimensions, caption.as_deref()),
            });
            if done.sha256 != sent_sha256 {
                result["originalSha256"] = json!(sent_sha256);
            }
            // A server that stored other bytes than were sent transformed
            // them. The spec has both hashes reported: `x` what is at the
            // URL, `ox` what was handed over. Dimensions read from the
            // original are then not claimed for the stored file.
            if done.sha256 == blob.sha256 {
                if let Some((width, height)) = dimensions {
                    result["dimensions"] = json!({ "width": width, "height": height });
                }
            }
            result
        }
        // The user said no to signing: that is an answer about this upload,
        // not a server failing, so it is not turned into a local success.
        // The copy kept here stays, as any kept blob does.
        Err(e) if e.code == UploadErrorCode::UserCancelled || !kept => {
            tracing::info!(error = %e, "napplet upload failed");
            let state = if e.code == UploadErrorCode::UserCancelled {
                "cancelled"
            } else {
                "failed"
            };
            json!({
                "ok": false,
                "uploadId": upload_id,
                "status": state,
                "rail": RAIL_BLOSSOM,
                "error": e.code.as_str(),
            })
        }
        // No server took it — offline-only, no internet, no server, or every
        // one refused — but this device has it. That is a complete upload
        // to this device's Blossom, said plainly.
        Err(e) => {
            tracing::info!(error = %e, "napplet upload kept on this device only");
            let url = format!("blossom:sha256:{}", blob.sha256);
            let local = crate::seams::Uploaded {
                url: url.clone(),
                fallback_urls: Vec::new(),
                sha256: blob.sha256.clone(),
                size: blob.bytes.len() as u64,
                mime: None,
            };
            let mut result = json!({
                "ok": true,
                "uploadId": upload_id,
                "status": "complete",
                "rail": RAIL_BLOSSOM,
                "url": url,
                "sha256": blob.sha256,
                "size": blob.bytes.len(),
                "mimeType": blob.mime,
                "nip94": nip94(&local, &blob.sha256, &blob.mime, dimensions, caption.as_deref()),
                "localOnly": true,
                "remoteError": e.code.as_str(),
            });
            if let Some(original) = &original_sha256 {
                result["originalSha256"] = json!(original);
            }
            if let Some((width, height)) = dimensions {
                result["dimensions"] = json!({ "width": width, "height": height });
            }
            result
        }
    };
    let mut status = result.clone();
    status["updatedAt"] = json!(now_ms());
    session.record_upload(&upload_id, status);
    message.to_result().with_field("result", result)
}

/// `upload.status` — the last result for one of this session's uploads.
fn status(session: &Session, message: &Envelope) -> Envelope {
    let Some(upload_id) = message.field("uploadId").and_then(Value::as_str) else {
        return message.to_error(INVALID_REQUEST);
    };
    match session.upload_status(upload_id) {
        Some(status) => message.to_result().with_field("status", status),
        None => message.to_error("unknown upload"),
    }
}

/// An id for one upload: unique in this process, and naming the blob so a
/// log line can be matched to it.
fn new_upload_id(sha256: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{n}", &sha256[..sha256.len().min(12)])
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// NIP-94 tags for the uploaded file, ready to attach to an event (an
/// `imeta` or a kind 1063).
/// `ox` and no `dim` when the server transformed the file (see `upload`).
fn nip94(
    done: &crate::seams::Uploaded,
    sent_sha256: &str,
    mime: &str,
    dimensions: Option<(u32, u32)>,
    caption: Option<&str>,
) -> Vec<Vec<String>> {
    let mut tags = vec![
        vec!["url".to_string(), done.url.clone()],
        vec!["m".to_string(), mime.to_string()],
        vec!["x".to_string(), done.sha256.clone()],
    ];
    let transformed = done.sha256 != sent_sha256;
    if transformed {
        tags.push(vec!["ox".to_string(), sent_sha256.to_string()]);
    }
    tags.push(vec!["size".to_string(), done.size.to_string()]);
    if let (false, Some((width, height))) = (transformed, dimensions) {
        tags.push(vec!["dim".to_string(), format!("{width}x{height}")]);
    }
    for url in &done.fallback_urls {
        tags.push(vec!["fallback".to_string(), url.clone()]);
    }
    if let Some(caption) = caption {
        tags.push(vec!["alt".to_string(), caption.to_string()]);
    }
    tags
}

/// Pixel size when the header says it plainly: PNG and GIF. Anything else
/// gets none rather than a decoder.
fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        let be = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        return Some((be(16), be(20))).filter(|(w, h)| *w > 0 && *h > 0);
    }
    if bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let le = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32;
        return Some((le(6), le(8))).filter(|(w, h)| *w > 0 && *h > 0);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::NappletIdentity;
    use crate::testing::{test_context_with_uploads, MemUploads};
    use std::sync::Arc;

    /// A whole 3x2 PNG, CRCs and all, with nothing to strip — or with the
    /// given chunks after its header.
    fn png_with(extra: &[Vec<u8>]) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend(strip::tests::chunk(
            b"IHDR",
            b"\0\0\0\x03\0\0\0\x02\x08\x06\0\0\0",
        ));
        for c in extra {
            p.extend_from_slice(c);
        }
        p.extend(strip::tests::chunk(
            b"IDAT",
            b"\x78\x9c\x63\x60\0\0\0\x02\0\x01",
        ));
        p.extend(strip::tests::chunk(b"IEND", b""));
        p
    }

    fn png() -> Vec<u8> {
        png_with(&[])
    }

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("doodle", "agg"), granted.to_vec());
        s.on_ready();
        s
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn upload_msg(request: Value) -> Envelope {
        Envelope::new("upload.upload")
            .with_id("u1")
            .with_field("request", request)
    }

    async fn call(ctx: &NapContext, s: &mut Session, e: Envelope) -> Value {
        let out = dispatch(ctx, s, &e).await.envelopes().to_vec();
        assert_eq!(out.len(), 1, "{out:?}");
        serde_json::to_value(&out[0]).unwrap()
    }

    /// Not granted: refused before anything is decoded, signed or sent.
    #[tokio::test]
    async fn an_ungranted_upload_is_refused() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["relay"]);
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({"dataBase64": b64(&png()), "mimeType": "image/png"})),
        )
        .await;
        assert_eq!(r["type"], "upload.upload.result");
        assert_eq!(r["id"], "u1");
        assert_eq!(r["error"], "policy denied");
        assert!(r.get("result").is_none());
        assert!(uploads.uploaded().is_empty());
    }

    /// The whole happy path: the sink gets the decoded bytes, their hash and
    /// type; the napplet gets a complete result with NIP-94 tags; the blob is
    /// kept here; and `upload.status` answers for it afterwards.
    #[tokio::test]
    async fn an_upload_answers_complete_with_nip94() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({
                "dataBase64": b64(&png()),
                "dataType": "image/png",
                "filename": "doodleduo-7.png",
                "caption": "we scored 7",
            })),
        )
        .await;
        assert_eq!(r["type"], "upload.upload.result");
        assert_eq!(r["id"], "u1");
        let result = &r["result"];
        let sha = nsite_deck::sync::sha256_hex(&png());
        assert_eq!(result["ok"], true);
        assert_eq!(result["status"], "complete");
        assert_eq!(result["rail"], "blossom");
        assert_eq!(result["sha256"], sha);
        assert_eq!(result["size"], png().len());
        assert_eq!(result["mimeType"], "image/png");
        assert_eq!(result["url"], format!("https://blossom.test/{sha}.png"));
        assert_eq!(
            result["fallbackUrls"],
            json!([format!("https://mirror.test/{sha}.png")])
        );
        assert_eq!(result["dimensions"], json!({"width": 3, "height": 2}));
        assert_eq!(
            result["nip94"],
            json!([
                ["url", format!("https://blossom.test/{sha}.png")],
                ["m", "image/png"],
                ["x", sha],
                ["size", png().len().to_string()],
                ["dim", "3x2"],
                ["fallback", format!("https://mirror.test/{sha}.png")],
                ["alt", "we scored 7"],
            ])
        );

        let sent = uploads.uploaded();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].bytes, png());
        assert_eq!(sent[0].sha256, sha);
        assert_eq!(sent[0].filename.as_deref(), Some("doodleduo-7.png"));
        assert!(ctx.kept_blobs.has(&sha).await, "not kept on this device");

        let upload_id = result["uploadId"].as_str().unwrap();
        let st = call(
            &ctx,
            &mut s,
            Envelope::new("upload.status")
                .with_id("s1")
                .with_field("uploadId", upload_id),
        )
        .await;
        assert_eq!(st["status"]["status"], "complete");
        assert_eq!(st["status"]["url"], result["url"]);
        assert!(st["status"]["updatedAt"].as_u64().unwrap() > 0);

        let unknown = call(
            &ctx,
            &mut s,
            Envelope::new("upload.status")
                .with_id("s2")
                .with_field("uploadId", "nope"),
        )
        .await;
        assert_eq!(unknown["error"], "unknown upload");
    }

    /// No server took it — offline-only, no internet, no server, every one
    /// refusing: still `complete`, on this device, with a `blossom:` URL the
    /// napplet can resolve and `localOnly` saying so.
    #[tokio::test]
    async fn an_upload_no_server_took_is_kept_here_and_complete() {
        let mut s = session(&["upload"]);
        let sha = nsite_deck::sync::sha256_hex(&png());
        for code in [
            UploadErrorCode::PolicyDenied,
            UploadErrorCode::NoServerConfigured,
            UploadErrorCode::ServerRejected,
            UploadErrorCode::UploadFailed,
        ] {
            let (mut ctx, _uploads) = test_context_with_uploads();
            ctx.uploads = Arc::new(MemUploads::failing(code));
            let r = call(
                &ctx,
                &mut s,
                upload_msg(json!({"dataBase64": b64(&png()), "caption": "offline"})),
            )
            .await;
            assert!(r.get("error").is_none(), "{r}");
            let result = &r["result"];
            assert_eq!(result["ok"], true);
            assert_eq!(result["status"], "complete");
            assert_eq!(result["rail"], "blossom");
            assert_eq!(result["localOnly"], true);
            assert_eq!(result["remoteError"], code.as_str());
            assert_eq!(result["url"], format!("blossom:sha256:{sha}"));
            assert_eq!(result["sha256"], sha);
            assert_eq!(result["size"], png().len());
            assert_eq!(result["mimeType"], "image/png");
            assert_eq!(result["dimensions"], json!({"width": 3, "height": 2}));
            assert!(result.get("fallbackUrls").is_none());
            assert_eq!(
                result["nip94"],
                json!([
                    ["url", format!("blossom:sha256:{sha}")],
                    ["m", "image/png"],
                    ["x", sha],
                    ["size", png().len().to_string()],
                    ["dim", "3x2"],
                    ["alt", "offline"],
                ])
            );
            assert!(ctx.kept_blobs.has(&sha).await, "not kept on this device");
            let st = call(
                &ctx,
                &mut s,
                Envelope::new("upload.status")
                    .with_id("s")
                    .with_field("uploadId", result["uploadId"].clone()),
            )
            .await;
            assert_eq!(st["status"]["localOnly"], true);
        }
    }

    /// The user said no to signing: `cancelled`, not a local success.
    #[tokio::test]
    async fn a_signer_no_is_cancelled() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.uploads = Arc::new(MemUploads::failing(UploadErrorCode::UserCancelled));
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(b"hi")}))).await;
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["status"], "cancelled");
        assert_eq!(r["result"]["error"], "user cancelled");
        assert!(r["result"].get("url").is_none());
    }

    /// A store that takes nothing, as a full disk would.
    struct FullDisk;

    #[async_trait::async_trait]
    impl nsite_deck::BlobStore for FullDisk {
        async fn has(&self, _: &str) -> bool {
            false
        }
        async fn get(&self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        async fn put(&self, _: &[u8]) -> anyhow::Result<String> {
            anyhow::bail!("no space left")
        }
        async fn wipe(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Neither here nor on a server: a result with `ok: false`,
    /// `status: "failed"` and the server's error string. A server alone is
    /// still enough.
    #[tokio::test]
    async fn an_upload_kept_nowhere_is_a_failed_result() {
        let mut s = session(&["upload"]);
        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.kept_blobs = Arc::new(FullDisk);
        ctx.uploads = Arc::new(MemUploads::failing(UploadErrorCode::UploadFailed));
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(b"hi")}))).await;
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["status"], "failed");
        assert_eq!(r["result"]["error"], "upload failed");

        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.kept_blobs = Arc::new(FullDisk);
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(b"hi")}))).await;
        assert_eq!(r["result"]["ok"], true);
        assert!(r["result"].get("localOnly").is_none());
    }

    /// Metadata goes before anything is kept or sent: the server and this
    /// device get the clean file, the napplet learns its hash, and
    /// `originalSha256` names what it handed over.
    #[tokio::test]
    async fn an_upload_is_stripped_before_it_is_kept_or_sent() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        let tagged = png_with(&[strip::tests::chunk(b"tEXt", b"GPS\x0052.37N 4.89E")]);
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({"dataBase64": b64(&tagged)})),
        )
        .await;
        let result = &r["result"];
        let clean = nsite_deck::sync::sha256_hex(&png());
        assert_eq!(result["sha256"], clean);
        assert_eq!(
            result["originalSha256"],
            nsite_deck::sync::sha256_hex(&tagged)
        );
        assert_eq!(result["size"], png().len());
        assert_eq!(result["dimensions"], json!({"width": 3, "height": 2}));
        let tags = result["nip94"].as_array().unwrap();
        assert!(tags.contains(&json!(["x", clean])));
        assert!(
            !tags.iter().any(|t| t[0] == "ox"),
            "the original's hash is not published"
        );
        assert_eq!(uploads.uploaded()[0].bytes, png());
        assert_eq!(uploads.uploaded()[0].sha256, clean);
        assert_eq!(ctx.kept_blobs.get(&clean).await.unwrap(), Some(png()));
    }

    /// A file of a stripped type that does not parse is refused, not sent
    /// with its metadata — and not kept either.
    #[tokio::test]
    async fn a_malformed_picture_is_refused() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        let mut cut = png_with(&[strip::tests::chunk(b"eXIf", b"MM\0*")]);
        cut.truncate(cut.len() - 12);
        for request in [
            json!({"dataBase64": b64(&cut)}),
            json!({"dataBase64": b64(b"not really"), "mimeType": "image/jpeg"}),
        ] {
            let r = call(&ctx, &mut s, upload_msg(request)).await;
            assert_eq!(r["error"], "malformed file", "{r}");
            assert!(r.get("result").is_none());
        }
        assert!(uploads.uploaded().is_empty());
    }

    /// A server that transformed the file: both hashes reported, `x` the
    /// stored one and `ox` the original, and no dimensions claimed.
    #[tokio::test]
    async fn a_transformed_upload_reports_both_hashes() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        let stored = "ee".repeat(32);
        ctx.uploads = Arc::new(MemUploads::transforming(&stored));
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(&png())}))).await;
        let result = &r["result"];
        let sha = nsite_deck::sync::sha256_hex(&png());
        assert_eq!(result["sha256"], stored);
        assert_eq!(result["originalSha256"], sha);
        assert!(result.get("dimensions").is_none());
        let tags = result["nip94"].as_array().unwrap();
        assert!(tags.contains(&json!(["x", stored])));
        assert!(tags.contains(&json!(["ox", sha])));
        assert!(!tags.iter().any(|t| t[0] == "dim"));
    }

    /// NIP-96 and anything else that is not Blossom: refused, nothing sent.
    #[tokio::test]
    async fn other_rails_are_refused() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        for rail in [json!("nip96"), json!("s3"), json!(7)] {
            let r = call(
                &ctx,
                &mut s,
                upload_msg(json!({"rail": rail, "dataBase64": b64(&png())})),
            )
            .await;
            assert_eq!(r["error"], "unsupported rail", "{rail}: {r}");
            assert!(r.get("result").is_none());
        }
        let ok = call(
            &ctx,
            &mut s,
            upload_msg(json!({"rail": "blossom", "dataBase64": b64(&png())})),
        )
        .await;
        assert_eq!(ok["result"]["status"], "complete");
        assert_eq!(uploads.uploaded().len(), 1);
    }

    /// The bytes: missing, empty, mangled, or over the cap — each refused
    /// with a sentence, and never sent.
    #[tokio::test]
    async fn bad_or_oversized_bytes_are_refused() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        for (request, says) in [
            (json!({}), "invalid request"),
            (json!({"dataBase64": ""}), "invalid request"),
            (json!({"dataBase64": "not base64!!"}), "invalid request"),
            (json!({"dataSize": MAX_BYTES + 1}), "file too large"),
            (
                json!({"dataBase64": "A".repeat(MAX_BYTES.div_ceil(3) * 4 + 4)}),
                "file too large",
            ),
        ] {
            let r = call(&ctx, &mut s, upload_msg(request.clone())).await;
            assert_eq!(r["error"], says, "{request}");
            assert_eq!(r["id"], "u1");
        }
        let r = call(&ctx, &mut s, Envelope::new("upload.upload").with_id("x")).await;
        assert!(r["error"].is_string());
        assert!(uploads.uploaded().is_empty());
    }

    /// The type: the napplet's if it is one, else the blob's, else sniffed.
    #[test]
    fn the_mime_type_is_said_or_sniffed() {
        let parsed = |request: Value| parse(&upload_msg(request)).unwrap().blob.mime;
        assert_eq!(
            parsed(json!({"dataBase64": b64(&png()), "mimeType": "Image/PNG; q=1"})),
            "image/png"
        );
        assert_eq!(
            parsed(
                json!({"dataBase64": b64(&png()), "mimeType": "nonsense", "dataType": "image/x-doodle"})
            ),
            "image/x-doodle"
        );
        assert_eq!(
            parsed(json!({"dataBase64": b64(&png()), "dataType": ""})),
            "image/png"
        );
        assert_eq!(
            parsed(json!({"dataBase64": b64(&[0u8, 159, 146, 150])})),
            "application/octet-stream"
        );
    }

    /// `upload.info` names Blossom, its state, and the cap.
    #[tokio::test]
    async fn info_names_the_rails_and_the_cap() {
        let (ctx, _uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, Envelope::new("upload.info").with_id("i")).await;
        assert_eq!(r["info"]["rails"][0]["rail"], "blossom");
        assert_eq!(r["info"]["rails"][0]["enabled"], true);
        assert_eq!(
            r["info"]["rails"][0]["returns"],
            json!(["https", "blossom"])
        );
        assert_eq!(r["info"]["rails"].as_array().unwrap().len(), 1);
        assert_eq!(r["info"]["maxBytes"], MAX_BYTES);
    }

    /// The shell declines to read past the same cap Rust enforces.
    #[test]
    fn the_shell_cap_matches() {
        let shell = include_str!("../../assets/shell.html");
        assert!(
            shell.contains(&format!("var UPLOAD_MAX_BYTES = {MAX_BYTES};")),
            "shell.html's UPLOAD_MAX_BYTES drifted from nap::upload::MAX_BYTES"
        );
    }

    #[test]
    fn dimensions_come_from_png_and_gif_headers() {
        assert_eq!(dimensions(&png()), Some((3, 2)));
        assert_eq!(dimensions(b"GIF89a\x0a\x00\x05\x00"), Some((10, 5)));
        assert_eq!(dimensions(b"\xff\xd8\xff\xe0"), None);
    }
}
