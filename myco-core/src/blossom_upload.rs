//! NAP-UPLOAD's uploader (spec: napplet/naps PR #33; Myco's choices in
//! `docs/design/napplet/NAP-UPLOAD.md`): a napplet's bytes onto the
//! **user's** Blossom servers, signed as the user.
//!
//! The runtime crate has already checked the grant, capped and hashed the
//! bytes and settled the type ([`myco_napplet_runtime::nap::upload`]). What
//! happens here:
//!
//! 1. **Servers.** The user's BUD-03 list (kind 10063, `server` tags): this
//!    device's relay first, then the user's own relays (their NIP-65 write
//!    relays, through the outbox lanes) when the internet is up. With no
//!    list, [`DEFAULT_SERVERS`].
//! 2. **Authorization.** A BUD-02 kind-24242 event — `t` `upload`, `x` the
//!    hash, `expiration` five minutes out — signed through the napplet
//!    [`Signer`]: the user key, guest or a NIP-55 signer app (which may ask
//!    the user). Sent as `Authorization: Nostr <base64(event json)>`.
//! 3. **Upload.** `PUT <server>/upload`, one server at a time, until one
//!    answers 2xx with a blob descriptor. What it says it stored is what is
//!    reported — a hash other than ours is a transform, reported as both
//!    hashes, never success the server did not confirm. Then one more server
//!    is tried as a mirror, briefly; it becomes `fallbackUrls`.
//!
//! Offline-only refuses at once, and so does an internet the breaker says is
//! down. Every stage is bounded; the sum stays under the prelude's
//! [`myco_napplet_runtime::SIGNING_TIMEOUT`], so the napplet hears a clear
//! failure rather than "timed out".

use std::sync::Arc;
use std::time::Duration;

use myco_napplet_runtime::seams::{
    Direction, LaneTransport, OutboxResolver, RelayBackend, Signer, UploadBlob, UploadError,
    UploadErrorCode, UploadSink, Uploaded,
};
use nostr::base64::prelude::{Engine as _, BASE64_STANDARD};
use nostr::{Event, EventBuilder, Kind, PublicKey, Tag, UnsignedEvent};

/// Where an upload goes when the user has no server list: public Blossom
/// servers that take uploads from any key. `blossom.primal.net` is also in
/// the read defaults (`ip_source::default_blossom_servers`); the rest of those
/// are read replicas that do not take uploads from strangers.
pub const DEFAULT_SERVERS: &[&str] = &["https://blossom.primal.net", "https://cdn.hzrd149.com"];

/// BUD-02's authorization kind.
const KIND_BLOSSOM_AUTH: u16 = 24242;
/// BUD-03's server list kind.
const KIND_SERVER_LIST: u16 = 10_063;
/// How long the authorization is valid. It names one blob, and is minted
/// right before use.
const AUTH_TTL_SECS: u64 = 300;
/// The largest blob descriptor read from a server.
const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;
/// The most servers from a list that are tried.
const MAX_SERVERS: usize = 5;

/// How long finding the user's server list may take, both lookups together.
const LIST_LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the uploads to the primary candidates may take, together.
const UPLOAD_BUDGET: Duration = Duration::from_secs(60);
/// How long the mirror gets once the upload landed.
const MIRROR_BUDGET: Duration = Duration::from_secs(15);
/// How long one server gets to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one upload may go without progress.
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// A yes/no the uploader asks per call — offline-only, the internet breaker.
type Gate = Arc<dyn Fn() -> bool + Send + Sync>;

/// The [`UploadSink`] Myco wires into the napplet runtime.
pub struct BlossomUploader {
    /// This device's relay, asked first for the user's server list.
    relay: Arc<dyn RelayBackend>,
    /// The user switched the internet off (`Content::is_offline_only`).
    offline_only: Gate,
    /// The internet looks down right now (`Content::internet_looks_down`).
    internet_down: Gate,
    signer: Arc<dyn Signer>,
    outbox: Arc<dyn OutboxResolver>,
    lanes: Arc<dyn LaneTransport>,
    defaults: Vec<String>,
    http: reqwest::Client,
    /// Only `https://` servers whose names resolve to public addresses. Off
    /// only in tests, whose "Blossom" is on `127.0.0.1` over plain HTTP.
    guard: bool,
}

