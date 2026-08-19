//! [`RemoteBlobStore`] — a [`BlobStore`] backed by **someone else's** Blossom
//! server.
//!
//! The blob half of the same idea as [`crate::remote_backend`]: keep Myco's
//! behaviour in front and speak nothing but the standard protocol to whatever is
//! storing the bytes. Here that protocol is BUD-01 — `GET`/`HEAD /<sha256>` to
//! read, `PUT /upload` to write — so an unmodified Blossom server can serve.
//!
//! **Every read is hash-checked.** The embedded store deliberately skips
//! re-hashing on read, because the only way bytes get in is a verified write.
//! That assumption dies the moment the store is shared: another writer, or a
//! server that simply returns the wrong body, would otherwise hand us bytes we
//! serve as an app's content. Blobs are content-addressed, so the check is exact
//! and needs no trust — unlike the signature question on the relay side, this
//! one is a cost decision, and the cost is worth paying.
//!
//! Uploads are signed with the device key as a kind-24242 authorization event
//! (BUD-01). The embedded server does not ask for one, because the circle gate
//! covers it; a public server will.
//!
//! See `reference/thinning-custom-relay.md` (D9).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nostr::base64::prelude::{Engine as _, BASE64_STANDARD};
use nostr::{EventBuilder, Keys, Kind, Tag};
use nsite_deck::seams::BlobStore;
use nsite_deck::sha256_hex;

/// Generous: a blob can be megabytes, and the server may be across a slow link.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long an upload authorization stays valid. Short, because it authorises
/// one specific blob and is minted immediately before use.
const AUTH_TTL_SECS: u64 = 300;

/// BUD-01's authorization event kind.
const KIND_BLOSSOM_AUTH: u16 = 24242;

/// A Blossom server we do not own, used as the blob store.
pub struct RemoteBlobStore {
    base: String,
    http: reqwest::Client,
    /// The device key, shared with [`crate::content::Content`] because it is set
    /// after construction — the store is built before the identity is loaded.
    keys: Arc<Mutex<Option<Keys>>>,
    /// Why the last attempt failed, or `None` while it is working.
    last_error: Arc<Mutex<Option<String>>>,
}

impl RemoteBlobStore {
    pub fn new(base: impl Into<String>, keys: Arc<Mutex<Option<Keys>>>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
            keys,
            last_error: Arc::new(Mutex::new(None)),
        }
    }

    /// The key holder this store signs with, so the owner can share it rather
    /// than keeping a second copy that could fall out of step.
    pub fn keys(&self) -> Arc<Mutex<Option<Keys>>> {
        self.keys.clone()
    }

    /// What to tell the user about this server right now.
    pub fn health(&self) -> crate::remote_backend::BackendHealth {
        crate::remote_backend::BackendHealth {
            url: self.base.clone(),
            error: self.last_error.lock().unwrap().clone().unwrap_or_default(),
        }
    }

    fn note(&self, err: Option<String>) {
        *self.last_error.lock().unwrap() = err;
    }

    fn url_for(&self, hash: &str) -> String {
        format!("{}/{}", self.base, hash)
    }

    /// A BUD-01 `Authorization: Nostr <base64>` header authorising one upload.
    ///
    /// `None` when the device key is not loaded yet, which is a real state
    /// during early startup rather than an error — the caller sends the request
    /// unsigned and lets the server decide, since our own embedded server does
    /// not ask for one.
    fn upload_auth(&self, hash: &str) -> Option<String> {
        let keys = self.keys.lock().unwrap().clone()?;
        let expiration = crate::content::now_secs() + AUTH_TTL_SECS;
        let event = EventBuilder::new(Kind::from(KIND_BLOSSOM_AUTH), "Upload blob")
            .tags([
                Tag::parse(["t", "upload"]).ok()?,
                Tag::parse(["x", hash]).ok()?,
                Tag::parse(["expiration", &expiration.to_string()]).ok()?,
            ])
            .sign_with_keys(&keys)
            .ok()?;
        let json = serde_json::to_string(&event).ok()?;
        Some(format!("Nostr {}", BASE64_STANDARD.encode(json)))
    }
}

#[async_trait]
impl BlobStore for RemoteBlobStore {
    async fn has(&self, want_hex: &str) -> bool {
        match self.http.head(self.url_for(want_hex)).send().await {
            Ok(r) => {
                self.note(None);
                r.status().is_success()
            }
            Err(e) => {
                // The seam has no way to say "I could not tell" here, so an
                // unreachable server and a missing blob both read as `false`.
                // Recording the failure is what keeps them distinguishable: the
                // settings screen shows the reason, rather than the user seeing
                // a site stuck at "incomplete" with nothing to explain it.
                self.note(Some(format!("Could not reach {}: {e}", self.base)));
                false
            }
        }
    }

