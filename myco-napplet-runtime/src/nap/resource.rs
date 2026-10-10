//! NAP-RESOURCE — byte resources through the runtime: `blossom:` and `https:`.
//!
//! A sandboxed napplet has no network; it names a resource and the runtime
//! fetches, checks and classifies it. Two schemes are offered. `blossom:`
//! (`blossom:sha256:<hex>`) is the one Myco speaks everywhere else: content
//! addressed, verified by hash before it is delivered. `https:` is the open
//! web — a profile picture on someone's CDN — under the spec's Default
//! Resource Policy. Everything else, `http:` included, is
//! `unsupported-scheme`.
//!
//! ## Local first, and everything fetched is kept
//!
//! Every `blossom:` ask goes to this device's Blossom store before anywhere
//! else. A miss goes through the [`BlobFetcher`](crate::seams::BlobFetcher) —
//! the Circle's stores over the mesh, the public servers when reachable — and
//! what comes back is **stored** before it is handed over. The second ask,
//! from this or any napplet, is local; and a picture one phone fetched is a
//! picture the whole room can now get over the mesh.
//!
//! That last sentence is also a privacy question, and an open one: the ask
//! tells every Circle member what you are looking at, and the keep makes you
//! a host of it. See `docs/design/napplet/napplet-runtime.md` §7.11 before
//! changing the fetch order or the keep rule.
//!
//! ## `https:` — the internet, under policy
//!
//! An `https:` URL never goes to the mesh: it names one server, and only
//! that server can answer it. The fetch reveals the user's interest to that
//! server (and, through DNS, to whoever resolves its name), which is the
//! price of the scheme; offline-only refuses it outright. The policy is split
//! between the two places that can enforce it:
//!
//! - **Here**, before anything is dialled: [`validate_https_url`] — `https`
//!   only, parsed by the same `url` parser the HTTP client uses, no userinfo,
//!   no private, loopback or link-local address literal (shorthands like
//!   `127.1` are normalised before they are judged), no `localhost`, `.local`,
//!   `.fips` or dotless LAN name, at most [`MAX_URL_LEN`] bytes. Then the
//!   per-napplet rate ([`MAX_HTTPS_PER_MINUTE`]) and the recent-URL memory
//!   ([`HttpsMemory`]).
//! - **At the dial**, behind the [`HttpsFetcher`](crate::seams::HttpsFetcher)
//!   seam: what a name resolves to, every redirect hop judged again by
//!   [`validate_https_url`], GET only with no cookies or credentials, the size
//!   cap enforced while downloading, a bound on time, and offline-only.
//!
//! What comes back is hashed and put in the same store a `blossom:` fetch
//! lands in, so it is delivered exactly as one is: by `blobRef`. It is cached,
//! not kept — the device does not become a host of the open web's pictures
//! unless the napplet asks `resource.keep`.
//!
//! ## Bytes on this wire
//!
//! None. The shell ↔ Rust channel is JSON, and bytes do not belong in it: a
//! result carries `blobRef`, the sha256 of a blob now in the store, and the
//! shell (`assets/shell.html`) fetches it from its own origin — served as
//! bytes by the window host, only for a blob this napplet was delivered —
//! and hands the napplet a `Blob` typed by `mime`, which is what the
//! vendored shim resolves `resource.bytes()` with. The napplet sees the spec's
//! `blob`, never `blobRef`. `mime` is sniffed from the bytes here, never
//! taken from anyone's header — and raw SVG is refused rather than delivered,
//! since this runtime has no sandboxed rasterizer to make it safe. The sniff
//! looks for `<svg` across the whole body, not a leading window, so a prolog
//! or comment long enough to push it past the first kilobyte does not
//! smuggle it through as XML.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nsite_deck::sync::sha256_hex;

use crate::dispatch::NapContext;
use crate::seams::Envelope;
use crate::session::{NappletIdentity, Session};

/// The most one resource may be: 64 MiB, the largest body this device's own
/// Blossom accepts. Above the spec's recommended 10 MiB because a short video
/// is routinely larger, and bytes no longer cross the JSON channel (the shell
/// fetches them as bytes; see the module docs). Held in memory whole while it
/// is fetched, checked and handed over, so not higher without a path that
/// streams from disk.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// The spec's recommended bulk cap.
pub const MAX_URLS: usize = 100;
/// The longest `https:` URL fetched. Far past any real media link; a URL
/// longer than this is a payload, not an address.
pub const MAX_URL_LEN: usize = 2048;
/// The most `https:` fetches one napplet may start in a minute — the spec's
/// recommended rate. Counted per URL that reaches the network: a `bytesMany`
/// of ten is ten, and a URL answered from [`HttpsMemory`] is none.
pub const MAX_HTTPS_PER_MINUTE: usize = 60;
/// How long an `https:` URL's answer is reused without asking the server
/// again. Short: the URL names a server's *current* bytes, not fixed ones, so
/// this is a session's worth of not refetching an avatar, not a cache that
/// outlives an edit.
pub const HTTPS_REMEMBERED: Duration = Duration::from_secs(5 * 60);
/// The most `https:` answers remembered, across every napplet.
pub const MAX_HTTPS_REMEMBERED: usize = 256;
/// The most one `bytesMany` may return in total — one resource's worth. A
/// hundred blobs at the per-blob cap would be gigabytes handed to one
/// napplet at once; past this the remaining URLs are answered `too-large`
/// without being fetched.
pub const MAX_TOTAL_BYTES: usize = MAX_BYTES;

/// Handle an inbound `resource.*` message.
pub async fn handle(ctx: &NapContext, session: &Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "info" => vec![info(message)],
        "bytes" => vec![bytes(ctx, session, message).await],
        "bytesMany" => vec![bytes_many(ctx, session, message).await],
        "keep" => vec![keep(ctx, session, message).await],
        // A fetch here is bounded and has no partial state to abandon; the
        // shim drops a late result for a cancelled id on its own.
        "cancel" => Vec::new(),
        _ => Vec::new(),
    }
}

