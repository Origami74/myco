//! `nsite-deck` — the reusable, transport-agnostic **nsite host**: the localhost
//! gateway engine (manifest → path → sha256 → serve), the sync/import engine, and
//! (P3) the propagator. It reaches relay / Blossom / transport through four trait
//! seams ([`RelayBackend`], [`BlobStore`], [`PeerSource`], [`FanoutSink`]) and
//! names no concrete relay, blob store, or radio.
//!
//! P2 lands the gateway + sync; the propagator (`FanoutSink`) is a P3 no-op stub.
//! See `docs/design/nsite/nsite-layer.md`.
//!
//! [`RelayBackend`]: seams::RelayBackend
//! [`BlobStore`]: seams::BlobStore
//! [`PeerSource`]: seams::PeerSource
//! [`FanoutSink`]: seams::FanoutSink

pub mod aggregate;
pub mod base36;
pub mod content_type;
pub mod gateway;
pub mod host;
pub mod model;
pub mod seams;
pub mod sync;

#[cfg(feature = "testing")]
pub mod testing;

pub use aggregate::{
    aggregate_tag_value, check_aggregate, compute_aggregate_hash, path_entries, path_entries_of,
    AggregateCheck, PathEntry,
};
pub use gateway::{serve, GatewayResponse, Readiness};
pub use host::{parse_link, resolve_host, SiteAddr};
pub use model::{kind_for, site_key, Manifest, KIND_NAMED, KIND_ROOT};
pub use seams::{
    cap_per_filter, newest_in_slot, AdminBackend, BlobStore, FanoutSink, NoopFanout, PeerSource,
    RelayBackend,
};
pub use sync::{import_site, sha256_hex, sync_site, verify_and_store_event, SyncOutcome};

#[cfg(test)]
mod tests {
    use super::testing::{
        build_test_site, build_test_site_full, MemBlobs, MemRelay, TestAggregate,
    };
    use super::*;

    fn host_for(author: &nostr::PublicKey) -> String {
        use nostr::nips::nip19::ToBech32;
        format!("{}.nsite", author.to_bech32().unwrap())
    }

    /// Import a generated site, then serve it through the gateway — the full M1
    /// path: signed manifest verify → store → host resolve → manifest → path →
    /// sha256 → verified blob → response, with the right content-type.
    #[tokio::test]
    async fn import_then_serve_root_site() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();

        let site = build_test_site(
            &[
                ("/index.html", b"<h1>hello</h1>"),
                ("/style.css", b"body{}"),
            ],
            None,
            Some("Test Site"),
        );
        let host = host_for(&site.author);

        let outcome = import_site(&relay, &blobs, site.manifest.clone(), &site.blobs)
            .await
            .unwrap();
        assert_eq!(outcome, SyncOutcome::Ready);