    async fn get(&self, want_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let want = want_hex.to_ascii_lowercase();
        let resp = match self.http.get(self.url_for(&want)).send().await {
            Ok(r) => r,
            Err(e) => {
                self.note(Some(format!("Could not reach {}: {e}", self.base)));
                anyhow::bail!("blob store unreachable: {e}");
            }
        };
        self.note(None);
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            anyhow::bail!("blob store returned {}", resp.status());
        }
        let bytes = resp.bytes().await?.to_vec();
        // The hash is the identity, so bytes that do not match it are not the
        // blob — whatever the server meant by sending them. Treated as absent
        // rather than an error, so the sync path falls through to another source
        // exactly as it would for a missing blob.
        if sha256_hex(&bytes) != want {
            tracing::warn!(
                server = %self.base,
                want = %want,
                "blob store returned bytes that do not match the requested hash"
            );
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let hash = sha256_hex(bytes);
        let mut req = self
            .http
            .put(format!("{}/upload", self.base))
            .body(bytes.to_vec());
        if let Some(auth) = self.upload_auth(&hash) {
            req = req.header("Authorization", auth);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                self.note(Some(format!("Could not reach {}: {e}", self.base)));
                anyhow::bail!("blob store unreachable: {e}");
            }
        };
        self.note(None);
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            anyhow::bail!(
                "{} refused the upload — it may not accept this device's key",
                self.base
            );
        }
        if !resp.status().is_success() {
            anyhow::bail!("blob store returned {} on upload", resp.status());
        }
        Ok(hash)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        // Not ours to clear. BUD-02 can delete blobs we uploaded, one call each,
        // but a server we do not run holds other people's data too and its
        // contents are the operator's business — the same call made for a custom
        // relay (`reference/thinning-custom-relay.md`, D4).
        tracing::info!(
            server = %self.base,
            "blob store: skipping wipe, a custom Blossom's contents are not ours to clear"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand up the embedded Blossom and drive it through `RemoteBlobStore`:
    /// Myco talking to a blob server over nothing but BUD-01.
    async fn spawn_blossom() -> (Arc<myco_blossom::FsBlobStore>, String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "myco-remote-blobs-{}-{}",
            std::process::id(),
            nostr::Keys::generate().public_key().to_hex()
        ));
        let store = Arc::new(myco_blossom::FsBlobStore::open(&dir).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(myco_blossom::server::serve_on(store.clone(), listener));
        (store, format!("http://{addr}"), dir)
    }

    fn keys() -> Arc<Mutex<Option<Keys>>> {
        Arc::new(Mutex::new(Some(Keys::generate())))
    }

    #[tokio::test]
    async fn round_trips_a_blob_over_bud01() {
        let (local, base, dir) = spawn_blossom().await;
        let store = RemoteBlobStore::new(&base, keys());

        let hash = store.put(b"hello blossom").await.unwrap();
        assert_eq!(hash, sha256_hex(b"hello blossom"));
        assert_eq!(local.count(), 1, "the bytes landed on the server");

        assert!(store.has(&hash).await);
        assert_eq!(
            store.get(&hash).await.unwrap().as_deref(),
            Some(&b"hello blossom"[..]),
        );

        // A blob nobody stored is absent, not an error.
        let missing = sha256_hex(b"never uploaded");
        assert!(!store.has(&missing).await);
        assert!(store.get(&missing).await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bytes that do not hash to the name we asked for are not that blob, so
    /// they are treated as absent.
    ///
    /// The embedded store can skip this check because the only way bytes get in
    /// is a verified write. A shared server has other writers, so serving what
    /// it hands back unchecked would mean serving whatever it happened to store
    /// under that name as an app's content.
    #[tokio::test]
    async fn bytes_that_do_not_match_the_hash_are_rejected() {
        // A server that answers every GET with the same wrong body.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let app = axum::Router::new().route(
                "/{hash}",
                axum::routing::get(|| async { "not what you asked for" }),
            );
            let _ = axum::serve(listener, app).await;
        });

        let store = RemoteBlobStore::new(format!("http://{addr}"), keys());
        let want = sha256_hex(b"the real blob");
        assert!(
            store.get(&want).await.unwrap().is_none(),
            "mismatched bytes must not be served as the blob"
        );
    }

    /// An unreachable server is an error on read, not an empty answer — the same
    /// distinction the relay backend makes, for the same reason.
    ///
    /// `has` is the exception, because the seam returns a bare `bool` with no
    /// way to say "could not tell". It records the failure instead, so the
    /// reason is on the settings screen rather than a site sitting at
    /// "incomplete" with nothing to explain it.
    #[tokio::test]
    async fn an_unreachable_server_is_an_error() {
        let store = RemoteBlobStore::new("http://127.0.0.1:1", keys());
        assert!(store.get(&sha256_hex(b"x")).await.is_err());
        assert!(store.put(b"x").await.is_err());
        assert!(
            !store.health().error.is_empty(),
            "the failure is recorded for the settings screen"
        );

        let unreachable = RemoteBlobStore::new("http://127.0.0.1:1", keys());
        assert!(!unreachable.has(&sha256_hex(b"x")).await);
        assert!(
            !unreachable.health().error.is_empty(),
            "has() cannot report the failure in its return value, so it must \
             leave it where the user will see it"
        );
    }

    /// Wiping a server we do not run is a no-op, not a mass delete.
    #[tokio::test]
    async fn wiping_a_custom_server_leaves_it_alone() {
        let (local, base, dir) = spawn_blossom().await;
        let store = RemoteBlobStore::new(&base, keys());
        store.put(b"keep me").await.unwrap();

        store.wipe().await.unwrap();
        assert_eq!(local.count(), 1, "a custom server's blobs are not ours");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