impl BlossomUploader {
    /// Over the content layer: its relay, its offline-only setting and its
    /// internet breaker, read per call.
    pub fn new(
        content: Arc<crate::content::Content>,
        signer: Arc<dyn Signer>,
        outbox: Arc<dyn OutboxResolver>,
        lanes: Arc<dyn LaneTransport>,
    ) -> Self {
        let (offline, down) = (content.clone(), content.clone());
        Self::with_parts(
            content.relay(),
            Arc::new(move || offline.is_offline_only()),
            Arc::new(move || down.internet_looks_down()),
            signer,
            outbox,
            lanes,
        )
    }

    fn with_parts(
        relay: Arc<dyn RelayBackend>,
        offline_only: Gate,
        internet_down: Gate,
        signer: Arc<dyn Signer>,
        outbox: Arc<dyn OutboxResolver>,
        lanes: Arc<dyn LaneTransport>,
    ) -> Self {
        Self {
            relay,
            offline_only,
            internet_down,
            signer,
            outbox,
            lanes,
            defaults: DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect(),
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                // Never follow a redirect: the public-address check in `put`
                // covers the URL it was given, and a redirect could send the
                // signed upload somewhere it never looked — this phone's own
                // relay or Blossom on loopback, say. A 3xx is that server's
                // refusal, and the next server is tried.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            guard: true,
        }
    }

    /// Use `servers` as the defaults and allow plain HTTP to loopback — for
    /// tests, which must never reach the internet.
    #[cfg(test)]
    fn for_test(mut self, servers: Vec<String>) -> Self {
        self.defaults = servers;
        self.guard = false;
        self
    }

    /// The servers to try, in order: the user's list, else the defaults.
    async fn servers_for(&self, user: PublicKey) -> Vec<String> {
        let found = tokio::time::timeout(LIST_LOOKUP_TIMEOUT, self.server_list(user))
            .await
            .unwrap_or_default();
        let servers = servers_from_lists(&found, self.guard);
        if servers.is_empty() {
            self.defaults.clone()
        } else {
            servers
        }
    }

    /// The user's kind 10063: this device's copy, else their relays'.
    async fn server_list(&self, user: PublicKey) -> Vec<Event> {
        let filter = nostr::Filter::new()
            .kind(Kind::from(KIND_SERVER_LIST))
            .author(user)
            .limit(1);
        if let Ok(held) = self.relay.query(std::slice::from_ref(&filter)).await {
            if !held.is_empty() {
                return held;
            }
        }
        let plan = self.outbox.plan(Direction::Read, &[user]).await;
        let lanes = myco_napplet_runtime::seams::remote_lanes(plan.lanes);
        if lanes.is_empty() {
            return Vec::new();
        }
        self.lanes
            .query(&lanes, &[filter], LIST_LOOKUP_TIMEOUT)
            .await
            .into_iter()
            .flat_map(|(_, events)| events.unwrap_or_default())
            .filter(|e| e.pubkey == user && e.kind == Kind::from(KIND_SERVER_LIST))
            .collect()
    }

