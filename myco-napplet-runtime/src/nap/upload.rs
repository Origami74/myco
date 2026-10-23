//! NAP-UPLOAD — put a napplet's file on the user's Blossom servers, or keep
//! it on this device.
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
//! Two rails, both Blossom:
//!
//! - **`blossom`** (the default): the user's servers. `complete` only when one
//!   confirmed the file, with an `https` URL anyone can open. The file is also
//!   kept in this device's own Blossom store, which nothing evicts and Circle
//!   peers read over the mesh — after a failure too, when no server could take
//!   it, so a retry or a peer still finds it. The napplet is not told: it can't
//!   share that copy publicly.
//! - **`blossom-local`**: this device's store only, asked for on purpose. No
//!   signature, no internet, works with offline-only on; `complete` with a
//!   `blossom:sha256:<hex>` URL, which NAP-RESOURCE resolves here and from
//!   Circle peers. A rail of the shell's own, as the spec allows.
//!
//! Keeping without a signature is free for the napplet, so each may keep only
//! [`LOCAL_BUDGET_BYTES`] that way a day ([`LocalUploads`]). An upload the user
//! did not approve keeps nothing.
//!
//! - `upload.info` — advisory: both rails, the size cap. Never a preflight.
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

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

use crate::dispatch::NapContext;
use crate::seams::{Envelope, UploadBlob, UploadErrorCode};
use crate::session::{NappletIdentity, Session};

/// The largest upload, in bytes. Mirrored in `assets/shell.html`
/// (`UPLOAD_MAX_BYTES`), which declines to read anything bigger; a test pins
/// the two together.
///
/// 16 MiB: a picture, a short clip, a document. The bytes cross the shell's
/// JSON channel as base64 (a third larger) and are copied a few times on the
/// way into Rust, so the cap is also what one upload may cost in memory.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

/// The user's Blossom servers: the default rail.
pub const RAIL_BLOSSOM: &str = "blossom";
/// This device's own Blossom store, for a file meant to stay on the mesh.
pub const RAIL_BLOSSOM_LOCAL: &str = "blossom-local";

/// How many bytes one napplet may keep on this device, per
/// [`LOCAL_WINDOW`], without a server having taken them: `blossom-local`
/// uploads, and the copy of a `blossom` upload no server could take. Those
/// need no signature, so nothing else stops a napplet from filling the phone
/// with files nothing evicts: four uploads at the size cap a day.
pub const LOCAL_BUDGET_BYTES: u64 = 4 * MAX_BYTES as u64;
const LOCAL_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// What each napplet kept without a server taking it, within
/// [`LOCAL_WINDOW`]. One per device, like the seams; keyed by napplet inside.
/// In memory: a restart starts every napplet's day again, which bounds the
/// rate without a store of its own.
#[derive(Default)]
pub struct LocalUploads {
    spent: Mutex<HashMap<NappletIdentity, VecDeque<(Instant, u64)>>>,
}

impl LocalUploads {
    /// Charge `size` bytes to `napplet`, or `false` if that would pass
    /// [`LOCAL_BUDGET_BYTES`].
    fn charge(&self, napplet: &NappletIdentity, size: u64) -> bool {
        let mut spent = self.spent.lock().unwrap();
        let entries = spent.entry(napplet.clone()).or_default();
        while entries
            .front()
            .is_some_and(|(at, _)| at.elapsed() >= LOCAL_WINDOW)
        {
            entries.pop_front();
        }
        let used: u64 = entries.iter().map(|(_, n)| n).sum();
        if used + size > LOCAL_BUDGET_BYTES {
            return false;
        }
        entries.push_back((Instant::now(), size));
        true
    }

    /// Give back the newest charge of `size`, for a keep that failed.
    fn refund(&self, napplet: &NappletIdentity, size: u64) {
        let mut spent = self.spent.lock().unwrap();
        if let Some(entries) = spent.get_mut(napplet) {
            if let Some(i) = entries.iter().rposition(|(_, n)| *n == size) {
                entries.remove(i);
            }
        }
    }
}