        // Root path → index.html, served with the html content-type.
        let resp = serve(&relay, &blobs, &host, "/", None).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type, "text/html; charset=utf-8");
        assert_eq!(resp.body, b"<h1>hello</h1>");

        // A real subresource path with its own content-type.
        let css = serve(&relay, &blobs, &host, "/style.css", None).await;
        assert_eq!(css.status, 200);
        assert_eq!(css.content_type, "text/css; charset=utf-8");
        assert_eq!(css.body, b"body{}");

        // Unknown path with no /404.html → gateway 404.
        let missing = serve(&relay, &blobs, &host, "/nope.html", None).await;
        assert_eq!(missing.status, 404);
    }

    /// A manifest whose blobs aren't all present must NOT serve — the gateway
    /// shows the loading page until sync completes (the offline-correctness gate).
    #[tokio::test]
    async fn incomplete_site_shows_loading_not_content() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();

        let site = build_test_site(&[("/index.html", b"hi"), ("/big.bin", b"....")], None, None);
        let host = host_for(&site.author);

        // Store the manifest + only ONE of the two blobs.
        verify_and_store_event(&relay, site.manifest.clone())
            .await
            .unwrap();
        blobs.put(b"hi").await.unwrap();

        let resp = serve(&relay, &blobs, &host, "/index.html", None).await;
        assert_eq!(resp.status, 503, "incomplete site must not serve content");
        assert!(String::from_utf8_lossy(&resp.body).contains("1/2"));
    }

    /// A blob whose bytes don't match the manifest hash is rejected by import.
    #[tokio::test]
    async fn import_rejects_hash_mismatch() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();
        let site = build_test_site(&[("/index.html", b"real")], None, None);

        // Tamper: same hash key, wrong bytes.
        let tampered = vec![(site.blobs[0].0.clone(), b"FAKE".to_vec())];
        let err = import_site(&relay, &blobs, site.manifest.clone(), &tampered).await;
        assert!(err.is_err(), "hash mismatch must abort import");
    }

    /// Replaceable semantics: one slot per (kind, author), duplicates ignored.
    #[tokio::test]
    async fn replaceable_slot_dedup() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();
        let site = build_test_site(&[("/index.html", b"v1")], None, None);

        import_site(&relay, &blobs, site.manifest.clone(), &site.blobs)
            .await
            .unwrap();
        assert_eq!(relay.len(), 1);

        // Publishing is idempotent: re-storing the same manifest is a no-op
        // rather than a second entry in the slot. The seam reports nothing back,
        // so the store's own size is what pins the behaviour.
        relay.publish(site.manifest).await.unwrap();
        assert_eq!(
            relay.len(),
            1,
            "an equal-timestamp duplicate does not stack"
        );
    }

    /// An nsite manifest whose aggregate `x` tag disagrees with its own `path`
    /// tags still imports and serves — every blob is hash-verified against the
    /// signed path tags regardless — but records no verified aggregate. A
    /// mismatch is a warning until the formula has been checked against
    /// enough published sites to refuse one on its say-so. Napplets are the
    /// strict case, tested in `myco-napplet-runtime`.
    #[tokio::test]
    async fn corrupt_aggregate_serves_without_a_verified_aggregate() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();
        let site = build_test_site_full(
            &nostr::Keys::generate(),
            &[("/index.html", b"<h1>hi</h1>")],
            None,
            None,
            TestAggregate::Corrupt,
        );
        let host = host_for(&site.author);

        import_site(&relay, &blobs, site.manifest.clone(), &site.blobs)
            .await
            .unwrap();
        let resp = serve(&relay, &blobs, &host, "/index.html", None).await;
        assert_eq!(
            resp.status, 200,
            "per-blob verified content must still serve"
        );

        let manifest = Manifest::from_event(site.manifest).unwrap();
        assert_eq!(
            manifest.aggregate, None,
            "a mismatched aggregate must not be recorded as verified"
        );
    }

    /// Lenient the other way: most published nsites predate the aggregate tag.
    /// Omitting it is not an error — every blob is still hash-verified — so
    /// those sites keep serving.
    #[tokio::test]
    async fn a_missing_aggregate_still_serves() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();
        let site = build_test_site_full(
            &nostr::Keys::generate(),
            &[("/index.html", b"legacy")],
            None,
            None,
            TestAggregate::Omitted,
        );
        let host = host_for(&site.author);

        import_site(&relay, &blobs, site.manifest.clone(), &site.blobs)
            .await
            .unwrap();
        let resp = serve(&relay, &blobs, &host, "/index.html", None).await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"legacy");

        let manifest = Manifest::from_event(site.manifest).unwrap();
        assert_eq!(manifest.aggregate, None);
    }

    /// A valid site records its verified aggregate — the primitive NIP-5D
    /// promotes to being a napplet's identity.
    #[tokio::test]
    async fn a_valid_aggregate_is_recorded() {
        let site = build_test_site(&[("/index.html", b"hi"), ("/app.js", b"x")], None, None);
        let manifest = Manifest::from_event(site.manifest).unwrap();
        let expected = compute_aggregate_hash(&[
            PathEntry {
                path: "/index.html".into(),
                sha256: sha256_hex(b"hi"),
            },
            PathEntry {
                path: "/app.js".into(),
                sha256: sha256_hex(b"x"),
            },
        ]);
        assert_eq!(manifest.aggregate.as_deref(), Some(expected.as_str()));
    }

    #[tokio::test]
    async fn unresolvable_host_bounces() {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();
        let resp = serve(&relay, &blobs, "example.com", "/", None).await;
        assert_eq!(resp.status, 404);
        assert!(String::from_utf8_lossy(&resp.body).contains("Not in your Library"));
    }

    /// A sync asks for each blob with the manifest's `server` hints and then
    /// the servers the source says the author lists (BUD-03): an app hosted
    /// only on its author's own Blossom is reachable.
    #[tokio::test]
    async fn sync_offers_the_authors_servers_after_the_manifests() {
        use std::sync::Mutex;
        struct Recording {
            manifest: nostr::Event,
            blobs: Vec<(String, Vec<u8>)>,
            asked_with: Mutex<Vec<Vec<String>>>,
        }
        #[async_trait::async_trait]
        impl PeerSource for Recording {
            async fn fetch_manifest(
                &self,
                _author: &nostr::PublicKey,
                _d_tag: Option<&str>,
            ) -> anyhow::Result<Option<nostr::Event>> {
                Ok(Some(self.manifest.clone()))
            }
            async fn fetch_blob(
                &self,
                sha256_hex: &str,
                servers: &[String],
            ) -> anyhow::Result<Option<Vec<u8>>> {
                self.asked_with.lock().unwrap().push(servers.to_vec());
                Ok(self
                    .blobs
                    .iter()
                    .find(|(h, _)| h == sha256_hex)
                    .map(|(_, b)| b.clone()))
            }
            async fn author_servers(&self, author: &nostr::PublicKey) -> Vec<String> {
                assert_eq!(*author, self.manifest.pubkey);
                vec![
                    "https://author.example".into(),
                    "https://shared.example".into(),
                ]
            }
        }
        // A manifest naming one server the author lists too.
        let keys = nostr::Keys::generate();
        let site = super::testing::build_test_site_with_keys(
            &keys,
            &[("/index.html", b"<h1>mine</h1>")],
            None,
            None,
        );
        let mut tags: Vec<nostr::Tag> = site.manifest.tags.iter().cloned().collect();
        tags.push(nostr::Tag::parse(["server", "https://shared.example"]).unwrap());
        let manifest = nostr::EventBuilder::new(site.manifest.kind, "")
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap();
        let source = Recording {
            manifest,
            blobs: site.blobs.clone(),
            asked_with: Mutex::new(Vec::new()),
        };
        let (relay, blobs) = (MemRelay::new(), MemBlobs::new());
        let addr = SiteAddr {
            author: site.author,
            d_tag: None,
        };
        let outcome = sync_site(&relay, &blobs, &source, &addr, &|_, _| {})
            .await
            .unwrap();
        assert_eq!(outcome, SyncOutcome::Ready);
        assert_eq!(
            source.asked_with.lock().unwrap()[0],
            vec![
                "https://shared.example".to_string(),
                "https://author.example".to_string()
            ]
        );
    }
}