    /// One `PUT <server>/upload`: what the server confirmed it stored.
    async fn put(&self, server: &str, blob: &UploadBlob, auth: &str) -> Result<Stored, PutFailure> {
        if self.guard
            && (!server.starts_with("https://") || !crate::outbox::dials_public(server).await)
        {
            return Err(PutFailure::Failed("not a public https server".into()));
        }
        let mut response = self
            .http
            .put(format!("{server}/upload"))
            .header(reqwest::header::AUTHORIZATION, auth)
            .header(reqwest::header::CONTENT_TYPE, &blob.mime)
            .body(blob.bytes.clone())
            .send()
            .await
            .map_err(|e| {
                PutFailure::Failed(if e.is_timeout() {
                    "timed out".into()
                } else if e.is_connect() {
                    "could not connect".into()
                } else {
                    "the connection failed".into()
                })
            })?;
        let status = response.status();
        if !status.is_success() {
            // BUD-01: a refusal says why in `X-Reason`.
            let reason = response
                .headers()
                .get("x-reason")
                .and_then(|v| v.to_str().ok())
                .map(|r| r.chars().take(200).collect::<String>())
                .unwrap_or_default();
            return Err(PutFailure::Rejected(status.as_u16(), reason));
        }
        // A descriptor is a few hundred bytes; a server sending more is not
        // answering this.
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| PutFailure::Failed("the answer was cut off".into()))?
        {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_DESCRIPTOR_BYTES {
                return Err(PutFailure::Failed(
                    "the answer was not a blob descriptor".into(),
                ));
            }
        }
        descriptor(&body, server, blob.bytes.len() as u64)
    }

    async fn upload_inner(&self, blob: &UploadBlob) -> Result<Uploaded, UploadError> {
        let user = self.signer.public_key().await.map_err(|_| {
            UploadError::new(
                UploadErrorCode::PolicyDenied,
                "not signed in: an upload is signed as you",
            )
        })?;
        let servers = self.servers_for(user).await;
        if servers.is_empty() {
            return Err(UploadError::new(
                UploadErrorCode::NoServerConfigured,
                "no Blossom server to try",
            ));
        }
        let template = auth_template(
            user,
            &blob.sha256,
            blob.filename.as_deref(),
            crate::content::now_secs(),
        );
        // The signer bounds itself (a signer app gets its own answer
        // timeout). A signer app's "no" is `external_signer::REJECTED`.
        let auth = self.signer.sign(template).await.map_err(|e| {
            let said_no = said_no(&e);
            UploadError::new(
                if said_no {
                    UploadErrorCode::UserCancelled
                } else {
                    UploadErrorCode::UploadFailed
                },
                format!("the upload was not signed: {e}"),
            )
        })?;
        let header = authorization_header(&auth);

        let mut failures: Vec<PutFailure> = Vec::new();
        let mut described: Vec<String> = Vec::new();
        let primary = async {
            for (i, server) in servers.iter().enumerate() {
                match self.put(server, blob, &header).await {
                    Ok(stored) => return Some((i, stored)),
                    Err(why) => {
                        described.push(why.describe(server));
                        failures.push(why);
                    }
                }
            }
            None
        };
        let landed = match tokio::time::timeout(UPLOAD_BUDGET, primary).await {
            Ok(found) => found,
            Err(_) => {
                failures.push(PutFailure::Failed("ran out of time".into()));
                described.push("ran out of time".into());
                None
            }
        };
        let Some((i, stored)) = landed else {
            return Err(UploadError::new(
                failure_code(&failures),
                described.join("; "),
            ));
        };
        tracing::info!(server = %host_of(&servers[i]), url = %stored.url, "napplet upload landed");

        // One mirror, briefly: a second place for the same stored bytes. A
        // mirror that stored something else is not the same file.
        let mirror = async {
            for server in servers.iter().skip(i + 1) {
                if let Ok(copy) = self.put(server, blob, &header).await {
                    if copy.sha256 == stored.sha256 {
                        return Some(copy.url);
                    }
                }
            }
            None
        };
        let fallback_urls = tokio::time::timeout(MIRROR_BUDGET, mirror)
            .await
            .ok()
            .flatten()
            .into_iter()
            .collect();
        Ok(Uploaded {
            url: stored.url,
            fallback_urls,
            sha256: stored.sha256,
            size: stored.size,
            mime: stored.mime,
        })
    }
}

#[async_trait::async_trait]
impl UploadSink for BlossomUploader {
    async fn available(&self) -> bool {
        !(self.offline_only)()
    }

    async fn upload(&self, blob: &UploadBlob) -> Result<Uploaded, UploadError> {
        if (self.offline_only)() {
            return Err(UploadError::new(
                UploadErrorCode::PolicyDenied,
                "offline-only is on in Settings, so nothing is uploaded to the internet",
            ));
        }
        if (self.internet_down)() {
            return Err(UploadError::new(
                UploadErrorCode::UploadFailed,
                "no internet right now: an upload has to reach a Blossom server",
            ));
        }
        self.upload_inner(blob).await
    }
}