/// `resource.info` — what this build will say about itself. Advisory: a
/// napplet that skips it and asks for `nostr:` gets `unsupported-scheme` on
/// that request, as the spec requires — and one that asks for `https:` with
/// the internet off gets `blocked-by-policy`, whatever this said.
fn info(message: &Envelope) -> Envelope {
    message.to_result().with_field(
        "info",
        serde_json::json!({
            "schemes": [
                { "scheme": "blossom", "enabled": true },
                { "scheme": "data", "enabled": false },
                { "scheme": "https", "enabled": true },
                { "scheme": "htree", "enabled": false },
                { "scheme": "nostr", "enabled": false },
            ],
            "maxBytes": MAX_BYTES,
            "maxUrls": MAX_URLS,
            "maxTotalBytes": MAX_TOTAL_BYTES,
        }),
    )
}

/// `resource.bytes` — one resource, or one error.
async fn bytes(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let Some(url) = message.field("url").and_then(|v| v.as_str()) else {
        return error_for(message, "invalid-request", Some("bytes needs a url"));
    };
    match fetch(ctx, session.identity(), url).await {
        Ok(Fetched { mime, sha, .. }) => {
            record(session, &sha);
            message
                .to_result()
                .with_field("blobRef", sha)
                .with_field("mime", mime)
        }
        Err(e) => {
            // A napplet's own error is invisible from outside; without this a
            // "not found" on its screen cannot be told apart from a scheme it
            // never had.
            tracing::info!(url, code = e.code, message = ?e.message, "resource: not delivered");
            error_for(message, e.code, e.message.as_deref())
        }
    }
}

/// `resource.bytesMany` — each URL as if it were its own `bytes`, in order;
/// one failure never discards its siblings.
async fn bytes_many(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
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
    let mut total = 0usize;
    for url in urls {
        if total >= MAX_TOTAL_BYTES {
            items.push(serde_json::json!({
                "url": url, "ok": false, "error": "too-large",
                "message": format!("this request already carries {MAX_TOTAL_BYTES} bytes"),
            }));
            continue;
        }
        let item = match fetch(ctx, session.identity(), &url).await {
            Ok(Fetched { mime, len, sha }) => {
                record(session, &sha);
                total += len;
                serde_json::json!({ "url": url, "ok": true, "blobRef": sha, "mime": mime })
            }
            Err(e) => {
                tracing::info!(url, code = e.code, message = ?e.message, "resource: not delivered");
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

/// One delivered resource, now in the store: the sniffed type, the size, and
/// the hash it was verified against — which is how the shell fetches it.
struct Fetched {
    mime: String,
    len: usize,
    sha: String,
}

/// Note a delivered blob, so the napplet may `resource.keep` it.
fn record(session: &Session, sha: &str) {
    if let Some(key) = crate::delivered::parse_hex32(sha) {
        session.record_delivered_blob(&key);
    }
}

/// `resource.keep` — keep a blob this napplet was delivered in this device's
/// own Blossom, where nothing evicts it and Circle peers can fetch it
/// (NAP-LOCAL). Nothing is sent anywhere. Takes `url` (`blossom:sha256:…`) or
/// a bare `sha256`.
async fn keep(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let sha = match (
        message.field("sha256").and_then(|v| v.as_str()),
        message.field("url").and_then(|v| v.as_str()),
    ) {
        (Some(sha), _) => sha.to_ascii_lowercase(),
        (None, Some(url)) => match parse_blossom_url(url) {
            Ok((sha, _)) => sha,
            Err(e) => return error_for(message, e.code, e.message.as_deref()),
        },
        (None, None) => {
            return error_for(
                message,
                "invalid-request",
                Some("keep needs a url or sha256"),
            )
        }
    };
    // Keeping is NAP-LOCAL's power, not NAP-RESOURCE's: switching `local` off
    // must stop it, or the toggle promises what it cannot deliver.
    if !session.is_granted("local") {
        return error_for(
            message,
            "blocked-by-policy",
            Some("keeping needs the local capability"),
        );
    }
    let Some(key) = crate::delivered::parse_hex32(&sha) else {
        return error_for(message, "invalid-request", Some("not a sha256"));
    };
    if !session.was_delivered_blob(&key) {
        return error_for(
            message,
            "blocked-by-policy",
            Some("only a blob delivered to this napplet can be kept"),
        );
    }
    if ctx.kept_blobs.has(&sha).await {
        return message.to_result().with_field("ok", true);
    }
    let bytes = match ctx.blobs.get(&sha).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return error_for(
                message,
                "not-found",
                Some("no longer held here; fetch it again first"),
            )
        }
        Err(e) => return error_for(message, "network-error", Some(&e.to_string())),
    };
    match ctx.kept_blobs.put(&bytes).await {
        Ok(_) => {
            tracing::info!(sha = %sha, "napplet kept a blob");
            message.to_result().with_field("ok", true)
        }
        Err(e) => error_for(message, "network-error", Some(&e.to_string())),
    }
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

/// Resolve one URL, by its scheme.
async fn fetch(ctx: &NapContext, napplet: &NappletIdentity, url: &str) -> Result<Fetched, Failure> {
    if scheme_of(url).eq_ignore_ascii_case("https") {
        fetch_https(ctx, napplet, url).await
    } else {
        fetch_blossom(ctx, url).await
    }
}

/// Resolve one `blossom:` URL: parse, local store, fetcher, verify, store,
/// classify.
async fn fetch_blossom(ctx: &NapContext, url: &str) -> Result<Fetched, Failure> {
    let (sha, hints) = parse_blossom_url(url)?;

    // Size before read: the local store may hold an nsite asset far over the
    // cap (Blossom accepts uploads to 64 MiB), and a `bytesMany` naming it a
    // hundred times must not read it a hundred times to say `too-large`.
    let local_size = ctx
        .blobs
        .size(&sha)
        .await
        .map_err(|e| Failure::new("network-error", format!("local store: {e}")))?;
    if let Some(size) = local_size {
        if size > MAX_BYTES as u64 {
            return Err(Failure::new(
                "too-large",
                format!("{size} bytes, cap is {MAX_BYTES}"),
            ));
        }
    }

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
                .fetch(&sha, MAX_BYTES, &hints)
                .await
                .map_err(|e| Failure::new("network-error", e.to_string()))?
                .ok_or(Failure {
                    code: "not-found",
                    message: None,
                })?;
            // Verified here whatever the fetcher did, then kept: the spec's
            // hash check, and the "anything queried is saved" rule, in the
            // one place every miss passes through. The size is checked
            // **before** the store, so an oversized blob is refused rather
            // than kept and then refused.
            if fetched.len() > MAX_BYTES {
                return Err(Failure::new(
                    "too-large",
                    format!("{} bytes, cap is {MAX_BYTES}", fetched.len()),
                ));
            }
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
        mime: mime.to_string(),
        len: raw.len(),
        sha,
    })
}

/// Resolve one `https:` URL: judge it, answer from memory if this napplet
/// was just delivered it, else fetch through the seam, then sniff, store and
/// remember.
async fn fetch_https(
    ctx: &NapContext,
    napplet: &NappletIdentity,
    url: &str,
) -> Result<Fetched, Failure> {
    let parsed = validate_https_url(url)
        .map_err(|refusal| Failure::new(refusal.code(), refusal.reason()))?;

    // Keyed by the string the napplet wrote, as the spec has cache keys —
    // and answered only while the store still holds the bytes, since the
    // store is what the shell serves them from.
    if let Some(known) = ctx.https_memory.recall(napplet, url) {
        if ctx.blobs.has(&known.sha).await {
            return Ok(known);
        }
    }
    if !ctx.https_memory.admit(napplet) {
        return Err(Failure::new(
            "blocked-by-policy",
            format!("at most {MAX_HTTPS_PER_MINUTE} https: fetches a minute"),
        ));
    }

    // The parser's spelling goes to the dialler, not the napplet's: the host
    // judged above is then the host dialled. The fragment is the page's, not
    // the server's.
    let mut target = parsed;
    target.set_fragment(None);
    let body = ctx
        .https
        .get(target.as_str(), MAX_BYTES)
        .await
        .map_err(|e| Failure::new(e.code.as_str(), e.detail))?;
    // Checked here whatever the seam did: the cap is the spec's promise, and
    // the seam is someone else's code.
    if body.len() > MAX_BYTES {
        return Err(Failure::new(
            "too-large",
            format!("{} bytes, cap is {MAX_BYTES}", body.len()),
        ));
    }
    // Sniffed **before** the store: an SVG is refused, so there is nothing
    // to keep it for.
    let mime = sniff_mime(&body);
    if mime == "image/svg+xml" {
        return Err(Failure::new(
            "blocked-by-policy",
            "SVG is not delivered raw by this runtime",
        ));
    }
    // Unlike a `blossom:` fetch, a failed store is a failed delivery: the
    // store is the only place the shell can fetch these bytes from.
    let sha = ctx
        .blobs
        .put(&body)
        .await
        .map_err(|e| Failure::new("network-error", format!("local store: {e}")))?;
    let fetched = Fetched {
        mime: mime.to_string(),
        len: body.len(),
        sha,
    };
    ctx.https_memory.remember(napplet, url, &fetched);
    Ok(fetched)
}

/// Why [`validate_https_url`] refused a URL, as the spec's two codes for it:
/// a string that is no `https:` URL at all is the napplet's mistake
/// (`invalid-request`); one that is, but points somewhere this device will
/// not go, is the policy's (`blocked-by-policy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsUrlRefusal {
    /// Not a URL this scheme can carry.
    Invalid(String),
    /// A URL the policy will not fetch.
    Blocked(String),
}