// The spec's error strings used here (napplet/naps PR #33, Error Handling).
pub(crate) const POLICY_DENIED: &str = "policy denied";
const UNSUPPORTED_RAIL: &str = "unsupported rail";
const FILE_TOO_LARGE: &str = "file too large";
/// Not one of the spec's (its list is "common errors", not closed): the
/// request carried no bytes, or bytes that did not decode.
const INVALID_REQUEST: &str = "invalid request";

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

/// `upload.info` — advisory: the user's servers (disabled while the user has
/// the internet off) with `https` URLs, and this device's store, always
/// there, with `blossom:` ones. Every type is accepted, so no `mimeTypes`.
async fn info(ctx: &NapContext, message: &Envelope) -> Envelope {
    let enabled = ctx.uploads.available().await;
    message.to_result().with_field(
        "info",
        json!({
            "rails": [
                {
                    "rail": RAIL_BLOSSOM,
                    "enabled": enabled,
                    "returns": ["https"],
                },
                {
                    "rail": RAIL_BLOSSOM_LOCAL,
                    "enabled": true,
                    "returns": ["blossom"],
                },
            ],
            "maxBytes": MAX_BYTES,
        }),
    )
}

/// A request read off the wire, before anything leaves the device.
#[derive(Debug, PartialEq)]
struct Parsed {
    blob: UploadBlob,
    caption: Option<String>,
    /// `blossom-local`: this device's store, not the servers.
    local: bool,
}