/// The servers in the newest of `lists` (BUD-03), in the order it gives
/// them: trailing slash trimmed, deduplicated, at most [`MAX_SERVERS`].
/// With `https_only`, anything but `https://` is skipped; userinfo, queries
/// and fragments always are.
pub fn servers_from_lists(lists: &[Event], https_only: bool) -> Vec<String> {
    let newest = lists
        .iter()
        .filter(|e| e.kind == Kind::from(KIND_SERVER_LIST))
        .max_by_key(|e| e.created_at);
    let mut servers: Vec<String> = Vec::new();
    for tag in newest.into_iter().flat_map(|e| e.tags.iter()) {
        let tag = tag.as_slice();
        if tag.first().map(String::as_str) != Some("server") {
            continue;
        }
        let Some(url) = tag.get(1).map(|u| u.trim().trim_end_matches('/')) else {
            continue;
        };
        let scheme_ok = url.starts_with("https://") || (!https_only && url.starts_with("http://"));
        let parsed = nostr::Url::parse(url).ok();
        let clean = parsed.as_ref().is_some_and(|u| {
            u.username().is_empty()
                && u.password().is_none()
                && u.host_str().is_some()
                && u.query().is_none()
                && u.fragment().is_none()
        });
        if scheme_ok && clean && !servers.iter().any(|s| s == url) {
            servers.push(url.to_string());
        }
        if servers.len() >= MAX_SERVERS {
            break;
        }
    }
    servers
}

/// The BUD-02 authorization for uploading the blob `sha256`, unsigned.
pub fn auth_template(
    user: PublicKey,
    sha256: &str,
    filename: Option<&str>,
    now: u64,
) -> UnsignedEvent {
    let content = match filename {
        Some(name) => format!("Upload {name}"),
        None => "Upload a file".to_string(),
    };
    let tags = [
        ["t", "upload"].map(String::from),
        ["x".to_string(), sha256.to_string()],
        ["expiration".to_string(), (now + AUTH_TTL_SECS).to_string()],
    ]
    .into_iter()
    .filter_map(|t| Tag::parse(t).ok());
    EventBuilder::new(Kind::from(KIND_BLOSSOM_AUTH), content)
        .tags(tags)
        .custom_created_at(nostr::Timestamp::from(now))
        .build(user)
}

/// `Authorization: Nostr <base64(event json)>`, as BUD-01 has it.
pub fn authorization_header(auth: &Event) -> String {
    format!(
        "Nostr {}",
        BASE64_STANDARD.encode(serde_json::to_string(auth).unwrap_or_default())
    )
}

/// What one server confirmed it stored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stored {
    url: String,
    sha256: String,
    size: u64,
    mime: Option<String>,
}

/// Why one server did not take the upload.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PutFailure {
    /// It answered, with this HTTP status: a refusal.
    Rejected(u16, String),
    /// It could not be reached, or its answer was not a descriptor.
    Failed(String),
}

impl PutFailure {
    fn describe(&self, server: &str) -> String {
        match self {
            Self::Rejected(status, why) if why.is_empty() => {
                format!("{}: {status}", host_of(server))
            }
            Self::Rejected(status, why) => format!("{}: {status} ({why})", host_of(server)),
            Self::Failed(why) => format!("{}: {why}", host_of(server)),
        }
    }
}

/// The spec's error for an upload no server took: `quota exceeded` when a
/// server said so (402, 507), `server rejected` when every server answered
/// with a refusal, else `upload failed`.
fn failure_code(failures: &[PutFailure]) -> UploadErrorCode {
    let quota = |f: &PutFailure| matches!(f, PutFailure::Rejected(402 | 507, _));
    if failures.iter().any(quota) {
        UploadErrorCode::QuotaExceeded
    } else if !failures.is_empty()
        && failures
            .iter()
            .all(|f| matches!(f, PutFailure::Rejected(..)))
    {
        UploadErrorCode::ServerRejected
    } else {
        UploadErrorCode::UploadFailed
    }
}

