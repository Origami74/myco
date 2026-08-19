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
    newest_in_slot, AdminBackend, BlobStore, FanoutSink, NoopFanout, PeerSource, RelayBackend,
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

    /// A manifest whose aggregate `x` tag disagrees with its own `path` tags is
    /// rejected at parse — so it never imports and never serves. This is the
    /// case per-blob hashing cannot catch: every listed file is intact, but the
    /// *set* is not the one the author signed.
    #[tokio::test]
    async fn corrupt_aggregate_never_imports_or_serves() {
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

        let err = import_site(&relay, &blobs, site.manifest.clone(), &site.blobs)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("aggregate mismatch"),
            "unexpected error: {err}"
        );

        // Even if the manifest reaches the store by another route (a peer's
        // gossip, say), the gateway refuses to serve it.
        verify_and_store_event(&relay, site.manifest.clone())
            .await
            .unwrap();
        blobs.put(b"<h1>hi</h1>").await.unwrap();
        let resp = serve(&relay, &blobs, &host, "/index.html", None).await;
        assert_ne!(resp.status, 200, "a mismatched aggregate must not serve");
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
}