impl HttpsUrlRefusal {
    /// The spec's error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid-request",
            Self::Blocked(_) => "blocked-by-policy",
        }
    }

    /// What was wrong, for the log and the error's `message`.
    pub fn reason(&self) -> &str {
        match self {
            Self::Invalid(reason) | Self::Blocked(reason) => reason,
        }
    }
}

impl std::fmt::Display for HttpsUrlRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.reason())
    }
}

/// Whether `url` is an `https:` URL this device will fetch for a napplet, and
/// the parsed form to fetch if so. The runtime's half of the private-address
/// policy; the dialler's half — what a name resolves to — is the
/// [`HttpsFetcher`](crate::seams::HttpsFetcher)'s, which also runs this
/// again on every redirect hop.
///
/// Parsed by the `url` crate, as [`validate_relay_url`] is and for the same
/// reason: it is the parser the HTTP client uses, so the host judged here is
/// the host dialled, and it normalises `127.1`, `2130706433` and
/// `0x7f000001` to `127.0.0.1` before anyone looks. Refused:
///
/// - anything but `https` (`http:` is not canonical, and the spec forbids
///   enabling it by default);
/// - userinfo — a credential the napplet would be making this device send,
///   and the classic way to make a URL read as one host and dial another;
/// - an address literal [`is_private_ip`] calls private: loopback (this
///   device's own relay and Blossom), RFC 1918, link-local (and so the
///   `169.254.169.254` metadata address), unique-local, CGNAT;
/// - a name for this device or the LAN: `localhost`, `.local`, a dotless
///   name a LAN's DNS answers, `.internal`, `.lan`, `.home.arpa`;
/// - a `.fips` mesh name, which reaches a peer by its key and is NAP-MESH's
///   and `blossom:`'s, not the open web's;
/// - over [`MAX_URL_LEN`] bytes.
///
/// [`validate_relay_url`]: crate::nap::outbox::validate_relay_url
/// [`is_private_ip`]: crate::nap::outbox::is_private_ip
pub fn validate_https_url(url: &str) -> Result<nostr::Url, HttpsUrlRefusal> {
    use nostr::types::url::Host;
    if url.len() > MAX_URL_LEN {
        return Err(HttpsUrlRefusal::Invalid(format!(
            "URL is over {MAX_URL_LEN} bytes"
        )));
    }
    let parsed =
        nostr::Url::parse(url).map_err(|e| HttpsUrlRefusal::Invalid(format!("not a URL ({e})")))?;
    if parsed.scheme() != "https" {
        return Err(HttpsUrlRefusal::Invalid(format!(
            "only https: is fetched, not {}:",
            parsed.scheme()
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(HttpsUrlRefusal::Blocked(
            "a URL with userinfo is not fetched".into(),
        ));
    }
    let blocked = |what: &str| HttpsUrlRefusal::Blocked(format!("{what} is not fetched"));
    match parsed.host() {
        None => return Err(HttpsUrlRefusal::Invalid("URL has no host".into())),
        Some(Host::Ipv4(v4)) => {
            if crate::nap::outbox::is_private_ip(std::net::IpAddr::V4(v4)) {
                return Err(blocked("a private address"));
            }
        }
        Some(Host::Ipv6(v6)) => {
            if crate::nap::outbox::is_private_ip(std::net::IpAddr::V6(v6)) {
                return Err(blocked("a private address"));
            }
        }
        Some(Host::Domain(domain)) => {
            // The parser lowercases a domain; a trailing dot is the same name
            // to a resolver, so it must be the same name here.
            let name = domain.trim_end_matches('.');
            if name.ends_with(".fips") || name == "fips" {
                return Err(blocked("a mesh name"));
            }
            if crate::nap::outbox::is_private_name(name)
                || !name.contains('.')
                || [".internal", ".lan", ".home.arpa"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
            {
                return Err(blocked("a local name"));
            }
        }
    }
    Ok(parsed)
}

/// What `https:` URLs each napplet was recently delivered, and how many
/// fetches it has started in the last minute — the runtime's memory for the
/// `https:` scheme. One per device, shared by every session, keyed by
/// napplet identity inside.
///
/// Per napplet, not per device, on purpose: the spec scopes a resource cache
/// to the napplet, and a shared one would let a napplet learn — by how fast
/// an answer came — what another napplet's user had been looking at.
///
/// The memory holds a hash, never bytes: an answer is the `blobRef` and type
/// of what the store already holds, and is only used while it still does.
/// Bounded by [`MAX_HTTPS_REMEMBERED`] and [`HTTPS_REMEMBERED`].
///
/// Not single-flight: two asks for one URL racing each other both reach the
/// seam, which is where that belongs — it is the side with an async runtime
/// to wait on, and the one that knows when two dials are the same.
#[derive(Default)]
pub struct HttpsMemory {
    inner: Mutex<MemoryInner>,
}

#[derive(Default)]
struct MemoryInner {
    recent: HashMap<(NappletIdentity, String), Remembered>,
    started: HashMap<NappletIdentity, VecDeque<Instant>>,
}

struct Remembered {
    sha: String,
    mime: String,
    len: usize,
    at: Instant,
}

impl HttpsMemory {
    /// What `napplet` was delivered for `url` within [`HTTPS_REMEMBERED`].
    fn recall(&self, napplet: &NappletIdentity, url: &str) -> Option<Fetched> {
        let inner = self.inner.lock().unwrap();
        let known = inner.recent.get(&(napplet.clone(), url.to_string()))?;
        (known.at.elapsed() < HTTPS_REMEMBERED).then(|| Fetched {
            mime: known.mime.clone(),
            len: known.len,
            sha: known.sha.clone(),
        })
    }

    fn remember(&self, napplet: &NappletIdentity, url: &str, fetched: &Fetched) {
        let mut inner = self.inner.lock().unwrap();
        let recent = &mut inner.recent;
        if recent.len() >= MAX_HTTPS_REMEMBERED {
            recent.retain(|_, known| known.at.elapsed() < HTTPS_REMEMBERED);
        }
        if recent.len() >= MAX_HTTPS_REMEMBERED {
            if let Some(oldest) = recent
                .iter()
                .min_by_key(|(_, known)| known.at)
                .map(|(key, _)| key.clone())
            {
                recent.remove(&oldest);
            }
        }
        recent.insert(
            (napplet.clone(), url.to_string()),
            Remembered {
                sha: fetched.sha.clone(),
                mime: fetched.mime.clone(),
                len: fetched.len,
                at: Instant::now(),
            },
        );
    }

    /// Count one fetch for `napplet`, or say no if it has started
    /// [`MAX_HTTPS_PER_MINUTE`] in the last minute.
    fn admit(&self, napplet: &NappletIdentity) -> bool {
        const WINDOW: Duration = Duration::from_secs(60);
        let mut inner = self.inner.lock().unwrap();
        // A napplet that went quiet leaves an empty queue behind; sweep them
        // before the map grows past what the recent memory may hold.
        if inner.started.len() > MAX_HTTPS_REMEMBERED {
            inner
                .started
                .retain(|_, at| at.back().is_some_and(|t| t.elapsed() < WINDOW));
        }
        let started = inner.started.entry(napplet.clone()).or_default();
        while started.front().is_some_and(|t| t.elapsed() >= WINDOW) {
            started.pop_front();
        }
        if started.len() >= MAX_HTTPS_PER_MINUTE {
            return false;
        }
        started.push_back(Instant::now());
        true
    }
}

/// The sha256 named by a `blossom:` URL, and where it says to look. Accepted:
///
/// - `blossom:sha256:<hex>`, this runtime's canonical form;
/// - `blossom:<hex>`, the shim's examples;
/// - BUD-10's `blossom:<hex>.<ext>?xs=<server>&as=<pubkey>&sz=<bytes>`, as
///   napplets rewrite `https://` media links into. The extension is ignored
///   (the bytes are sniffed); `xs` and `as` become [`BlobHints`], and any
///   other parameter is ignored.
///
/// Anything else is the spec's `unsupported-scheme`, with a malformed
/// blossom URL as `invalid-request`.
///
/// [`BlobHints`]: crate::seams::BlobHints
fn parse_blossom_url(url: &str) -> Result<(String, crate::seams::BlobHints), Failure> {
    let Some(rest) = url.strip_prefix("blossom:") else {
        return Err(Failure::new(
            "unsupported-scheme",
            format!(
                "only blossom: and https: are supported, not {}",
                scheme_of(url)
            ),
        ));
    };
    let rest = rest.strip_prefix("sha256:").unwrap_or(rest);
    let rest = rest.split('#').next().unwrap_or("");
    let (name, query) = rest.split_once('?').unwrap_or((rest, ""));
    let name = name.split('/').next().unwrap_or("");
    let hex = name.split_once('.').map_or(name, |(hex, _ext)| hex);
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Failure::new(
            "invalid-request",
            "blossom URL must name a sha256 hex",
        ));
    }
    Ok((hex.to_ascii_lowercase(), blob_hints(query)))
}

/// Most hints of each kind taken from one URL: a napplet may name a handful
/// of places, not make the fetcher walk a list it wrote.
const MAX_HINTS: usize = 4;

/// The `xs` servers and `as` authors in a BUD-10 query string. A server is a
/// domain (`https://` is added) or an `https://` URL; an author is 64 hex.
/// Anything malformed is dropped, never an error: hints are only hints.
fn blob_hints(query: &str) -> crate::seams::BlobHints {
    let mut hints = crate::seams::BlobHints::default();
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let value = percent_decode(value);
        match key {
            "xs" if hints.servers.len() < MAX_HINTS => {
                if let Some(server) = hint_server(&value) {
                    if !hints.servers.contains(&server) {
                        hints.servers.push(server);
                    }
                }
            }
            "as" if hints.authors.len() < MAX_HINTS => {
                let pk = value.to_ascii_lowercase();
                if pk.len() == 64
                    && pk.chars().all(|c| c.is_ascii_hexdigit())
                    && !hints.authors.contains(&pk)
                {
                    hints.authors.push(pk);
                }
            }
            _ => {}
        }
    }
    hints
}

/// A server hint as a base URL, or `None` if it is not one. Only a host
/// (and port) is kept: a path in a hint would let a napplet steer the fetch
/// at an arbitrary URL rather than a Blossom server's `/<sha256>`.
fn hint_server(value: &str) -> Option<String> {
    let (scheme, host) = match value.split_once("://") {
        Some(("https", rest)) => ("https", rest),
        Some(_) => return None,
        None => ("https", value),
    };
    let host = host.split(['/', '?', '#']).next().unwrap_or("");
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    valid.then(|| format!("{scheme}://{}", host.to_ascii_lowercase()))
}

/// `%XX` escapes decoded; a malformed escape is kept as written.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
        // AVIF and HEIC pictures are ISO-BMFF too: the major brand after
        // `ftyp` tells a picture from a video.
        return match &bytes[8..12] {
            b"avif" | b"avis" => "image/avif",
            b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1" => "image/heic",
            _ => "video/mp4",
        };
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
    // XML is text, and text would be delivered. Whether the body is text is
    // decided on its first kilobyte — a multibyte character cut by the window
    // is tolerated — and the search then runs over the **whole** body: an XML
    // prolog or a comment can put `<svg` anywhere. Byte windows, no
    // allocation, O(n) over at most `MAX_BYTES`.
    let head = &bytes[..bytes.len().min(1024)];
    let head_is_text = match std::str::from_utf8(head) {
        Ok(_) => true,
        // `error_len() == None` is an incomplete sequence at the very end of
        // the window — a character the cut split, not bad UTF-8.
        Err(e) => e.error_len().is_none() && head.len() - e.valid_up_to() < 4,
    };
    if head_is_text && bytes.windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg")) {
        return "image/svg+xml";
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
        assert_eq!(r["blobRef"], sha);
        assert!(r.get("blob").is_none(), "bytes crossed the JSON channel");
        assert!(
            fetcher.asked().is_empty(),
            "the fetcher was asked for a stored blob"
        );
    }

    /// A delivered blob can be kept in this device's own Blossom; one never
    /// delivered cannot.
    #[tokio::test]
    async fn keep_takes_only_a_delivered_blob() {
        let (mut ctx, _fetcher) = test_context_with_fetcher();
        let kept = std::sync::Arc::new(nsite_deck::testing::MemBlobs::new());
        ctx.kept_blobs = kept.clone();
        let shown = ctx.blobs.put(PNG).await.unwrap();
        let hidden = ctx.blobs.put(b"not shown").await.unwrap();
        let mut s = Session::new(
            NappletIdentity::new("pics", "aggregate"),
            ["resource", "local"],
        );
        s.on_ready();
        let ask = |id: &str, sha: &str| {
            Envelope::new("resource.keep")
                .with_id(id)
                .with_field("url", format!("blossom:sha256:{sha}"))
        };

        let r = dispatch(&ctx, &mut s, &ask("k0", &shown)).await;
        assert_eq!(
            r.envelopes()[0].msg_type,
            "resource.keep.error",
            "kept before shown"
        );

        dispatch(
            &ctx,
            &mut s,
            &Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{shown}")),
        )
        .await;
        let r = dispatch(&ctx, &mut s, &ask("k1", &shown)).await;
        let r = serde_json::to_value(&r.envelopes()[0]).unwrap();
        assert_eq!(r["type"], "resource.keep.result", "{r}");
        assert!(crate::seams::BlobStore::has(kept.as_ref(), &shown).await);

        let r = dispatch(&ctx, &mut s, &ask("k2", &hidden)).await;
        assert_eq!(r.envelopes()[0].msg_type, "resource.keep.error");

        // Switching `local` off stops keeping, whatever was delivered.
        s.set_granted(["resource"]);
        let r = dispatch(&ctx, &mut s, &ask("k3", &shown)).await;
        let r = serde_json::to_value(&r.envelopes()[0]).unwrap();
        assert_eq!(r["type"], "resource.keep.error");
        assert!(r["message"].as_str().unwrap().contains("local"));
        assert!(!crate::seams::BlobStore::has(kept.as_ref(), &hidden).await);
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

    /// A BUD-10 URL — extension, `xs`, `as` — is fetched by its hash, and
    /// the fetcher is told where the URL said to look.
    #[tokio::test]
    async fn a_bud10_url_carries_its_hints_to_the_fetcher() {
        let (ctx, fetcher) = test_context_with_fetcher();
        let sha = fetcher.hold(PNG);
        let author = "AB".repeat(32);
        let r = call(
            &ctx,
            Envelope::new("resource.bytes").with_id("b1").with_field(
                "url",
                format!(
                    "blossom:{sha}.png?xs=cdn.example.com&xs=https%3A%2F%2Fb.example%3A8443%2Fignored\
                     &xs=ftp://nope&as={author}&as=short&sz=12"
                ),
            ),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.result", "{r}");
        assert_eq!(fetcher.asked(), vec![sha]);
        assert_eq!(
            fetcher.hints(),
            vec![crate::seams::BlobHints {
                servers: vec![
                    "https://cdn.example.com".into(),
                    "https://b.example:8443".into()
                ],
                authors: vec!["ab".repeat(32)],
            }]
        );
    }

    #[test]
    fn blossom_urls_parse_with_and_without_hints() {
        let hex = "c".repeat(64);
        for url in [
            format!("blossom:{hex}"),
            format!("blossom:sha256:{hex}"),
            format!("blossom:{hex}.jpg"),
            format!("blossom:{hex}.jpg?sz=10#frag"),
        ] {
            let (sha, hints) = parse_blossom_url(&url).ok().unwrap();
            assert_eq!(sha, hex, "{url}");
            assert_eq!(hints, crate::seams::BlobHints::default(), "{url}");
        }
        assert!(parse_blossom_url(&format!("blossom:{}.jpg", "c".repeat(63))).is_err());
        let many: String = (0..10).map(|i| format!("&xs=s{i}.example")).collect();
        let (_, hints) = parse_blossom_url(&format!("blossom:{hex}?{many}"))
            .ok()
            .unwrap();
        assert_eq!(hints.servers.len(), MAX_HINTS);
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
            ("http://example.com/a.png", "unsupported-scheme"),
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

    /// `<svg` past the first kilobyte is still SVG: the sniff runs over the
    /// whole body, so a long prolog or comment cannot turn it into deliverable
    /// XML — L6 of the PR #52 review.
    #[tokio::test]
    async fn an_svg_past_the_first_kilobyte_is_still_svg() {
        let mut svg = b"<?xml version=\"1.0\"?>\n<!-- ".to_vec();
        svg.extend(std::iter::repeat_n(b'x', 1_100));
        svg.extend_from_slice(
            b" -->\n<svg xmlns=\"http://www.w3.org/2000/svg\"><script>1</script></svg>",
        );
        assert_eq!(sniff_mime(&svg), "image/svg+xml");
        // Case does not hide it either.
        let shouted = String::from_utf8(svg.clone())
            .unwrap()
            .replace("<svg", "<SVG");
        assert_eq!(sniff_mime(shouted.as_bytes()), "image/svg+xml");
        // A split multibyte character at the window's edge is still text.
        let mut split = vec![b' '; 1_023];
        split.extend_from_slice("é".as_bytes());
        split.extend_from_slice(b"<svg/>");
        assert_eq!(sniff_mime(&split), "image/svg+xml");
        // And XML without an svg stays XML.
        let mut xml = b"<?xml version=\"1.0\"?><!-- ".to_vec();
        xml.extend(std::iter::repeat_n(b'x', 1_100));
        xml.extend_from_slice(b" --><doc/>");
        assert_eq!(sniff_mime(&xml), "application/xml");

        let (ctx, _fetcher) = test_context_with_fetcher();
        let sha = ctx.blobs.put(&svg).await.unwrap();
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{sha}")),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.error");
        assert_eq!(r["error"], "blocked-by-policy");
    }

    /// A [`BlobStore`](nsite_deck::seams::BlobStore) that counts reads, so a
    /// test can prove a blob was refused from its size alone.
    struct CountingBlobs {
        inner: nsite_deck::testing::MemBlobs,
        gets: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl nsite_deck::seams::BlobStore for CountingBlobs {
        async fn has(&self, sha256_hex: &str) -> bool {
            self.inner.has(sha256_hex).await
        }
        async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.gets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.get(sha256_hex).await
        }
        async fn size(&self, sha256_hex: &str) -> anyhow::Result<Option<u64>> {
            self.inner.size(sha256_hex).await
        }
        async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
            self.inner.put(bytes).await
        }
        async fn wipe(&self) -> anyhow::Result<()> {
            self.inner.wipe().await
        }
    }

    /// An oversized blob already in the local store is refused from its size
    /// and never read: `bytesMany` naming one 64 MiB asset a hundred times
    /// must not allocate it a hundred times to say `too-large` — L8 of the
    /// PR #52 review.
    #[tokio::test]
    async fn an_oversized_local_blob_is_refused_without_being_read() {
        use nsite_deck::seams::BlobStore as _;

        let (mut ctx, fetcher) = test_context_with_fetcher();
        let counting = std::sync::Arc::new(CountingBlobs {
            inner: nsite_deck::testing::MemBlobs::new(),
            gets: std::sync::atomic::AtomicUsize::new(0),
        });
        ctx.blobs = counting.clone();
        let big = vec![9u8; MAX_BYTES + 1];
        let sha = counting.put(&big).await.unwrap();
        let small = counting.put(PNG).await.unwrap();

        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m1")
                .with_field(
                    "urls",
                    json!([
                        format!("blossom:sha256:{sha}"),
                        format!("blossom:sha256:{sha}"),
                        format!("blossom:sha256:{small}"),
                    ]),
                ),
        )
        .await;
        let items = r["items"].as_array().unwrap();
        assert_eq!(items[0]["error"], "too-large");
        assert_eq!(items[1]["error"], "too-large");
        assert_eq!(
            items[2]["ok"], true,
            "the small blob beside it is delivered"
        );
        assert_eq!(
            counting.gets.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the oversized blob was read"
        );
        assert!(fetcher.asked().is_empty(), "a stored blob was fetched");
    }

    /// Bulk: order and length preserved, one failure beside its successful
    /// siblings; an empty list or too many is a top-level error.
    /// A blob over the cap is refused before it is stored: the store must not
    /// end up holding what the napplet was told it could not have.
    #[tokio::test]
    async fn an_oversized_fetch_is_refused_and_not_kept() {
        let (ctx, fetcher) = crate::testing::test_context_with_fetcher();
        let big = vec![7u8; MAX_BYTES + 1];
        let sha = nsite_deck::sync::sha256_hex(&big);
        // The fetcher ignores the cap, as a misbehaving one might.
        fetcher.ignore_cap();
        fetcher.lie(&sha, &big);
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", format!("blossom:sha256:{sha}")),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.error");
        assert_eq!(r["error"], "too-large");
        assert!(!ctx.blobs.has(&sha).await, "the oversized blob was kept");
    }

    /// `bytesMany` stops fetching once the reply is already at the total cap;
    /// the rest are answered `too-large` without a fetch.
    #[tokio::test]
    async fn bytes_many_stops_at_the_total_cap() {
        let (ctx, fetcher) = crate::testing::test_context_with_fetcher();
        // Five eighths of the total each: the first fits, the second crosses.
        let chunk = vec![1u8; MAX_TOTAL_BYTES / 8 * 5];
        let held: Vec<String> = (0..3)
            .map(|i| {
                let mut b = chunk.clone();
                b[0] = i;
                fetcher.hold(&b)
            })
            .collect();
        let urls: Vec<String> = held.iter().map(|h| format!("blossom:sha256:{h}")).collect();
        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m1")
                .with_field("urls", urls),
        )
        .await;
        let items = r["items"].as_array().unwrap();
        let ok: Vec<bool> = items.iter().map(|i| i["ok"].as_bool().unwrap()).collect();
        // The first fits, the second crosses the line and is delivered, the
        // third is refused unfetched.
        assert_eq!(ok, vec![true, true, false], "{items:?}");
        assert_eq!(items[2]["error"], "too-large");
        assert_eq!(
            fetcher.asked().len(),
            2,
            "the third blob was fetched anyway"
        );
    }

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
                        "http://example.com/x",
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
    async fn info_says_blossom_and_https() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let r = call(&ctx, Envelope::new("resource.info").with_id("i1")).await;
        assert_eq!(r["type"], "resource.info.result");
        let schemes = r["info"]["schemes"].as_array().unwrap();
        let enabled: Vec<&str> = schemes
            .iter()
            .filter(|s| s["enabled"] == true)
            .map(|s| s["scheme"].as_str().unwrap())
            .collect();
        assert_eq!(enabled, vec!["blossom", "https"]);
        assert_eq!(r["info"]["maxBytes"], MAX_BYTES);
    }

    /// The URL gate: public `https:` passes; every way of pointing the shell
    /// at itself, its LAN or the mesh does not — shorthands included, since
    /// the parser normalises them before they are judged.
    #[test]
    fn https_urls_are_judged_before_anything_is_dialled() {
        for url in [
            "https://example.com/a.png",
            "https://cdn.example.com:8443/p/a.jpg?w=64#top",
            "https://93.184.215.14/a.png",
            "https://[2606:4700::1]/a.png",
            "HTTPS://Example.COM/a.png",
        ] {
            assert!(validate_https_url(url).is_ok(), "{url} was refused");
        }
        for (url, code) in [
            ("http://example.com/a.png", "invalid-request"),
            ("ftp://example.com/a.png", "invalid-request"),
            ("https://", "invalid-request"),
            ("not a url", "invalid-request"),
            ("https://user:pw@example.com/a.png", "blocked-by-policy"),
            ("https://user@example.com/a.png", "blocked-by-policy"),
            ("https://127.0.0.1/a.png", "blocked-by-policy"),
            ("https://127.1/a.png", "blocked-by-policy"),
            ("https://2130706433/a.png", "blocked-by-policy"),
            ("https://0x7f000001/a.png", "blocked-by-policy"),
            ("https://10.0.0.8/a.png", "blocked-by-policy"),
            ("https://192.168.1.1/a.png", "blocked-by-policy"),
            (
                "https://169.254.169.254/latest/meta-data",
                "blocked-by-policy",
            ),
            ("https://100.64.0.1/a.png", "blocked-by-policy"),
            ("https://0.0.0.0/a.png", "blocked-by-policy"),
            ("https://[::1]/a.png", "blocked-by-policy"),
            ("https://[fe80::1]/a.png", "blocked-by-policy"),
            ("https://[fd00::1]/a.png", "blocked-by-policy"),
            ("https://[::ffff:127.0.0.1]/a.png", "blocked-by-policy"),
            ("https://localhost/a.png", "blocked-by-policy"),
            ("https://LOCALHOST./a.png", "blocked-by-policy"),
            ("https://shell.localhost/a.png", "blocked-by-policy"),
            ("https://printer.local/a.png", "blocked-by-policy"),
            ("https://router/a.png", "blocked-by-policy"),
            ("https://nas.lan/a.png", "blocked-by-policy"),
            ("https://npub1peer.fips/a.png", "blocked-by-policy"),
            ("https://npub1peer.fips:24243/a.png", "blocked-by-policy"),
        ] {
            match validate_https_url(url) {
                Ok(_) => panic!("{url} was accepted"),
                Err(refusal) => assert_eq!(refusal.code(), code, "{url}: {refusal}"),
            }
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(
            validate_https_url(&long).unwrap_err().code(),
            "invalid-request"
        );
    }

    /// A public `https:` URL is fetched through the seam, stored, and
    /// delivered by `blobRef` with a sniffed type — never the bytes.
    #[tokio::test]
    async fn an_https_url_is_fetched_stored_and_delivered_by_ref() {
        let (ctx, https) = crate::testing::test_context_with_https();
        let url = "https://cdn.example.com/avatar.png#frag";
        https.serve("https://cdn.example.com/avatar.png", PNG);
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("h1")
                .with_field("url", url),
        )
        .await;
        assert_eq!(r["type"], "resource.bytes.result", "{r}");
        let sha = sha256_hex(PNG);
        assert_eq!(r["blobRef"], sha);
        assert_eq!(r["mime"], "image/png");
        assert!(r.get("blob").is_none(), "bytes crossed the JSON channel");
        assert!(ctx.blobs.has(&sha).await, "the shell has nothing to serve");
        // The fragment is the page's, not the server's.
        assert_eq!(https.asked(), vec!["https://cdn.example.com/avatar.png"]);
    }

    /// The same URL asked again by the same napplet is answered from memory;
    /// another napplet asking it goes to the server itself.
    #[tokio::test]
    async fn a_repeated_https_url_is_not_refetched_for_the_same_napplet() {
        let (ctx, https) = crate::testing::test_context_with_https();
        let url = "https://cdn.example.com/a.png";
        https.serve(url, PNG);
        let ask = |id: &str| {
            Envelope::new("resource.bytes")
                .with_id(id)
                .with_field("url", url)
        };
        let mut pics = granted();
        for id in ["h1", "h2"] {
            let r = dispatch(&ctx, &mut pics, &ask(id)).await;
            assert_eq!(r.envelopes()[0].msg_type, "resource.bytes.result");
        }
        assert_eq!(https.asked().len(), 1, "the second ask was refetched");

        let mut other = Session::new(NappletIdentity::new("other", "agg2"), ["resource"]);
        other.on_ready();
        let r = dispatch(&ctx, &mut other, &ask("h3")).await;
        assert_eq!(r.envelopes()[0].msg_type, "resource.bytes.result");
        assert_eq!(
            https.asked().len(),
            2,
            "one napplet's memory answered another"
        );

        // Bytes gone from the store are fetched again, memory or not.
        ctx.blobs.wipe().await.unwrap();
        let r = dispatch(&ctx, &mut pics, &ask("h4")).await;
        assert_eq!(r.envelopes()[0].msg_type, "resource.bytes.result");
        assert_eq!(https.asked().len(), 3);
    }

    /// The seam's failures reach the napplet as the spec's codes; a private
    /// host never reaches the seam at all.
    #[tokio::test]
    async fn https_failures_carry_the_spec_codes() {
        use crate::seams::HttpsErrorCode;
        let (ctx, https) = crate::testing::test_context_with_https();
        https.serve("https://big.example.com/x", &vec![0u8; MAX_BYTES + 1]);
        https.fail("https://slow.example.com/x", HttpsErrorCode::Timeout);
        https.fail("https://down.example.com/x", HttpsErrorCode::NetworkError);
        https.fail("https://off.example.com/x", HttpsErrorCode::BlockedByPolicy);
        for (url, code) in [
            ("https://gone.example.com/x", "not-found"),
            ("https://big.example.com/x", "too-large"),
            ("https://slow.example.com/x", "timeout"),
            ("https://down.example.com/x", "network-error"),
            ("https://off.example.com/x", "blocked-by-policy"),
            ("https://127.0.0.1/x", "blocked-by-policy"),
            ("https://u@example.com/x", "blocked-by-policy"),
        ] {
            let r = call(
                &ctx,
                Envelope::new("resource.bytes")
                    .with_id("h1")
                    .with_field("url", url),
            )
            .await;
            assert_eq!(r["type"], "resource.bytes.error", "{url}");
            assert_eq!(r["error"], code, "{url}");
        }
        assert!(
            !https
                .asked()
                .iter()
                .any(|u| u.contains("127.0.0.1") || u.contains('@')),
            "a refused URL reached the seam: {:?}",
            https.asked()
        );
        assert!(!ctx.blobs.has(&sha256_hex(&vec![0u8; MAX_BYTES + 1])).await);
    }

    /// An SVG from the web is refused like one from Blossom — and not kept.
    #[tokio::test]
    async fn an_https_svg_is_blocked_and_not_stored() {
        let (ctx, https) = crate::testing::test_context_with_https();
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>1</script></svg>";
        https.serve("https://example.com/logo.png", svg);
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("h1")
                .with_field("url", "https://example.com/logo.png"),
        )
        .await;
        assert_eq!(r["error"], "blocked-by-policy");
        assert!(!ctx.blobs.has(&sha256_hex(svg)).await);
    }

    /// `bytesMany` mixes schemes, item by item; the rate counts each URL
    /// that reaches the network.
    #[tokio::test]
    async fn bytes_many_mixes_schemes_and_the_rate_counts_per_url() {
        let (ctx, https) = crate::testing::test_context_with_https();
        let local = ctx.blobs.put(PNG).await.unwrap();
        https.serve("https://example.com/a.txt", b"hello");
        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m1")
                .with_field(
                    "urls",
                    json!([
                        format!("blossom:sha256:{local}"),
                        "https://example.com/a.txt",
                        "https://10.0.0.1/a.txt",
                    ]),
                ),
        )
        .await;
        let items = r["items"].as_array().unwrap();
        assert_eq!(items[0]["mime"], "image/png");
        assert_eq!(items[1]["mime"], "text/plain");
        assert_eq!(items[1]["blobRef"], sha256_hex(b"hello"));
        assert_eq!(items[2]["error"], "blocked-by-policy");

        // One fetch spent above; the rest of the minute's budget, then no.
        let urls: Vec<String> = (0..MAX_HTTPS_PER_MINUTE)
            .map(|i| format!("https://example.com/{i}"))
            .collect();
        let r = call(
            &ctx,
            Envelope::new("resource.bytesMany")
                .with_id("m2")
                .with_field("urls", urls),
        )
        .await;
        let items = r["items"].as_array().unwrap();
        assert_eq!(items[MAX_HTTPS_PER_MINUTE - 2]["error"], "not-found");
        assert_eq!(
            items[MAX_HTTPS_PER_MINUTE - 1]["error"],
            "blocked-by-policy"
        );
        assert_eq!(https.asked().len(), MAX_HTTPS_PER_MINUTE);
    }

    /// With nothing wired behind the seam, `https:` is a policy refusal, not
    /// a scheme this build lacks.
    #[tokio::test]
    async fn no_https_behind_the_seam_is_blocked_by_policy() {
        let (ctx, _fetcher) = test_context_with_fetcher();
        let r = call(
            &ctx,
            Envelope::new("resource.bytes")
                .with_id("h1")
                .with_field("url", "https://example.com/a.png"),
        )
        .await;
        assert_eq!(r["error"], "blocked-by-policy");
    }

    #[test]
    fn sniffing_is_by_bytes() {
        assert_eq!(sniff_mime(PNG), "image/png");
        assert_eq!(sniff_mime(b"\xff\xd8\xff\xe0JFIF"), "image/jpeg");
        assert_eq!(sniff_mime(b"GIF89a"), "image/gif");
        assert_eq!(sniff_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "image/webp");
        assert_eq!(
            sniff_mime(b"\x00\x00\x00\x1cftypavif\x00\x00\x00\x00"),
            "image/avif"
        );
        assert_eq!(
            sniff_mime(b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00"),
            "image/heic"
        );
        assert_eq!(
            sniff_mime(b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00"),
            "video/mp4"
        );
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
        assert_eq!(r["type"], "resource.bytes.error");
        assert_eq!(r["error"], "blocked-by-policy");
        assert!(r["message"].as_str().unwrap().contains("not granted"));
    }
}