/// Read and check an `upload.upload` request. `Err` is the spec's error
/// string: nothing was uploaded, so it is the top-level `error`.
fn parse(message: &Envelope) -> Result<Parsed, &'static str> {
    let Some(request) = message.field("request").and_then(Value::as_object) else {
        return Err(INVALID_REQUEST);
    };
    let local = match request.get("rail") {
        // Omitted: the shell picks, and that is the user's servers.
        None | Some(Value::Null) => false,
        Some(Value::String(rail)) if rail == RAIL_BLOSSOM => false,
        Some(Value::String(rail)) if rail == RAIL_BLOSSOM_LOCAL => true,
        Some(_) => return Err(UNSUPPORTED_RAIL),
    };
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
    let sha256 = nsite_deck::sync::sha256_hex(&bytes);
    let mime = [request.get("mimeType"), request.get("dataType")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(clean_mime)
        .unwrap_or_else(|| crate::nap::resource::sniff_mime(&bytes).to_string());
    Ok(Parsed {
        blob: UploadBlob {
            bytes,
            sha256,
            mime,
            filename: short_text(request.get("filename"), 200),
        },
        caption: short_text(request.get("caption"), 1000),
        local,
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

/// `upload.upload` — upload or keep it, and answer once that is done.
async fn upload(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let Parsed {
        blob,
        caption,
        local,
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
        local,
        "napplet upload"
    );
    let dimensions = dimensions(&blob.bytes);
    let result = if local {
        match keep_local(ctx, session.identity(), &blob).await {
            Ok(()) => kept_result(&upload_id, &blob, dimensions, caption.as_deref()),
            Err(code) => {
                tracing::info!(error = code.as_str(), "napplet upload not kept");
                failed(&upload_id, RAIL_BLOSSOM_LOCAL, "failed", code)
            }
        }
    } else {
        match ctx.uploads.upload(&blob).await {
            Ok(done) => {
                // Kept here too, so phones nearby can fetch it by hash. Best
                // effort: the upload already succeeded, and that is the answer.
                if let Err(e) = ctx.kept_blobs.put(&blob.bytes).await {
                    tracing::debug!(error = %e, "uploaded blob not kept on this device");
                }
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
                // The server stored other bytes than were sent — a transform.
                // The spec has both hashes reported: `x` what is at the URL,
                // `ox` what the napplet handed over. Dimensions read from the
                // original are then not claimed for the stored file.
                if done.sha256 != blob.sha256 {
                    result["originalSha256"] = json!(blob.sha256);
                } else if let Some((width, height)) = dimensions {
                    result["dimensions"] = json!({ "width": width, "height": height });
                }
                result
            }
            Err(e) => {
                tracing::info!(error = %e, "napplet upload failed");
                // No server could take it (offline-only, no internet, no
                // server, every one refused): a copy stays here for a retry
                // or a peer, within the budget. Not when the user did not
                // approve it or it never got its turn.
                if e.keep_locally && e.code != UploadErrorCode::UserCancelled {
                    if let Err(code) = keep_local(ctx, session.identity(), &blob).await {
                        tracing::debug!(error = code.as_str(), "failed upload not kept");
                    }
                }
                let state = if e.code == UploadErrorCode::UserCancelled {
                    "cancelled"
                } else {
                    "failed"
                };
                failed(&upload_id, RAIL_BLOSSOM, state, e.code)
            }
        }
    };
    let mut status = result.clone();
    status["updatedAt"] = json!(now_ms());
    session.record_upload(&upload_id, status);
    message.to_result().with_field("result", result)
}

/// A `blossom-local` upload kept on this device: `complete`, with the
/// `blossom:` URL its own store answers to.
fn kept_result(
    upload_id: &str,
    blob: &UploadBlob,
    dimensions: Option<(u32, u32)>,
    caption: Option<&str>,
) -> Value {
    let kept = crate::seams::Uploaded {
        url: format!("blossom:sha256:{}", blob.sha256),
        fallback_urls: Vec::new(),
        sha256: blob.sha256.clone(),
        size: blob.bytes.len() as u64,
        mime: None,
    };
    let mut result = json!({
        "ok": true,
        "uploadId": upload_id,
        "status": "complete",
        "rail": RAIL_BLOSSOM_LOCAL,
        "url": kept.url,
        "sha256": blob.sha256,
        "size": blob.bytes.len(),
        "mimeType": blob.mime,
        "nip94": nip94(&kept, &blob.sha256, &blob.mime, dimensions, caption),
    });
    if let Some((width, height)) = dimensions {
        result["dimensions"] = json!({ "width": width, "height": height });
    }
    result
}

/// An upload that ended without a file: `ok: false`, `state`, and the spec's
/// error string.
fn failed(upload_id: &str, rail: &str, state: &str, code: UploadErrorCode) -> Value {
    json!({
        "ok": false,
        "uploadId": upload_id,
        "status": state,
        "rail": rail,
        "error": code.as_str(),
    })
}

/// Keep a file on this device without a server having taken it, charged to
/// `napplet`'s [`LocalUploads`] budget. Always written to the kept store: a
/// file only in the cache could be evicted while its `blossom:` URL is out.
/// A file already on this device, kept or cached, costs nothing. `Err` is the
/// error to answer with: `quota exceeded` over budget, `upload failed` when
/// the store refused it.
async fn keep_local(
    ctx: &NapContext,
    napplet: &NappletIdentity,
    blob: &UploadBlob,
) -> Result<(), UploadErrorCode> {
    let size = blob.bytes.len() as u64;
    let charged = !ctx.kept_blobs.has(&blob.sha256).await;
    if charged && !ctx.local_uploads.charge(napplet, size) {
        return Err(UploadErrorCode::QuotaExceeded);
    }
    if let Err(e) = ctx.kept_blobs.put(&blob.bytes).await {
        tracing::warn!(error = %e, "uploaded blob not kept on this device");
        if charged {
            ctx.local_uploads.refund(napplet, size);
        }
        return Err(UploadErrorCode::UploadFailed);
    }
    Ok(())
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

    /// A 3x2 PNG header — enough for the sniffer and `dimensions`.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x03\x00\x00\x00\x02\x08\x06\x00\x00\x00";

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
            upload_msg(json!({"dataBase64": b64(PNG), "mimeType": "image/png"})),
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
                "dataBase64": b64(PNG),
                "dataType": "image/png",
                "filename": "doodleduo-7.png",
                "caption": "we scored 7",
            })),
        )
        .await;
        assert_eq!(r["type"], "upload.upload.result");
        assert_eq!(r["id"], "u1");
        let result = &r["result"];
        let sha = nsite_deck::sync::sha256_hex(PNG);
        assert_eq!(result["ok"], true);
        assert_eq!(result["status"], "complete");
        assert_eq!(result["rail"], "blossom");
        assert_eq!(result["sha256"], sha);
        assert_eq!(result["size"], PNG.len());
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
                ["size", PNG.len().to_string()],
                ["dim", "3x2"],
                ["fallback", format!("https://mirror.test/{sha}.png")],
                ["alt", "we scored 7"],
            ])
        );

        let sent = uploads.uploaded();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].bytes, PNG);
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
    /// refusing: `failed` with the spec's error string and no URL, since the
    /// napplet can't share a copy here publicly. That copy is kept anyway,
    /// for a retry or a Circle peer.
    #[tokio::test]
    async fn an_upload_no_server_took_fails_but_is_kept_here() {
        let mut s = session(&["upload"]);
        let sha = nsite_deck::sync::sha256_hex(PNG);
        for code in [
            UploadErrorCode::PolicyDenied,
            UploadErrorCode::NoServerConfigured,
            UploadErrorCode::ServerRejected,
            UploadErrorCode::UploadFailed,
        ] {
            let (mut ctx, _uploads) = test_context_with_uploads();
            ctx.uploads = Arc::new(MemUploads::failing(code));
            let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(PNG)}))).await;
            assert!(r.get("error").is_none(), "{r}");
            let result = &r["result"];
            assert_eq!(result["ok"], false);
            assert_eq!(result["status"], "failed");
            assert_eq!(result["rail"], "blossom");
            assert_eq!(result["error"], code.as_str());
            assert!(result.get("url").is_none());
            assert!(ctx.kept_blobs.has(&sha).await, "not kept on this device");
        }
    }

    /// `blossom-local`: kept on this device on purpose, with no server and
    /// no signature — `complete`, with the `blossom:` URL its store answers
    /// to and NIP-94 tags to match.
    #[tokio::test]
    async fn the_local_rail_keeps_it_here_and_is_complete() {
        let (mut ctx, uploads) = test_context_with_uploads();
        ctx.uploads = Arc::new(MemUploads::failing(UploadErrorCode::PolicyDenied));
        let mut s = session(&["upload"]);
        let sha = nsite_deck::sync::sha256_hex(PNG);
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({"rail": "blossom-local", "dataBase64": b64(PNG), "caption": "here"})),
        )
        .await;
        let result = &r["result"];
        assert_eq!(result["ok"], true, "{r}");
        assert_eq!(result["status"], "complete");
        assert_eq!(result["rail"], "blossom-local");
        assert_eq!(result["url"], format!("blossom:sha256:{sha}"));
        assert_eq!(result["sha256"], sha);
        assert_eq!(result["size"], PNG.len());
        assert_eq!(result["mimeType"], "image/png");
        assert_eq!(result["dimensions"], json!({"width": 3, "height": 2}));
        assert!(result.get("fallbackUrls").is_none());
        assert_eq!(
            result["nip94"],
            json!([
                ["url", format!("blossom:sha256:{sha}")],
                ["m", "image/png"],
                ["x", sha],
                ["size", PNG.len().to_string()],
                ["dim", "3x2"],
                ["alt", "here"],
            ])
        );
        assert!(ctx.kept_blobs.has(&sha).await, "not kept on this device");
        assert!(uploads.uploaded().is_empty(), "sent to a server");
        let st = call(
            &ctx,
            &mut s,
            Envelope::new("upload.status")
                .with_id("s")
                .with_field("uploadId", result["uploadId"].clone()),
        )
        .await;
        assert_eq!(st["status"]["rail"], "blossom-local");
    }

    /// The user said no to signing: `cancelled`, and nothing left on this
    /// device.
    #[tokio::test]
    async fn a_signer_no_is_cancelled_and_keeps_nothing() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.uploads = Arc::new(MemUploads::failing(UploadErrorCode::UserCancelled));
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(b"hi")}))).await;
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["status"], "cancelled");
        assert_eq!(r["result"]["error"], "user cancelled");
        assert!(r["result"].get("url").is_none());
        let sha = nsite_deck::sync::sha256_hex(b"hi");
        assert!(!ctx.kept_blobs.has(&sha).await, "kept after a no");
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

    /// A full disk: `blossom-local` fails with `upload failed`; a server
    /// alone is still a complete `blossom` upload.
    #[tokio::test]
    async fn a_full_disk_fails_only_the_local_rail() {
        let mut s = session(&["upload"]);
        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.kept_blobs = Arc::new(FullDisk);
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({"rail": "blossom-local", "dataBase64": b64(b"hi")})),
        )
        .await;
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["status"], "failed");
        assert_eq!(r["result"]["rail"], "blossom-local");
        assert_eq!(r["result"]["error"], "upload failed");

        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(b"hi")}))).await;
        assert_eq!(r["result"]["ok"], true);
        assert_eq!(r["result"]["status"], "complete");
    }

    /// Keeping without a signature has a daily budget per napplet: past it,
    /// `blossom-local` is `quota exceeded` and a failed `blossom` upload is
    /// no longer kept. A file already on this device costs nothing, and
    /// another napplet has its own budget.
    #[tokio::test]
    async fn keeping_stops_at_the_napplets_budget() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.uploads = Arc::new(MemUploads::failing(UploadErrorCode::PolicyDenied));
        let local =
            |data: &[u8]| upload_msg(json!({"rail": "blossom-local", "dataBase64": b64(data)}));
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, local(b"first")).await;
        assert_eq!(r["result"]["status"], "complete");

        assert!(ctx
            .local_uploads
            .charge(s.identity(), LOCAL_BUDGET_BYTES - 5));
        let r = call(&ctx, &mut s, local(b"second")).await;
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["status"], "failed");
        assert_eq!(r["result"]["error"], "quota exceeded");
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({"dataBase64": b64(b"second")})),
        )
        .await;
        assert_eq!(r["result"]["error"], "policy denied");
        let sha = nsite_deck::sync::sha256_hex(b"second");
        assert!(!ctx.kept_blobs.has(&sha).await, "kept past the budget");

        let r = call(&ctx, &mut s, local(b"first")).await;
        assert_eq!(
            r["result"]["status"], "complete",
            "a file already here costs nothing"
        );

        let mut other = Session::new(NappletIdentity::new("other", "agg"), vec!["upload"]);
        other.on_ready();
        let r = call(&ctx, &mut other, local(b"second")).await;
        assert_eq!(r["result"]["status"], "complete", "budgets are per napplet");
    }

    /// Unsigned for any reason but a "no" (the signer never answered), or
    /// refused because another upload runs: `failed`, and no copy kept.
    #[tokio::test]
    async fn an_upload_that_never_ran_is_not_kept() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        ctx.uploads = Arc::new(MemUploads::refusing(UploadErrorCode::UploadFailed));
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(b"hi")}))).await;
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["status"], "failed");
        assert_eq!(r["result"]["error"], "upload failed");
        let sha = nsite_deck::sync::sha256_hex(b"hi");
        assert!(!ctx.kept_blobs.has(&sha).await, "kept without an upload");
    }

    /// A store that says it has everything — as the device's does for a
    /// file only in its cache — and counts what it is asked to keep.
    #[derive(Default)]
    struct CachedOnly(std::sync::Mutex<usize>);

    #[async_trait::async_trait]
    impl nsite_deck::BlobStore for CachedOnly {
        async fn has(&self, _: &str) -> bool {
            true
        }
        async fn get(&self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
            *self.0.lock().unwrap() += 1;
            Ok(nsite_deck::sync::sha256_hex(bytes))
        }
        async fn wipe(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// A file the device has only in its cache is still written to the kept
    /// store, which nothing evicts, and costs the napplet nothing.
    #[tokio::test]
    async fn a_cached_file_is_kept_for_real() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        let store = Arc::new(CachedOnly::default());
        ctx.kept_blobs = store.clone();
        let mut s = session(&["upload"]);
        assert!(ctx.local_uploads.charge(s.identity(), LOCAL_BUDGET_BYTES));
        let r = call(
            &ctx,
            &mut s,
            upload_msg(json!({"rail": "blossom-local", "dataBase64": b64(b"hi")})),
        )
        .await;
        assert_eq!(r["result"]["status"], "complete", "{r}");
        assert_eq!(*store.0.lock().unwrap(), 1, "not written to the kept store");
    }

    #[test]
    fn a_failed_keep_is_refunded() {
        let budget = LocalUploads::default();
        let napplet = NappletIdentity::new("doodle", "agg");
        assert!(budget.charge(&napplet, LOCAL_BUDGET_BYTES));
        assert!(!budget.charge(&napplet, 1));
        budget.refund(&napplet, LOCAL_BUDGET_BYTES);
        assert!(budget.charge(&napplet, 1));
    }

    /// A server that transformed the file: both hashes reported, `x` the
    /// stored one and `ox` the original, and no dimensions claimed.
    #[tokio::test]
    async fn a_transformed_upload_reports_both_hashes() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        let stored = "ee".repeat(32);
        ctx.uploads = Arc::new(MemUploads::transforming(&stored));
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, upload_msg(json!({"dataBase64": b64(PNG)}))).await;
        let result = &r["result"];
        let sha = nsite_deck::sync::sha256_hex(PNG);
        assert_eq!(result["sha256"], stored);
        assert_eq!(result["originalSha256"], sha);
        assert!(result.get("dimensions").is_none());
        let tags = result["nip94"].as_array().unwrap();
        assert!(tags.contains(&json!(["x", stored])));
        assert!(tags.contains(&json!(["ox", sha])));
        assert!(!tags.iter().any(|t| t[0] == "dim"));
    }

    /// NIP-96 and anything else that is not one of the two Blossom rails:
    /// refused, nothing sent.
    #[tokio::test]
    async fn other_rails_are_refused() {
        let (ctx, uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        for rail in [json!("nip96"), json!("s3"), json!("blossomLocal"), json!(7)] {
            let r = call(
                &ctx,
                &mut s,
                upload_msg(json!({"rail": rail, "dataBase64": b64(PNG)})),
            )
            .await;
            assert_eq!(r["error"], "unsupported rail", "{rail}: {r}");
            assert!(r.get("result").is_none());
        }
        let ok = call(
            &ctx,
            &mut s,
            upload_msg(json!({"rail": "blossom", "dataBase64": b64(PNG)})),
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
            parsed(json!({"dataBase64": b64(PNG), "mimeType": "Image/PNG; q=1"})),
            "image/png"
        );
        assert_eq!(
            parsed(
                json!({"dataBase64": b64(PNG), "mimeType": "nonsense", "dataType": "image/x-doodle"})
            ),
            "image/x-doodle"
        );
        assert_eq!(
            parsed(json!({"dataBase64": b64(PNG), "dataType": ""})),
            "image/png"
        );
        assert_eq!(
            parsed(json!({"dataBase64": b64(&[0u8, 159, 146, 150])})),
            "application/octet-stream"
        );
    }

    /// `upload.info` names both rails, what each returns, and the cap. The
    /// servers' rail is off while the internet is; this device's never is.
    #[tokio::test]
    async fn info_names_the_rails_and_the_cap() {
        let (mut ctx, _uploads) = test_context_with_uploads();
        let mut s = session(&["upload"]);
        let r = call(&ctx, &mut s, Envelope::new("upload.info").with_id("i")).await;
        assert_eq!(
            r["info"]["rails"],
            json!([
                {"rail": "blossom", "enabled": true, "returns": ["https"]},
                {"rail": "blossom-local", "enabled": true, "returns": ["blossom"]},
            ])
        );
        assert_eq!(r["info"]["maxBytes"], MAX_BYTES);

        ctx.uploads = Arc::new(MemUploads::failing(UploadErrorCode::PolicyDenied));
        let r = call(&ctx, &mut s, Envelope::new("upload.info").with_id("i")).await;
        assert_eq!(r["info"]["rails"][0]["enabled"], false);
        assert_eq!(r["info"]["rails"][1]["enabled"], true);
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
        assert_eq!(dimensions(PNG), Some((3, 2)));
        assert_eq!(dimensions(b"GIF89a\x0a\x00\x05\x00"), Some((10, 5)));
        assert_eq!(dimensions(b"\xff\xd8\xff\xe0"), None);
    }
}
