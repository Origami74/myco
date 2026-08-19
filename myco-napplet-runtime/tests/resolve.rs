//! The S0 gate: a fixture napplet's aggregate recomputes and matches
//! off-device, and every way of getting it wrong is refused.
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
use nsite_deck::aggregate::{compute_aggregate_hash, PathEntry};
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
/// it is the aggregate recomputed from the bytes — not anything the manifest
/// asserted.
#[tokio::test]
async fn the_fixture_resolves_and_its_aggregate_recomputes() {
    let napplet = build_test_napplet();
    let blobs = store_for(&napplet).await;

    let expected = compute_aggregate_hash(&[PathEntry {
        path: "/index.html".into(),
        sha256: sha256_hex(FIXTURE_INDEX_HTML.as_bytes()),
    }]);

    let resolved = resolve(napplet.manifest, &blobs).await.unwrap();
    assert_eq!(resolved.identity(), ("fixture", expected.as_str()));
    assert_eq!(resolved.index_html, FIXTURE_INDEX_HTML);
    assert_eq!(resolved.manifest.requires, vec!["shell", "relay"]);
}

/// The same fixture, pinned to the hex the **reference JS runtime** computes
/// for it. Recomputing the expected value with the same code that produced it
/// (as the test above does) proves self-consistency, not interoperability —
/// this constant is what proves a napplet built for kehto resolves here.
///
/// Cross-checked against `@kehto/nip/5a`'s `computeAggregateHash` over
/// `FIXTURE_INDEX_HTML`. If it ever fails, the algorithm diverged from the
/// reference and every published napplet stops loading — do not "fix" it by
/// updating the constant.
#[tokio::test]
async fn the_fixture_aggregate_matches_the_reference_implementation() {
    let manifest = NappletManifest::from_event(build_test_napplet().manifest).unwrap();
    assert_eq!(
        manifest.aggregate,
        "ae61a6e95ad666d2294f88e69769961148aa933ca1fdf59a7f31cf0d2c97c1cb"
    );

    // And a second, two-file vector, so the sort-then-concatenate step is
    // pinned too and not just the single-line case where sorting is a no-op.
    // Computed directly: a two-file *napplet* is refused at parse, but the
    // aggregate algorithm underneath it still has to match the reference.
    assert_eq!(
        compute_aggregate_hash(&[
            PathEntry {
                path: "/index.html".into(),
                sha256: sha256_hex(FIXTURE_INDEX_HTML.as_bytes()),
            },
            PathEntry {
                path: "/app.js".into(),
                sha256: sha256_hex(b"console.log('hi')"),
            },
        ]),
        "d33c30b3f7ad101e27f2ed4fae460edc51139e41454d66d8c08760d0f561f168"
    );
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

#[tokio::test]
async fn rejects_an_aggregate_mismatch() {
    expect_rejection(
        NappletBuilder::new()
            .aggregate(FixtureAggregate::Corrupt)
            .build(),
        NappletErrorCode::AggregateMismatch,
    )
    .await;
}

/// The `x` tag is corroboration, not the source. NIP-5D Identity step 3 says
/// the runtime recomputes the aggregate from the `path` tags and that the tag,
/// *if carried*, must match — so a napplet without one still has an identity,
/// and it is the same one it would have had with the tag present.
#[tokio::test]
async fn a_missing_aggregate_tag_still_resolves_to_the_same_identity() {
    let declared = build_test_napplet();
    let undeclared = NappletBuilder::new()
        .aggregate(FixtureAggregate::Omitted)
        .build();

    let blobs = store_for(&undeclared).await;
    let resolved = resolve(undeclared.manifest, &blobs).await.unwrap();

    let with_tag = NappletManifest::from_event(declared.manifest).unwrap();
    assert_eq!(resolved.aggregate, with_tag.aggregate);
}

/// NIP-5D: "A napplet is a single self-contained /index.html". A manifest
/// listing more than one file is not a napplet, so it is refused at parse —
/// before any blob is fetched, and never inlined, which would assemble bytes
/// the author never signed as a unit.
#[tokio::test]
async fn rejects_a_multi_file_bundle() {
    let napplet = NappletBuilder::new()
        .file("/app.js", b"console.log('hi')")
        .build();
    let blobs = store_for(&napplet).await;
    let err = resolve(napplet.manifest.clone(), &blobs).await.unwrap_err();
    assert_eq!(err.code, NappletErrorCode::MultiFile);

    // Rejected by the manifest layer, not by the resolve pipeline — so the same
    // refusal happens everywhere a manifest is parsed, not only on the load path.
    let parsed = NappletManifest::from_event(napplet.manifest).unwrap_err();
    assert_eq!(parsed.code, NappletErrorCode::MultiFile);

    // The error names the offending files, and cites the rule rather than
    // restating a Myco preference.
    assert!(err.message.contains("/app.js"), "unhelpful error: {err}");
    assert!(
        err.message.contains("single self-contained"),
        "unhelpful error: {err}"
    );
    // Myco loads napplets, it never builds them. The person who sees this did
    // not publish the napplet and cannot rebuild it, so the message says what
    // the publisher did — it does not hand out build instructions.
    assert!(
        !err.message.to_lowercase().contains("rebuild"),
        "error tells our user to rebuild someone else's napplet: {err}"
    );
}

#[tokio::test]
async fn rejects_a_manifest_with_no_index() {
    expect_rejection(
        NappletBuilder::new()
            .files(&[("/main.html", b"<h1>wrong name</h1>")])
            .build(),
        NappletErrorCode::MissingIndex,
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