/// Read a BUD-02 blob descriptor: what the server says it stored. It must
/// name a hash. A hash other than the one sent means the server transformed
/// the file; that is reported (as NAP-UPLOAD's `sha256` / `originalSha256`),
/// never hidden. A descriptor without a URL gets `<server>/<stored hash>`.
fn descriptor(body: &[u8], server: &str, sent: u64) -> Result<Stored, PutFailure> {
    let not = || PutFailure::Failed("the answer was not a blob descriptor".into());
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| not())?;
    let sha256 = value
        .get("sha256")
        .and_then(|v| v.as_str())
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(not)?;
    let url = value
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{server}/{sha256}"));
    let size = value.get("size").and_then(|v| v.as_u64()).unwrap_or(sent);
    let mime = value
        .get("type")
        .and_then(|v| v.as_str())
        .filter(|t| t.contains('/') && t.len() <= 129)
        .map(str::to_string);
    Ok(Stored {
        url,
        sha256,
        size,
        mime,
    })
}

/// A server as a person reads it in an error: its host.
fn host_of(server: &str) -> String {
    nostr::Url::parse(server)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| server.to_string())
}

/// Whether signing failed because the user said no, rather than broke.
/// Anywhere in the error's chain, so added context doesn't hide it.
fn said_no(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|cause| cause.to_string() == crate::external_signer::REJECTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_napplet_runtime::testing::{OutboxFixture, TestSigner};
    use std::sync::Mutex;

    fn list(keys: &nostr::Keys, servers: &[&str], at: u64) -> Event {
        EventBuilder::new(Kind::from(KIND_SERVER_LIST), "")
            .tags(servers.iter().map(|s| Tag::parse(["server", s]).unwrap()))
            .custom_created_at(nostr::Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    /// The newest list wins, in its order, cleaned; non-https is skipped
    /// when guarded; userinfo and junk never.
    #[test]
    fn servers_come_from_the_newest_list() {
        let keys = nostr::Keys::generate();
        let old = list(&keys, &["https://old.example"], 100);
        let new = list(
            &keys,
            &[
                "https://b.example/",
                "http://plain.example",
                "https://b.example",
                "https://user:pw@evil.example",
                "not a url",
                "https://a.example",
            ],
            200,
        );
        assert_eq!(
            servers_from_lists(&[old.clone(), new.clone()], true),
            ["https://b.example", "https://a.example"]
        );
        assert_eq!(
            servers_from_lists(&[new], false),
            [
                "https://b.example",
                "http://plain.example",
                "https://a.example"
            ]
        );
        assert!(servers_from_lists(&[], true).is_empty());
    }

    /// Kind 24242, `t` upload, `x` the hash, `expiration` five minutes out.
    #[test]
    fn the_auth_event_is_bud02() {
        let keys = nostr::Keys::generate();
        let sha = "ab".repeat(32);
        let unsigned = auth_template(keys.public_key(), &sha, Some("score.png"), 1_000);
        let event = unsigned.sign_with_keys(&keys).unwrap();
        assert_eq!(event.kind.as_u16(), 24242);
        assert_eq!(event.content, "Upload score.png");
        let tags: Vec<Vec<String>> = event.tags.iter().map(|t| t.as_slice().to_vec()).collect();
        assert_eq!(
            tags,
            vec![
                vec!["t".to_string(), "upload".into()],
                vec!["x".to_string(), sha.clone()],
                vec!["expiration".to_string(), "1300".into()],
            ]
        );

        let header = authorization_header(&event);
        let b64 = header.strip_prefix("Nostr ").unwrap();
        let back: Event = serde_json::from_slice(&BASE64_STANDARD.decode(b64).unwrap()).unwrap();
        assert_eq!(back, event);
        back.verify().unwrap();
    }

    #[test]
    fn a_descriptor_says_what_was_stored() {
        let sha = "cd".repeat(32);
        let ok = serde_json::json!({
            "url": "https://x.example/a.png", "sha256": sha, "size": 3, "type": "image/png"
        });
        let got = descriptor(ok.to_string().as_bytes(), "https://x.example", 9).unwrap();
        assert_eq!(got.url, "https://x.example/a.png");
        assert_eq!((got.sha256.as_str(), got.size), (sha.as_str(), 3));
        assert_eq!(got.mime.as_deref(), Some("image/png"));
        let no_url = serde_json::json!({"sha256": sha});
        let got = descriptor(no_url.to_string().as_bytes(), "https://x.example", 9).unwrap();
        assert_eq!((got.url, got.size), (format!("https://x.example/{sha}"), 9));
        let no_hash = serde_json::json!({"url": "https://x.example/b"});
        assert!(descriptor(no_hash.to_string().as_bytes(), "https://x.example", 9).is_err());
        assert!(descriptor(b"<html>", "https://x.example", 9).is_err());
    }

    #[test]
    fn failures_map_to_the_spec_strings() {
        use PutFailure::*;
        let r = |s| Rejected(s, String::new());
        assert_eq!(
            failure_code(&[r(401), r(403)]),
            UploadErrorCode::ServerRejected
        );
        assert_eq!(
            failure_code(&[r(401), Failed("x".into())]),
            UploadErrorCode::UploadFailed
        );
        assert_eq!(
            failure_code(&[Failed("x".into()), r(402)]),
            UploadErrorCode::QuotaExceeded
        );
        assert_eq!(failure_code(&[]), UploadErrorCode::UploadFailed);
    }

    /// The bounds sum to less than the prelude's wait, so the napplet hears
    /// why an upload failed rather than "timed out". The signer's own wait
    /// is the external signer's answer timeout.
    #[test]
    fn the_upload_gives_up_before_the_napplet_prelude_does() {
        assert!(
            LIST_LOOKUP_TIMEOUT
                + crate::external_signer::ANSWER_TIMEOUT
                + UPLOAD_BUDGET
                + MIRROR_BUDGET
                < myco_napplet_runtime::SIGNING_TIMEOUT
        );
    }

    /// What the fake Blossom saw: the `Authorization` header and the body.
    type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// A Blossom server on loopback: `PUT /upload` records what it was sent
    /// and answers with a descriptor — honest, or naming `lie` as the hash.
    async fn fake_blossom(lie: Option<String>, refuse: bool) -> (String, Seen) {
        use axum::{body::Bytes, http::HeaderMap, routing::put, Router};
        let seen: Seen = Arc::default();
        let record = seen.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let me = base.clone();
        let app = Router::new().route(
            "/upload",
            put(move |headers: HeaderMap, body: Bytes| {
                let record = record.clone();
                let lie = lie.clone();
                let me = me.clone();
                async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    record.lock().unwrap().push((auth, body.to_vec()));
                    if refuse {
                        return (
                            axum::http::StatusCode::UNAUTHORIZED,
                            [("x-reason", "who are you")],
                            String::new(),
                        );
                    }
                    let sha = lie.unwrap_or_else(|| nsite_deck::sync::sha256_hex(&body));
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "url": format!("{me}/{sha}.png"),
                            "sha256": sha,
                            "size": body.len(),
                            "type": "image/png",
                        })
                        .to_string(),
                    )
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });
        (base, seen)
    }

    /// An uploader over in-memory seams, with `servers` as the defaults.
    /// The returned flag is offline-only.
    fn uploader(
        relay: Arc<dyn RelayBackend>,
        signer: Arc<dyn Signer>,
        servers: Vec<String>,
    ) -> (BlossomUploader, Arc<std::sync::atomic::AtomicBool>) {
        let offline = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = offline.clone();
        let outbox = Arc::new(OutboxFixture::new(relay.clone()));
        let up = BlossomUploader::with_parts(
            relay,
            Arc::new(move || flag.load(std::sync::atomic::Ordering::Relaxed)),
            Arc::new(|| false),
            signer,
            outbox.clone(),
            outbox,
        )
        .for_test(servers);
        (up, offline)
    }

    fn mem_relay() -> Arc<dyn RelayBackend> {
        Arc::new(nsite_deck::testing::MemRelay::new())
    }

    fn blob(bytes: &[u8]) -> UploadBlob {
        UploadBlob {
            bytes: bytes.to_vec(),
            sha256: nsite_deck::sync::sha256_hex(bytes),
            mime: "image/png".into(),
            filename: Some("score.png".into()),
        }
    }

    /// The whole PUT against a loopback Blossom: a refusing server is
    /// skipped, the next takes it, the one after mirrors it; each was sent
    /// the bytes and a valid BUD-02 authorization signed by the user.
    #[tokio::test]
    async fn uploads_to_the_first_server_that_takes_it_and_mirrors() {
        let (refusing, refused) = fake_blossom(None, true).await;
        let (first, seen) = fake_blossom(None, false).await;
        let (mirror, mirrored) = fake_blossom(None, false).await;
        let signer = Arc::new(TestSigner::new());
        let (up, _) = uploader(
            mem_relay(),
            signer.clone(),
            vec![refusing.clone(), first.clone(), mirror.clone()],
        );
        let b = blob(b"\x89PNG not really");

        let done = up.upload(&b).await.unwrap();
        assert_eq!(done.url, format!("{first}/{}.png", b.sha256));
        assert_eq!(
            done.fallback_urls,
            vec![format!("{mirror}/{}.png", b.sha256)]
        );
        assert_eq!(done.size, b.bytes.len() as u64);
        assert_eq!(refused.lock().unwrap().len(), 1);

        let (auth, body) = seen.lock().unwrap()[0].clone();
        assert_eq!(body, b.bytes);
        let event: Event = serde_json::from_slice(
            &BASE64_STANDARD
                .decode(auth.strip_prefix("Nostr ").unwrap())
                .unwrap(),
        )
        .unwrap();
        event.verify().unwrap();
        assert_eq!(event.pubkey, signer.public_key());
        assert_eq!(event.kind.as_u16(), 24242);
        assert!(event
            .tags
            .iter()
            .any(|t| t.as_slice() == ["x".to_string(), b.sha256.clone()]));
        assert_eq!(mirrored.lock().unwrap()[0].1, b.bytes);
    }

    /// A server that answers with another hash transformed the file: what it
    /// stored is reported, not hidden and not refused.
    #[tokio::test]
    async fn a_descriptor_with_another_hash_is_reported_as_stored() {
        let stored = "00".repeat(32);
        let (transforming, _) = fake_blossom(Some(stored.clone()), false).await;
        let (up, _) = uploader(mem_relay(), Arc::new(TestSigner::new()), vec![transforming]);
        let done = up.upload(&blob(b"bytes")).await.unwrap();
        assert_eq!(done.sha256, stored);
        assert!(done.url.contains(&stored));
    }

    /// A server answering with a redirect is not followed: the signed upload
    /// never reaches where it points (here, another loopback server standing
    /// in for this phone's own services), and the next server takes it.
    #[tokio::test]
    async fn a_redirect_is_a_refusal_and_never_followed() {
        use axum::{routing::put, Router};
        let (target, hit) = fake_blossom(None, false).await;
        let (next, seen) = fake_blossom(None, false).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirecting = format!("http://{}", listener.local_addr().unwrap());
        let to = format!("{target}/upload");
        let app = Router::new().route(
            "/upload",
            put(move || {
                let to = to.clone();
                async move {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [("location", to)],
                        String::new(),
                    )
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });

        let (up, _) = uploader(
            mem_relay(),
            Arc::new(TestSigner::new()),
            vec![redirecting, next.clone()],
        );
        let b = blob(b"bytes");
        let done = up.upload(&b).await.unwrap();
        assert_eq!(done.url, format!("{next}/{}.png", b.sha256));
        assert!(hit.lock().unwrap().is_empty(), "the redirect was followed");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// The user saying no in the signer app is `user cancelled`; any other
    /// signing failure is `upload failed`. Nothing is sent either way.
    #[tokio::test]
    async fn a_signer_no_is_a_cancel_and_a_failure_is_not() {
        struct Refusing(&'static str);
        #[async_trait::async_trait]
        impl Signer for Refusing {
            async fn public_key(&self) -> anyhow::Result<PublicKey> {
                Ok(nostr::Keys::generate().public_key())
            }
            async fn sign(&self, _: UnsignedEvent) -> anyhow::Result<Event> {
                Err(anyhow::anyhow!("{}", self.0))
            }
        }
        let (server, seen) = fake_blossom(None, false).await;
        for (why, code) in [
            (
                crate::external_signer::REJECTED,
                UploadErrorCode::UserCancelled,
            ),
            (
                "the signer app did not answer",
                UploadErrorCode::UploadFailed,
            ),
        ] {
            let (up, _) = uploader(mem_relay(), Arc::new(Refusing(why)), vec![server.clone()]);
            let err = up.upload(&blob(b"bytes")).await.unwrap_err();
            assert_eq!(err.code, code, "{why}");
        }
        assert!(seen.lock().unwrap().is_empty());
        // Context added on top of the signer's "no" still reads as a no.
        assert!(said_no(
            &anyhow::anyhow!(crate::external_signer::REJECTED).context("signing the upload")
        ));
    }

    /// Every server refusing is `server rejected`, with the reasons logged.
    #[tokio::test]
    async fn all_servers_refusing_is_server_rejected() {
        let (a, _) = fake_blossom(None, true).await;
        let (b, _) = fake_blossom(None, true).await;
        let (up, _) = uploader(mem_relay(), Arc::new(TestSigner::new()), vec![a, b]);
        let err = up.upload(&blob(b"bytes")).await.unwrap_err();
        assert_eq!(err.code, UploadErrorCode::ServerRejected);
        assert!(err.detail.contains("401 (who are you)"), "{}", err.detail);
    }

    /// The user's own list is used before the defaults.
    #[tokio::test]
    async fn the_users_server_list_comes_first() {
        let (theirs, seen) = fake_blossom(None, false).await;
        let (default, unused) = fake_blossom(None, false).await;
        let keys = nostr::Keys::generate();
        let relay = mem_relay();
        relay.publish(list(&keys, &[&theirs], 100)).await.unwrap();
        let (up, _) = uploader(relay, Arc::new(TestSigner::with_keys(keys)), vec![default]);
        let done = up.upload(&blob(b"mine")).await.unwrap();
        assert!(done.url.starts_with(&theirs), "{}", done.url);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(unused.lock().unwrap().is_empty());
    }

    /// With no list held here, the user's own relays are asked for it.
    #[tokio::test]
    async fn the_users_list_is_looked_up_on_their_relays() {
        let (theirs, seen) = fake_blossom(None, false).await;
        let keys = nostr::Keys::generate();
        let relay = mem_relay();
        let outbox = Arc::new(OutboxFixture::new(relay.clone()));
        outbox.set_plan(
            keys.public_key(),
            Direction::Read,
            &["wss://their.relay"],
            myco_napplet_runtime::PlanSource::Nip65,
        );
        outbox
            .relay("wss://their.relay")
            .publish(list(&keys, &[&theirs], 100))
            .await
            .unwrap();
        let up = BlossomUploader::with_parts(
            relay,
            Arc::new(|| false),
            Arc::new(|| false),
            Arc::new(TestSigner::with_keys(keys)),
            outbox.clone(),
            outbox,
        )
        .for_test(Vec::new());
        let done = up.upload(&blob(b"found it")).await.unwrap();
        assert!(done.url.starts_with(&theirs), "{}", done.url);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// Offline-only refuses before anything is signed or sent.
    #[tokio::test]
    async fn offline_only_refuses() {
        let (server, seen) = fake_blossom(None, false).await;
        let (up, offline) = uploader(mem_relay(), Arc::new(TestSigner::new()), vec![server]);
        offline.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(!up.available().await);
        let err = up.upload(&blob(b"x")).await.unwrap_err();
        assert_eq!(err.code, UploadErrorCode::PolicyDenied);
        assert!(err.detail.contains("offline-only"), "{}", err.detail);
        assert!(seen.lock().unwrap().is_empty());
    }
}
