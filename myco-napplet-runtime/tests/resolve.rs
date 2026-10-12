//! The S0 gate: a fixture napplet's artifact hash verifies off-device and
//! becomes its identity, and every way of getting it wrong is refused.
//!
//! Each test breaks exactly one thing and asserts the *code*, not the message —
//! a napplet that fails must fail for the reason we think it does, or a later
//! change can quietly turn a signature check into a missing-file check and
//! nothing here would notice.

use myco_napplet_runtime::error::NappletErrorCode;
use myco_napplet_runtime::manifest::{NappletManifest, KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT};
use myco_napplet_runtime::resolve::resolve;
use myco_napplet_runtime::testing::{
    build_test_napplet, FixtureAggregate, NappletBuilder, TestNapplet, FIXTURE_INDEX_HTML,
};
use nsite_deck::seams::BlobStore;
use nsite_deck::sync::sha256_hex;
use nsite_deck::testing::MemBlobs;

/// A blob store holding exactly the fixture's files.
async fn store_for(napplet: &TestNapplet) -> MemBlobs {
    let blobs = MemBlobs::new();
    for (_, bytes) in &napplet.blobs {
        blobs.put(bytes).await.unwrap();
    }
    blobs
}

async fn expect_rejection(napplet: TestNapplet, code: NappletErrorCode) {
    let blobs = store_for(&napplet).await;
    let err = resolve(napplet.manifest, &blobs)
        .await
        .expect_err("this napplet must not resolve");
    assert_eq!(err.code, code, "wrong guard rejected: {err}");
}

/// The gate itself: the fixture resolves, and the identity the runtime assigns
/// it is the sha256 of the bytes it fetched and verified.
#[tokio::test]
async fn the_fixture_resolves_and_its_identity_is_the_artifact_hash() {
    let napplet = build_test_napplet();
    let blobs = store_for(&napplet).await;

    let expected = sha256_hex(FIXTURE_INDEX_HTML.as_bytes());

    let resolved = resolve(napplet.manifest, &blobs).await.unwrap();
    assert_eq!(resolved.identity(), ("fixture", expected.as_str()));
    assert_eq!(resolved.index_html, FIXTURE_INDEX_HTML);
    assert_eq!(resolved.manifest.requires, vec!["shell", "relay"]);
}

#[tokio::test]
async fn rejects_a_bad_signature() {
    expect_rejection(
        NappletBuilder::new().break_signature().build(),
        NappletErrorCode::InvalidSignature,
    )
    .await;
}

/// Bytes that do not hash to what the manifest claims. The store is the
/// untrusted party here: it is handed the right hash and the wrong bytes, which
/// is what a compromised Blossom server or a corrupted cache looks like.
#[tokio::test]
async fn rejects_a_blob_hash_mismatch() {
    let napplet = build_test_napplet();
    let blobs = MemBlobs::new();
    blobs.put(b"<h1>not the signed bytes</h1>").await.unwrap();

    let err = resolve(napplet.manifest, &LyingStore(blobs))
        .await
        .expect_err("mismatched bytes must not render");
    assert_eq!(err.code, NappletErrorCode::BlobHashMismatch);
}

/// A store that answers every hash with the one blob it holds — the "trust the
/// server" failure the re-hash in `resolve` exists to catch.
struct LyingStore(MemBlobs);

#[async_trait::async_trait]
impl BlobStore for LyingStore {
    async fn has(&self, _sha256_hex: &str) -> bool {
        true
    }
    async fn get(&self, _sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(Some(b"<h1>not the signed bytes</h1>".to_vec()))
    }
    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        self.0.put(bytes).await
    }
    async fn wipe(&self) -> anyhow::Result<()> {
        self.0.wipe().await
    }
}

/// An `x` naming bytes nobody has: no source can serve it, and the bytes that
/// are there are not fetched in its place.
#[tokio::test]
async fn an_x_naming_other_bytes_does_not_resolve() {
    expect_rejection(
        NappletBuilder::new()
            .aggregate(FixtureAggregate::Corrupt)
            .build(),
        NappletErrorCode::BlobUnavailable,
    )
    .await;
}

/// NIP-5D Identity step 2: a manifest without exactly one `x` tag is rejected.
#[tokio::test]
async fn rejects_a_manifest_with_no_x() {
    expect_rejection(
        NappletBuilder::new()
            .aggregate(FixtureAggregate::Omitted)
            .build(),
        NappletErrorCode::InvalidManifest,
    )
    .await;
}

#[tokio::test]
async fn rejects_a_manifest_whose_blob_is_absent() {
    let napplet = build_test_napplet();
    let empty = MemBlobs::new();
    let err = resolve(napplet.manifest, &empty).await.unwrap_err();
    assert_eq!(err.code, NappletErrorCode::BlobUnavailable);
}

/// The kind trap: 35128 is Myco's nsite kind, not a napplet's. NIP-5D is
/// 5129 / 15129 / 35129.
#[tokio::test]
async fn rejects_the_nsite_kind() {
    let site =
        nsite_deck::testing::build_test_site(&[("/index.html", b"<h1>nsite</h1>")], None, None);
    let blobs = MemBlobs::new();
    blobs.put(b"<h1>nsite</h1>").await.unwrap();
    let err = resolve(site.manifest, &blobs).await.unwrap_err();
    assert_eq!(err.code, NappletErrorCode::InvalidManifest);
    assert!(err.message.contains("15128"), "unexpected error: {err}");
}

#[tokio::test]
async fn root_and_snapshot_kinds_resolve_with_an_empty_d_tag() {
    for kind in [KIND_ROOT, KIND_SNAPSHOT] {
        let napplet = NappletBuilder::new().kind(kind).build();
        let blobs = store_for(&napplet).await;
        let resolved = resolve(napplet.manifest, &blobs).await.unwrap();
        assert_eq!(resolved.d_tag, "");
        assert_eq!(resolved.manifest.kind, kind);
    }
}

#[tokio::test]
async fn a_named_manifest_needs_a_d_tag() {
    let napplet = NappletBuilder::new().d_tag(None).build();
    assert_eq!(napplet.manifest.kind.as_u16(), KIND_NAMED);
    let err = NappletManifest::from_event(napplet.manifest).unwrap_err();
    assert_eq!(err.code, NappletErrorCode::InvalidManifest);
}
